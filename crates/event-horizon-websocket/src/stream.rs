//! A WebSocket read and written as a byte stream, in two shapes over the same [`Inbound`] state:
//! [`WsStream`], blocking, whose clones read on one thread while another writes (a client's network
//! thread and its game thread, a server's login thread), over TCP or TLS; and [`Framed`], owned by one
//! poller's thread, which never blocks and buffers what the socket won't take yet.

use super::{
    codec::{self, Decoder, Role},
    handshake::{self, Answer, Head, Trusted, Url},
};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

/// What arrived from the peer and wasn't read yet.
pub struct Inbound {
    decoder: Decoder,
    /// Bytes read past the handshake's head, not decoded yet.
    raw: Vec<u8>,
    data: Vec<u8>,
    at: usize,
}

impl Inbound {
    fn new(role: Role, raw: Vec<u8>) -> Self {
        Self {
            decoder: Decoder::new(role),
            raw,
            data: Vec::new(),
            at: 0,
        }
    }

    /// Reads payload into `out`, reading `source` once its decoded bytes run out: 0 at the end of the
    /// stream (the socket's, or the peer's close). Frames that answer the peer go to `replies`, also when
    /// this fails; a peer that broke the protocol gets a close that says so. `None` when replies came
    /// before any payload: they are to be sent now, not after the next payload, then this called again.
    fn read(
        &mut self,
        source: &mut impl Read,
        out: &mut [u8],
        replies: &mut Vec<u8>,
    ) -> io::Result<Option<usize>> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if self.at < self.data.len() {
                let n = out.len().min(self.data.len() - self.at);
                out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
                self.at += n;
                if self.at == self.data.len() {
                    self.data.clear();
                    self.at = 0;
                }
                return Ok(Some(n));
            }
            if !replies.is_empty() {
                return Ok(None);
            }
            if self.decoder.closed() || out.is_empty() {
                return Ok(Some(0));
            }
            let leftovers;
            let raw = if self.raw.is_empty() {
                let n = source.read(&mut chunk)?;
                if n == 0 {
                    return Ok(Some(0));
                }
                &chunk[..n]
            } else {
                // Once: the handshake's leftovers.
                leftovers = std::mem::take(&mut self.raw);
                &leftovers[..]
            };
            let decoded = self.decoder.decode(raw, &mut self.data, replies);
            if let Err(refused) = decoded {
                codec::encode_close(replies, refused.code, self.role());
                return Err(io::Error::new(io::ErrorKind::InvalidData, refused));
            }
        }
    }

    /// This side's role: the frames it sends are this role's.
    fn role(&self) -> Role {
        self.decoder.role()
    }
}

/// The halves of a [`WsStream`]: the reading side with what arrived, the writing side.
struct Shared {
    reader: Mutex<(Box<dyn Read + Send>, Inbound)>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// The socket under both halves: timeouts and shutdown.
    socket: TcpStream,
    role: Role,
    /// Set for a `wss://` connection.
    secure: bool,
}

/// A blocking WebSocket carrying a byte stream. Clones share it: one may read while another writes.
#[derive(Clone)]
pub struct WsStream {
    shared: Arc<Shared>,
}

impl WsStream {
    /// Opens a WebSocket to `url` on `socket`, a TCP connection to its host: TLS first for `wss://`, then
    /// the upgrade. `timeout` bounds each read and write until the stream is open.
    pub fn connect(socket: TcpStream, url: &Url, timeout: Duration) -> io::Result<Self> {
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;
        let (mut reader, mut writer): (Box<dyn Read + Send>, Box<dyn Write + Send>) = if url.secure
        {
            let (read, write) = super::tls::client(&socket, &url.host)?;
            (Box::new(read), Box::new(write))
        } else {
            (Box::new(socket.try_clone()?), Box::new(socket.try_clone()?))
        };
        let key = handshake::new_key();
        writer.write_all(handshake::request(url, &key).as_bytes())?;
        writer.flush()?;
        let (head, rest) = handshake::read_head(&mut reader)?;
        handshake::check_response(&head, &key)?;
        socket.set_read_timeout(None)?;
        socket.set_write_timeout(None)?;
        Ok(Self::over(
            socket,
            reader,
            writer,
            Role::Client,
            rest,
            url.secure,
        ))
    }

