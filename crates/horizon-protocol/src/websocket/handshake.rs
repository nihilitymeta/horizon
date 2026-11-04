//! The HTTP/1.1 upgrade that opens a WebSocket (RFC 6455, section 4), both ways, and the addresses a
//! trusted proxy reports for the player behind it.

use base64::Engine;
use sha1::{Digest, Sha1};
use std::{
    io::{self, Read},
    net::IpAddr,
};

/// The longest request or response head read: browsers and proxies send a few hundred bytes, cookies
/// included.
const MAX_HEAD: usize = 16 * 1024;

/// What `Sec-WebSocket-Accept` answers for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut sha = Sha1::new();
    sha.update(key.trim().as_bytes());
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha.finalize())
}

/// A `ws://` or `wss://` address, as a player types it in the server list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    /// The host name or address, without the brackets of an IPv6 one.
    pub host: String,
    pub port: u16,
    /// The path and query, starting with `/`.
    pub path: String,
}

impl Url {
    /// Gets the address in `text`, or `None` when it isn't a WebSocket one. The scheme is matched without
    /// regard to case; the port defaults to 80 for `ws` and 443 for `wss`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (scheme, rest) = text.split_once("://")?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "ws" => false,
            "wss" => true,
            _ => return None,
        };
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(split);
        let path = match path {
            "" => "/".to_owned(),
            p if p.starts_with('?') => format!("/{p}"),
            p => p.to_owned(),
        };
        // No user name or password: the address has nowhere to put them.
        if authority.contains('@') {
            return None;
        }
        let default = if secure { 443 } else { 80 };
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (host, tail) = v6.split_once(']')?;
            match tail.strip_prefix(':') {
                Some(port) => (host, port.parse().ok()?),
                None if tail.is_empty() => (host, default),
                None => return None,
            }
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, port.parse().ok()?),
                None => (authority, default),
            }
        };
        if host.is_empty() || port == 0 {
            return None;
        }
        Some(Self {
            secure,
            host: host.to_owned(),
            port,
            path,
        })
    }

    /// The `Host` header: the port only when it isn't the scheme's default.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = if self.secure { 443 } else { 80 };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.secure { "wss" } else { "ws" };
        write!(f, "{scheme}://{}{}", self.host_header(), self.path)
    }
}

/// Reads up to the blank line that ends an HTTP head: the head, and the bytes read past it (the first
/// frames, when the peer didn't wait for the answer).
pub fn read_head(source: &mut impl Read) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = source.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let searched = buf.len().saturating_sub(3);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf[searched..].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = searched + at + 4;
            let rest = buf.split_off(end);
            let head = String::from_utf8(buf).map_err(|_| invalid("the HTTP head isn't text"))?;
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(invalid("the HTTP head is too long"));
        }
    }
}

/// An HTTP head: its first line and its headers.
pub struct Head {
    pub first_line: String,
    headers: Vec<(String, String)>,
}

impl Head {
    pub fn parse(text: &str) -> io::Result<Self> {
        let mut lines = text.split("\r\n");
        let first_line = lines.next().unwrap_or_default().to_owned();
        let mut headers = Vec::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| invalid("a header line has no colon"))?;
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
        Ok(Self {
            first_line,
            headers,
        })
    }

    /// The value of the header `name` (lowercase), the first if it repeats.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of the header `name`, in order, the repeated headers' joined as one list.
    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.headers
            .iter()
            .filter(move |(n, _)| n == name)
            .flat_map(|(_, v)| v.split(','))
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    /// Whether the comma-separated header `name` lists `token`, without regard to case.
    fn lists(&self, name: &str, token: &str) -> bool {
        self.all(name).any(|v| v.eq_ignore_ascii_case(token))
    }
}

/// A client's request to open a WebSocket at `url`.
pub fn request(url: &Url, key: &str) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: Horizon/{}\r\n\r\n",
        url.path,
        url.host_header(),
        crate::GAME_VERSION,
    )
}

/// A new `Sec-WebSocket-Key`: 16 random bytes in base64.
pub fn new_key() -> String {
    let mut nonce = [0u8; 16];
    if getrandom::fill(&mut nonce).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        nonce = nanos.to_le_bytes();
    }
    base64::engine::general_purpose::STANDARD.encode(nonce)
}

