//! Local tab-state churn measurement, separate from native/network latency.
use anyhow::ensure;
use clap::Parser;
use rds_bench::report::Percentiles;
use rds_desktop::render::workspace::{TabProfile, TabSpec, WorkspaceModel};
use serde::Serialize;
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    json: PathBuf,
    #[arg(long)]
    md: PathBuf,
}
#[derive(Serialize)]
struct Row {
    open_tabs: usize,
    transitions_per_sample: usize,
    timing: Percentiles,
}
#[derive(Serialize)]
struct Report {
    source: String,
    tree: String,
    clean: bool,
    toolchain: String,
    os: &'static str,
    architecture: &'static str,
    rows: Vec<Row>,
    limitations: &'static str,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut rows = vec![];
    for count in [1, 4, 8] {
        let mut samples = vec![];
        for _ in 0..100 {
            let mut workspace = WorkspaceModel::default();
            for n in 0..count {
                workspace.open(spec(n))?;
            }
            let start = Instant::now();
            for n in 0..1024 {
                let id = workspace.cycle(n % 2 == 0).expect("nonempty workspace");
                let revision = workspace.configure(
                    id,
                    TabProfile {
                        max_fps: if n % 2 == 0 { 30 } else { 60 },
                        ..Default::default()
                    },
                )?;
                ensure!(
                    workspace.accepts(id, revision) && !workspace.accepts(id, revision - 1),
                    "stale revision accepted"
                );
                workspace.move_tab(id, n % count)?;
                if n % 16 == 0 {
                    let removed = workspace.close(id)?;
                    let (new, fresh) = workspace.open(removed.spec)?;
                    ensure!(
                        fresh && new != id && !workspace.accepts(id, revision),
                        "closed identity reused"
                    );
                }
                ensure!(
                    workspace.tabs().len() == count,
                    "tab ownership grew during churn"
                );
                std::hint::black_box(&workspace);
            }
            samples.push(start.elapsed().as_nanos() as u64);
            while let Some(id) = workspace.active() {
                workspace.close(id)?;
            }
            ensure!(workspace.tabs().is_empty(), "closed tabs retained");
        }
        rows.push(Row {
            open_tabs: count,
            transitions_per_sample: 1024,
            timing: Percentiles::of(&samples).expect("100 samples"),
        });
    }
    let report = Report {
        source: command("git", &["rev-parse", "HEAD"])?,
        tree: command("git", &["rev-parse", "HEAD^{tree}"])?,
        clean: command("git", &["status", "--porcelain"])?.is_empty(),
        toolchain: command("rustc", &["--version"])?,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        rows,
        limitations: "Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.",
    };
    let mut md = format!(
        "# Workspace state churn\n\nSource: `{}`; tree: `{}`; clean: `{}`; `{}`; {} / {}.\n\n| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |\n|---|---|---|---|---|---|\n",
        report.source, report.tree, report.clean, report.toolchain, report.os, report.architecture
    );
    for row in &report.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.3} |\n",
            row.open_tabs,
            row.timing.count,
            row.transitions_per_sample,
            row.timing.p50_ns as f64 / 1000.,
            row.timing.p95_ns as f64 / 1000.,
            row.timing.p99_ns as f64 / 1000.
        ));
    }
    md.push_str(&format!("\nEvery sample checks capacity, exact revisions, non-reused identities and empty final ownership.\n\n{}\n", report.limitations));
    std::fs::write(args.json, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(args.md, md)?;
    Ok(())
}
fn spec(n: usize) -> TabSpec {
    TabSpec {
        device: format!("computer-{}", n / 2),
        label: format!("Computer {}", n / 2),
        display: (n % 2) as u32,
        profile: TabProfile::default(),
    }
}
fn command(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    ensure!(output.status.success(), "source metadata command failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
