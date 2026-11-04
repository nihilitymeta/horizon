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
}
