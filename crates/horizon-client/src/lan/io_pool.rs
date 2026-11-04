//! The network threads of the play state: a fixed pool, one thread per two
//! cores, each polling its players' sockets (epoll, kqueue or Windows' AFD,
//! through mio), where every player used to have a reader and a writer thread
//! of its own. The game thread only queues packet bodies in a player's
//! [`Outbox`] (bounded by [`MAX_PENDING`] bytes); framing, compression,
//! encryption, the socket writes, reading and the keep-alives happen here. A
//! connection stays on one thread, so its bytes keep their order. The
//! integrated server's own player, over an in-memory pipe, is served the same
//! way: the pipe tells the thread when bytes arrive. A WebSocket player's
//! socket is polled like a TCP one, its frames made and read on the way
//! ([`horizon_protocol::websocket::Framed`]).
use horizon_protocol::{
    connection::{ConnReader, ConnWriter, Pipe, Socket},
    websocket::Framed,
};
use mio::{Events, Interest, Poll, Registry, Token, Waker};
use std::{
    io::{ErrorKind, Read, Write},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Packets queued for one client beyond this many bytes disconnect it.
pub(super) const MAX_PENDING: usize = 96 << 20;
/// How often a thread looks at its connections' clocks: keep-alives,
/// timeouts and the closes that wait for a disconnect's last bytes.
const SWEEP: Duration = Duration::from_millis(100);
/// Framed bytes a connection may hold unwritten before its thread stops
/// taking more from its outbox: the rest wait there, counted in `pending`, so
/// that a client which reads slowly has the hub hold its far chunks back
/// (`lan_far`'s backlog) and, past [`MAX_PENDING`], is disconnected.
const AHEAD: usize = 256 << 10;
/// How long a disconnect's last bytes are given before the socket closes.
const LINGER: Duration = Duration::from_millis(50);
const WAKER: Token = Token(usize::MAX);

/// The play state's side of a connection (conn.rs's `PlayConn`).
pub(super) trait Session: Send {
    /// A packet arrived.
    fn packet(&mut self, id: i32, payload: Vec<u8>) -> Verdict;
    /// A packet to send of its own accord now (a keep-alive).
    fn tick(&mut self, now: Instant) -> Option<Vec<u8>>;
    /// The connection is ending, for this reason: no packet arrives after it.
    fn left(&mut self, reason: String);
    /// The connection is closed, after [`Self::left`].
    fn closed(&mut self);
}

/// What becomes of a connection after one of its packets.
pub(super) enum Verdict {
    Go,
    /// Close it now, for this reason.
    Close(String),
    /// Send this body (a disconnect) after what is queued, then close, for this reason.
    Kick(String, Vec<u8>),
}

/// How an outbox ends, after the bodies queued before.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum End {
    /// Write everything queued (a disconnect last), then close.
    Disconnect,
    /// Close now: what is queued is dropped.
    Close,
}

/// Where an outbox's connection is.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum At {
    /// Not on its thread yet: the thread takes what waits when it arrives.
    #[default]
    Coming,
    Slot(usize),
    Closed,
}

#[derive(Default)]
struct Queue {
    bodies: Vec<Vec<u8>>,
    end: Option<End>,
    at: At,
    /// In its thread's list of outboxes to serve.
    listed: bool,
}

