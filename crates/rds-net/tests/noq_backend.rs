//! Owned-transport backend: loopback connectivity and cross-backend
//! interop with the shipping iroh endpoint on `rds/0`.
#![cfg(feature = "transport-noq")]

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use iroh::{EndpointAddr, EndpointId, TransportAddr};
use rds_net::backends::noq::{self, EndpointConfig};

const PING: &[u8] = b"rds-ping";

fn loopback_config() -> EndpointConfig {
    EndpointConfig {
        bind_addr: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
        ..Default::default()
    }
}

fn addr_of(id: EndpointId, sock: SocketAddr) -> EndpointAddr {
    let mut addrs = BTreeSet::new();
    addrs.insert(TransportAddr::Ip(sock));
    EndpointAddr { id, addrs }
}

/// Echo every incoming bidi stream until the peer closes the connection.
async fn serve_noq(conn: noq::Connection) {
    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
        if let Ok(buf) = recv.read_to_end(1024).await {
            let _ = send.write_all(&buf).await;
            let _ = send.finish();
        }
    }
}

async fn serve_iroh(conn: iroh::endpoint::Connection) {
    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
        if let Ok(buf) = recv.read_to_end(1024).await {
            let _ = send.write_all(&buf).await;
            let _ = send.finish();
        }
    }
}

/// Client side of the ping: write PING on a new stream, read it back.
async fn ping_noq(conn: &noq::Connection) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await?;
    send.finish()?;
    let mut buf = vec![0u8; PING.len()];
    recv.read_exact(&mut buf).await?;
    assert_eq!(&buf, PING);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn noq_to_noq_ping() -> anyhow::Result<()> {
    let a = noq::bind_endpoint(loopback_config()).await?;
    let b = noq::bind_endpoint(loopback_config()).await?;

    // a dials b
    let server = tokio::spawn({
        let b = b.clone();
        let a_id = a.id();
        async move {
            let incoming = b.accept().await.expect("accept");
            let conn = incoming.await.expect("handshake");
            assert_eq!(conn.remote_id(), a_id);
            serve_noq(conn).await;
        }
    });

    let conn = a
        .connect(addr_of(b.id(), b.local_addr()), rds_core::ALPN)
        .await?;
    assert_eq!(conn.remote_id(), b.id());
    ping_noq(&conn).await?;
    conn.close(0u32.into(), b"done");
    server.await?;

    // b dials a (reverse direction)
    let server = tokio::spawn({
        let a = a.clone();
        let b_id = b.id();
        async move {
            let incoming = a.accept().await.expect("accept");
            let conn = incoming.await.expect("handshake");
            assert_eq!(conn.remote_id(), b_id);
            serve_noq(conn).await;
        }
    });
    let conn = b
        .connect(addr_of(a.id(), a.local_addr()), rds_core::ALPN)
        .await?;
    ping_noq(&conn).await?;
    conn.close(0u32.into(), b"done");
    server.await?;

    a.close().await;
    b.close().await;
    Ok(())
}

/// iroh client → noq server on `rds/0`: the owned TLS layer must be
/// wire-compatible with the shipping backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_client_to_noq_server() -> anyhow::Result<()> {
    let server_ep = noq::bind_endpoint(loopback_config()).await?;
    let client_ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind()
        .await?;

    let server = tokio::spawn({
        let server_ep = server_ep.clone();
        let client_id = client_ep.id();
        async move {
            let incoming = server_ep.accept().await.expect("accept");
            let conn = incoming.await.expect("handshake");
            assert_eq!(conn.remote_id(), client_id);
            serve_noq(conn).await;
        }
    });

    let conn = client_ep
        .connect(
            addr_of(server_ep.id(), server_ep.local_addr()),
            rds_core::ALPN,
        )
        .await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await?;
    send.finish()?;
    let mut buf = vec![0u8; PING.len()];
    recv.read_exact(&mut buf).await?;
    assert_eq!(&buf, PING);
    conn.close(0u32.into(), b"done");
    server.await?;

    client_ep.close().await;
    server_ep.close().await;
    Ok(())
}

/// noq client → iroh server on `rds/0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn noq_client_to_iroh_server() -> anyhow::Result<()> {
    let server_ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
        .bind()
        .await?;
    let client_ep = noq::bind_endpoint(loopback_config()).await?;

    let server_sock = server_ep
        .addr()
        .addrs
        .iter()
        .find_map(|a| match a {
            TransportAddr::Ip(sock) if sock.ip().is_loopback() => Some(*sock),
            _ => None,
        })
        .expect("loopback addr");

    let server = tokio::spawn({
        let server_ep = server_ep.clone();
        let client_id = client_ep.id();
        async move {
            let incoming = server_ep.accept().await.expect("accept");
            let conn = incoming.await.expect("handshake");
            assert_eq!(conn.remote_id(), client_id);
            serve_iroh(conn).await;
        }
    });

    let conn = client_ep
        .connect(addr_of(server_ep.id(), server_sock), rds_core::ALPN)
        .await?;
    assert_eq!(conn.remote_id(), server_ep.id());
    ping_noq(&conn).await?;
    conn.close(0u32.into(), b"done");
    server.await?;

    client_ep.close().await;
    server_ep.close().await;
    Ok(())
}

/// QUIC datagrams must round-trip: the owned relay payload rides on them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn noq_datagrams() -> anyhow::Result<()> {
    let a = noq::bind_endpoint(loopback_config()).await?;
    let b = noq::bind_endpoint(loopback_config()).await?;

    let server = tokio::spawn({
        let b = b.clone();
        async move {
            let incoming = b.accept().await.expect("accept");
            let conn = incoming.await.expect("handshake");
            let data = conn.read_datagram().await.expect("read_datagram");
            assert_eq!(&data[..], PING);
            conn
        }
    });

    let conn = a
        .connect(addr_of(b.id(), b.local_addr()), rds_core::ALPN)
        .await?;
    conn.send_datagram(bytes::Bytes::from_static(PING))?;
    let server_conn = tokio::time::timeout(std::time::Duration::from_secs(10), server).await??;
    drop(server_conn);
    conn.close(0u32.into(), b"done");

    a.close().await;
    b.close().await;
    Ok(())
}
