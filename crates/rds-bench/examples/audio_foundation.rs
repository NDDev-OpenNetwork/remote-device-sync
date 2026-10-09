//! Synchronous codec/reorder measurements, not audio-device or WAN acceptance.

use anyhow::{bail, ensure};
use clap::Parser;
use rds_audio::{AudioFormat, JitterBuffer, OpusDecoder, OpusEncoder, PopOutcome};
use serde::Serialize;
use std::{path::PathBuf, time::Instant};

const WARMUP: u64 = 32;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    json: PathBuf,
    #[arg(long)]
    md: PathBuf,
    #[arg(long, default_value_t = 500)]
    frames: u64,
}

#[derive(Serialize)]
struct Timing {
    samples: usize,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
    max_ns: u128,
}

impl Timing {
    fn measured(mut values: Vec<u128>) -> Self {
        values.sort_unstable();
        let percentile = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
        Self {
            samples: values.len(),
            p50_ns: percentile(50),
            p95_ns: percentile(95),
            p99_ns: percentile(99),
            max_ns: values[values.len() - 1],
        }
    }
}

#[derive(Serialize)]
struct Row {
    sample_rate: u32,
    channels: u16,
    frames: u64,
    decoded: u64,
    concealed: u64,
    max_queued: usize,
    max_packet_bytes: usize,
    encoded_bytes: u64,
    encoded_digest: String,
    encode: Timing,
    decode_or_plc: Timing,
    pipeline_ns: u128,
}

#[derive(Serialize)]
struct Report {
    schema_version: u8,
    scenario: &'static str,
    source_commit: String,
    source_tree: String,
    worktree_clean: bool,
    binary_digest: String,
    lockfile_digest: String,
    toolchain: String,
    os: &'static str,
    architecture: &'static str,
    features: [&'static str; 1],
    warmup_frames: u64,
    failures: u64,
    skips: u64,
    rows: Vec<Row>,
    limitations: &'static str,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    ensure!(
        (100..=10_000).contains(&args.frames) && args.frames.is_multiple_of(4),
        "frames must be 100..=10000 and divisible by four"
    );
    let mut rows = Vec::new();
    for sample_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        for channels in [1, 2] {
            rows.push(measure(
                AudioFormat {
                    sample_rate,
                    channels,
                },
                args.frames,
            )?);
        }
    }
    let report = Report {
        schema_version: 1,
        scenario: "audio-codec-reorder-v1",
        source_commit: command("git", &["rev-parse", "HEAD"])?,
        source_tree: command("git", &["rev-parse", "HEAD^{tree}"])?,
        worktree_clean: command("git", &["status", "--porcelain"])?.is_empty(),
        binary_digest: blake3::hash(&std::fs::read(std::env::current_exe()?)?)
            .to_hex()
            .to_string(),
        lockfile_digest: blake3::hash(&std::fs::read("Cargo.lock")?)
            .to_hex()
            .to_string(),
        toolchain: command("rustc", &["--version"])?,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        features: ["default"],
        warmup_frames: WARMUP,
        failures: 0,
        skips: 0,
        rows,
        limitations: "Synthetic synchronous release-build wall time, 20 ms frames and 440 Hz PCM; groups of four arrive in reverse order, with frame 21 of every 50 omitted unless it is the final packet. Every output has the exact PCM length and finite samples, every loss has one PLC interval, and the buffer never exceeds four packets. No timer, device callback, network, optical latency, A/V drift or perceptual-quality measurement. Decode timing includes packet validation/PCM allocation or PLC. No machine-independent speed threshold.",
    };
    let mut md = format!(
        "# Audio foundation benchmark\n\nSource: `{}`; tree: `{}`; clean: `{}`; `{}`; {} / {}.\n\n| Rate | Channels | Frames | PLC | Encode p50/p95/p99 µs | Decode/PLC p50/p95/p99 µs | Peak packets |\n|---:|---:|---:|---:|---|---|---:|\n",
        report.source_commit,
        report.source_tree,
        report.worktree_clean,
        report.toolchain,
        report.os,
        report.architecture
    );
    for row in &report.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:.1}/{:.1}/{:.1} | {:.1}/{:.1}/{:.1} | {} |\n",
            row.sample_rate,
            row.channels,
            row.frames,
            row.concealed,
            row.encode.p50_ns as f64 / 1000.0,
            row.encode.p95_ns as f64 / 1000.0,
            row.encode.p99_ns as f64 / 1000.0,
            row.decode_or_plc.p50_ns as f64 / 1000.0,
            row.decode_or_plc.p95_ns as f64 / 1000.0,
            row.decode_or_plc.p99_ns as f64 / 1000.0,
            row.max_queued
        ));
    }
    md.push_str(&format!("\n{}\n", report.limitations));
    std::fs::write(args.json, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(args.md, md)?;
    Ok(())
}

