//! Diagnose whether the existing transport fixture provokes codec failures.
use rds_desktop::{
    Decoder, FrameProducer, H264Decoder, ProducerControls, SessionClock, SyntheticProducer,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64},
};
use std::time::Instant;

fn main() -> Result<(), rds_desktop::DesktopError> {
    for bytes in [256, 1024] {
        let controls = ProducerControls {
            bitrate: Arc::new(AtomicU64::new(4_000_000)),
            idr: Arc::new(AtomicBool::new(true)),
            deadline_misses: Arc::new(AtomicU64::new(0)),
            requested: Arc::new(AtomicU64::new(0)),
            input_refresh_until_ms: Arc::new(AtomicU64::new(0)),
            input_refresh_pending: Arc::new(AtomicBool::new(false)),
        };
        // Cadence is accelerated for this codec-only probe; payload bytes,
        // sequence prefixes and keyframe flags come from the real fixture.
        let mut producer = SyntheticProducer::new(100_000, 640, 480, bytes).keyframe_every(60);
        let mut decoder = H264Decoder::new()?;
        let clock = SessionClock::default();
        let (mut decoded, mut buffered, mut failed) = (0, 0, 0);
        let mut durations = vec![];
        for seq in 0..1024 {
            let frame = producer
                .produce(seq, &controls, &clock)
                .expect("synthetic producer");
            let encoded = rds_desktop::EncodedFrame {
                codec: frame.header.codec,
                keyframe: frame.header.keyframe,
                data: frame.payload,
            };
            let start = Instant::now();
            match decoder.decode(&encoded) {
                Ok(Some(_)) => decoded += 1,
                Ok(None) => buffered += 1,
                Err(_) => {
                    failed += 1;
                    decoder = H264Decoder::new()?;
                }
            }
            durations.push(start.elapsed().as_nanos());
        }
        durations.sort_unstable();
        println!(
            "payload_bytes={bytes} samples=1024 decoded={decoded} buffered={buffered} failed={failed} decode_p95_ns={} decode_max_ns={}",
            durations[972], durations[1023]
        );
    }
    Ok(())
}