/// One player's packets on their way to its network thread.
pub(super) struct Outbox {
    queue: Mutex<Queue>,
    /// The bytes queued and not framed yet (`PlayerSender::pending`).
    pending: Arc<AtomicUsize>,
    worker: Arc<Worker>,
}
impl Outbox {
    /// An outbox on the least busy network thread, started the first time.
    pub(super) fn new(pending: Arc<AtomicUsize>) -> Result<Arc<Self>, String> {
        let workers = &pool()?.workers;
        let worker = workers
            .iter()
            .min_by_key(|w| w.load.load(Ordering::Relaxed))
            .expect("a pool has a thread");
        Ok(Arc::new(Self {
            queue: Mutex::default(),
            pending,
            worker: worker.clone(),
        }))
    }
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Queue `bodies` (already counted in `pending`), then `end`. Nothing
    /// follows an end, but a close overrides a disconnect still writing.
    /// Past [`MAX_PENDING`] the connection closes. The thread to wake, if
    /// this is the first news it has since it last looked.
    pub(super) fn push(&self, bodies: Vec<Vec<u8>>, end: Option<End>) -> Option<&Arc<Worker>> {
        let mut q = self.queue();
        match (q.at, q.end) {
            (At::Closed, _) => return None,
            (_, None) => {
                if q.bodies.is_empty() {
                    q.bodies = bodies;
                } else {
                    q.bodies.extend(bodies);
                }
                q.end = if self.pending.load(Ordering::Relaxed) > MAX_PENDING {
                    Some(End::Close)
                } else {
                    end
                };
            }
            (_, Some(End::Disconnect)) if end == Some(End::Close) => q.end = end,
            (_, Some(_)) => return None,
        }
        let At::Slot(slot) = q.at else {
            return None;
        };
        if std::mem::replace(&mut q.listed, true) {
            return None;
        }
        drop(q);
        self.worker
            .post(|i| i.serve.push(slot))
            .then_some(&self.worker)
    }
    /// [`Self::push`], waking the thread at once.
    pub(super) fn send(&self, bodies: Vec<Vec<u8>>, end: Option<End>) {
        if let Some(worker) = self.push(bodies, end) {
            worker.wake();
        }
    }
    /// What waits, for the thread.
    /// What waits, for the thread: the bodies and the end after them with
    /// `room` for them, else only a close. An end stays, so that nothing is
    /// queued after it.
    fn take(&self, room: bool) -> (Vec<Vec<u8>>, Option<End>) {
        let mut q = self.queue();
        q.listed = false;
        match room {
            true => (std::mem::take(&mut q.bodies), q.end),
            false => (Vec::new(), q.end.filter(|e| *e == End::Close)),
        }
    }
    fn attach(&self, slot: usize) {
        let mut q = self.queue();
        q.at = At::Slot(slot);
        q.listed = false;
    }
    fn closed(&self) {
        let mut q = self.queue();
        q.at = At::Closed;
        q.bodies = Vec::new();
        q.end = None;
    }
}

/// What a network thread is told between polls.
#[derive(Default)]
struct Inbox {
    /// Connections with something in their outbox.
    serve: Vec<usize>,
    /// Pipes with bytes to read.
    read: Vec<usize>,
    adopt: Vec<Adopted>,
}

/// One network thread, as the others see it.
pub(super) struct Worker {
    waker: Waker,
    inbox: Mutex<Inbox>,
    /// Its connections, to give a new one to the least busy.
    load: AtomicUsize,
}
impl Worker {
    /// Change the inbox: whether the thread needs waking for it (it was empty).
    fn post(&self, f: impl FnOnce(&mut Inbox)) -> bool {
        let mut inbox = self.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        let empty = inbox.serve.is_empty() && inbox.read.is_empty() && inbox.adopt.is_empty();
        f(&mut inbox);
        empty
    }
    pub(super) fn wake(&self) {
        if let Err(e) = self.waker.wake() {
            log::warn!("[lan] waking a network thread: {e}");
        }
    }
}

struct Pool {
    workers: Vec<Arc<Worker>>,
}

/// The pool, started the first time: one thread per two cores.
fn pool() -> Result<&'static Pool, String> {
    static POOL: OnceLock<Result<Pool, String>> = OnceLock::new();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().div_ceil(2));
    POOL.get_or_init(|| start(threads).map_err(|e| format!("network threads: {e}")))
        .as_ref()
        .map_err(Clone::clone)
}

fn start(threads: usize) -> std::io::Result<Pool> {
    let mut workers = Vec::new();
    for n in 0..threads {
        let poll = Poll::new()?;
        let worker = Arc::new(Worker {
            waker: Waker::new(poll.registry(), WAKER)?,
            inbox: Mutex::default(),
            load: AtomicUsize::new(0),
        });
        let mine = worker.clone();
        std::thread::Builder::new()
            .name(format!("horizon-net-{n}"))
            .spawn(move || run(poll, &mine))?;
        workers.push(worker);
    }
    Ok(Pool { workers })
}

/// A connection handed to its thread.
struct Adopted {
    socket: Socket,
    reader: ConnReader,
    writer: ConnWriter,
    outbox: Arc<Outbox>,
    session: Box<dyn Session>,
    timeout: Duration,
}