    /// Answers a client's upgrade on `socket`, accepted by a server: also the player's address when a
    /// proxy among `trusted` handed it over ([`handshake::forwarded_address`]). The request must arrive
    /// within `deadline`, however slowly its bytes come. A request that isn't an upgrade gets an HTTP
    /// error, and the error here.
    pub fn accept(
        socket: TcpStream,
        deadline: Duration,
        trusted: &[Trusted],
    ) -> io::Result<(Self, Option<IpAddr>)> {
        let peer = socket.peer_addr()?.ip();
        socket.set_write_timeout(Some(deadline))?;
        let mut timed = Deadline::new(socket.try_clone()?, deadline);
        let (head, rest) = handshake::read_head(&mut timed)?;
        let head = Head::parse(&head)?;
        let mut writer = socket.try_clone()?;
        match handshake::answer(&head) {
            Answer::Open(response) => writer.write_all(response.as_bytes())?,
            Answer::Refuse(response, why) => {
                let _ = writer.write_all(response.as_bytes());
                let _ = socket.shutdown(Shutdown::Both);
                return Err(io::Error::new(io::ErrorKind::InvalidData, why));
            }
        }
        socket.set_read_timeout(None)?;
        socket.set_write_timeout(None)?;
        let forwarded = handshake::forwarded_address(&head, peer, trusted);
        let reader = Box::new(socket.try_clone()?);
        let stream = Self::over(socket, reader, Box::new(writer), Role::Server, rest, false);
        Ok((stream, forwarded))
    }

    fn over(
        socket: TcpStream,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        role: Role,
        rest: Vec<u8>,
        secure: bool,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                reader: Mutex::new((reader, Inbound::new(role, rest))),
                writer: Mutex::new(writer),
                socket,
                role,
                secure,
            }),
        }
    }

    /// Whether it runs over TLS (`wss://`).
    pub fn is_secure(&self) -> bool {
        self.shared.secure
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.shared.socket.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.shared.socket.set_write_timeout(timeout)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.shared.socket.peer_addr()
    }

    /// Closes the connection: a close frame first when no write holds the writing side, then the socket.
    /// A read or a write blocked on it fails.
    pub fn shutdown(&self) {
        if let Ok(mut writer) = self.shared.writer.try_lock() {
            let mut close = Vec::new();
            codec::encode_close(&mut close, codec::NORMAL, self.shared.role);
            let _ = writer.write_all(&close).and_then(|()| writer.flush());
        }
        let _ = self.shared.socket.shutdown(Shutdown::Both);
    }

    /// The socket and what arrived on it, for a poller to go on with ([`Framed`]): a server's plain
    /// connection, which nothing reads from now on.
    pub fn take_parts(&self) -> io::Result<(TcpStream, Inbound)> {
        if self.shared.secure {
            return Err(io::Error::other("a TLS WebSocket stays blocking"));
        }
        let socket = self.shared.socket.try_clone()?;
        let mut reader = self.shared.reader.lock().unwrap_or_else(|e| e.into_inner());
        let inbound = std::mem::replace(&mut reader.1, Inbound::new(self.shared.role, Vec::new()));
        Ok((socket, inbound))
    }

    fn send(&self, frames: &[u8]) -> io::Result<()> {
        let mut writer = self.shared.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(frames)?;
        writer.flush()
    }
}

