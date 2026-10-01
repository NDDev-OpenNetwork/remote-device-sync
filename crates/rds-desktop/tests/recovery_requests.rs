//! One missing reference must not queue another recovery while its key is pending.
use rds_core::{
    Codec, DesktopCaps, DesktopControl, DesktopHello, FrameHeader, HelloAck, StreamHello, UniHello,
};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_net::{Backend, Connection, EndpointConfig, read_frame, write_frame};
use std::time::Duration;

async fn frame(conn: &Connection, id: [u8; 16], seq: u64, keyframe: bool) {
    let mut stream = conn.open_uni().await.unwrap();
    write_frame(&mut stream, &UniHello::DesktopFrames { id })
        .await
        .unwrap();
    write_frame(
        &mut stream,
        &FrameHeader {
            seq,
            keyframe,
            capture_ts_ms: 0,
            encode_done_ts_ms: 0,
            send_ts_ms: 0,
            codec: Codec::H264,
            width: 32,
            height: 32,
        },
    )
    .await
    .unwrap();
    stream
        .write_all(b"synthetic encoded relay payload")
        .await
        .unwrap();
    stream.finish().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_gap_waits_for_one_recovery_but_a_later_gap_requests_another() {
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
                    panic!("expected tagged session")
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
        let (_control_send, mut control_recv, id) = server;
        let (control_tx, mut controls) = tokio::sync::mpsc::channel(16);
        let reader = tokio::spawn(async move {
            while let Ok(message) = read_frame::<_, DesktopControl>(&mut control_recv).await {
                if control_tx.send(message).await.is_err() {
                    break;
                }
            }
        });
        frame(&b, id, 0, true).await;
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
        frame(&b, id, 2, false).await; // reference 1 never arrives
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), controls.recv())
                .await
                .unwrap(),
            Some(DesktopControl::RequestIdr)
        ));
        tokio::time::sleep(Duration::from_millis(600)).await;
        frame(&b, id, 3, false).await; // another successor of the same missing reference
        assert!(
            tokio::time::timeout(Duration::from_millis(800), controls.recv())
                .await
                .is_err(),
            "same recovery episode queued a second expensive keyframe"
        );
        frame(&b, id, 4, true).await;
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
        frame(&b, id, 6, false).await; // a new gap after successful recovery
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(2), controls.recv())
                    .await
                    .unwrap(),
                Some(DesktopControl::RequestIdr)
            ),
            "a new broken chain still needs recovery"
        );
        drop(client);
        reader.abort();
        let _ = reader.await;
        a.close(0u32.into(), b"fixture complete");
        b.close(0u32.into(), b"fixture complete");
        client_ep.close().await;
        server_ep.close().await;
    }
}