/// Serve a connection from now on, on its outbox's thread: `timeout` is how
/// long it may read nothing, or have a write wait.
pub(super) fn adopt(
    socket: Socket,
    (reader, writer): (ConnReader, ConnWriter),
    outbox: Arc<Outbox>,
    session: Box<dyn Session>,
    timeout: Duration,
) {
    let worker = outbox.worker.clone();
    worker.load.fetch_add(1, Ordering::Relaxed);
    let adopted = Adopted {
        socket,
        reader,
        writer,
        outbox,
        session,
        timeout,
    };
    if worker.post(|i| i.adopt.push(adopted)) {
        worker.wake();
    }
}

fn run(mut poll: Poll, worker: &Arc<Worker>) {
    let mut events = Events::with_capacity(1024);
    let mut conns: Vec<Option<Conn>> = Vec::new();
    let mut free: Vec<usize> = Vec::new();
    let mut touched: Vec<usize> = Vec::new();
    let mut sweep_at = Instant::now() + SWEEP;
    loop {
        let wait = sweep_at.saturating_duration_since(Instant::now());
        if let Err(e) = poll.poll(&mut events, Some(wait))
            && e.kind() != ErrorKind::Interrupted
        {
            log::warn!("[lan] network poll: {e}");
            std::thread::sleep(SWEEP);
        }
        let now = Instant::now();
        for event in &events {
            let i = event.token().0;
            let Some(Some(c)) = conns.get_mut(i) else {
                continue;
            };
            if event.is_readable() || event.is_read_closed() || event.is_error() {
                c.read(now);
            }
            if event.is_writable() || event.is_write_closed() || event.is_error() {
                c.serve(now);
            }
            touched.push(i);
        }
        let inbox =
            std::mem::take(&mut *worker.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        for adopted in inbox.adopt {
            let i = free.pop().unwrap_or(conns.len());
            if i == conns.len() {
                conns.push(None);
            }
            match Conn::register(adopted, poll.registry(), worker, i, now) {
                Ok(mut c) => {
                    c.outbox.attach(i);
                    c.read(now);
                    c.serve(now);
                    conns[i] = Some(c);
                    touched.push(i);
                }
                Err(()) => free.push(i),
            }
        }
        for i in inbox.read {
            if let Some(Some(c)) = conns.get_mut(i) {
                c.read(now);
                touched.push(i);
            }
        }
        for i in inbox.serve {
            if let Some(Some(c)) = conns.get_mut(i) {
                c.serve(now);
                touched.push(i);
            }
        }
        if now >= sweep_at {
            sweep_at = now + SWEEP;
            touched.clear();
            for (i, slot) in conns.iter_mut().enumerate() {
                if let Some(c) = slot {
                    c.sweep(now);
                    touched.push(i);
                }
            }
        }
        for i in touched.drain(..) {
            if conns[i].as_ref().is_some_and(|c| c.done(now)) {
                let c = conns[i].take().expect("checked");
                c.close(poll.registry(), worker);
                free.push(i);
            }
        }
    }
}

/// A connection's stream, polled.
enum Io {
    Tcp(mio::net::TcpStream),
    Memory(Pipe),
    WebSocket(Framed<mio::net::TcpStream>),
}
impl Read for Io {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.read(out),
            Self::Memory(p) => p.read(out),
            Self::WebSocket(f) => f.read(out),
        }
    }
}
impl Write for Io {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.write(data),
            Self::Memory(p) => p.write(data),
            Self::WebSocket(f) => f.write(data),
        }
    }
    /// A WebSocket's frames wait in it until the socket takes them: `WouldBlock` until then.
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::WebSocket(f) => f.flush(),
            _ => Ok(()),
        }
    }
}

/// Why a read error ends a connection: a client that closed it is
/// Connection.channelInactive without details, multiplayer.disconnect.generic.
fn read_error(e: std::io::Error) -> String {
    match e.kind() {
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => "Disconnected".to_owned(),
        _ => horizon_protocol::Error::from(e).to_string(),
    }
}

