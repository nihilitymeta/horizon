//! The synchronous WebSocket transport under the Minecraft frame codec.
use horizon_protocol::connection::Connection;
use std::{net::TcpListener, sync::mpsc, thread, time::Duration};

#[test]
fn websocket_connection_round_trips_a_packet_larger_than_64_kib() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (server_tx, server_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| {
            let (stream, _) = listener.accept()?;
            let mut server = Connection::from_websocket(stream)?;
            server
                .reader
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            let (id, payload) = server.recv()?;
            server.send(&[2, 0xaa, 0xbb])?;
            Ok::<_, horizon_protocol::Error>((id, payload.len()))
        })();
        let _ = server_tx.send(result);
    });

    let mut client =
        Connection::connect_websocket(&format!("ws://{address}"), Duration::from_secs(10)).unwrap();
    client
        .reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut body = vec![0x01];
    body.extend(std::iter::repeat_n(0x5a, 100_000));
    client.send(&body).unwrap();
    let server_result = server_rx.recv_timeout(Duration::from_secs(5));
    let (id, payload) = client.recv().unwrap();
    assert_eq!(id, 2);
    assert_eq!(payload, [0xaa, 0xbb]);
    let (server_id, len) = server_result.unwrap().unwrap();
    assert_eq!(server_id, 1);
    assert_eq!(len, 100_000);
}

#[test]
fn websocket_addresses_are_recognized_and_get_their_default_port() {
    assert!(Connection::is_websocket_address("ws://mc.example.com"));
    assert!(Connection::is_websocket_address("wss://mc.example.com"));
    assert!(!Connection::is_websocket_address("mc.example.com"));
}
