//! Hostile/incomplete desktop streams over both real transport backends.
use std::time::Duration;

use rds_core::{
    Codec, DesktopCaps, DesktopControl, DesktopEvent, DesktopHello, FrameHeader, HelloAck,
    StreamHello, UniHello,
};
use rds_desktop::client::{DesktopSession, SessionOpts};
use rds_net::{
    Backend, Connection, Endpoint, EndpointConfig, RecvStream, SendStream, read_frame, write_frame,
};

// These scenarios intentionally consume the process-wide eight-reader budget.
// Isolate fixtures while retaining concurrency within each real connection.
static SESSION_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pair(backend: Backend) -> (Endpoint, Endpoint, Connection, Connection) {
    let config = EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let client = rds_net::bind_endpoint(config.clone()).await.unwrap();
    let server = rds_net::bind_endpoint(config).await.unwrap();
    let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
        server.accept().await.unwrap().await
    });
    (client, server, a.unwrap(), b.unwrap())
}

async fn session(
    a: &Connection,
    b: &Connection,
) -> (DesktopSession, SendStream, RecvStream, [u8; 16]) {
    let (client, streams) = tokio::join!(DesktopSession::connect(a, 0, 30, Codec::H264), async {
        let (mut send, mut recv) = b.accept_bi().await.unwrap();
        let StreamHello::DesktopV2 { session, .. } = read_frame(&mut recv).await.unwrap() else {
            panic!("expected DesktopV2 hello")
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
    });
    (client.unwrap(), streams.0, streams.1, streams.2)
}

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("session resources did not reach the expected state");
}

async fn tagged(b: &Connection, session: [u8; 16]) -> SendStream {
    let mut stream = b.open_uni().await.unwrap();
    write_frame(&mut stream, &UniHello::DesktopFrames { id: session })
        .await
        .unwrap();
    stream
}

fn header(seq: u64) -> FrameHeader {
    FrameHeader {
        seq,
        capture_ts_ms: 0,
        encode_done_ts_ms: 0,
        send_ts_ms: 0,
        keyframe: true,
        codec: Codec::H264,
        width: 640,
        height: 480,
    }
}

