//! Synchronous binary WebSocket transport.
//!
//! `horizon-protocol` is blocking and splits each connection into a reader
//! and a writer used on different threads. `tungstenite`'s `WebSocket` needs
//! `&mut self` for both directions, so one actor thread owns it: it reads
//! frames into a channel and drains a second channel into binary frames. The
//! `WsStream` halves are cheap `Arc` clones over those channels, and a raw
//! `TcpStream` clone interrupts the actor on shutdown.
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{
    Message, WebSocket,
    handshake::server::{ErrorResponse, Request, Response},
    http::HeaderMap,
    protocol::WebSocketConfig,
    stream::MaybeTlsStream,
};

/// Cloudflare's edge buffers frames up to 64 KiB; larger packets are split.
pub const MAX_FRAME: usize = 64 * 1024;
/// The largest message a peer may send: one Minecraft frame (`frame::MAX_FRAME`
/// plus its three-byte length prefix). Our own end never sends more than
/// [`MAX_FRAME`] in one message; the limit is for a peer that is not us, so
/// that it cannot make this side buffer tungstenite's default of 64 MiB.
pub const MAX_MESSAGE: usize = crate::frame::MAX_FRAME + 3;
/// Frames tungstenite may keep for a peer that reads slowly before `write`
/// reports the buffer full and the actor waits.
const MAX_WRITE_BUFFER: usize = 16 * MAX_FRAME;
/// Bytes read from the peer but not yet taken by the reader: past this the
/// actor stops reading, and TCP tells the peer to wait, as a plain socket would.
const MAX_BACKLOG: usize = 64 * MAX_FRAME;
/// Origin-side chunk streaming budget per connection.
const BYTES_PER_SECOND: usize = 2 * 1024 * 1024;
/// Cloudflare closes idle WebSocket connections; ping well inside that.
const PING_INTERVAL: Duration = Duration::from_secs(30);

fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .max_write_buffer_size(MAX_WRITE_BUFFER)
}

/// The player's address as a trusted proxy on loopback reports it:
/// Cloudflare's `CF-Connecting-IP`, else the first hop of `X-Forwarded-For`,
/// else `X-Real-IP`. The caller decides whether the peer is trusted.
pub fn forwarded_address(headers: &HeaderMap) -> Option<IpAddr> {
    let parse = |text: &str| {
        let text = text.trim();
        let text = text
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
            .unwrap_or(text);
        text.parse::<IpAddr>().ok()
    };
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    header("cf-connecting-ip")
        .and_then(parse)
        .or_else(|| {
            header("x-forwarded-for")
                .and_then(|list| list.split(',').next())
                .and_then(parse)
        })
        .or_else(|| header("x-real-ip").and_then(parse))
}

struct WsReader {
    incoming: Receiver<Vec<u8>>,
    buffer: Vec<u8>,
    timeout: Option<Duration>,
}

struct WsWriter {
    outgoing: Sender<Outgoing>,
    pending: Vec<u8>,
}

struct Outgoing {
    bytes: Vec<u8>,
    done: Sender<()>,
}

struct Shared {
    reader: Mutex<WsReader>,
    writer: Mutex<WsWriter>,
    raw: TcpStream,
    stop: Arc<AtomicBool>,
    /// Bytes in `incoming` the reader has not taken yet ([`MAX_BACKLOG`]).
    backlog: Arc<AtomicUsize>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.raw.shutdown(Shutdown::Both);
    }
}

/// One end of a binary WebSocket, read and written like a byte stream.
#[derive(Clone)]
pub struct WsStream {
    shared: Arc<Shared>,
}