/// A connection on its thread.
struct Conn {
    io: Io,
    reader: ConnReader,
    writer: ConnWriter,
    /// Framed bytes, written from `written` on.
    out: Vec<u8>,
    written: usize,
    outbox: Arc<Outbox>,
    session: Box<dyn Session>,
    timeout: Duration,
    last_read: Instant,
    /// Since when `out` has waited on a client that reads nothing.
    stalled: Option<Instant>,
    /// [`Session::left`] was called: what arrives now is dropped.
    left: bool,
    /// A disconnect is in `out`: nothing more is taken from the outbox.
    draining: bool,
    close_at: Option<Instant>,
}
impl Conn {
    fn register(
        a: Adopted,
        registry: &Registry,
        worker: &Arc<Worker>,
        i: usize,
        now: Instant,
    ) -> Result<Self, ()> {
        let poll = |stream: std::net::TcpStream| {
            stream.set_nonblocking(true)?;
            let mut stream = mio::net::TcpStream::from_std(stream);
            let interest = Interest::READABLE | Interest::WRITABLE;
            registry.register(&mut stream, Token(i), interest)?;
            Ok::<_, std::io::Error>(stream)
        };
        let registered = match a.socket {
            Socket::Tcp(stream) => poll(stream).map(Io::Tcp),
            Socket::WebSocket(ws) => ws
                .take_parts()
                .and_then(|(stream, inbound)| Ok(Framed::new(poll(stream)?, inbound)))
                .map(Io::WebSocket),
            Socket::Memory(mut pipe) => {
                pipe.set_nonblocking(true);
                let worker = worker.clone();
                pipe.on_ready(move || {
                    if worker.post(|inbox| inbox.read.push(i)) {
                        worker.wake();
                    }
                });
                Ok(Io::Memory(pipe))
            }
        };
        let io = match registered {
            Ok(io) => io,
            Err(e) => {
                let mut session = a.session;
                a.outbox.closed();
                session.left(e.to_string());
                session.closed();
                worker.load.fetch_sub(1, Ordering::Relaxed);
                return Err(());
            }
        };
        Ok(Self {
            io,
            reader: a.reader,
            writer: a.writer,
            out: Vec::new(),
            written: 0,
            outbox: a.outbox,
            session: a.session,
            timeout: a.timeout,
            last_read: now,
            stalled: None,
            left: false,
            draining: false,
            close_at: None,
        })
    }

    fn left(&mut self, reason: String) {
        if !std::mem::replace(&mut self.left, true) {
            self.session.left(reason);
        }
    }
    /// Close now.
    fn end(&mut self, now: Instant, reason: impl Into<String>) {
        self.left(reason.into());
        self.close_at = Some(now);
    }
    fn done(&self, now: Instant) -> bool {
        self.close_at.is_some_and(|at| now >= at)
    }

