//! Minecraft over a WebSocket (RFC 6455), for tunnels and proxies that carry only HTTP, such as
//! Cloudflare Tunnel: the connection's bytes travel in binary messages, unchanged otherwise
//! (docs/websocket.md). A client connects to a `ws://` or `wss://` address; a server takes the upgrade
//! on its game port, beside plain connections.

pub mod codec;
pub mod handshake;
mod stream;
mod tls;

pub use handshake::{Trusted, Url};
pub use stream::{Framed, Inbound, WsStream};

use horizon_transport::{Address, Connector, Door, Polled, Registry, Transport, Upgrade, Upgraded};
use std::{
    collections::HashMap,
    io,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{Arc, Once},
    time::{Duration, Instant},
};

/// Registers the `ws://` and `wss://` connector and the server door once per process.
pub fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let registry = Registry::global();
        registry.register_connector(Arc::new(WebSocketConnector));
        registry.register_door(Arc::new(WebSocketDoor));
    });
}

struct WebSocketConnector;

impl Connector for WebSocketConnector {
    fn parse(&self, text: &str) -> Option<Address> {
        let url = Url::parse(text)?;
        Some(Address {
            text: text.trim().to_owned(),
            port: url.port,
            handshake_host: url.host,
        })
    }

    fn connect(&self, address: &Address, timeout: Duration) -> io::Result<Box<dyn Transport>> {
        let url = Url::parse(&address.text).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid WebSocket address")
        })?;
        let addrs: Vec<_> = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .map_err(|e| io::Error::other(format!("Unknown host {}: {e}", url.host)))?
            .collect();
        let socket = connect_socket(&url.host, addrs, timeout)?;
        let stream = WsStream::connect(socket, &url, timeout)
            .map_err(|e| io::Error::other(format!("WebSocket connection to {url} failed: {e}")))?;
        Ok(Box::new(stream))
    }
}

struct WebSocketDoor;

impl WebSocketDoor {
    fn upgrade(properties: Option<&HashMap<String, String>>) -> Arc<dyn Upgrade> {
        let trusted = match properties.and_then(|p| p.get(TRUSTED_PROXIES)) {
            None => Trusted::loopback(),
            Some(text) => match Trusted::parse_list(text) {
                Ok(list) => list,
                Err(bad) => {
                    log::warn!(
                        "trusted_proxies: left out what isn't an address or a network: {}",
                        bad.join(", ")
                    );
                    text.split([',', ' ']).filter_map(Trusted::parse).collect()
                }
            },
        };
        Arc::new(WebSocketUpgrade {
            trusted: trusted.into(),
        })
    }
}

impl Door for WebSocketDoor {
    fn open(&self, properties: &HashMap<String, String>) -> Option<Arc<dyn Upgrade>> {
        if properties
            .get(WEBSOCKET)
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("false"))
        {
            return None;
        }
        Some(Self::upgrade(Some(properties)))
    }

    fn lan(&self) -> Option<Arc<dyn Upgrade>> {
        Some(Self::upgrade(None))
    }
}

struct WebSocketUpgrade {
    trusted: Arc<[Trusted]>,
}

impl Upgrade for WebSocketUpgrade {
    fn accept(&self, stream: TcpStream, deadline: Duration) -> Option<Result<Upgraded, String>> {
        if !opens_upgrade(&stream, deadline) {
            return None;
        }
        let address = match stream.peer_addr() {
            Ok(address) => address,
            Err(e) => return Some(Err(format!("WebSocket handshake: {e}"))),
        };
        Some(
            WsStream::accept(stream, deadline, &self.trusted)
                .map_err(|e| format!("WebSocket handshake: {e}"))
                .map(|(stream, forwarded)| {
                    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                    let peer = forwarded.map_or(address, |ip| SocketAddr::new(ip, address.port()));
                    Upgraded {
                        transport: Box::new(stream),
                        peer,
                    }
                }),
        )
    }
}

impl Transport for WsStream {
    fn try_clone(&self) -> io::Result<Box<dyn Transport>> {
        Ok(Box::new(self.clone()))
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        WsStream::set_read_timeout(self, timeout)
    }

    fn shutdown(&self) {
        WsStream::shutdown(self);
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        WsStream::peer_addr(self)
    }

    fn name(&self) -> &'static str {
        if self.is_secure() {
            "WebSocket (wss)"
        } else {
            "WebSocket (ws)"
        }
    }

    fn into_polled(self: Box<Self>) -> io::Result<Box<dyn Polled>> {
        let (socket, inbound) = self.take_parts()?;
        socket.set_nonblocking(true)?;
        Ok(Box::new(Framed::new(
            mio::net::TcpStream::from_std(socket),
            inbound,
        )))
    }
}

