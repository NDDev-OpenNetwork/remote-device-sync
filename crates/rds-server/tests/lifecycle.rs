//! Exercise only child servers owned by these temporary loopback fixtures.
#![cfg(unix)]

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rds_discovery::client::Client;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rds-server-lifecycle-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn command(&self, relay: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rds-server"));
        command
            .args(["--http-addr", "127.0.0.1:0", "--relay-addr", relay])
            .arg("--directory")
            .arg(self.0.join("records"))
            .env("RUST_LOG", "rds_server=info")
            .env("RDS_LOG_FORMAT", "text")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn loopback_address(line: &str) -> std::net::SocketAddr {
    let start = line.find("127.0.0.1:").unwrap();
    let address: String = line[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ':'))
        .collect();
    address.parse().unwrap()
}

#[tokio::test]
async fn signal_joins_both_services_and_same_catalog_reopens() {
    let scratch = Scratch::new();
    for _ in 0..2 {
        let mut child = scratch.command("127.0.0.1:0").spawn().unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let (directory, relay) = tokio::time::timeout(Duration::from_secs(15), async {
            let mut directory = None;
            while let Some(line) = lines.next_line().await.unwrap() {
                if line.contains("discovery directory listening") {
                    directory = Some(loopback_address(&line));
                } else if line.contains("relay listening") {
                    return (directory.unwrap(), loopback_address(&line));
                }
            }
            panic!("child server exited before readiness");
        })
        .await
        .unwrap();
        Client::new(directory).health().await.unwrap();
        let mut stalled = TcpStream::connect(directory).await.unwrap();
        stalled
            .write_all(b"GET /v1/health HTTP/1.1\r\n")
            .await
            .unwrap();
        let pid = rustix::process::Pid::from_raw(child.id().unwrap().try_into().unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "server did not finish graceful shutdown: {status}"
        );
        let mut wire = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), stalled.read_to_end(&mut wire))
            .await
            .unwrap();
        assert!(wire.is_empty());
        for addr in [directory, relay] {
            let _listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        }
    }
}

#[tokio::test]
async fn relay_startup_failure_closes_the_partially_started_host() {
    let scratch = Scratch::new();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        scratch
            .command(&occupied.local_addr().unwrap().to_string())
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("discovery directory listening"));
    assert!(output.stdout.is_empty(), "diagnostics belong on stderr");
}

#[tokio::test]
async fn invalid_relay_allowlist_fails_before_creating_catalog() {
    let scratch = Scratch::new();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        scratch
            .command("127.0.0.1:0")
            .args(["--allow", "invalid-key"])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    assert!(!scratch.0.join("records").exists());
}
