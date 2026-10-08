//! Reproducible snapshot partification comparison, never a runtime CPU claim.

use anyhow::ensure;
use clap::Parser;
use rds_sync::directory::{
    DirectoryEntry, DirectoryManifest, DirectorySnapshotPart, MAX_DIRECTORY_PART_BYTES,
    MAX_DIRECTORY_PART_ENTRIES,
};
use serde::Serialize;
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    json: PathBuf,
    #[arg(long)]
    md: PathBuf,
    #[arg(long, default_value_t = 10)]
    samples: usize,
}

#[derive(Serialize)]
struct Row {
    entries: usize,
    samples: usize,
    parts: usize,
    serialized_bytes: usize,
    payload_digest: String,
    reference_median_ns: u128,
    borrowed_median_ns: u128,
}

#[derive(Serialize)]
struct Report {
    schema_version: u8,
    scenario: &'static str,
    reference_source: &'static str,
    source_commit: String,
    worktree_clean: bool,
    toolchain: String,
    os: &'static str,
    architecture: &'static str,
    rows: Vec<Row>,
    limitations: &'static str,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    ensure!((3..=100).contains(&args.samples), "samples must be 3..=100");
    let mut rows = Vec::new();
    for count in [1024, 8192, 32768] {
        let manifest = DirectoryManifest::from_entries(
            (0..count)
                .map(|index| {
                    let path = format!("entry-{index:06}");
                    if index % 20 == 0 {
                        DirectoryEntry::symlink(path, vec![b'x'; 4096])
                    } else {
                        let digest = *blake3::hash(path.as_bytes()).as_bytes();
                        DirectoryEntry::file(path, index as u64, digest)
                    }
                })
                .collect(),
        )?;
        let expected = emit_reference(&manifest)?;
        ensure!(
            emit_borrowed(&manifest)? == expected,
            "wire mismatch in warm-up"
        );
        let mut old = Vec::new();
        let mut new = Vec::new();
        for index in 0..args.samples {
            // Alternate order to reduce a systematic warm-cache/order bias.
            if index % 2 == 0 {
                old.push(measure(|| emit_reference(&manifest), &expected)?);
                new.push(measure(|| emit_borrowed(&manifest), &expected)?);
            } else {
                new.push(measure(|| emit_borrowed(&manifest), &expected)?);
                old.push(measure(|| emit_reference(&manifest), &expected)?);
            }
        }
        rows.push(Row {
            entries: count,
            samples: args.samples,
            parts: expected.0,
            serialized_bytes: expected.1,
            payload_digest: expected.2.clone(),
            reference_median_ns: median(old),
            borrowed_median_ns: median(new),
        });
    }
    let report = Report {
        schema_version: 1,
        scenario: "directory snapshot partification and encoding",
        reference_source: "cc7cc8cc directory/wire.rs clone-and-serialize algorithm",
        source_commit: command("git", &["rev-parse", "HEAD"])?,
        worktree_clean: command("git", &["status", "--porcelain"])?.is_empty(),
        toolchain: command("rustc", &["--version"])?,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        rows,
        limitations: "Synthetic in-process release-build wall time; does not measure installed CPU, filesystem throughput, network, or remote recursive service acceptance. Reference is preserved algorithm code, not a second deployed binary. No hardware-independent speed threshold.",
    };
    let mut md = format!(
        "# Directory snapshot benchmark\n\nSource: `{}`; clean: `{}`; `{}`; {} / {}.\n\n| Entries | Samples | Parts | Reference median ms | Borrowed median ms |\n|---:|---:|---:|---:|---:|\n",
        report.source_commit,
        report.worktree_clean,
        report.toolchain,
        report.os,
        report.architecture
    );
    for row in &report.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} | {:.3} |\n",
            row.entries,
            row.samples,
            row.parts,
            row.reference_median_ns as f64 / 1e6,
            row.borrowed_median_ns as f64 / 1e6
        ));
    }
    md.push_str(&format!("\nEvery sample verifies identical part count, serialized byte count and framed payload digest.\n\n{}\n", report.limitations));
    std::fs::write(args.json, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(args.md, md)?;
    Ok(())
}

fn command(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    ensure!(output.status.success(), "{program} failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

type Output = (usize, usize, String);
fn measure(
    operation: impl FnOnce() -> anyhow::Result<Output>,
    expected: &Output,
) -> anyhow::Result<u128> {
    let start = Instant::now();
    let actual = operation()?;
    let elapsed = start.elapsed().as_nanos();
    ensure!(
        &actual == expected,
        "snapshot payload changed in measurement"
    );
    Ok(elapsed)
}

fn emit_borrowed(manifest: &DirectoryManifest) -> anyhow::Result<Output> {
    let mut output = EncodedOutput::default();
    for part in manifest.snapshot_part_iter()? {
        output.add(&postcard::to_stdvec(&part?)?);
    }
    Ok(output.finish())
}

// Preserve the pre-optimization algorithm explicitly. Do not call the current
// owned convenience method here: that would benchmark the new iterator twice.
fn emit_reference(manifest: &DirectoryManifest) -> anyhow::Result<Output> {
    manifest.verify()?;
    let mut parts = Vec::new();
    let mut current = Vec::new();
    for entry in &manifest.entries {
        let mut candidate = current.clone();
        candidate.push(entry.clone());
        let part = DirectorySnapshotPart {
            entries: candidate.clone(),
        };
        let length = postcard::to_stdvec(&part)?.len();
        if candidate.len() > MAX_DIRECTORY_PART_ENTRIES || length > MAX_DIRECTORY_PART_BYTES {
            ensure!(!current.is_empty(), "entry exceeds part bounds");
            parts.push(DirectorySnapshotPart::new(std::mem::take(&mut current))?);
            current.push(entry.clone());
            ensure!(
                postcard::to_stdvec(&DirectorySnapshotPart {
                    entries: current.clone()
                })?
                .len()
                    <= MAX_DIRECTORY_PART_BYTES,
                "entry exceeds part bounds"
            );
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        parts.push(DirectorySnapshotPart::new(current)?);
    }
    let mut output = EncodedOutput::default();
    for part in parts {
        output.add(&postcard::to_stdvec(&part)?);
    }
    Ok(output.finish())
}

#[derive(Default)]
struct EncodedOutput {
    parts: usize,
    bytes: usize,
    hasher: blake3::Hasher,
}
impl EncodedOutput {
    fn add(&mut self, bytes: &[u8]) {
        self.parts += 1;
        self.bytes += bytes.len();
        self.hasher.update(&(bytes.len() as u64).to_le_bytes());
        self.hasher.update(bytes);
    }
    fn finish(self) -> Output {
        (
            self.parts,
            self.bytes,
            self.hasher.finalize().to_hex().to_string(),
        )
    }
}