impl WsStream {
    /// Connects to `ws://` or `wss://` (TLS via rustls) and starts its actor.
    pub fn connect(url: &str) -> io::Result<Self> {
        let (ws, _response) = tungstenite::client::connect_with_config(url, Some(config()), 3)
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        let raw = match ws.get_ref() {
            MaybeTlsStream::Plain(stream) => stream.try_clone()?,
            MaybeTlsStream::Rustls(stream) => stream.sock.try_clone()?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported WebSocket transport",
                ));
            }
        };
        raw.set_nodelay(true)?;
        Ok(Self::spawn("client", ws, raw))
    }

    /// Completes the server handshake on a connection forwarded by
    /// `cloudflared tunnel run` (plaintext `ws://` on loopback). A peer that
    /// does not finish its upgrade request within `handshake_timeout` is
    /// dropped instead of holding the thread. Also returns the player's
    /// address as the proxy's headers report it ([`forwarded_address`]).
    pub fn accept(
        stream: TcpStream,
        handshake_timeout: Option<Duration>,
    ) -> io::Result<(Self, Option<IpAddr>)> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(handshake_timeout)?;
        stream.set_write_timeout(handshake_timeout)?;
        let handshake = stream.try_clone()?;
        let mut forwarded = None;
        // tungstenite's callback type returns its (large) refusal response by value.
        #[allow(clippy::result_large_err)]
        let headers = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            forwarded = forwarded_address(request.headers());
            Ok(response)
        };
        // The error carries the callback, which borrows `forwarded`: keep its text only.
        let ws = tungstenite::accept_hdr_with_config(handshake, headers, Some(config()))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        stream.set_nodelay(true)?;
        Ok((Self::spawn("server", ws, stream), forwarded))
    }

    fn spawn<S: Read + Write + Send + 'static>(
        role: &'static str,
        mut ws: WebSocket<S>,
        raw: TcpStream,
    ) -> Self {
        let (incoming_tx, incoming) = mpsc::channel();
        let (outgoing, outgoing_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let backlog = Arc::new(AtomicUsize::new(0));
        let (actor_stop, actor_backlog) = (stop.clone(), backlog.clone());
        raw.set_nonblocking(true).ok();
        thread::Builder::new()
            .name(format!("horizon-websocket-{role}"))
            .spawn(move || {
                actor(
                    &mut ws,
                    outgoing_rx,
                    incoming_tx,
                    &actor_stop,
                    &actor_backlog,
                )
            })
            .expect("websocket actor");
        Self {
            shared: Arc::new(Shared {
                reader: Mutex::new(WsReader {
                    incoming,
                    buffer: Vec::new(),
                    timeout: None,
                }),
                writer: Mutex::new(WsWriter {
                    outgoing,
                    pending: Vec::new(),
                }),
                raw,
                stop,
                backlog,
            }),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.shared.reader.lock().unwrap().timeout = timeout;
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = self.shared.raw.shutdown(Shutdown::Both);
    }

    /// The TCP peer (Cloudflare edge for `wss://`, the origin for `ws://`).
    pub fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.shared.raw.peer_addr()
    }
}

fn actor<S: Read + Write>(
    ws: &mut WebSocket<S>,
    outgoing: Receiver<Outgoing>,
    incoming: Sender<Vec<u8>>,
    stop: &AtomicBool,
    backlog: &AtomicUsize,
) {
    let mut window = Instant::now();
    let mut sent = 0usize;
    let mut last_ping = Instant::now();
    let mut outgoing_open = true;
    let mut queue: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
    // Each `flush` waits for its batch to reach tungstenite's write buffer.
    let mut batches: std::collections::VecDeque<(usize, Sender<()>)> =
        std::collections::VecDeque::new();
    let mut acks: Vec<Sender<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        if window.elapsed() >= Duration::from_secs(1) {
            window = Instant::now();
            sent = 0;
        }
        if queue.is_empty() && outgoing_open {
            match outgoing.try_recv() {
                Ok(Outgoing { bytes, done }) => {
                    let chunks: Vec<Vec<u8>> = bytes
                        .chunks(MAX_FRAME)
                        .map(|chunk| chunk.to_vec())
                        .collect();
                    if chunks.is_empty() {
                        acks.push(done);
                    } else {
                        batches.push_back((chunks.len(), done));
                        queue.extend(chunks);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => outgoing_open = false,
            }
        }
        let mut queued = false;
        // Cloudflare-friendly frame size and a 2 MB/s origin budget.
        if sent < BYTES_PER_SECOND
            && let Some(chunk) = queue.front().cloned()
        {
            match ws.write(Message::binary(chunk.clone())) {
                Ok(()) => {
                    sent += chunk.len();
                    queue.pop_front();
                    queued = true;
                }
                Err(tungstenite::Error::WriteBufferFull(_)) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Tungstenite keeps the frame and retries it on the next
                    // write or flush.
                    sent += chunk.len();
                    queue.pop_front();
                    queued = true;
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return;
                }
                Err(_) => return,
            }
            if queued && let Some((remaining, _)) = batches.front_mut() {
                *remaining -= 1;
                if *remaining == 0
                    && let Some((_, done)) = batches.pop_front()
                {
                    acks.push(done);
                }
            }
        } else if sent >= BYTES_PER_SECOND {
            thread::sleep(Duration::from_millis(10));
        }
        match ws.flush() {
            Ok(()) => {
                for done in acks.drain(..) {
                    let _ = done.send(());
                }
            }
            Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return,
            Err(_) => return,
        }
        // A reader that lags leaves its bytes in the socket, not in memory.
        if backlog.load(Ordering::Acquire) >= MAX_BACKLOG {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        match ws.read() {
            Ok(Message::Binary(data)) => {
                backlog.fetch_add(data.len(), Ordering::AcqRel);
                if incoming.send(data.to_vec()).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                if last_ping.elapsed() >= PING_INTERVAL {
                    let _ = ws.write(Message::Ping(Vec::new().into()));
                    let _ = ws.flush();
                    last_ping = Instant::now();
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return,
            Err(_) => return,
        }
    }
}

impl Read for WsStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.shared.reader.lock().unwrap();
        if reader.buffer.is_empty() {
            let data = match reader.timeout {
                None => match reader.incoming.recv() {
                    Ok(data) => data,
                    Err(_) => return Ok(0),
                },
                Some(timeout) => match reader.incoming.recv_timeout(timeout) {
                    Ok(data) => data,
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(io::ErrorKind::WouldBlock.into());
                    }
                    Err(RecvTimeoutError::Disconnected) => return Ok(0),
                },
            };
            self.shared.backlog.fetch_sub(data.len(), Ordering::AcqRel);
            reader.buffer = data;
        }
        let n = out.len().min(reader.buffer.len());
        out[..n].copy_from_slice(&reader.buffer[..n]);
        reader.buffer.drain(..n);
        Ok(n)
    }
}