impl Polled for Framed<mio::net::TcpStream> {
    fn socket(&mut self) -> &mut mio::net::TcpStream {
        Framed::socket(self)
    }

    fn close(&mut self) {
        Framed::close(self);
    }
}

/// server.properties' WebSocket switch, on unless "false".
pub const WEBSOCKET: &str = "horizon-websocket";
/// server.properties' trusted proxy list, this computer when absent.
pub const TRUSTED_PROXIES: &str = "horizon-trusted-proxies";
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the connection opens with an HTTP request (a WebSocket upgrade) rather than
/// Minecraft's handshake. The first bytes are looked at, not taken; a peer that sends too few
/// of them within `deadline` is served as Minecraft, whose read times out the same.
fn opens_upgrade(stream: &TcpStream, deadline: Duration) -> bool {
    let until = Instant::now() + deadline;
    let mut first = [0u8; 4];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        let n = match stream.peek(&mut first) {
            Ok(n) if n > 0 => n,
            _ => return false,
        };
        if let Some(upgrade) = is_upgrade(&first[..n]) {
            return upgrade;
        }
        // "G" or "GE" so far: the rest is on its way.
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ipv4_first(mut addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.sort_by_key(SocketAddr::is_ipv6);
    addrs
}

/// RFC 8305's connection attempt delay: how long an address has alone before the next
/// one is tried beside it.
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

fn connect_socket(host: &str, addrs: Vec<SocketAddr>, timeout: Duration) -> io::Result<TcpStream> {
    let addrs = ipv4_first(addrs);
    let (send, recv) = std::sync::mpsc::channel();
    let mut errors: Vec<Option<io::Error>> = addrs.iter().map(|_| None).collect();
    let (mut started, mut pending) = (0, 0);
    loop {
        if let Some(&addr) = addrs.get(started) {
            let (send, index) = (send.clone(), started);
            let spawned = std::thread::Builder::new()
                .name("horizon-connect".into())
                .spawn(move || {
                    let _ = send.send((index, TcpStream::connect_timeout(&addr, timeout)));
                });
            match spawned {
                Ok(_) => pending += 1,
                Err(e) => errors[index] = Some(e),
            }
            started += 1;
        }
        if pending == 0 {
            if started < addrs.len() {
                continue;
            }
            break;
        }
        let reply = if started < addrs.len() {
            match recv.recv_timeout(ATTEMPT_DELAY) {
                Ok(reply) => reply,
                Err(_) => continue,
            }
        } else {
            recv.recv().expect("a sender is held here")
        };
        pending -= 1;
        match reply {
            (_, Ok(stream)) => return Ok(stream),
            (index, Err(e)) => errors[index] = Some(e),
        }
    }
    match errors.into_iter().flatten().next() {
        Some(e) => Err(io::Error::other(format!("Connection refused: {e}"))),
        None => Err(io::Error::other(format!("Unknown host {host}"))),
    }
}

/// Whether `first`, the first bytes a server received, start an HTTP request rather than Minecraft's
/// handshake: `Some` once that is known, `None` while they could be either. A Minecraft connection opens
/// with a frame length and the handshake's id 0, never with "GE".
pub fn is_upgrade(first: &[u8]) -> Option<bool> {
    let prefix = b"GET ";
    let n = first.len().min(prefix.len());
    if first[..n] != prefix[..n] {
        return Some(false);
    }
    (n == prefix.len()).then_some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upgrade_is_told_from_a_handshake() {
        assert_eq!(is_upgrade(b""), None);
        assert_eq!(is_upgrade(b"GE"), None);
        assert_eq!(is_upgrade(b"GET /"), Some(true));
        // A handshake whose frame is 71 bytes long starts with 'G' too, then its packet id.
        assert_eq!(is_upgrade(b"G\x00\x89\x06"), Some(false));
        assert_eq!(is_upgrade(&[0x10, 0x00]), Some(false));
        assert_eq!(is_upgrade(&[0xfe, 0x01]), Some(false));
    }

    #[test]
    fn installation_registers_addresses_and_the_lan_door() {
        install();
        install();
        let address = Registry::global()
            .parse_address(" wss://mc.example.com:8443/play ")
            .unwrap();
        assert_eq!(address.text, "wss://mc.example.com:8443/play");
        assert_eq!(address.port, 8443);
        assert_eq!(address.handshake_host, "mc.example.com");
        assert!(Registry::global().lan_door().is_some());
        let properties = HashMap::from([(WEBSOCKET.to_owned(), "false".to_owned())]);
        assert!(Registry::global().door(&properties).is_none());
    }
}
