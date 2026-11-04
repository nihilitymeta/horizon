//! A `wss://` client's TLS, split so that one thread can read while another writes: the session is
//! locked only to move records in or out, never while the socket is waited on. Server certificates are
//! checked against the Mozilla roots (`webpki-roots`), as browsers do.

use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex, OnceLock},
};

struct Tls {
    session: Mutex<rustls::ClientConnection>,
    /// The socket's reading side and the bytes read from it that the session hasn't taken yet.
    input: Mutex<(TcpStream, Vec<u8>)>,
    output: Mutex<TcpStream>,
}

impl Tls {
    /// Writes out the records the session has ready.
    fn send(&self, session: &mut rustls::ClientConnection) -> io::Result<()> {
        let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        while session.wants_write() {
            session.write_tls(&mut *output)?;
        }
        output.flush()
    }
}

/// The reading half: decrypted bytes.
pub struct TlsRead(Arc<Tls>);
/// The writing half: what it is given goes out encrypted.
pub struct TlsWrite(Arc<Tls>);

fn config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("ring supports TLS 1.2 and 1.3")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Starts a TLS session with `host` on `socket`; the handshake runs as the halves are first used.
pub fn client(socket: &TcpStream, host: &str) -> io::Result<(TlsRead, TlsWrite)> {
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let session = rustls::ClientConnection::new(config(), name).map_err(io::Error::other)?;
    let tls = Arc::new(Tls {
        session: Mutex::new(session),
        input: Mutex::new((socket.try_clone()?, Vec::new())),
        output: Mutex::new(socket.try_clone()?),
    });
    Ok((TlsRead(tls.clone()), TlsWrite(tls)))
}

impl Read for TlsRead {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let tls = &*self.0;
        let mut input = tls.input.lock().unwrap_or_else(|e| e.into_inner());
        let (socket, pending) = &mut *input;
        loop {
            {
                let mut session = tls.session.lock().unwrap_or_else(|e| e.into_inner());
                match session.reader().read(out) {
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                if !pending.is_empty() {
                    let mut rest = &pending[..];
                    let read = session.read_tls(&mut rest);
                    let taken = pending.len() - rest.len();
                    pending.drain(..taken);
                    read?;
                    let processed = session.process_new_packets();
                    // The handshake's next records, the data held for its end, or the alert that says
                    // why it failed.
                    tls.send(&mut session)?;
                    processed.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    continue;
                }
            }
            let mut raw = [0u8; 16 * 1024];
            let n = socket.read(&mut raw)?;
            if n == 0 {
                return Ok(0);
            }
            pending.extend_from_slice(&raw[..n]);
        }
    }
}

impl Write for TlsWrite {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let tls = &*self.0;
        let mut session = tls.session.lock().unwrap_or_else(|e| e.into_inner());
        session.writer().write_all(data)?;
        tls.send(&mut session)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