/// Checks a server's answer to [`request`] sent with `key`: an error says why it isn't a WebSocket.
pub fn check_response(head: &str, key: &str) -> io::Result<()> {
    let head = Head::parse(head)?;
    let mut status = head.first_line.split_whitespace();
    let (version, code) = (status.next(), status.next());
    if !version.is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err(invalid("the server didn't answer in HTTP/1.1"));
    }
    if code != Some("101") {
        let text = head.first_line.split_once(' ').map_or("", |(_, t)| t);
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("the server answered {text} instead of opening a WebSocket"),
        ));
    }
    if !head.lists("upgrade", "websocket") || !head.lists("connection", "upgrade") {
        return Err(invalid(
            "the server's answer doesn't upgrade to a WebSocket",
        ));
    }
    if head.get("sec-websocket-accept") != Some(accept_key(key).as_str()) {
        return Err(invalid(
            "the server's Sec-WebSocket-Accept doesn't match the key",
        ));
    }
    // An extension changes how frames are read: none was offered, so none may be agreed.
    if head.get("sec-websocket-extensions").is_some() {
        return Err(invalid(
            "the server agreed to an extension that wasn't offered",
        ));
    }
    Ok(())
}

/// What a server answers a request head.
pub enum Answer {
    /// The upgrade: the `101` response to send.
    Open(String),
    /// Not a WebSocket request: the response to send before closing, and why.
    Refuse(String, &'static str),
}

/// Answers a client's request head. Any path is served. When the client offers subprotocols the first is
/// taken, since browsers fail a connection whose answer names none of those offered; what flows is
/// Minecraft's byte stream whatever it is called.
pub fn answer(head: &Head) -> Answer {
    let refuse = |status: &str, extra: &str, why: &'static str| {
        Answer::Refuse(
            format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Type: text/plain\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{why}\n",
                why.len() + 1
            ),
            why,
        )
    };
    let mut first = head.first_line.split_whitespace();
    if first.next() != Some("GET") || !first.nth(1).is_some_and(|v| v == "HTTP/1.1") {
        return refuse(
            "400 Bad Request",
            "",
            "only a GET in HTTP/1.1 opens a WebSocket",
        );
    }
    if !head.lists("upgrade", "websocket") || !head.lists("connection", "upgrade") {
        return refuse(
            "426 Upgrade Required",
            "Upgrade: websocket\r\nConnection: Upgrade\r\n",
            "this is a Minecraft server: connect with a WebSocket",
        );
    }
    if head.get("sec-websocket-version") != Some("13") {
        return refuse(
            "426 Upgrade Required",
            "Sec-WebSocket-Version: 13\r\n",
            "only WebSocket version 13 is spoken",
        );
    }
    let Some(key) = head.get("sec-websocket-key").filter(|k| !k.is_empty()) else {
        return refuse(
            "400 Bad Request",
            "",
            "the request has no Sec-WebSocket-Key",
        );
    };
    let protocol = head
        .all("sec-websocket-protocol")
        .next()
        .map(|p| format!("Sec-WebSocket-Protocol: {p}\r\n"))
        .unwrap_or_default();
    Answer::Open(format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n{protocol}\r\n",
        accept_key(key)
    ))
}

/// An address or a network (`127.0.0.1`, `10.0.0.0/8`, `::1`, `fd00::/8`) trusted to report the
/// address of the player it hands over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trusted {
    network: IpAddr,
    bits: u8,
}

impl Trusted {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (address, bits) = match text.split_once('/') {
            Some((a, b)) => (a, Some(b.parse::<u8>().ok()?)),
            None => (text, None),
        };
        let network: IpAddr = address.trim_matches(['[', ']']).parse().ok()?;
        let most = if network.is_ipv4() { 32 } else { 128 };
        let bits = bits.unwrap_or(most);
        (bits <= most).then_some(Self { network, bits })
    }

    /// Parses a comma- or space-separated list: the entries that don't parse, as the error.
    pub fn parse_list(text: &str) -> Result<Vec<Self>, Vec<String>> {
        let entries = text.split([',', ' ']).filter(|e| !e.trim().is_empty());
        let (mut good, mut bad) = (Vec::new(), Vec::new());
        for entry in entries {
            match Self::parse(entry) {
                Some(t) => good.push(t),
                None => bad.push(entry.trim().to_owned()),
            }
        }
        if bad.is_empty() { Ok(good) } else { Err(bad) }
    }

    /// This computer: `127.0.0.0/8` and `::1`.
    pub fn loopback() -> Vec<Self> {
        Self::parse_list("127.0.0.0/8, ::1").expect("valid")
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        match (self.network, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => prefix(&n.octets(), &a.octets(), self.bits),
            (IpAddr::V6(n), IpAddr::V6(a)) => prefix(&n.octets(), &a.octets(), self.bits),
            _ => false,
        }
    }
}

