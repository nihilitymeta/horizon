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
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

/// Cloudflare's edge buffers frames up to 64 KiB; larger packets are split.
pub const MAX_FRAME: usize = 64 * 1024;
/// Origin-side chunk streaming budget per connection.
const BYTES_PER_SECOND: usize = 2 * 1024 * 1024;
/// Cloudflare closes idle WebSocket connections; ping well inside that.
const PING_INTERVAL: Duration = Duration::from_secs(30);

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
        let (ws, _response) = tungstenite::connect(url)
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
    /// `cloudflared tunnel run` (plaintext `ws://` on loopback).
    pub fn accept(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(false)?;
        let handshake = stream.try_clone()?;
        let ws = tungstenite::accept(handshake)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        stream.set_nodelay(true)?;
        Ok(Self::spawn("server", ws, stream))
    }

    fn spawn<S: Read + Write + Send + 'static>(
        role: &'static str,
        mut ws: WebSocket<S>,
        raw: TcpStream,
    ) -> Self {
        let (incoming_tx, incoming) = mpsc::channel();
        let (outgoing, outgoing_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let actor_stop = stop.clone();
        raw.set_nonblocking(true).ok();
        thread::Builder::new()
            .name(format!("horizon-websocket-{role}"))
            .spawn(move || actor(&mut ws, outgoing_rx, incoming_tx, &actor_stop))
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
        match ws.read() {
            Ok(Message::Binary(data)) => {
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
            let mut ws = WsStream::accept(stream).unwrap();
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
}
