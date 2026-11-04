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

/// A server's WebSocket on a socket that doesn't block, owned by one thread: what is written is framed
/// and buffered, and goes out as the socket takes it ([`Write::flush`]).
pub struct Framed<S> {
    socket: S,
    inbound: Inbound,
    out: Vec<u8>,
    sent: usize,
}

impl<S: Read + Write> Framed<S> {
    /// Goes on with a connection a [`WsStream`] opened ([`WsStream::into_parts`]).
    pub fn new(socket: S, inbound: Inbound) -> Self {
        Self {
            socket,
            inbound,
            out: Vec::new(),
            sent: 0,
        }
    }

    pub fn socket(&mut self) -> &mut S {
        &mut self.socket
    }

    /// Queues a close frame and tries to send what is buffered.
    pub fn close(&mut self) {
        codec::encode_close(&mut self.out, codec::NORMAL, self.inbound.role());
        let _ = self.drain();
    }

    fn pending(&self) -> usize {
        self.out.len() - self.sent
    }

    /// Writes what is buffered until it is all out or the socket takes no more (`WouldBlock`).
    fn drain(&mut self) -> io::Result<()> {
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
        Ok(())
    }
}

impl<S: Read + Write> Read for Framed<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut replies = Vec::new();
        loop {
            let read = self.inbound.read(&mut self.socket, out, &mut replies);
            if !replies.is_empty() {
                self.out.append(&mut replies);
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
}