    /// Read and hand over every packet that arrived.
    fn read(&mut self, now: Instant) {
        loop {
            match self.reader.buffered() {
                Ok(Some((id, payload))) => {
                    if !self.left {
                        match self.session.packet(id, payload) {
                            Verdict::Go => {}
                            Verdict::Close(reason) => return self.end(now, reason),
                            Verdict::Kick(reason, body) => self.kick(now, reason, &body),
                        }
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => return self.end(now, e.to_string()),
            }
            if self.close_at.is_some() {
                return;
            }
            match self.reader.read_from(&mut self.io) {
                Ok(0) => return self.end(now, "Disconnected"),
                Ok(_) => self.last_read = now,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return self.end(now, read_error(e)),
            }
        }
    }

    /// Frame `bodies` into `out`, `queued` ones counted off `pending`: false
    /// if one could not be.
    fn frame(&mut self, now: Instant, bodies: Vec<Vec<u8>>, queued: bool) -> bool {
        for body in bodies {
            if queued {
                self.outbox.pending.fetch_sub(body.len(), Ordering::Relaxed);
            }
            if let Err(e) = super::conn::frame_body(&mut self.writer, &body, &mut self.out) {
                self.end(now, e.to_string());
                return false;
            }
        }
        true
    }

    /// Send `disconnect` after everything queued, then close.
    fn kick(&mut self, now: Instant, reason: String, disconnect: &[u8]) {
        self.left(reason);
        if !self.draining {
            let (bodies, end) = self.outbox.take(true);
            if end == Some(End::Close) {
                return self.end(now, "Disconnected");
            }
            // The hub's own disconnect, if it was the last queued, does for this one.
            let ours = (end.is_none()).then(|| disconnect.to_vec());
            if !self.frame(now, bodies, true) || !self.frame(now, ours.into_iter().collect(), false)
            {
                return;
            }
            self.draining = true;
        }
        self.serve(now);
    }

    /// Write what is framed, and frame what the outbox holds, while the
    /// client takes it.
    fn serve(&mut self, now: Instant) {
        loop {
            if !self.flush(now) || self.close_at.is_some() {
                return;
            }
            // A close is seen even while the client takes nothing.
            let room = !self.draining && self.out.len() - self.written <= AHEAD;
            let (bodies, end) = self.outbox.take(room);
            if end == Some(End::Close) {
                return self.end(now, "Disconnected");
            }
            if bodies.is_empty() {
                break;
            }
            if !self.frame(now, bodies, true) {
                return;
            }
            self.draining = end == Some(End::Disconnect);
        }
        // Written means taken by the socket: a WebSocket's frames may still wait in it (`stalled`).
        if self.draining
            && self.written == self.out.len()
            && self.stalled.is_none()
            && self.close_at.is_none()
        {
            self.close_at = Some(now + LINGER);
        }
    }

    /// Write what is framed until the client takes no more: false once the
    /// connection failed.
    fn flush(&mut self, now: Instant) -> bool {
        if self.close_at.is_some_and(|at| at <= now) {
            return false;
        }
        while self.written < self.out.len() {
            match self.io.write(&self.out[self.written..]) {
                Ok(0) => {
                    self.end(now, "Disconnected");
                    return false;
                }
                Ok(n) => {
                    self.written += n;
                    self.stalled = None;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    self.stalled.get_or_insert(now);
                    return true;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.end(now, "Disconnected");
                    return false;
                }
            }
        }
        self.out.clear();
        self.written = 0;
        if self.out.capacity() > 4 * AHEAD {
            self.out.shrink_to(AHEAD);
        }
        match self.io.flush() {
            Ok(()) => self.stalled = None,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                self.stalled.get_or_insert(now);
            }
            Err(_) => {
                self.end(now, "Disconnected");
                return false;
            }
        }
        true
    }

    /// The connection's clocks: a keep-alive due, a client that sent nothing
    /// or took nothing for too long.
    fn sweep(&mut self, now: Instant) {
        if self.close_at.is_some() {
            return;
        }
        if self
            .stalled
            .is_some_and(|t| now.duration_since(t) > self.timeout)
        {
            return self.end(now, "Disconnected");
        }
        if !self.left && now.duration_since(self.last_read) > self.timeout {
            return self.end(now, horizon_protocol::Error::timed_out().to_string());
        }
        if !self.draining
            && let Some(body) = self.session.tick(now)
            && self.frame(now, vec![body], false)
        {
            self.serve(now);
        }
    }

    fn close(mut self, registry: &Registry, worker: &Worker) {
        match &mut self.io {
            Io::Tcp(stream) => {
                let _ = registry.deregister(stream);
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Io::WebSocket(framed) => {
                framed.close();
                let _ = registry.deregister(framed.socket());
                let _ = framed.socket().shutdown(std::net::Shutdown::Both);
            }
            Io::Memory(pipe) => {
                pipe.on_ready(|| {});
                pipe.shutdown();
            }
        }
        self.outbox.closed();
        self.left("Disconnected".to_owned());
        self.session.closed();
        worker.load.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::{
        connection::Connection,
        websocket::{
            WsStream,
            codec::{self, Role},
        },
    };
    use std::sync::mpsc::{Receiver, Sender, channel};

    #[derive(Debug, PartialEq)]
    enum Seen {
        Packet(i32, Vec<u8>),
        Left(String),
        Closed,
    }
    struct Probe {
        seen: Sender<Seen>,
        /// The packet id answered with a kick.
        kick_on: Option<i32>,
        /// Sent at the first sweep.
        unprompted: Option<Vec<u8>>,
    }
    impl Session for Probe {
        fn packet(&mut self, id: i32, payload: Vec<u8>) -> Verdict {
            let _ = self.seen.send(Seen::Packet(id, payload));
            if self.kick_on == Some(id) {
                Verdict::Kick("kicked".into(), body(0x7f, 3))
            } else {
                Verdict::Go
            }
        }
        fn tick(&mut self, _: Instant) -> Option<Vec<u8>> {
            self.unprompted.take()
        }
        fn left(&mut self, reason: String) {
            let _ = self.seen.send(Seen::Left(reason));
        }
        fn closed(&mut self) {
            let _ = self.seen.send(Seen::Closed);
        }
    }

    /// A packet body: a one-byte id, then `len` bytes.
    fn body(id: u8, len: usize) -> Vec<u8> {
        std::iter::once(id)
            .chain((0..len).map(|j| (j * 31 + usize::from(id)) as u8))
            .collect()
    }
    fn packet(body: &[u8]) -> (i32, Vec<u8>) {
        (i32::from(body[0]), body[1..].to_vec())
    }

    struct Served {
        outbox: Arc<Outbox>,
        pending: Arc<AtomicUsize>,
        seen: Receiver<Seen>,
    }
    impl Served {
        fn send(&self, bodies: Vec<Vec<u8>>, end: Option<End>) {
            let len: usize = bodies.iter().map(Vec::len).sum();
            self.pending.fetch_add(len, Ordering::Relaxed);
            self.outbox.send(bodies, end);
        }
        fn next(&self) -> Seen {
            self.seen.recv_timeout(Duration::from_secs(10)).unwrap()
        }
    }
    fn serve(
        server: Connection,
        probe: impl FnOnce(Sender<Seen>) -> Probe,
        timeout: Duration,
    ) -> Served {
        let pending = Arc::new(AtomicUsize::new(0));
        let outbox = Outbox::new(pending.clone()).unwrap();
        let (tx, seen) = channel();
        let (socket, reader, writer) = server.detach();
        adopt(
            socket,
            (reader, writer),
            outbox.clone(),
            Box::new(probe(tx)),
            timeout,
        );
        Served {
            outbox,
            pending,
            seen,
        }
    }
    fn quiet(seen: Sender<Seen>) -> Probe {
        Probe {
            seen,
            kick_on: None,
            unprompted: None,
        }
    }

    #[test]
    fn a_socket_carries_packets_both_ways_in_order_then_disconnects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut client = Connection::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let mut server = Connection::from_stream(listener.accept().unwrap().0).unwrap();
        for c in [&mut client, &mut server] {
            c.set_compression(64);
            c.enable_encryption(&[5; 16]);
        }
        // The second arrives with the first, before the pool has the connection.
        client.send(&body(1, 10)).unwrap();
        client.send(&body(2, 10)).unwrap();
        assert_eq!(server.recv().unwrap(), packet(&body(1, 10)));
        let served = serve(server, quiet, Duration::from_secs(30));
        assert_eq!(served.next(), Seen::Packet(2, body(2, 10)[1..].to_vec()));
        // Small, compressed and shared (past 8 KiB) frames.
        let bodies: Vec<Vec<u8>> = (0..60)
            .map(|i| body(i as u8 + 3, [0, 63, 64, 300, 9000, 70_000][i % 6]))
            .collect();
        served.send(bodies.clone(), None);
        for b in &bodies {
            assert_eq!(client.recv().unwrap(), packet(b));
        }
        for i in 0..20u8 {
            client.send(&body(i + 3, 50)).unwrap();
        }
        for i in 0..20u8 {
            assert_eq!(
                served.next(),
                Seen::Packet(i32::from(i + 3), body(i + 3, 50)[1..].to_vec())
            );
        }
        assert_eq!(served.pending.load(Ordering::Relaxed), 0);
        served.send(vec![body(0x1d, 5)], Some(End::Disconnect));
        assert_eq!(client.recv().unwrap(), packet(&body(0x1d, 5)));
        assert!(client.recv().unwrap_err().is_end_of_stream());
        assert_eq!(served.next(), Seen::Left("Disconnected".into()));
        assert_eq!(served.next(), Seen::Closed);
    }

    /// The client's end of a WebSocket, read frame by frame: what the server's frames say, not
    /// only their payload.
    struct WsClient {
        socket: std::net::TcpStream,
        raw: Vec<u8>,
        /// Binary payload not yet taken as Minecraft frames.
        data: Vec<u8>,
    }
    impl WsClient {
        fn send(&mut self, opcode: u8, payload: &[u8]) {
            let mut frame = Vec::new();
            codec::encode(&mut frame, opcode, payload, Role::Client);
            self.socket.write_all(&frame).unwrap();
        }
        /// A packet body framed as Minecraft frames it, uncompressed.
        fn frame(body: &[u8]) -> Vec<u8> {
            let mut framed = Vec::new();
            horizon_protocol::buf::write_varint(&mut framed, body.len() as i32);
            framed.extend_from_slice(body);
            framed
        }
        /// The next server frame: its opcode and payload; `None` at the end of the stream.
        fn next_frame(&mut self) -> Option<(u8, Vec<u8>)> {
            loop {
                if self.raw.len() >= 2 {
                    assert_eq!(self.raw[1] & 0x80, 0, "a server's frames are not masked");
                    let (len, at) = match self.raw[1] {
                        126 if self.raw.len() >= 4 => (
                            usize::from(u16::from_be_bytes([self.raw[2], self.raw[3]])),
                            4,
                        ),
                        127 if self.raw.len() >= 10 => (
                            u64::from_be_bytes(self.raw[2..10].try_into().unwrap()) as usize,
                            10,
                        ),
                        126 | 127 => (usize::MAX, 0),
                        n => (usize::from(n), 2),
                    };
                    if len != usize::MAX && self.raw.len() >= at + len {
                        let opcode = self.raw[0] & 0x0f;
                        let payload = self.raw[at..at + len].to_vec();
                        self.raw.drain(..at + len);
                        return Some((opcode, payload));
                    }
                }
                let mut chunk = [0u8; 65536];
                let n = self.socket.read(&mut chunk).unwrap();
                if n == 0 {
                    return None;
                }
                self.raw.extend_from_slice(&chunk[..n]);
            }
        }
        /// The next packet in the binary messages: control frames on the way go to `control`.
        fn next_packet(&mut self, control: &mut Vec<(u8, Vec<u8>)>) -> Option<(i32, Vec<u8>)> {
            loop {
                if let Some((prefix, len)) = horizon_protocol::frame::header(&self.data).unwrap()
                    && self.data.len() >= prefix + len
                {
                    let body: Vec<u8> = self.data.drain(..prefix + len).skip(prefix).collect();
                    return Some(packet(&body));
                }
                match self.next_frame()? {
                    (codec::BINARY | codec::CONTINUATION, payload) => self.data.extend(payload),
                    other => control.push(other),
                }
            }
        }
    }

    /// A WebSocket player in play: the packets sent with the upgrade arrive, a burst far past
    /// what the WebSocket buffers arrives whole and in order to a client that reads late, a ping
    /// gets its pong, and a disconnect ends with a close frame.
    #[test]
    fn a_websocket_carries_packets_and_answers_pings() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut client = WsClient {
            socket,
            raw: Vec::new(),
            data: Vec::new(),
        };
        let mut opening =
            b"GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
                .to_vec();
        for i in 1..=2 {
            codec::encode(
                &mut opening,
                codec::BINARY,
                &WsClient::frame(&body(i, 10)),
                Role::Client,
            );
        }
        client.socket.write_all(&opening).unwrap();
        let (ws, _) =
            WsStream::accept(listener.accept().unwrap().0, Duration::from_secs(5), &[]).unwrap();
        let served = serve(
            Connection::from_websocket(ws).unwrap(),
            quiet,
            Duration::from_secs(30),
        );
        assert_eq!(served.next(), Seen::Packet(1, body(1, 10)[1..].to_vec()));
        assert_eq!(served.next(), Seen::Packet(2, body(2, 10)[1..].to_vec()));
        let (head, _) =
            horizon_protocol::websocket::handshake::read_head(&mut client.socket).unwrap();
        assert!(head.starts_with("HTTP/1.1 101 "));

        // About 4 MiB: the client reads nothing for a while, so most of it waits.
        let bodies: Vec<Vec<u8>> = (0..120)
            .map(|i| body(i as u8, [0, 63, 300, 9000, 70_000, 100_000][i % 6]))
            .collect();
        served.send(bodies.clone(), None);
        std::thread::sleep(Duration::from_millis(300));
        client.send(codec::PING, b"still there?");
        let mut control = Vec::new();
        for b in &bodies {
            assert_eq!(client.next_packet(&mut control), Some(packet(b)));
        }
        assert_eq!(served.pending.load(Ordering::Relaxed), 0);
        // The pong comes once the pool reads the ping: it may trail the burst.
        while control.is_empty() {
            served.send(vec![body(0x30, 1)], None);
            assert_eq!(
                client.next_packet(&mut control),
                Some(packet(&body(0x30, 1)))
            );
        }
        assert_eq!(control, [(codec::PONG, b"still there?".to_vec())]);

        client
            .socket
            .write_all(&{
                let mut f = Vec::new();
                codec::encode(
                    &mut f,
                    codec::BINARY,
                    &WsClient::frame(&body(7, 3)),
                    Role::Client,
                );
                f
            })
            .unwrap();
        assert_eq!(served.next(), Seen::Packet(7, body(7, 3)[1..].to_vec()));
        served.send(vec![body(0x1d, 5)], Some(End::Disconnect));
        assert_eq!(
            client.next_packet(&mut control),
            Some(packet(&body(0x1d, 5)))
        );
        assert_eq!(client.next_frame().map(|f| f.0), Some(codec::CLOSE));
        assert_eq!(served.next(), Seen::Left("Disconnected".into()));
        assert_eq!(served.next(), Seen::Closed);
    }

    #[test]
    fn a_kick_goes_after_what_was_queued_and_ends_the_packets() {
        let (a, b) = Pipe::pair();
        let mut client = Connection::local(a);
        let probe = |seen| Probe {
            seen,
            kick_on: Some(9),
            unprompted: Some(body(0x21, 2)),
        };
        let served = serve(Connection::local(b), probe, Duration::from_secs(30));
        served.send(vec![body(3, 10)], None);
        assert_eq!(client.recv().unwrap(), packet(&body(3, 10)));
        // What the session sends of its own accord.
        assert_eq!(client.recv().unwrap(), packet(&body(0x21, 2)));
        client.send(&body(9, 1)).unwrap();
        client.send(&body(10, 1)).unwrap();
        assert_eq!(client.recv().unwrap(), packet(&body(0x7f, 3)));
        assert!(client.recv().is_err());
        assert_eq!(served.next(), Seen::Packet(9, body(9, 1)[1..].to_vec()));
        assert_eq!(served.next(), Seen::Left("kicked".into()));
        assert_eq!(served.next(), Seen::Closed);
        // Nothing after the kick, and nothing queued once closed.
        assert!(served.seen.try_recv().is_err());
        assert!(served.outbox.push(vec![body(4, 1)], None).is_none());
    }

    #[test]
    fn a_silent_client_times_out() {
        let (a, b) = Pipe::pair();
        let mut client = Connection::local(a);
        let served = serve(Connection::local(b), quiet, Duration::from_millis(300));
        assert_eq!(served.next(), Seen::Left("Timed out".into()));
        assert_eq!(served.next(), Seen::Closed);
        assert!(client.recv().is_err());
    }

    /// A client that reads nothing holds its disconnect unwritten; a close
    /// then ends the connection at once.
    #[test]
    fn a_close_ends_a_disconnect_the_client_does_not_take() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = Connection::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let server = Connection::from_stream(listener.accept().unwrap().0).unwrap();
        let served = serve(server, quiet, Duration::from_secs(30));
        // Fill the socket until the outbox waits. A fixed amount may all go
        // through: Windows takes a write of any size whole while its buffer
        // has room.
        let full = || {
            std::thread::sleep(Duration::from_millis(50));
            served.pending.load(Ordering::Relaxed) > 0
        };
        for i in 0..=u8::MAX {
            if full() && full() && full() {
                break;
            }
            assert!(i < u8::MAX, "the socket took everything");
            served.send(vec![body(i, 1 << 20)], None);
        }
        served.send(Vec::new(), Some(End::Disconnect));
        std::thread::sleep(Duration::from_millis(300));
        assert!(served.seen.try_recv().is_err(), "still writing");
        served.send(Vec::new(), Some(End::Close));
        assert_eq!(served.next(), Seen::Left("Disconnected".into()));
        assert_eq!(served.next(), Seen::Closed);
        drop(client);
    }

    #[test]
    fn a_close_or_too_much_queued_drops_what_waits() {
        for overflow in [false, true] {
            let (a, b) = Pipe::pair();
            let mut client = Connection::local(a);
            let served = serve(Connection::local(b), quiet, Duration::from_secs(30));
            if overflow {
                served.pending.store(MAX_PENDING, Ordering::Relaxed);
                served.send(vec![body(3, 10)], None);
            } else {
                served.send(Vec::new(), Some(End::Close));
            }
            assert!(client.recv().is_err());
            assert_eq!(served.next(), Seen::Left("Disconnected".into()));
            assert_eq!(served.next(), Seen::Closed);
        }
    }
}