async fn rejected_header(b: &Connection, session: [u8; 16], h: FrameHeader) {
    let mut stream = tagged(b, session).await;
    write_frame(&mut stream, &h).await.unwrap();
    // No body or FIN: refusal must happen from the header alone.
    assert!(
        tokio::time::timeout(Duration::from_secs(3), stream.stopped())
            .await
            .expect("invalid header retained its stream")
            .unwrap()
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completing_a_delta_first_preserves_the_inflight_keyframe() {
    let _isolation = SESSION_TEST.lock().await;
    for backend in [Backend::Iroh, Backend::Noq] {
        let (client_ep, server_ep, a, b) = pair(backend).await;
        let (mut client, control_send, control_recv, id) = session(&a, &b).await;
        let mut keyframe = tagged(&b, id).await;
        write_frame(&mut keyframe, &header(0)).await.unwrap();
        keyframe.write_all(b"keyframe prefix").await.unwrap();
        let mut delta = tagged(&b, id).await;
        let mut delta_header = header(1);
        delta_header.keyframe = false;
        write_frame(&mut delta, &delta_header).await.unwrap();
        delta.write_all(b"later delta").await.unwrap();
        delta.finish().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(350), client.frame_headers.recv())
                .await
                .is_err()
        );
        keyframe.write_all(b" keyframe tail").await.unwrap();
        keyframe.finish().unwrap();
        for seq in [0, 1] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), client.frame_headers.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .seq,
                seq
            );
        }
        drop((client, control_send, control_recv));
        tokio::join!(client_ep.close(), server_ep.close());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_progressing_delta_reference_survives_the_reorder_window() {
    let _isolation = SESSION_TEST.lock().await;
    for backend in [Backend::Iroh, Backend::Noq] {
        let (client_ep, server_ep, a, b) = pair(backend).await;
        let (mut client, control_send, control_recv, id) = session(&a, &b).await;
        let mut keyframe = tagged(&b, id).await;
        write_frame(&mut keyframe, &header(0)).await.unwrap();
        keyframe.write_all(b"initial keyframe").await.unwrap();
        keyframe.finish().unwrap();
        assert_eq!(client.frame_headers.recv().await.unwrap().seq, 0);

        let mut reference = tagged(&b, id).await;
        let mut h = header(1);
        h.keyframe = false;
        write_frame(&mut reference, &h).await.unwrap();
        reference.write_all(b"reference prefix").await.unwrap();
        until(|| client.receive_stats().in_flight == 1).await;
        let mut successor = tagged(&b, id).await;
        h.seq = 2;
        write_frame(&mut successor, &h).await.unwrap();
        successor.write_all(b"successor").await.unwrap();
        successor.finish().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(350), client.frame_headers.recv())
                .await
                .is_err()
        );
        reference.write_all(b" reference tail").await.unwrap();
        reference.finish().unwrap();
        for seq in [1, 2] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), client.frame_headers.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .seq,
                seq,
                "a later completed frame discarded its admitted reference"
            );
        }
        drop((client, control_send, control_recv));
        tokio::join!(client_ep.close(), server_ep.close());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wan_reference_header_can_arrive_after_the_lan_reorder_window() {
    let _isolation = SESSION_TEST.lock().await;
    for backend in [Backend::Iroh, Backend::Noq] {
        let (client_ep, server_ep, a, b) = pair(backend).await;
        let (client, streams) = tokio::join!(
            DesktopSession::connect_opts(
                &a,
                DesktopHello {
                    display: 0,
                    codec: Codec::H264,
                    max_fps: 30,
                    input_acks: false
                },
                SessionOpts {
                    session: Some([73; 16]),
                    relay_encoded: true,
                    ..Default::default()
                },
            ),
            async {
                let (mut send, mut recv) = b.accept_bi().await.unwrap();
                let StreamHello::DesktopV2 { session, .. } = read_frame(&mut recv).await.unwrap()
                else {
                    panic!("isolated desktop expected")
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
        let (mut control_send, mut control_recv, id) = streams;
        // The managed viewer's clock predates this desktop session after
        // reconnect. Echo timestamps remain caller-owned on the wire.
        client
            .send_control(DesktopControl::Heartbeat {
                seq: 734,
                ts_ms: 1_000_000,
            })
            .await
            .unwrap();
        let DesktopControl::Heartbeat { seq, ts_ms } = read_frame(&mut control_recv).await.unwrap()
        else {
            panic!("heartbeat expected")
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        write_frame(&mut control_send, &DesktopEvent::Heartbeat { seq, ts_ms })
            .await
            .unwrap();
        until(|| {
            client
                .control_rtt()
                .is_some_and(|rtt| rtt >= Duration::from_millis(200))
        })
        .await;

        let mut key = tagged(&b, id).await;
        write_frame(&mut key, &header(0)).await.unwrap();
        key.write_all(b"initial").await.unwrap();
        key.finish().unwrap();
        assert_eq!(client.frame_headers.recv().await.unwrap().seq, 0);
        assert_eq!(
            client
                .encoded
                .as_mut()
                .unwrap()
                .recv()
                .await
                .unwrap()
                .header
                .seq,
            0
        );
        let mut successor = tagged(&b, id).await;
        let mut h = header(2);
        h.keyframe = false;
        write_frame(&mut successor, &h).await.unwrap();
        successor.write_all(b"successor").await.unwrap();
        successor.finish().unwrap();
        // No reference reader has been admitted yet. A WAN-sized gap in tag
        // arrival must not cause an IDR before its missing stream arrives.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(250),
                read_frame::<_, DesktopControl>(&mut control_recv)
            )
            .await
            .is_err()
        );
        let mut reference = tagged(&b, id).await;
        h.seq = 1;
        write_frame(&mut reference, &h).await.unwrap();
        reference.write_all(b"reference").await.unwrap();
        reference.finish().unwrap();
        for expected in [1, 2] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), client.frame_headers.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .seq,
                expected
            );
            assert_eq!(
                client
                    .encoded
                    .as_mut()
                    .unwrap()
                    .recv()
                    .await
                    .unwrap()
                    .header
                    .seq,
                expected
            );
        }
        drop((client, control_send, control_recv));
        tokio::join!(client_ep.close(), server_ep.close());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_keyframe_completes_while_delta_deadline_stays_short() {
    let _isolation = SESSION_TEST.lock().await;
    for backend in [Backend::Iroh, Backend::Noq] {
        let (client_ep, server_ep, a, b) = pair(backend).await;
        let (mut client, control_send, control_recv, id) = session(&a, &b).await;
        let mut keyframe = tagged(&b, id).await;
        write_frame(&mut keyframe, &header(0)).await.unwrap();
        keyframe.write_all(b"recovery prefix").await.unwrap();
        until(|| client.receive_stats().in_flight == 1).await;
        tokio::time::sleep(Duration::from_millis(4200)).await;
        keyframe.write_all(b" recovery tail").await.unwrap();
        keyframe.finish().unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), client.frame_headers.recv())
                .await
                .unwrap()
                .unwrap()
                .seq,
            0,
            "a progressing recovery keyframe was discarded"
        );
        let mut delta = tagged(&b, id).await;
        let mut delta_header = header(1);
        delta_header.keyframe = false;
        write_frame(&mut delta, &delta_header).await.unwrap();
        delta.write_all(b"unfinished delta").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(3800), delta.stopped())
                .await
                .expect("delta inherited the longer recovery deadline")
                .unwrap()
                .is_some()
        );
        drop((client, control_send, control_recv));
        tokio::join!(client_ep.close(), server_ep.close());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readers_are_bounded_and_session_owns_all_streams() {
    let _isolation = SESSION_TEST.lock().await;
    for backend in [Backend::Iroh, Backend::Noq] {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (client_ep, server_ep, a, b) = pair(backend).await;
            let (mut client, mut control_send, mut control_recv, session_id) =
                session(&a, &b).await;

            // Reclaiming this session's own route is refused locally —
            // before another greeting reaches the wire.
            assert!(
                DesktopSession::connect_opts(
                    &a,
                    DesktopHello {
                        display: 0,
                        max_fps: 30,
                        codec: Codec::H264,
                        input_acks: false,
                    },
                    SessionOpts {
                        session: Some(session_id),
                        ..Default::default()
                    },
                )
                .await
                .is_err()
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(30), b.accept_bi())
                    .await
                    .is_err()
            );

            // A different session ID is a different route: concurrent v2
            // sessions share the connection without colliding, and each
            // sees only its own frame streams.
            let (second, second_streams) =
                tokio::join!(DesktopSession::connect(&a, 0, 30, Codec::H264), async {
                    let (mut send, mut recv) = b.accept_bi().await.unwrap();
                    let StreamHello::DesktopV2 { session, .. } =
                        read_frame(&mut recv).await.unwrap()
                    else {
                        panic!("expected DesktopV2 hello")
                    };
                    assert_ne!(session, session_id, "minted session IDs collide");
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
                });
            let mut second = second.unwrap();
            let (s2_send, s2_recv, session2_id) = second_streams;
            let mut probe = tagged(&b, session_id).await;
            write_frame(&mut probe, &header(0)).await.unwrap();
            probe.write_all(b"only session one's route").await.unwrap();
            probe.finish().unwrap();
            let mut probe2 = tagged(&b, session2_id).await;
            write_frame(&mut probe2, &header(9)).await.unwrap();
            probe2.write_all(b"only session two's route").await.unwrap();
            probe2.finish().unwrap();
            assert_eq!(client.frame_headers.recv().await.unwrap().seq, 0);
            assert_eq!(second.frame_headers.recv().await.unwrap().seq, 9);
            drop(second);
            drop((s2_send, s2_recv));
            until(|| a.uni_routing_stats().routes == 1).await;

            let mut stalled = Vec::new();
            for _ in 0..client.receive_stats().max_in_flight {
                let mut stream = tagged(&b, session_id).await;
                stream.write_all(&[0]).await.unwrap(); // incomplete length prefix
                stalled.push(stream);
            }
            until(|| client.receive_stats().in_flight == stalled.len()).await;
            let stats = client.receive_stats();
            assert!(stats.global_in_flight <= stats.global_max_in_flight);
            let mut excess = tagged(&b, session_id).await;
            excess.write_all(&[0]).await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(3), excess.stopped())
                    .await
                    .expect("over-budget reader was not refused")
                    .unwrap()
                    .is_some()
            );
            assert_eq!(client.receive_stats().in_flight, stalled.len());

            // Video saturation cannot block the independent control leg.
            // Decoded probes above legitimately queue resync requests on
            // the same leg (a real decoder rejects the synthetic payload);
            // they share the leg, they must not starve the heartbeat.
            let seq = client.heartbeat().await.unwrap();
            let (got, ts_ms) = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let DesktopControl::Heartbeat { seq, ts_ms } =
                        read_frame(&mut control_recv).await.unwrap()
                    {
                        break (seq, ts_ms);
                    }
                }
            })
            .await
            .expect("heartbeat lost under video pressure");
            assert_eq!(seq, got);
            write_frame(&mut control_send, &DesktopEvent::Heartbeat { seq, ts_ms })
                .await
                .unwrap();
            assert!(matches!(
                client.events.recv().await,
                Some(DesktopEvent::Heartbeat { .. })
            ));

            drop(client);
            for stream in stalled {
                assert!(
                    tokio::time::timeout(Duration::from_secs(3), stream.stopped())
                        .await
                        .expect("frame reader survived session drop")
                        .unwrap()
                        .is_some()
                );
            }
            assert!(
                read_frame::<_, DesktopControl>(&mut control_recv)
                    .await
                    .is_err()
            );
            assert!(control_send.stopped().await.unwrap().is_some());
            until(|| a.uni_routing_stats().routes == 0).await;
            drop((control_send, control_recv));

            let (mut client, mut control_send, mut control_recv, session_id) =
                session(&a, &b).await;
            rejected_header(&b, session_id, header(u64::MAX)).await;
            let mut oversized = header(0);
            oversized.width = u32::MAX;
            rejected_header(&b, session_id, oversized).await;
            let mut empty = header(0);
            empty.height = 0;
            rejected_header(&b, session_id, empty).await;
            assert!(client.frame_headers.is_empty());
            let mut good = tagged(&b, session_id).await;
            write_frame(&mut good, &header(0)).await.unwrap();
            good.write_all(b"synthetic payload for header tap")
                .await
                .unwrap();
            good.finish().unwrap();
            assert_eq!(client.frame_headers.recv().await.unwrap().seq, 0);

            let mut stalled = tagged(&b, session_id).await;
            stalled.write_all(&[0]).await.unwrap();
            until(|| client.receive_stats().in_flight == 1).await;
            // Control EOF must end the session while its public handle lives.
            control_send.finish().unwrap();
            until(|| a.uni_routing_stats().routes == 0).await;
            assert!(client.frame_headers.recv().await.is_none());
            assert!(client.heartbeat().await.is_err());
            assert!(stalled.stopped().await.unwrap().is_some());
            while read_frame::<_, DesktopControl>(&mut control_recv)
                .await
                .is_ok()
            {}
            drop(client);

            // Canceling a handshake owns/reset its streams and releases route.
            let connect = async {
                tokio::time::timeout(
                    Duration::from_millis(100),
                    DesktopSession::connect(&a, 0, 30, Codec::H264),
                )
                .await
            };
            let (result, (send, mut recv)) = tokio::join!(connect, async {
                let (send, mut recv) = b.accept_bi().await.unwrap();
                assert!(matches!(
                    read_frame(&mut recv).await.unwrap(),
                    StreamHello::Desktop(_) | StreamHello::DesktopV2 { .. }
                ));
                (send, recv) // deliberately withhold ACK
            });
            assert!(result.is_err());
            assert!(read_frame::<_, DesktopControl>(&mut recv).await.is_err());
            assert!(send.stopped().await.unwrap().is_some());
            until(|| a.uni_routing_stats().routes == 0).await;

            // The transport remains available to unrelated service streams.
            let (mut send, _) = a.open_bi().await.unwrap();
            send.write_all(b"still connected").await.unwrap();
            send.finish().unwrap();
            let (_, mut recv) = b.accept_bi().await.unwrap();
            assert_eq!(recv.read_to_end(32).await.unwrap(), b"still connected");
            client_ep.close().await;
            server_ep.close().await;
        })
        .await
        .unwrap_or_else(|_| panic!("{backend:?} client lifecycle exceeded fixture budget"));
    }
}