fn prefix(network: &[u8], address: &[u8], bits: u8) -> bool {
    let (whole, rest) = (usize::from(bits / 8), bits % 8);
    if network[..whole] != address[..whole] {
        return false;
    }
    rest == 0 || (network[whole] ^ address[whole]) >> (8 - rest) == 0
}

/// The player's address as the proxy at `peer` reports it, when `peer` is `trusted`:
/// `CF-Connecting-IP` (Cloudflare), else the nearest address in `Forwarded` (RFC 7239) or
/// `X-Forwarded-For` that isn't itself a trusted proxy, else `X-Real-IP` (nginx).
pub fn forwarded_address(head: &Head, peer: IpAddr, trusted: &[Trusted]) -> Option<IpAddr> {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|t| t.contains(ip));
    if !is_trusted(peer) {
        return None;
    }
    if let Some(ip) = head.get("cf-connecting-ip").and_then(parse_ip) {
        return Some(ip);
    }
    // Each proxy appends who it heard from: read from the nearest hop back, past the trusted ones; what
    // lies further back was written by the player and proves nothing.
    let nearest_untrusted = |hops: Vec<IpAddr>| hops.into_iter().rev().find(|ip| !is_trusted(*ip));
    let forwarded: Vec<IpAddr> = head
        .all("forwarded")
        .filter_map(|element| {
            element.split(';').find_map(|pair| {
                let (name, value) = pair.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("for")
                    .then(|| parse_ip(value.trim().trim_matches('"')))?
            })
        })
        .collect();
    if let Some(ip) = nearest_untrusted(forwarded) {
        return Some(ip);
    }
    let x_forwarded: Vec<IpAddr> = head.all("x-forwarded-for").filter_map(parse_ip).collect();
    if let Some(ip) = nearest_untrusted(x_forwarded) {
        return Some(ip);
    }
    head.get("x-real-ip").and_then(parse_ip)
}