impl Read for WsStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.shared.reader.lock().unwrap_or_else(|e| e.into_inner());
        let (source, inbound) = &mut *reader;
        let mut replies = Vec::new();
        loop {
            let read = inbound.read(source, out, &mut replies);
            if !replies.is_empty() {
                // A pong that can't go out now fails the next write the same way.
                let _ = self.send(&replies);
                replies.clear();
            }
            match read {
                Ok(None) => continue,
                Ok(Some(n)) => return Ok(n),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Write for WsStream {
    /// Sends `data` whole, as binary messages: it returns once they are written.
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut frames = Vec::with_capacity(data.len() + 16);
        codec::encode_binary(&mut frames, data, self.shared.role);
        self.send(&frames)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Bytes buffered for the socket past which [`Framed::write`] takes no more: about four chunk packets.
const HIGH_WATER: usize = 256 * 1024;

/// Bytes the socket has taken from `out` past which [`Framed::drain`] drops them from the buffer.
const COMPACT_AT: usize = 64 * 1024;

/// A server's WebSocket on a socket that doesn't block, owned by one thread: what is written is framed
/// and buffered, and goes out as the socket takes it ([`Write::flush`]).
pub struct Framed<S> {
    socket: S,
    inbound: Inbound,
    out: Vec<u8>,
    sent: usize,
    /// The latest ping's pong, answered once the earlier control bytes and data frames are out
    /// (RFC 6455 5.5.3 answers only the most recent ping). A ping flood holds at most this one pong
    /// here, however much data waits ahead of it in `out`. Bounded: when `out` is empty and the
    /// socket takes nothing, an earlier pong may already sit in `out`, so at most two pongs are
    /// buffered (one there, one here), never one per ping.
    pending_pong: Option<Vec<u8>>,
    closed: bool,
}

impl<S: Read + Write> Framed<S> {
    /// Goes on with a connection a [`WsStream`] opened ([`WsStream::into_parts`]).
    pub fn new(socket: S, inbound: Inbound) -> Self {
        Self {
            socket,
            inbound,
            out: Vec::new(),
            sent: 0,
            pending_pong: None,
            closed: false,
        }
    }

    pub fn socket(&mut self) -> &mut S {
        &mut self.socket
    }

    /// Queues a close frame and tries to send what is buffered. The pong still owed goes ahead
    /// of it: no data frames go out after a close (RFC 6455 5.5.1), but a pong for a ping that
    /// arrived before any close is still owed (5.5.2).
    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.flush_pong_before_close();
            codec::encode_close(&mut self.out, codec::NORMAL, self.inbound.role());
            let _ = self.drain();
        }
    }

    /// Moves the pending pong into `out`, ahead of a close frame about to be queued there.
    fn flush_pong_before_close(&mut self) {
        if let Some(pong) = self.pending_pong.take() {
            self.out.extend_from_slice(&pong);
        }
    }

    pub fn pending(&self) -> usize {
        self.out.len() - self.sent + self.pending_pong.as_ref().map_or(0, Vec::len)
    }

    /// Writes what is buffered until it is all out or the socket takes no more (`WouldBlock`). The
    /// pending pong moves into `out` only once the bytes already there are all out, so it never
    /// jumps the queue ahead of them. Buffered pongs stay bounded: at most one in `out` and one
    /// pending (see `pending_pong`), however many pings arrive.
    fn drain(&mut self) -> io::Result<()> {
        // A sent prefix that is large, or more than half the buffer, goes now: a socket that takes
        // only part of each write must not keep every byte it has taken.
        if self.sent >= COMPACT_AT || self.sent * 2 > self.out.len() {
            self.out.drain(..self.sent);
            self.sent = 0;
        }
        loop {
            while self.sent < self.out.len() {
                match self.socket.write(&self.out[self.sent..]) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(n) => self.sent += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            self.out.clear();
            self.sent = 0;
            if self.out.capacity() > 4 * HIGH_WATER {
                self.out.shrink_to(HIGH_WATER);
            }
            let Some(pong) = self.pending_pong.take() else {
                return Ok(());
            };
            self.out.extend_from_slice(&pong);
        }
    }

    /// Splits the codec's replies into the latest pong (only it is answered) and the rest (close
    /// frames, which still go out now). Anything unparsable is kept with the rest, never dropped:
    /// a close frame must not be lost to a parsing miss.
    fn split_replies(replies: &[u8]) -> (Option<Vec<u8>>, Vec<u8>) {
        let mut pong = None;
        let mut other = Vec::new();
        let mut at = 0;
        while at + 2 <= replies.len() {
            let opcode = replies[at] & 0x0f;
            let masked = replies[at + 1] & 0x80 != 0;
            let (len, mut head) = match replies[at + 1] & 0x7f {
                126 => {
                    if at + 4 > replies.len() {
                        break;
                    }
                    (
                        usize::from(u16::from_be_bytes([replies[at + 2], replies[at + 3]])),
                        4,
                    )
                }
                127 => {
                    if at + 10 > replies.len() {
                        break;
                    }
                    // The codec puts no 64-bit length in `replies` (control payloads are at most 125
                    // bytes): this only keeps a length past `usize` from truncating.
                    let len =
                        u64::from_be_bytes(replies[at + 2..at + 10].try_into().expect("8 bytes"));
                    (usize::try_from(len).unwrap_or(usize::MAX), 10)
                }
                n => (usize::from(n), 2),
            };
            if masked {
                head += 4;
            }
            let Some(end) = at.checked_add(head).and_then(|at| at.checked_add(len)) else {
                break;
            };
            if end > replies.len() {
                break;
            }
            if opcode == codec::PONG {
                pong = Some(replies[at..end].to_vec());
            } else {
                other.extend_from_slice(&replies[at..end]);
            }
            at = end;
        }
        if at < replies.len() {
            other.extend_from_slice(&replies[at..]);
        }
        (pong, other)
    }
}

impl<S: Read + Write> Read for Framed<S> {
    /// Reads payload into `out`. After [`Framed::close`] this keeps parsing what the peer sends:
    /// 0 comes when the peer's close frame arrives or the socket ends, and a write the peer can't
    /// take yet fails the same way it would have before the close.
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut replies = Vec::new();
        loop {
            let read = self.inbound.read(&mut self.socket, out, &mut replies);
            if self.closed {
                // Our close is queued, and nothing may follow it (RFC 6455 5.5.1): the peer's frames
                // are still parsed to the end, but their replies (pongs, a second close) are dropped.
                replies.clear();
            }
            if !replies.is_empty() {
                // Only the most recent ping is answered: earlier pongs are dropped, while close
                // frames still go out now. A peer that breaks the protocol still gets the close
                // frame the codec encoded for it, with this read's error.
                let (pong, other) = Self::split_replies(&replies);
                replies.clear();
                if !other.is_empty() {
                    // A close is about to be queued: the pong still owed goes ahead of it, and
                    // after it nothing is ever queued, so a ping that arrived too late is dropped.
                    // `other` holds only close frames (the echo, or a refusal's close), so the
                    // stream is closed from here on: writes fail, and a later close queues nothing.
                    self.closed = true;
                    if let Some(pong) = pong {
                        self.pending_pong = Some(pong);
                    }
                    self.flush_pong_before_close();
                    self.out.extend_from_slice(&other);
                } else if let Some(pong) = pong {
                    self.pending_pong = Some(pong);
                }
                // What the socket won't take now goes with the next flush.
                let _ = self.drain();
            }
            match read {
                Ok(None) => continue,
                Ok(Some(n)) => return Ok(n),
                Err(e) => return Err(e),
            }
        }
    }
}

impl<S: Read + Write> Write for Framed<S> {
    /// Takes up to [`codec::MAX_OUT_FRAME`] bytes of `data` as one binary message, unless the buffer is
    /// full and the socket takes none of it (`WouldBlock`, so a poller waits for it to be writable).
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if data.is_empty() {
            return Ok(0);
        }
        if self.pending() >= HIGH_WATER {
            self.drain()?;
        }
        let n = data.len().min(codec::MAX_OUT_FRAME);
        codec::encode(&mut self.out, codec::BINARY, &data[..n], Role::Server);
        match self.drain() {
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => Err(e),
            _ => Ok(n),
        }
    }

    /// Writes what is buffered: `WouldBlock` while the socket takes no more.
    fn flush(&mut self) -> io::Result<()> {
        self.drain()
    }
}