fn measure(format: AudioFormat, frames: u64) -> anyhow::Result<Row> {
    let mut encoder = OpusEncoder::new(format)?;
    let mut decoder = OpusDecoder::new(format)?;
    let samples = encoder.frame_samples();
    let pcm: Vec<f32> = (0..samples * format.channels as usize)
        .map(|i| {
            let sample = i / format.channels as usize;
            (sample as f32 * 440.0 * std::f32::consts::TAU / format.sample_rate as f32).sin() * 0.25
        })
        .collect();
    for seq in 0..WARMUP {
        decoder.decode(&encoder.encode(&pcm, seq * 20_000)?)?;
    }
    let mut jitter = JitterBuffer::new(format, 4, 0)?;
    jitter.start_at(WARMUP)?;
    let mut encode = Vec::new();
    let mut decode = Vec::new();
    let mut encoded_bytes = 0;
    let mut max_packet_bytes = 0;
    let mut max_queued = 0;
    let mut concealed = 0;
    let mut decoded = 0;
    let mut expected_loss = 0;
    let mut expected_sequence = WARMUP;
    let mut digest = blake3::Hasher::new();
    let pipeline = Instant::now();
    for group in (0..frames).step_by(4) {
        let mut packets = Vec::new();
        for index in group..group + 4 {
            let start = Instant::now();
            let packet = encoder.encode(&pcm, (index + WARMUP) * 20_000)?;
            encode.push(start.elapsed().as_nanos());
            encoded_bytes += packet.data.len() as u64;
            max_packet_bytes = max_packet_bytes.max(packet.data.len());
            digest.update(&(packet.data.len() as u64).to_le_bytes());
            digest.update(&packet.data);
            if index % 50 == 21 && index + 1 < frames {
                expected_loss += 1;
            } else {
                packets.push(packet);
            }
        }
        for packet in packets.into_iter().rev() {
            jitter.push(packet)?;
            max_queued = max_queued.max(jitter.len());
        }
        while let Some((outcome, packet)) = jitter.pop() {
            let start = Instant::now();
            let output = match (outcome, packet) {
                (PopOutcome::Packet, Some(packet)) => {
                    ensure!(packet.seq == expected_sequence, "playout sequence changed");
                    decoded += 1;
                    decoder.decode(&packet)?
                }
                (PopOutcome::Gap { sequence }, None) => {
                    ensure!(
                        sequence == expected_sequence,
                        "concealment sequence changed"
                    );
                    concealed += 1;
                    decoder.decode_plc()?
                }
                _ => bail!("unexpected audio discontinuity or packet shape"),
            };
            decode.push(start.elapsed().as_nanos());
            ensure!(
                output.len() == pcm.len() && output.iter().all(|s| s.is_finite()),
                "invalid decoded PCM"
            );
            expected_sequence += 1;
        }
    }
    let pipeline_ns = pipeline.elapsed().as_nanos();
    ensure!(
        concealed == expected_loss
            && decoded + concealed == frames
            && jitter.is_empty()
            && max_queued <= 4
            && jitter.overflow == 0,
        "audio loss or buffer accounting mismatch"
    );
    Ok(Row {
        sample_rate: format.sample_rate,
        channels: format.channels,
        frames,
        decoded,
        concealed,
        max_queued,
        max_packet_bytes,
        encoded_bytes,
        encoded_digest: digest.finalize().to_hex().to_string(),
        encode: Timing::measured(encode),
        decode_or_plc: Timing::measured(decode),
        pipeline_ns,
    })
}

fn command(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    ensure!(output.status.success(), "{program} failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
