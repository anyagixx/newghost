use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::{ProxyIntent, ProxyProtocol, Socks5Proxy, Socks5ProxyConfig, TargetAddr};

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let client = TcpStream::connect(addr).await.expect("client connect");
    let (server, _) = listener.accept().await.expect("accept");
    (client, server)
}

async fn send_no_auth_connect_domain(stream: &mut TcpStream, domain: &str, port: u16) -> [u8; 2] {
    let mut request = vec![0x05, 0x01, 0x00, 0x05, 0x01, 0x00, 0x03, domain.len() as u8];
    request.extend_from_slice(domain.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await.expect("write request");

    let mut auth_reply = [0_u8; 2];
    stream
        .read_exact(&mut auth_reply)
        .await
        .expect("read auth reply");
    auth_reply
}

#[tokio::test]
async fn parses_valid_connect_request_into_proxy_intent() {
    let (mut client, server) = tcp_pair().await;
    let parse_task = tokio::spawn(async move { Socks5Proxy::parse_request(server).await });

    let auth_reply = send_no_auth_connect_domain(&mut client, "example.com", 443).await;
    assert_eq!(auth_reply, [0x05, 0x00]);

    let intent = parse_task
        .await
        .expect("join parse task")
        .expect("parse request");

    assert_eq!(intent.protocol_kind, ProxyProtocol::Socks5);
    assert_eq!(
        intent.target,
        TargetAddr::Domain("example.com".to_string(), 443)
    );
    assert!(intent.request_id > 0);
}

#[tokio::test]
async fn queue_saturation_returns_failure_reply_quickly() {
    let (tx, mut rx) = mpsc::channel::<ProxyIntent>(1);
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    );

    let (mut first_client, first_server) = tcp_pair().await;
    let first_task = tokio::spawn(async move { Socks5Proxy::parse_request(first_server).await });
    let _ = send_no_auth_connect_domain(&mut first_client, "example.com", 443).await;
    let first_intent = first_task
        .await
        .expect("join parse task")
        .expect("parse request");
    proxy
        .intent_tx
        .try_send(first_intent)
        .expect("queue first intent");

    let (mut second_client, second_server) = tcp_pair().await;
    let start = Instant::now();
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(second_server).await }
    });

    let _ = send_no_auth_connect_domain(&mut second_client, "example.com", 443).await;
    let mut reply = [0_u8; 10];
    second_client
        .read_exact(&mut reply)
        .await
        .expect("read failure reply");

    assert!(start.elapsed() < Duration::from_millis(50));
    assert_eq!(reply[1], 0x01);
    handler_task
        .await
        .expect("join handler")
        .expect("handler result");
    assert!(rx.recv().await.is_some());
}

#[tokio::test]
async fn closed_queue_returns_failure_reply_and_closes_stream() {
    let (tx, rx) = mpsc::channel::<ProxyIntent>(1);
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    );
    drop(rx);

    let (mut client, server) = tcp_pair().await;
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(server).await }
    });

    let _ = send_no_auth_connect_domain(&mut client, "example.com", 443).await;
    let mut reply = [0_u8; 10];
    client
        .read_exact(&mut reply)
        .await
        .expect("read failure reply");

    assert_eq!(reply[1], 0x01);
    let mut eof = [0_u8; 1];
    let read = client.read(&mut eof).await.expect("read eof after close");
    assert_eq!(read, 0);

    handler_task
        .await
        .expect("join handler")
        .expect("handler result");
}

#[tokio::test]
async fn success_reply_does_not_close_client_socket() {
    let (mut client, mut server) = tcp_pair().await;

    let send_task = tokio::spawn(async move {
        Socks5Proxy::send_reply(&mut server, super::Socks5Reply::Succeeded)
            .await
            .expect("send reply");

        let mut trailing = [0_u8; 1];
        server
            .read_exact(&mut trailing)
            .await
            .expect("read trailing byte");
        trailing[0]
    });

    let mut reply = [0_u8; 10];
    client.read_exact(&mut reply).await.expect("read success reply");
    assert_eq!(reply[1], 0x00);

    client.write_all(&[0x7f]).await.expect("write trailing byte");

    let trailing = send_task.await.expect("join send task");
    assert_eq!(trailing, 0x7f);
}