/// A socket whose reads, together, must finish within a deadline: a peer that trickles a byte at a
/// time can't hold the connection open longer.
struct Deadline {
    socket: TcpStream,
    until: std::time::Instant,
}

impl Deadline {
    fn new(socket: TcpStream, deadline: Duration) -> Self {
        Self {
            socket,
            until: std::time::Instant::now() + deadline,
        }
    }
}

impl Read for Deadline {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let left = self
            .until
            .saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.socket.set_read_timeout(Some(left))?;
        self.socket.read(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read that waits for payload still answers the pings that arrive meanwhile: clients and
    /// proxies that ping an idle connection close it when no pong comes.
    #[test]
    fn a_waiting_read_answers_pings() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .write_all(
                b"GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            )
            .unwrap();
        let (server, _) =
            WsStream::accept(listener.accept().unwrap().0, Duration::from_secs(5), &[]).unwrap();
        let reading = std::thread::spawn(move || {
            let mut server = server;
            let mut byte = [0u8; 1];
            server.read_exact(&mut byte).map(|()| byte[0])
        });
        let (head, rest) = handshake::read_head(&mut client).unwrap();
        assert!(head.starts_with("HTTP/1.1 101 ") && rest.is_empty());
        let mut ping = Vec::new();
        codec::encode(&mut ping, codec::PING, b"hi", Role::Client);
        client.write_all(&ping).unwrap();
        let mut pong = [0u8; 4];
        client.read_exact(&mut pong).unwrap();
        assert_eq!(pong, [0x8a, 2, b'h', b'i']);
        let mut data = Vec::new();
        codec::encode(&mut data, codec::BINARY, &[9], Role::Client);
        client.write_all(&data).unwrap();
        assert_eq!(reading.join().unwrap().unwrap(), 9);
    }

    /// A peer scripted in memory: `incoming` feeds reads, while writes are collected once
    /// `draining` and refused with `WouldBlock` before that, as a socket whose send buffer is full.
    struct ScriptedPeer {
        incoming: Vec<u8>,
        at: usize,
        draining: bool,
        written: Vec<u8>,
    }
    impl Read for ScriptedPeer {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.incoming.len() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(self.incoming.len() - self.at);
            buf[..n].copy_from_slice(&self.incoming[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }
    impl Write for ScriptedPeer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if !self.draining {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            if !self.draining {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            Ok(())
        }
    }

    /// A ping flood against a socket that never drains, with data queued ahead of the pongs (the
    /// reviewer's case): the bytes held stay a data prefix plus one pong, and only the last ping
    /// is answered once the socket takes bytes again.
    #[test]
    fn a_ping_flood_behind_queued_data_holds_one_pong() {
        let mut framed = Framed::new(
            ScriptedPeer {
                incoming: Vec::new(),
                at: 0,
                draining: false,
                written: Vec::new(),
            },
            Inbound::new(Role::Server, Vec::new()),
        );
        // Data queued ahead of the pongs: the socket takes nothing.
        let data = vec![0x11u8; 8 * 1024];
        assert_eq!(framed.write(&data).unwrap(), data.len());
        let queued = framed.pending();
        // 1000 pings with distinct 100-byte payloads: ~102 KiB of pongs on an unbounded stack.
        let mut incoming = Vec::new();
        for i in 0..1000u16 {
            let mut payload = [0x55u8; 100];
            payload[..2].copy_from_slice(&i.to_be_bytes());
            codec::encode(&mut incoming, codec::PING, &payload, Role::Client);
        }
        framed.socket().incoming = incoming;
        let mut buf = [0u8; 128];
        let err = framed
            .read(&mut buf)
            .expect_err("a lone ping flood carries no payload");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        // One control frame at most (a pong is 2 header bytes plus at most 125 payload bytes).
        assert!(
            framed.pending() <= queued + 2 + 125,
            "a ping flood held {} bytes over the queued {}",
            framed.pending() - queued,
            queued
        );
        // The socket takes bytes again: the queued data goes whole, with exactly one pong after
        // it, the last ping's.
        framed.socket().draining = true;
        framed.flush().unwrap();
        // Server frames are unmasked (RFC 6455 5.1). The 8 KiB binary message: 0x82, 126, its
        // 16-bit length 0x2000, then the payload.
        let mut expected = vec![0x82, 0x7e, 0x20, 0x00];
        expected.extend_from_slice(&data);
        // The last ping's pong: 0x8a, length 100, the ping's payload `999` (0x03 0xe7) and 98
        // bytes of 0x55.
        expected.extend_from_slice(&[0x8a, 100, 0x03, 0xe7]);
        expected.extend(std::iter::repeat_n(0x55u8, 98));
        assert_eq!(framed.socket().written, expected);
    }

    /// A lone ping gets its pong with the same payload, and after the server's close the peer's
    /// close is still parsed: its echo goes out and the read ends the stream.
    #[test]
    fn a_ping_gets_its_pong_and_a_close_after_close_is_parsed() {
        let mut framed = Framed::new(
            ScriptedPeer {
                // A ping `hello` masked by 37 fa 21 3d (RFC 6455 5.7 masks the same way).
                incoming: vec![
                    0x89, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x5f, 0x9f, 0x4d, 0x51, 0x58,
                ],
                at: 0,
                draining: true,
                written: Vec::new(),
            },
            Inbound::new(Role::Server, Vec::new()),
        );
        // A lone ping carries no payload, but its pong goes out on its own.
        let mut buf = [0u8; 64];
        let err = framed
            .read(&mut buf)
            .expect_err("a lone ping carries no payload");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            framed.socket().written,
            [0x8a, 5, b'h', b'e', b'l', b'l', b'o']
        );
        // The server's close goes out after the pong, and writes are refused from here on.
        framed.close();
        assert_eq!(
            framed.socket().written,
            [0x8a, 5, b'h', b'e', b'l', b'l', b'o', 0x88, 2, 0x03, 0xe8]
        );
        assert_eq!(
            framed.write(b"x").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        // The peer's close (1000, masked by 37 fa 21 3d) is still parsed after that: the read
        // ends, and nothing is sent back, not even an echo of the close.
        framed.socket().incoming = vec![0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x34, 0x12];
        framed.socket().at = 0;
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(
            framed.socket().written,
            [0x8a, 5, b'h', b'e', b'l', b'l', b'o', 0x88, 2, 0x03, 0xe8]
        );
        // The stream stays ended.
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
    }

    /// After our close, a read holding the peer's ping (`AB`) and close sends nothing back: no pong
    /// and no second close (RFC 6455 5.5.1).
    #[test]
    fn a_ping_and_close_after_our_close_get_no_reply() {
        let mut framed = scripted(Vec::new(), true);
        framed.close();
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
        framed.socket().incoming = vec![
            // Ping `AB`, masked by 37 fa 21 3d.
            0x89, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x76, 0xb8,
            // Close 1000, masked by 37 fa 21 3d.
            0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x34, 0x12,
        ];
        let mut buf = [0u8; 64];
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
    }

    /// A `Framed` over a scripted peer: `incoming` feeds reads, while writes are collected once
    /// `draining` and refused with `WouldBlock` before that.
    fn scripted(incoming: Vec<u8>, draining: bool) -> Framed<ScriptedPeer> {
        Framed::new(
            ScriptedPeer {
                incoming,
                at: 0,
                draining,
                written: Vec::new(),
            },
            Inbound::new(Role::Server, Vec::new()),
        )
    }

    /// Data queued and the socket refusing writes, a ping `AB`, then close(): the wire order is
    /// data, pong, close (RFC 6455 5.5.2 still owes the pong: no close was received first).
    #[test]
    fn close_sends_a_waiting_pong_ahead_of_the_close() {
        let mut framed = scripted(Vec::new(), false);
        assert_eq!(framed.write(b"data").unwrap(), 4);
        // A ping `AB` masked by 37 fa 21 3d.
        framed.socket().incoming = vec![0x89, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x76, 0xb8];
        let mut buf = [0u8; 64];
        assert_eq!(
            framed.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        framed.close();
        framed.socket().draining = true;
        framed.flush().unwrap();
        assert_eq!(
            framed.socket().written,
            [
                0x82, 4, b'd', b'a', b't', b'a', // the data, unmasked, as a server sends it
                0x8a, 2, 0x41, 0x42, // the pong `AB`
                0x88, 2, 0x03, 0xe8, // the close, 1000
            ]
        );
    }

    /// One read holding a ping `AB` and the peer's close sends the pong before the echoed close.
    #[test]
    fn a_ping_and_close_in_one_read_send_pong_then_close() {
        let incoming = vec![
            // Ping `AB` and close 1000, both masked by 37 fa 21 3d.
            0x89, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x76, 0xb8, 0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d,
            0x34, 0x12,
        ];
        let mut framed = scripted(incoming, true);
        let mut buf = [0u8; 64];
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(
            framed.socket().written,
            [
                0x8a, 2, 0x41, 0x42, // the pong `AB`
                0x88, 2, 0x03, 0xe8, // the echoed close, 1000
            ]
        );
    }

    /// Two pings and the peer's close in one read: only the latest ping (`CD`) is answered, and its
    /// pong still goes before the echoed close (RFC 6455 5.5.2 answers the most recent ping only).
    #[test]
    fn two_pings_and_close_in_one_read_send_only_the_last_pong() {
        let incoming = vec![
            // Ping `AB`, then ping `CD`, then close 1000, all masked by 37 fa 21 3d.
            0x89, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x76, 0xb8, //
            0x89, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x74, 0xbe, //
            0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x34, 0x12,
        ];
        let mut framed = scripted(incoming, true);
        let mut buf = [0u8; 64];
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(
            framed.socket().written,
            [
                0x8a, 2, 0x43, 0x44, // the pong `CD`, and no pong `AB`
                0x88, 2, 0x03, 0xe8, // the echoed close, 1000
            ]
        );
    }

    /// After a close is queued nothing is ever appended: a ping arriving later gets no pong.
    #[test]
    fn pings_after_a_queued_close_get_no_pong() {
        // The peer's close (1000, masked by 37 fa 21 3d), then its ping `late` (masked the same
        // way): the echo goes out, the ping gets nothing.
        let peer_close = vec![0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x34, 0x12];
        let late_ping = vec![0x89, 0x84, 0x37, 0xfa, 0x21, 0x3d, 0x5b, 0x9b, 0x55, 0x58];
        let mut framed = scripted(peer_close, true);
        let mut buf = [0u8; 64];
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
        framed.socket().incoming = late_ping.clone();
        framed.socket().at = 0;
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
        // Our close, then the peer's ping: the close goes out, the ping gets nothing.
        let mut framed = scripted(Vec::new(), true);
        framed.close();
        framed.socket().incoming = late_ping;
        assert_eq!(
            framed.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
    }

    /// A socket that takes all but the last byte of each write, and refuses a write of one byte.
    struct ShortPeer {
        written: Vec<u8>,
    }
    impl Read for ShortPeer {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }
    impl Write for ShortPeer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() < 2 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len() - 1;
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A socket that always leaves one byte unsent keeps the sent prefix in the buffer unless it is
    /// dropped as it goes: over many writes the buffer holds about one frame, not all of them.
    #[test]
    fn a_socket_short_by_one_byte_does_not_grow_the_buffer() {
        let mut framed = Framed::new(
            ShortPeer {
                written: Vec::new(),
            },
            Inbound::new(Role::Server, Vec::new()),
        );
        let data = vec![0x22u8; 1000];
        for _ in 0..1000 {
            assert_eq!(framed.write(&data).unwrap(), data.len());
            assert!(
                framed.out.len() <= 2 * HIGH_WATER,
                "the buffer grew to {} bytes",
                framed.out.len()
            );
        }
    }

    /// Every byte the short socket accepted, over many writes that each compact the buffer mid-frame,
    /// is the exact concatenation of the frames written, in order; the rest is still buffered.
    #[test]
    fn a_short_socket_takes_every_byte_of_every_frame_in_order() {
        let mut framed = Framed::new(
            ShortPeer {
                written: Vec::new(),
            },
            Inbound::new(Role::Server, Vec::new()),
        );
        let mut expected = Vec::new();
        let mut largest = 0;
        for round in 0..600u32 {
            let payload = vec![round as u8; 1000];
            assert_eq!(framed.write(&payload).unwrap(), 1000);
            // 1000 bytes are a 126-form binary frame: 0x82, 126, length 0x03e8, then the payload.
            expected.extend_from_slice(&[0x82, 0x7e, 0x03, 0xe8]);
            expected.extend_from_slice(&payload);
            largest = largest.max(framed.out.len());
        }
        // The buffer was compacted as it went: it never held more than the threshold plus a frame.
        assert!(
            largest <= COMPACT_AT + 2 * 1004,
            "the buffer held {largest} bytes"
        );
        let mut got = framed.socket().written.clone();
        got.extend_from_slice(&framed.out[framed.sent..]);
        assert_eq!(got, expected);
    }

    /// The peer's close is echoed by a read, which also closes the stream: a write after that is
    /// refused and no data frame goes out behind the echo.
    #[test]
    fn a_write_after_an_echoed_peer_close_is_refused() {
        // Close 1000 from the client, masked by 37 fa 21 3d.
        let mut framed = scripted(vec![0x88, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x34, 0x12], true);
        let mut buf = [0u8; 64];
        assert_eq!(framed.read(&mut buf).unwrap(), 0);
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
        assert_eq!(
            framed.write(b"x").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xe8]);
    }

    /// A peer that breaks the protocol gets one close (1002) from the read that refused it; a close
    /// after that queues no second one.
    #[test]
    fn a_close_after_a_protocol_error_close_sends_no_second_close() {
        // An unmasked ping from the client: a protocol error.
        let mut framed = scripted(vec![0x89, 0x02, 0x41, 0x42], true);
        let mut buf = [0u8; 64];
        assert_eq!(
            framed.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xea]);
        framed.close();
        assert_eq!(framed.socket().written, [0x88, 2, 0x03, 0xea]);
    }
}
