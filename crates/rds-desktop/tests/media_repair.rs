//! Explicit media repair must bypass an obsolete key's delivery barrier.
//! Synthetic transport/input only: never touches a host display or input seat.
use bytes::Bytes;
use rds_core::{
    Codec, DesktopControl, DesktopEvent, DesktopHello, FrameHeader, InputEvent, InputKind,
};
use rds_desktop::{
    Decoder, DesktopError, EncodedFrame, Encoder, FrameProducer, InputSink, Produced,
    ProducerControls, RawFrame, SessionClock, SessionConfig, serve_desktop_with,
};
use rds_net::{Backend, EndpointConfig, read_frame, write_frame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct BlockedKey {
    stop: std::sync::mpsc::Receiver<()>,
    deltas: bool,
    produced: Arc<AtomicU64>,
    old_bytes: usize,
    encoder: Option<Box<dyn Encoder>>,
}

impl FrameProducer for BlockedKey {
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        let blocked_end = if self.deltas { 3 } else { 0 };
        if seq > blocked_end + 1 {
            let _ = self.stop.recv_timeout(Duration::from_secs(3));
            return None;
        }
        self.produced.fetch_add(1, Ordering::Release);
        let requested = controls.idr.swap(false, Ordering::AcqRel);
        let (keyframe, payload, side) = if let Some(encoder) = self.encoder.as_mut() {
            if requested {
                encoder.request_idr();
            }
            let shade = (50 + seq * 43 % 150) as u8;
            let raw = RawFrame {
                width: 64,
                height: 64,
                stride: 256,
                data: Bytes::from([shade, shade, shade, 255].repeat(64 * 64)),
            };
            let encoded = encoder.encode(&raw).unwrap();
            (encoded.keyframe, encoded.data, 64)
        } else {
            (requested, Bytes::from_static(&[7; 128]), 16)
        };
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: clock.now_ms(),
                encode_done_ts_ms: clock.now_ms(),
                send_ts_ms: 0,
                keyframe,
                codec: Codec::H264,
                width: side,
                height: side,
            },
            // Exceed the receiver's per-stream credit without draining it.
            // The following independent picture is deliberately small.
            payload: if seq <= blocked_end && (!self.deltas || seq > 0) {
                Bytes::from(vec![1; self.old_bytes])
            } else {
                payload
            },
        })
    }
}

