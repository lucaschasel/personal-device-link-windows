use personal_device_link::network::{
    client::Client, configure_stream, protocol::*, server::handle_connection,
};
use std::{
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    thread,
};

fn local(name: &str) -> Device {
    Device::local(name.to_owned()).unwrap()
}

fn start_session() -> (SocketAddr, thread::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept()?;
        handle_connection(stream, &local("Windows-Test"))
    });
    (address, worker)
}

#[test]
fn hello_multiple_heartbeats_and_clean_disconnect_over_real_tcp() {
    let (address, worker) = start_session();
    let mut client = Client::connect(address, local("Mac-Test")).unwrap();
    assert_eq!(client.peer.name, "Windows-Test");
    for _ in 0..3 {
        client.heartbeat().unwrap();
    }
    client.disconnect().unwrap();
    worker.join().unwrap().unwrap();
}

#[test]
fn server_rejects_wrong_version_and_messages_before_hello() {
    for message in [
        Message::Ping { sequence: 1 },
        Message::Hello {
            version: PROTOCOL_VERSION + 1,
            device: local("Mac-Test"),
            text: "Hello".into(),
        },
    ] {
        let (address, worker) = start_session();
        let mut stream = TcpStream::connect(address).unwrap();
        configure_stream(&stream).unwrap();
        write_message(&mut stream, &message).unwrap();
        assert!(matches!(
            read_message(&mut stream).unwrap(),
            Some(Message::Error { .. })
        ));
        assert_eq!(read_message(&mut stream).unwrap(), None);
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn client_rejects_mismatched_heartbeat() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        configure_stream(&stream).unwrap();
        assert!(matches!(
            read_message(&mut stream).unwrap(),
            Some(Message::Hello { .. })
        ));
        write_message(
            &mut stream,
            &Message::HelloAck {
                capabilities: Default::default(),
                version: PROTOCOL_VERSION,
                device: local("Windows-Test"),
                text: "Hello".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            read_message(&mut stream).unwrap(),
            Some(Message::Ping { .. })
        ));
        write_message(&mut stream, &Message::Pong { sequence: 999 }).unwrap();
    });
    let mut client = Client::connect(address, local("Mac-Test")).unwrap();
    assert_eq!(
        client.heartbeat().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    worker.join().unwrap();
}

#[test]
fn abrupt_peer_disconnect_does_not_panic() {
    let (address, worker) = start_session();
    let client = Client::connect(address, local("Mac-Test")).unwrap();
    drop(client);
    worker.join().unwrap().unwrap();
}

#[test]
fn server_rejects_duplicate_handshake() {
    let (address, worker) = start_session();
    let mut stream = TcpStream::connect(address).unwrap();
    configure_stream(&stream).unwrap();
    let hello = Message::Hello {
        version: PROTOCOL_VERSION,
        device: local("Mac-Test"),
        text: "Hello".into(),
    };
    write_message(&mut stream, &hello).unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::HelloAck { .. })
    ));
    write_message(&mut stream, &hello).unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::Error { .. })
    ));
    assert!(worker.join().unwrap().is_err());
}

#[test]
fn client_rejects_incompatible_server_version() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        configure_stream(&stream).unwrap();
        read_message(&mut stream).unwrap();
        write_message(
            &mut stream,
            &Message::HelloAck {
                capabilities: Default::default(),
                version: PROTOCOL_VERSION + 1,
                device: local("Windows-Test"),
                text: "Hello".into(),
            },
        )
        .unwrap();
    });
    assert!(Client::connect(address, local("Mac-Test")).is_err());
    worker.join().unwrap();
}
