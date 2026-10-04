//! A platform damage burst cannot bypass the negotiated frame-rate cap.
use bytes::Bytes;
use rds_core::{Codec, DesktopControl, DesktopHello, FrameHeader};
use rds_desktop::{FrameProducer, Produced, ProducerControls, SessionClock, SessionConfig};
use rds_net::{EndpointConfig, read_frame, write_frame};
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Burst;
impl FrameProducer for Burst {
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: clock.now_ms(),
                encode_done_ts_ms: clock.now_ms(),
                send_ts_ms: 0,
                keyframe: controls.idr.swap(false, Ordering::AcqRel),
                codec: Codec::H264,
                width: 16,
                height: 16,
            },
            payload: Bytes::from_static(&[7; 32]),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_damage_producer_cannot_exceed_negotiated_fps() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let config = EndpointConfig {
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let a = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let b = rds_net::bind_endpoint(config).await.unwrap();
        let (client, server) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        let (client, server) = (client.unwrap(), server.unwrap());
        let mut frames = client.uni_streams(rds_core::UniHello::Desktop).unwrap();
        let (mut control, _events) = client.open_bi().await.unwrap();
        write_frame(
            &mut control,
            &DesktopControl::Heartbeat { seq: 0, ts_ms: 0 },
        )
        .await
        .unwrap();
        let (send, recv) = server.accept_bi().await.unwrap();
        let task = tokio::spawn(rds_desktop::serve_desktop_with(
            server.clone(),
            send,
            recv,
            DesktopHello {
                display: 0,
                max_fps: 5,
                codec: Codec::H264,
                input_acks: false,
            },
            SessionConfig {
                view_only: true,
                producer: Some(Box::new(Burst)),
                ..Default::default()
            },
        ));
        let mut previous = None;
        for _ in 0..5 {
            let mut frame = frames.recv().await.unwrap();
            let header: FrameHeader = read_frame(&mut frame).await.unwrap();
            assert_eq!(frame.read_to_end(32).await.unwrap(), [7; 32]);
            if let Some(previous) = previous {
                assert!(
                    header.capture_ts_ms.saturating_sub(previous) >= 199,
                    "a ready producer bypassed the 5 FPS cap"
                );
            }
            previous = Some(header.capture_ts_ms);
        }
        control.finish().unwrap();
        task.await.unwrap().unwrap();
        client.close(0u32.into(), b"done");
        a.close().await;
        b.close().await;
    })
    .await
    .expect("frame-cadence fixture did not close");
}