impl Write for WsStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.shared
            .writer
            .lock()
            .unwrap()
            .pending
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let (done, ack) = mpsc::channel();
        let bytes = {
            let mut writer = self.shared.writer.lock().unwrap();
            if writer.pending.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut writer.pending)
        };
        self.shared
            .writer
            .lock()
            .unwrap()
            .outgoing
            .send(Outgoing { bytes, done })
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        ack.recv_timeout(Duration::from_secs(30))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "websocket closed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_stream_round_trips_across_64k_frames() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let (mut ws, forwarded) = WsStream::accept(stream, None).unwrap();
            assert_eq!(forwarded, None, "no proxy header on a direct connection");
            let mut request = vec![0u8; 200_000];
            ws.read_exact(&mut request).unwrap();
            let expected: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
            assert_eq!(request, expected);
            ws.write_all(b"pong").unwrap();
            ws.flush().unwrap();
        });

        let mut client = WsStream::connect(&format!("ws://{address}")).unwrap();
        let request: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        client.write_all(&request).unwrap();
        client.flush().unwrap();
        let mut reply = [0u8; 4];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong");
        server.join().unwrap();
    }

    #[test]
    fn a_peer_that_never_finishes_its_handshake_is_dropped() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let _silent = TcpStream::connect(address).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let result = WsStream::accept(stream, Some(Duration::from_millis(200)));
        assert!(result.is_err(), "a silent socket must not become a stream");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the handshake waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_message_past_the_limit_closes_the_connection() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let (mut ws, _) = WsStream::accept(stream, None).unwrap();
            ws.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut byte = [0u8; 1];
            // The oversized message is refused: the stream ends without data.
            ws.read(&mut byte)
        });
        // A raw tungstenite client with no limits of its own sends one
        // message larger than MAX_MESSAGE.
        let (mut raw, _) = tungstenite::connect(format!("ws://{address}")).unwrap();
        let _ = raw.send(Message::binary(vec![0u8; MAX_MESSAGE + 1]));
        let _ = raw.flush();
        let read = server.join().unwrap();
        assert!(matches!(read, Ok(0) | Err(_)), "got {read:?}");
    }

    #[test]
    fn the_forwarded_address_comes_from_the_proxy_headers() {
        let mut headers = HeaderMap::new();
        assert_eq!(forwarded_address(&headers), None);
        headers.insert("x-real-ip", "10.0.0.9".parse().unwrap());
        assert_eq!(forwarded_address(&headers), "10.0.0.9".parse().ok());
        headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(forwarded_address(&headers), "203.0.113.7".parse().ok());
        headers.insert("cf-connecting-ip", "[2001:db8::1]".parse().unwrap());
        assert_eq!(forwarded_address(&headers), "2001:db8::1".parse().ok());
        headers.insert("cf-connecting-ip", "not an address".parse().unwrap());
        assert_eq!(forwarded_address(&headers), "203.0.113.7".parse().ok());
    }
}