struct Input(Arc<AtomicU64>);
impl InputSink for Input {
    fn inject(&mut self, _: &InputEvent) -> Result<(), DesktopError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

type Case = (
    (Backend, bool, bool),
    Option<Box<dyn Encoder>>,
    Option<Box<dyn Decoder>>,
);

#[cfg(feature = "x11")]
fn native_cases() -> Vec<Case> {
    [Backend::Iroh, Backend::Noq]
        .into_iter()
        .map(|backend| {
            (
                (backend, false, false),
                Some(
                    Box::new(rds_desktop::H264Encoder::new(4_000_000, 60.0).unwrap())
                        as Box<dyn Encoder>,
                ),
                Some(Box::new(rds_desktop::H264Decoder::new().unwrap()) as Box<dyn Decoder>),
            )
        })
        .collect()
}
#[cfg(not(feature = "x11"))]
fn native_cases() -> Vec<Case> {
    Vec::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_repair_supersedes_blocked_media_without_closing_controls() {
    let cases = [Backend::Iroh, Backend::Noq]
        .into_iter()
        .flat_map(|backend| [false, true].map(move |deltas| (backend, deltas, false)))
        .chain([(Backend::Iroh, false, true)])
        .map(|case| (case, None, None))
        .chain(native_cases());
    for ((backend, deltas, delayed_receipt), encoder, mut decoder) in cases {
        tokio::time::timeout(Duration::from_secs(20), async {
            let config = EndpointConfig {
                backend,
                discovery: false,
                bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                max_multipath_paths: delayed_receipt.then_some(1),
                ..Default::default()
            };
            let client = rds_net::bind_endpoint(config.clone()).await.unwrap();
            let server = rds_net::bind_endpoint(config).await.unwrap();
            let mut target = server.addr();
            let proxy = if delayed_receipt {
                let upstream = target
                    .addrs
                    .iter()
                    .find_map(|addr| {
                        if let rds_core::TransportAddr::Ip(ip) = addr {
                            Some(*ip)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let proxy = rds_bench::impair::spawn(
                    upstream,
                    rds_bench::impair::Impairment {
                        delay_ms: 80,
                        rate_mbps: Some(1.0),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                target.addrs = [rds_core::TransportAddr::Ip(proxy.listen)]
                    .into_iter()
                    .collect();
                Some(proxy)
            } else {
                None
            };
            let (a, b) = tokio::join!(client.connect(target, rds_core::ALPN), async {
                server.accept().await.unwrap().await
            });
            let (a, b) = (a.unwrap(), b.unwrap());
            let mut frames = a.uni_streams(rds_core::UniHello::Desktop).unwrap();
            let (mut control, mut replies) = a.open_bi().await.unwrap();
            write_frame(
                &mut control,
                &DesktopControl::Heartbeat { seq: 0, ts_ms: 0 },
            )
            .await
            .unwrap();
            let (send, recv) = b.accept_bi().await.unwrap();
            let (stop, stopped) = std::sync::mpsc::channel();
            let injected = Arc::new(AtomicU64::new(0));
            let produced = Arc::new(AtomicU64::new(0));
            let serving = tokio::spawn(serve_desktop_with(
                b.clone(),
                send,
                recv,
                DesktopHello {
                    display: 0,
                    max_fps: 60,
                    codec: Codec::H264,
                    input_acks: true,
                },
                SessionConfig {
                    producer: Some(Box::new(BlockedKey {
                        stop: stopped,
                        deltas,
                        produced: produced.clone(),
                        // Below stream credit, enqueue/FIN complete while
                        // the rate-capped proxy keeps the receipt pending.
                        old_bytes: if delayed_receipt {
                            512 * 1024
                        } else {
                            16 * 1024 * 1024
                        },
                        encoder,
                    })),
                    input_sink: Some(Box::new(Input(injected.clone()))),
                    ..Default::default()
                },
            ));
            let mut old = frames.recv().await.unwrap();
            let mut old_header: FrameHeader = read_frame(&mut old).await.unwrap();
            assert_eq!(old_header.seq, 0);
            assert!(old_header.keyframe);
            let mut held = Vec::new();
            if deltas {
                assert_eq!(old.read_to_end(512).await.unwrap(), &[7; 128]);
                // One partial send and two queued references fill all three
                // admission slots. Do not require their blocked stream tags
                // to arrive before requesting the repair that releases them.
                let mut stream = frames.recv().await.unwrap();
                let header: FrameHeader = read_frame(&mut stream).await.unwrap();
                assert_eq!(header.seq, 1);
                assert!(!header.keyframe);
                held.push(stream);
                old_header = header;
                tokio::time::timeout(Duration::from_secs(2), async {
                    while produced.load(Ordering::Acquire) < 4 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("fixture did not fill its media admission slots");
            } else {
                held.push(old);
            }
            // Keep bodies unread: FIN cannot be delivered within stream credit.
            let observation_started = std::time::Instant::now();
            write_frame(&mut control, &DesktopControl::RequestIdr)
                .await
                .unwrap();
            write_frame(
                &mut control,
                &DesktopControl::Input(InputEvent {
                    seq: 17,
                    event_ts_ms: 0,
                    display_id: 0,
                    kind: InputKind::KeyDown { code: 30 },
                }),
            )
            .await
            .unwrap();
            write_frame(
                &mut control,
                &DesktopControl::Heartbeat { seq: 77, ts_ms: 11 },
            )
            .await
            .unwrap();
            let mut input_acked = false;
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    match read_frame::<_, DesktopEvent>(&mut replies).await.unwrap() {
                        DesktopEvent::InputAck { seq: 17, .. } => input_acked = true,
                        DesktopEvent::Heartbeat { seq: 77, ts_ms: 11 } => break,
                        _ => {}
                    }
                }
            })
            .await
            .expect("control was blocked by old media");
            assert!(input_acked);
            assert_eq!(injected.load(Ordering::Relaxed), 1);
            // Echo is a fence: the preceding RequestIdr was handled by the peer.
            let fresh = tokio::time::timeout(Duration::from_secs(1), async {
                let mut stream = frames.recv().await.unwrap();
                let header: FrameHeader = read_frame(&mut stream).await.unwrap();
                let body = stream.read_to_end(512).await.unwrap();
                (header, body)
            })
            .await;
            println!(
                "backend={backend:?} deltas={deltas} delayed_receipt={delayed_receipt} request_to_observation_ms={}",
                observation_started.elapsed().as_millis()
            );
            if let Some(proxy) = &proxy {
                assert!(
                    proxy.stats().forwarded > 10,
                    "receipt fixture escaped its proxy"
                );
                assert!(a.current_path_stats().unwrap().rtt >= Duration::from_millis(100));
            }
            if fresh.is_ok() {
                for mut old in held {
                    let reset = tokio::time::timeout(Duration::from_secs(2), async {
                        let mut chunk = [0; 16 * 1024];
                        loop {
                            match old.read(&mut chunk).await {
                                Ok(Some(_)) => {}
                                Err(rds_net::ReadError::Reset(code)) => break code,
                                other => panic!("old media was not reset: {other:?}"),
                            }
                        }
                    })
                    .await
                    .expect("old frame was retained after recovery");
                    assert_eq!(reset, 1u32.into());
                }
                // An unrelated stream still works after media-only cancellation.
                let (mut other, mut echo) = a.open_bi().await.unwrap();
                other.write_all(b"other").await.unwrap();
                let (mut other_send, mut other_recv) = b.accept_bi().await.unwrap();
                let mut bytes = [0; 5];
                other_recv.read_exact(&mut bytes).await.unwrap();
                assert_eq!(&bytes, b"other");
                other_send.write_all(b"usable").await.unwrap();
                let mut bytes = [0; 6];
                echo.read_exact(&mut bytes).await.unwrap();
                assert_eq!(&bytes, b"usable");
            }
            let _ = stop.send(());
            serving.abort();
            let _ = serving.await;
            a.close(0u32.into(), b"fixture done");
            b.close(0u32.into(), b"fixture done");
            tokio::join!(client.close(), server.close());
            let (header, body) =
                fresh.expect("repair waited for the obsolete key receipt deadline");
            assert!(header.seq > old_header.seq);
            assert!(
                header.keyframe,
                "repair must start an independent reference chain"
            );
            if let Some(decoder) = decoder.as_mut() {
                let raw = decoder.decode(&EncodedFrame { codec: header.codec,
                    keyframe: header.keyframe, data: Bytes::from(body) }).unwrap().unwrap();
                assert_eq!((raw.width, raw.height), (64, 64));
                let expected = (50 + header.seq * 43 % 150) as i16;
                assert!((i16::from(raw.data[0]) - expected).abs() < 12,
                    "recovered native pixel did not match the new independent picture");
            } else { assert_eq!(body, &[7; 128]); }
        })
        .await
        .expect("media repair fixture did not release its resources");
    }
}