/// An address as headers write it: bare, `[v6]`, with a port, or `[v6]:port`.
fn parse_ip(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    if let Ok(ip) = text.parse() {
        return Some(ip);
    }
    if let Some(v6) = text.strip_prefix('[') {
        return v6.split_once(']')?.0.parse().ok();
    }
    text.rsplit_once(':')?.0.parse().ok()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_accept_key_is_the_rfcs() {
        // RFC 6455, section 1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn addresses_parse_as_players_type_them() {
        let url = |secure, host: &str, port, path: &str| Url {
            secure,
            host: host.into(),
            port,
            path: path.into(),
        };
        assert_eq!(
            Url::parse("wss://mc.example.com"),
            Some(url(true, "mc.example.com", 443, "/"))
        );
        assert_eq!(
            Url::parse(" WS://127.0.0.1:25565 "),
            Some(url(false, "127.0.0.1", 25565, "/"))
        );
        assert_eq!(
            Url::parse("wss://mc.example.com:8443/play?world=a"),
            Some(url(true, "mc.example.com", 8443, "/play?world=a"))
        );
        assert_eq!(
            Url::parse("ws://[::1]:9000/x"),
            Some(url(false, "::1", 9000, "/x"))
        );
        assert_eq!(Url::parse("wss://h?q"), Some(url(true, "h", 443, "/?q")));
        for bad in [
            "mc.example.com",
            "http://h",
            "ws://",
            "ws://h:0",
            "ws://h:x",
            "ws://u@h",
            "ws://[::1",
        ] {
            assert_eq!(Url::parse(bad), None, "{bad}");
        }
        assert_eq!(
            Url::parse("ws://[::1]:9000/x").unwrap().host_header(),
            "[::1]:9000"
        );
        assert_eq!(Url::parse("wss://h:443").unwrap().to_string(), "wss://h/");
    }

    /// Requests as browsers and tools send them.
    #[test]
    fn real_requests_are_answered() {
        let firefox = "GET /mc HTTP/1.1\r\nHost: localhost:25565\r\nUser-Agent: Mozilla/5.0\r\n\
            Sec-WebSocket-Version: 13\r\nOrigin: null\r\nSec-WebSocket-Protocol: minecraft, binary\r\n\
            Sec-WebSocket-Extensions: permessage-deflate\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            Connection: keep-alive, Upgrade\r\nUpgrade: websocket\r\n\r\n";
        let Answer::Open(response) = answer(&Head::parse(firefox).unwrap()) else {
            panic!("refused");
        };
        assert!(response.starts_with("HTTP/1.1 101 "));
        assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(response.contains("Sec-WebSocket-Protocol: minecraft\r\n"));
        // permessage-deflate is offered and not taken.
        assert!(!response.contains("Extensions"));
        let plain = "GET / HTTP/1.1\r\nhost: x\r\nUPGRADE: WebSocket\r\nconnection: upgrade\r\n\
            sec-websocket-key: abc\r\nsec-websocket-version: 13\r\n\r\n";
        let Answer::Open(response) = answer(&Head::parse(plain).unwrap()) else {
            panic!("refused");
        };
        assert!(!response.contains("Sec-WebSocket-Protocol"));
        // Our own request, answered by our own server, passes our own check.
        let url = Url::parse("ws://h:1/").unwrap();
        let key = new_key();
        let (head, _) = read_head(&mut request(&url, &key).as_bytes()).unwrap();
        let Answer::Open(response) = answer(&Head::parse(&head).unwrap()) else {
            panic!("refused");
        };
        check_response(&response, &key).unwrap();
        assert!(check_response(&response, &new_key()).is_err());
    }

    #[test]
    fn what_isnt_an_upgrade_is_refused() {
        let browse = "GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let Answer::Refuse(response, _) = answer(&Head::parse(browse).unwrap()) else {
            panic!("opened");
        };
        assert!(response.starts_with("HTTP/1.1 426 "));
        let old = "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
            Sec-WebSocket-Key: a\r\nSec-WebSocket-Version: 8\r\n\r\n";
        let Answer::Refuse(response, _) = answer(&Head::parse(old).unwrap()) else {
            panic!("opened");
        };
        assert!(response.contains("Sec-WebSocket-Version: 13"));
        let post = "POST / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
        assert!(matches!(
            answer(&Head::parse(post).unwrap()),
            Answer::Refuse(..)
        ));
    }

    #[test]
    fn a_head_ends_at_its_blank_line() {
        let mut sent = b"HTTP/1.1 101 OK\r\nA: b\r\n\r\n\x82\x01x".as_slice();
        let (head, rest) = read_head(&mut sent).unwrap();
        assert_eq!(head, "HTTP/1.1 101 OK\r\nA: b\r\n\r\n");
        assert_eq!(rest, b"\x82\x01x");
        let endless = vec![b'a'; MAX_HEAD + 2048];
        assert!(read_head(&mut endless.as_slice()).is_err());
    }

    #[test]
    fn trusted_networks_match_their_addresses() {
        let list = Trusted::parse_list("127.0.0.0/8, ::1 10.1.0.0/23").unwrap();
        let trusted = |a| list.iter().any(|t| t.contains(ip(a)));
        assert!(trusted("127.4.5.6") && trusted("::1") && trusted("::ffff:127.0.0.1"));
        assert!(trusted("10.1.1.200") && !trusted("10.1.2.0") && !trusted("8.8.8.8"));
        assert_eq!(
            Trusted::parse_list("1.2.3.4, nope, 10.0.0.0/33"),
            Err(vec!["nope".into(), "10.0.0.0/33".into()])
        );
        assert_eq!(Trusted::parse_list(""), Ok(Vec::new()));
    }

    #[test]
    fn the_player_address_comes_from_a_trusted_proxy_only() {
        let loopback = Trusted::loopback();
        let head = |extra: &str| Head::parse(&format!("GET / HTTP/1.1\r\n{extra}\r\n")).unwrap();
        let cloudflare =
            head("CF-Connecting-IP: 203.0.113.7\r\nX-Forwarded-For: 198.51.100.1, 203.0.113.7\r\n");
        assert_eq!(
            forwarded_address(&cloudflare, ip("127.0.0.1"), &loopback),
            Some(ip("203.0.113.7"))
        );
        // From an untrusted peer the headers are the player's own words.
        assert_eq!(
            forwarded_address(&cloudflare, ip("203.0.113.9"), &loopback),
            None
        );
        // The nearest hop that isn't a trusted proxy; the spoofed one further back is ignored.
        let chain = head("X-Forwarded-For: 6.6.6.6, 203.0.113.8, 127.0.0.1\r\n");
        assert_eq!(
            forwarded_address(&chain, ip("::1"), &loopback),
            Some(ip("203.0.113.8"))
        );
        let rfc = head("Forwarded: for=\"[2001:db8::1]:4711\";proto=https, for=127.0.0.1\r\n");
        assert_eq!(
            forwarded_address(&rfc, ip("127.0.0.1"), &loopback),
            Some(ip("2001:db8::1"))
        );
        let nginx = head("X-Real-IP: 192.0.2.44\r\n");
        assert_eq!(
            forwarded_address(&nginx, ip("127.0.0.1"), &loopback),
            Some(ip("192.0.2.44"))
        );
        assert_eq!(
            forwarded_address(&head(""), ip("127.0.0.1"), &loopback),
            None
        );
    }
}
