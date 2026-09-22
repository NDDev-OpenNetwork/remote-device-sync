//! v3 uni demux lifecycle: a dead connection must end every claimed
//! `UniStreams` inbox (`recv` → `None`) — `rds-sync::receive` and the
//! desktop frame task both rely on end-of-stream to exit instead of
//! parking forever on a dead peer.

use std::collections::BTreeSet;
use std::time::Duration;

use rds_net::{EndpointAddr, EndpointConfig, TransportAddr, bind_endpoint};

/// Connected pair over the default (iroh) backend, discovery off —
/// returns the client-side connection plus both endpoints.
async fn connected_pair() -> (rds_net::Connection, rds_net::Endpoint, rds_net::Endpoint) {
    let cfg = || EndpointConfig::default().without_discovery();
    let server = bind_endpoint(cfg()).await.unwrap();
    let client = bind_endpoint(cfg()).await.unwrap();

    let server_ep = server.clone();
    let accept = tokio::spawn(async move { server_ep.accept().await.unwrap().await.unwrap() });

    let mut addrs = BTreeSet::new();
    for a in server.addr().addrs {
        if let TransportAddr::Ip(sa) = a {
            addrs.insert(TransportAddr::Ip(sa));
        }
    }
    let conn = client
        .connect(
            EndpointAddr {
                id: server.id(),
                addrs,
            },
            rds_core::ALPN,
        )
        .await
        .unwrap();
    let _server_conn = accept.await.unwrap();
    (conn, server, client)
}

/// `recv` must yield `None` once the connection dies — the demux exits
/// and its registered senders drop, ending the inbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uni_recv_ends_when_connection_dies() {
    let (conn, _server, _client) = connected_pair().await;
    let mut uni = conn.uni_streams(rds_core::UniHello::Sync).unwrap();

    conn.close(0u32.into(), b"done");
    tokio::time::sleep(Duration::from_millis(200)).await;

    match tokio::time::timeout(Duration::from_secs(10), uni.recv()).await {
        Ok(None) => {}
        Ok(Some(_)) => panic!("uni stream arrived after close"),
        Err(_) => panic!("uni.recv() hung after connection death"),
    }
}

/// A claim on an already-dead connection ends immediately — the demux
/// respawns, fails `accept_uni`, and drops the fresh sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uni_streams_on_dead_connection_end() {
    let (conn, _server, _client) = connected_pair().await;
    conn.close(0u32.into(), b"done");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut uni = conn.uni_streams(rds_core::UniHello::Desktop).unwrap();
    match tokio::time::timeout(Duration::from_secs(10), uni.recv()).await {
        Ok(None) => {}
        Ok(Some(_)) => panic!("uni stream arrived on dead connection"),
        Err(_) => panic!("uni.recv() hung on dead connection"),
    }
}
