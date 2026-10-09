//! Synthetic local journal admission; no network or physical-durability claim.
use anyhow::ensure;
use clap::Parser;
use rds_sync::{SyncError, journal::Journal};
use serde::Serialize;
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    json: PathBuf,
    #[arg(long)]
    md: PathBuf,
    #[arg(long, default_value_t = 100)]
    samples: usize,
}

#[derive(Serialize)]
struct Row {
    case: &'static str,
    samples: usize,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    max_us: u128,
}

fn measure(
    case: &'static str,
    samples: usize,
    mut f: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<Row> {
    f()?; // Warm-up, excluded from the cohort.
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        f()?;
        times.push(start.elapsed().as_micros());
    }
    times.sort_unstable();
    let p = |n: usize| times[(samples * n).div_ceil(100) - 1];
    Ok(Row {
        case,
        samples,
        p50_us: p(50),
        p95_us: p(95),
        p99_us: p(99),
        max_us: times[samples - 1],
    })
}

#[derive(Serialize)]
struct Report {
    schema_version: u8,
    source_commit: String,
    source_tree: String,
    worktree_clean: bool,
    binary_digest: String,
    lockfile_digest: String,
    toolchain: String,
    os: &'static str,
    architecture: &'static str,
    debug_assertions: bool,
    bytes: usize,
    chunks: usize,
    content_digest: String,
    foreign_entries: usize,
    failures: usize,
    skips: usize,
    rows: Vec<Row>,
    limitations: &'static str,
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> std::io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("rds-journal-bench-{:032x}", rand::random::<u128>()));
        // Claim exactly one fresh synthetic root before its drop owns cleanup.
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    ensure!(
        (100..=1000).contains(&args.samples),
        "samples must be 100..=1000"
    );
    let clean = command("git", &["status", "--porcelain"])?.is_empty();
    let root = Scratch::new()?;
    let data = (0..4 * 1024 * 1024)
        .map(|i| (i / 65536) as u8)
        .collect::<Vec<_>>();
    let manifest = rds_sync::manifest_of(&data);
    std::fs::write(root.0.join("data.bin"), &data)?;
    ensure!(Journal::open(&root.0, "data.bin", &manifest)?.complete());
    let mut rows = vec![measure("verified-resume-4MiB", args.samples, || {
        ensure!(Journal::open(&root.0, "data.bin", &manifest)?.complete());
        Ok(())
    })?];
    let absent = root.0.join("never-created");
    rows.push(measure("pre-canceled-admission", args.samples, || {
        let result = Journal::open_cancellable(&absent, "data.bin", &manifest, &|| true);
        ensure!(matches!(result, Err(SyncError::Io(ref e)) if e.kind() == std::io::ErrorKind::Interrupted));
        ensure!(!absent.exists());
        Ok(())
    })?);
    const FOREIGN: usize = 8192;
    let wide = root.0.join("wide");
    let state = wide.join(rds_sync::journal::STATE_DIR);
    std::fs::create_dir_all(&state)?;
    for n in 0..FOREIGN {
        std::fs::write(state.join(format!("foreign-{n}")), b"retain")?;
    }
    rows.push(measure(
        "admit-beside-8192-foreign-entries",
        args.samples,
        || {
            let journal = Journal::open(&wide, "new.bin", &manifest)?;
            ensure!(journal.have_set().is_empty());
            Ok(())
        },
    )?);
    // Verify the fixture after the timed cohort, not inside every sample.
    for n in 0..FOREIGN {
        ensure!(std::fs::read(state.join(format!("foreign-{n}")))? == b"retain");
    }
    ensure!(std::fs::read(root.0.join("data.bin"))? == data);
    ensure!(!wide.join("new.bin").exists());
    let report = Report {
        schema_version: 1,
        source_commit: command("git", &["rev-parse", "HEAD"])?,
        source_tree: command("git", &["rev-parse", "HEAD^{tree}"])?,
        worktree_clean: clean,
        binary_digest: blake3::hash(&std::fs::read(std::env::current_exe()?)?)
            .to_hex()
            .to_string(),
        lockfile_digest: blake3::hash(&std::fs::read("Cargo.lock")?)
            .to_hex()
            .to_string(),
        toolchain: command("rustc", &["--version"])?,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        debug_assertions: cfg!(debug_assertions),
        bytes: data.len(),
        chunks: manifest.chunks.len(),
        content_digest: blake3::hash(&data).to_hex().to_string(),
        foreign_entries: FOREIGN,
        failures: 0,
        skips: 0,
        rows,
        limitations: "Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.",
    };
    let mut md = format!(
        "# Journal preparation benchmark\n\nSource: `{}`; tree: `{}`; clean: `{}`; `{}`; {} / {}.\n\n| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |\n|---|---:|---:|---:|---:|---:|\n",
        report.source_commit,
        report.source_tree,
        report.worktree_clean,
        report.toolchain,
        report.os,
        report.architecture
    );
    for r in &report.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.case, r.samples, r.p50_us, r.p95_us, r.p99_us, r.max_us
        ));
    }
    md.push_str(&format!(
        "\nFixture: {} bytes, {} chunks; {} foreign entries retained.\n\n{}\n",
        report.bytes, report.chunks, report.foreign_entries, report.limitations
    ));
    std::fs::write(args.json, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(args.md, md)?;
    Ok(())
}

fn command(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let out = std::process::Command::new(program).args(args).output()?;
    ensure!(out.status.success(), "{program} failed");
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
