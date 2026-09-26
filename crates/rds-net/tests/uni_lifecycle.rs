//! Connection ownership across the tagged uni-stream router.
use rds_net::{Backend, Connection, Endpoint, EndpointConfig, bind_endpoint};
use std::time::Duration;

async fn pair(backend: Backend) -> (Endpoint, Endpoint, Connection, Connection) {
    let config = || EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let client = bind_endpoint(config()).await.unwrap();
    let server = bind_endpoint(config()).await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    (client, server, a.unwrap(), b.unwrap())
}

async fn discarded_router(backend: Backend) {
    let (client, server, a, b) = pair(backend).await;
    let inbox = a.uni_streams(rds_core::UniHello::Sync).unwrap();
    drop(inbox);
    drop(a);
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while !b.is_closed() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    // Teardown even on the baseline failure so this fixture owns its I/O.
    client.close().await;
    server.close().await;
    assert!(
        closed.is_ok(),
        "{backend:?} uni router retained its connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarded_iroh_uni_router_does_not_keep_connection_alive() {
    discarded_router(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarded_owned_uni_router_does_not_keep_connection_alive() {
    discarded_router(Backend::Noq).await;
}

fn backends() -> Vec<Backend> {
    #[cfg(feature = "transport-noq")]
    {
        vec![Backend::Iroh, Backend::Noq]
    }
    #[cfg(not(feature = "transport-noq"))]
    {
        vec![Backend::Iroh]
    }
}

async fn pending_is(conn: &Connection, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while conn.uni_routing_stats().pending != count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("router did not reach its expected task count");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_keeps_router_alive_after_facade_handles_drop() {
    for backend in backends() {
        let (client, server, a, b) = pair(backend).await;
        let mut inbox = a.uni_streams(rds_core::UniHello::Sync).unwrap();
        assert!(a.uni_streams(rds_core::UniHello::Sync).is_err());
        let clone = a.clone();
        drop(a);
        drop(clone);
        let mut send = b.open_uni().await.unwrap();
        rds_core::write_frame(&mut send, &rds_core::UniHello::Sync)
            .await
            .unwrap();
        send.write_all(b"owned inbox").await.unwrap();
        send.finish().unwrap();
        let mut stream = tokio::time::timeout(Duration::from_secs(2), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stream.read_to_end(32).await.unwrap(), b"owned inbox");
        drop(stream);
        drop(inbox);
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            while !b.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        client.close().await;
        server.close().await;
        assert!(closed.is_ok(), "last inbox did not release its router");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_tags_are_bounded_and_cancelled_on_close() {
    for backend in backends() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (client, server, a, b) = pair(backend).await;
            let mut inbox = a.uni_streams(rds_core::UniHello::Sync).unwrap();
            let limit = a.uni_routing_stats().max_pending;
            let mut stalled = Vec::new();
            for _ in 0..limit + 16 {
                let mut send = b.open_uni().await.unwrap();
                // open_uni alone does not announce a stream. A partial length
                // prefix creates real inbound work and stalls its tag read.
                send.write_all(&[0]).await.unwrap();
                stalled.push(send);
            }
            pending_is(&a, limit).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(a.uni_routing_stats().pending, limit);
            a.close(0u32.into(), b"cancel saturated router");
            pending_is(&a, 0).await;
            assert!(inbox.recv().await.is_none());
            drop(stalled);
            client.close().await;
            server.close().await;
        })
        .await
        .expect("silent-tag shutdown exceeded fixture budget");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_tag_and_reclaimed_kind_preserve_ready_routing() {
    for backend in backends() {
        let (client, server, a, b) = pair(backend).await;
        let old = a.uni_streams(rds_core::UniHello::Sync).unwrap();
        let mut stalled = b.open_uni().await.unwrap();
        stalled.write_all(&[0]).await.unwrap();
        pending_is(&a, 1).await;
        drop(old);
        let mut new = a.uni_streams(rds_core::UniHello::Sync).unwrap();
        let mut ready = b.open_uni().await.unwrap();
        rds_core::write_frame(&mut ready, &rds_core::UniHello::Sync)
            .await
            .unwrap();
        ready.write_all(b"new claim").await.unwrap();
        ready.finish().unwrap();
        let mut stream = tokio::time::timeout(Duration::from_secs(2), new.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stream.read_to_end(32).await.unwrap(), b"new claim");
        pending_is(&a, 1).await;
        a.close(0u32.into(), b"done");
        pending_is(&a, 0).await;
        client.close().await;
        server.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_transfer_tags_cannot_enter_replacement_inboxes_and_routes_are_bounded() {
    use rds_core::UniHello;
    for backend in backends() {
        let (client, server, a, b) = pair(backend).await;
        let old_tag = UniHello::SyncTransfer { id: [1; 16] };
        let old = a.uni_streams(old_tag).unwrap();
        let mut delayed = b.open_uni().await.unwrap();
        // Send only part of the tag prefix: its router worker stays pending.
        delayed.write_all(&[0]).await.unwrap();
        pending_is(&a, 1).await;
        drop(old);
        assert_eq!(a.uni_routing_stats().routes, 0);
        let new_tag = UniHello::SyncTransfer { id: [2; 16] };
        let mut current = a.uni_streams(new_tag).unwrap();
        let mut tag = Vec::new();
        rds_core::write_frame(&mut tag, &old_tag).await.unwrap();
        delayed.write_all(&tag[1..]).await.unwrap();
        delayed.write_all(b"stale").await.unwrap();
        delayed.finish().unwrap();
        let mut valid = b.open_uni().await.unwrap();
        rds_core::write_frame(&mut valid, &new_tag).await.unwrap();
        valid.write_all(b"fresh").await.unwrap();
        valid.finish().unwrap();
        let mut received = tokio::time::timeout(Duration::from_secs(3), current.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.read_to_end(5).await.unwrap(), b"fresh");
        assert!(
            tokio::time::timeout(Duration::from_millis(30), current.recv())
                .await
                .is_err()
        );
        let mut inboxes = Vec::new();
        for id in 3..(a.uni_routing_stats().max_routes + 2) {
            inboxes.push(
                a.uni_streams(UniHello::SyncTransfer { id: [id as u8; 16] })
                    .unwrap(),
            );
        }
        assert_eq!(
            a.uni_routing_stats().routes,
            a.uni_routing_stats().max_routes
        );
        assert!(
            a.uni_streams(UniHello::SyncTransfer { id: [255; 16] })
                .is_err()
        );
        drop(inboxes);
        drop(current);
        assert_eq!(a.uni_routing_stats().routes, 0);
        // Historical transfers do not accumulate map entries.
        for id in 0..128 {
            drop(
                a.uni_streams(UniHello::SyncTransfer { id: [id; 16] })
                    .unwrap(),
            );
        }
        assert_eq!(a.uni_routing_stats().routes, 0);
        client.close().await;
        server.close().await;
    }
}
