//! Extension points for byte-stream transports. A program registers connectors for address schemes and
//! doors for accepted sockets; the protocol and LAN code stay independent of each transport's format.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{Arc, RwLock},
    time::Duration,
};

/// A blocking byte stream whose clone can write while the original reads.
pub trait Transport: Read + Write + Send {
    fn try_clone(&self) -> io::Result<Box<dyn Transport>>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn shutdown(&self);
    fn peer_addr(&self) -> io::Result<SocketAddr>;
    /// The transport's display name, for diagnostics.
    fn name(&self) -> &'static str;
    /// Converts the stream for a mio poller, preserving bytes already read.
    fn into_polled(self: Box<Self>) -> io::Result<Box<dyn Polled>>;
}

/// A non-blocking byte stream registered through its underlying socket.
pub trait Polled: Read + Write + Send {
    fn socket(&mut self) -> &mut mio::net::TcpStream;
    /// Flushes any transport-level close before the socket is shut down.
    fn close(&mut self);
}

/// An address recognized by a registered connector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// The whole address, without surrounding whitespace.
    pub text: String,
    pub port: u16,
    /// The host the Minecraft handshake names.
    pub handshake_host: String,
}

/// A client-side address scheme.
pub trait Connector: Send + Sync {
    fn parse(&self, text: &str) -> Option<Address>;
    fn connect(&self, address: &Address, timeout: Duration) -> io::Result<Box<dyn Transport>>;
}

/// A server-side transport, configured from server properties or for a LAN world.
pub trait Door: Send + Sync {
    fn open(&self, properties: &HashMap<String, String>) -> Option<Arc<dyn Upgrade>>;
    fn lan(&self) -> Option<Arc<dyn Upgrade>>;
}

/// An accepted connection that a transport may claim after the legacy-protocol checks.
pub trait Upgrade: Send + Sync {
    /// Returns `None` without consuming a plain Minecraft connection.
    fn accept(&self, stream: TcpStream, deadline: Duration) -> Option<Result<Upgraded, String>>;
}

/// A successful server-side upgrade.
pub struct Upgraded {
    pub transport: Box<dyn Transport>,
    pub peer: SocketAddr,
}

/// Registered connectors and doors. [`Registry::global`] is the process-wide instance.
pub struct Registry {
    connectors: RwLock<Vec<Arc<dyn Connector>>>,
    doors: RwLock<Vec<Arc<dyn Door>>>,
}

impl Registry {
    pub const fn new() -> Self {
        Self {
            connectors: RwLock::new(Vec::new()),
            doors: RwLock::new(Vec::new()),
        }
    }

    pub fn global() -> &'static Self {
        static REGISTRY: Registry = Registry::new();
        &REGISTRY
    }

    pub fn register_connector(&self, connector: Arc<dyn Connector>) {
        self.connectors
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(connector);
    }

    pub fn register_door(&self, door: Arc<dyn Door>) {
        self.doors
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(door);
    }

    pub fn parse_address(&self, text: &str) -> Option<Address> {
        self.connector(text).map(|(_, address)| address)
    }

    /// Connects with the first connector that recognizes `text`; `None` leaves it to ordinary TCP.
    pub fn connect(
        &self,
        text: &str,
        timeout: Duration,
    ) -> Option<io::Result<(Box<dyn Transport>, Address)>> {
        let (connector, address) = self.connector(text)?;
        Some(
            connector
                .connect(&address, timeout)
                .map(|transport| (transport, address)),
        )
    }

    pub fn door(&self, properties: &HashMap<String, String>) -> Option<Arc<dyn Upgrade>> {
        self.doors
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find_map(|door| door.open(properties))
    }

    pub fn lan_door(&self) -> Option<Arc<dyn Upgrade>> {
        self.doors
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find_map(|door| door.lan())
    }

    fn connector(&self, text: &str) -> Option<(Arc<dyn Connector>, Address)> {
        self.connectors
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find_map(|connector| {
                connector
                    .parse(text)
                    .map(|address| (connector.clone(), address))
            })
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shuts down a polled transport after its close frame, if any, has been queued.
pub fn shutdown(polled: &mut dyn Polled) {
    let _ = polled.socket().shutdown(Shutdown::Both);
}
