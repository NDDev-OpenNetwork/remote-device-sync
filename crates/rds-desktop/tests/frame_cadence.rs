//! A platform damage burst cannot bypass the negotiated frame-rate cap.
use bytes::Bytes;
use rds_core::{Codec, DesktopControl, DesktopHello, FrameHeader};
use rds_desktop::{FrameProducer, Produced, ProducerControls, SessionClock, SessionConfig};
use rds_net::{EndpointConfig, read_frame, write_frame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Burst {
    synthetic: Option<rds_desktop::SyntheticProducer>,
    misses: Arc<AtomicU64>,
}
impl FrameProducer for Burst {
    fn resume_after_backpressure(&mut self) {
        if let Some(source) = &mut self.synthetic {
            source.resume_after_backpressure();
        }
    }
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        if let Some(source) = &mut self.synthetic {
            let frame = source.produce(seq, controls, clock);
            self.misses.store(
                controls.deadline_misses.load(Ordering::Relaxed),
                Ordering::Release,
            );
            return frame;
        }
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
    cadence_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_wait_does_not_manufacture_encoder_starvation() {
    cadence_case(true).await;
}

async fn cadence_case(synthetic: bool) {
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
        let misses = Arc::new(AtomicU64::new(0));
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
                producer: Some(Box::new(Burst {
                    synthetic: synthetic
                        .then(|| rds_desktop::SyntheticProducer::new(30, 16, 16, 64)),
                    misses: misses.clone(),
                })),
                ..Default::default()
            },
        ));
        let mut previous = None;
        let mut initial_misses = None;
        for _ in 0..5 {
            let mut frame = frames.recv().await.unwrap();
            let header: FrameHeader = read_frame(&mut frame).await.unwrap();
            let body = frame.read_to_end(64).await.unwrap();
            if synthetic {
                assert_eq!(&body[..8], &header.seq.to_le_bytes());
            } else {
                assert_eq!(body, [7; 32]);
            }
            if let Some(previous) = previous {
                assert!(
                    header.capture_ts_ms.saturating_sub(previous) >= 199,
                    "a ready producer bypassed the 5 FPS cap"
                );
            }
            previous = Some(header.capture_ts_ms);
            let observed = misses.load(Ordering::Acquire);
            if let Some(initial) = initial_misses {
                assert_eq!(
                    observed, initial,
                    "admission waiting counted as encoder starvation"
                );
            } else {
                initial_misses = Some(observed);
            }
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
