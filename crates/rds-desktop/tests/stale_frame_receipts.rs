//! A late predecessor after a newer key is obsolete, not a new broken chain.
use rds_core::{
    Codec, DesktopCaps, DesktopControl, DesktopHello, FrameHeader, HelloAck, StreamHello, UniHello,
};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_net::{Backend, Connection, EndpointConfig, SendStream, read_frame, write_frame};
use std::time::Duration;
fn header(seq: u64, keyframe: bool) -> FrameHeader {
    FrameHeader {
        seq,
        keyframe,
        capture_ts_ms: 0,
        encode_done_ts_ms: 0,
        send_ts_ms: 0,
        codec: Codec::H264,
        width: 32,
        height: 32,
    }
}
async fn tagged(conn: &Connection, id: [u8; 16]) -> SendStream {
    let mut stream = conn.open_uni().await.unwrap();
    write_frame(&mut stream, &UniHello::DesktopFrames { id })
        .await
        .unwrap();
    stream
}
async fn complete(conn: &Connection, id: [u8; 16], seq: u64) {
    let mut s = tagged(conn, id).await;
    write_frame(&mut s, &header(seq, true)).await.unwrap();
    s.write_all(b"encoded relay fixture").await.unwrap();
    s.finish().unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_header_after_recovery_does_not_request_another_key_but_malformed_does() {
    for backend in [Backend::Iroh, Backend::Noq] {
        let config = EndpointConfig {
            backend,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let client_ep = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let server_ep = rds_net::bind_endpoint(config).await.unwrap();
        let (a, b) = tokio::join!(client_ep.connect(server_ep.addr(), rds_core::ALPN), async {
            server_ep.accept().await.unwrap().await
        });
        let (a, b) = (a.unwrap(), b.unwrap());
        let (client, server) = tokio::join!(
            DesktopSession::connect_opts(
                &a,
                DesktopHello {
                    display: 0,
                    max_fps: 30,
                    codec: Codec::H264,
                    input_acks: false
                },
                SessionOpts {
                    session: Some(rand::random()),
                    relay_encoded: true,
                    ..Default::default()
                }
            ),
            async {
                let (mut send, mut recv) = b.accept_bi().await.unwrap();
                let StreamHello::DesktopV2 { session, .. } = read_frame(&mut recv).await.unwrap()
                else {
                    panic!("tagged desktop expected")
                };
                write_frame(
                    &mut send,
                    &HelloAck::Desktop(DesktopCaps {
                        displays: vec![],
                        codecs: vec![Codec::H264],
                    }),
                )
                .await
                .unwrap();
                (send, recv, session)
            }
        );
        let mut client = client.unwrap();
        let (_send, mut recv, id) = server;
        let (tx, mut controls) = tokio::sync::mpsc::channel(16);
        let reader = tokio::spawn(async move {
            while let Ok(message) = read_frame::<_, DesktopControl>(&mut recv).await {
                if tx.send(message).await.is_err() {
                    break;
                }
            }
        });
        complete(&b, id, 0).await;
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(2),
                client.encoded.as_mut().unwrap().recv()
            )
            .await
            .unwrap()
            .unwrap()
            .header
            .seq,
            0
        );
        // The predecessor's route is admitted, but its header remains in transit.
        let mut late = tagged(&b, id).await;
        complete(&b, id, 2).await;
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(2),
                client.encoded.as_mut().unwrap().recv()
            )
            .await
            .unwrap()
            .unwrap()
            .header
            .seq,
            2
        );
        write_frame(&mut late, &header(1, false)).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), late.stopped())
                .await
                .unwrap()
                .unwrap(),
            Some(rds_core::DESKTOP_FRAME_OBSOLETE.into()),
            "stale header must release its stream with the exact obsolete disposition"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(600), controls.recv())
                .await
                .is_err(),
            "an obsolete predecessor invalidated the already recovered chain"
        );
        let mut bad = tagged(&b, id).await;
        let mut invalid = header(3, false);
        invalid.width = 0;
        write_frame(&mut bad, &invalid).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), bad.stopped())
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(2), controls.recv())
                    .await
                    .unwrap(),
                Some(DesktopControl::RequestIdr)
            ),
            "malformed current frame still requires repair"
        );
        complete(&b, id, 4).await;
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(2),
                client.encoded.as_mut().unwrap().recv()
            )
            .await
            .unwrap()
            .unwrap()
            .header
            .seq,
            4
        );
        drop(client);
        reader.abort();
        let _ = reader.await;
        a.close(0u32.into(), b"done");
        b.close(0u32.into(), b"done");
        tokio::join!(client_ep.close(), server_ep.close());
    }
}
