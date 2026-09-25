// Actual daemon binaries; all processes, credentials and paths are synthetic.
// Including targets supply BINARY, ROLE and OWNED. No shared service is touched.
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt as _;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rds-admin-cli-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        rds_observe::admin::Token::create(&path.join("token")).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        self.command_with_relay(None)
    }
    fn command_with_relay(&self, unavailable_relay: Option<std::net::SocketAddr>) -> Command {
        let mut cmd = Command::new(BINARY);
        match ROLE {
            "agent" => {
                cmd.args(["--bind-address", "127.0.0.1:0", "--key-file"])
                    .arg(self.0.join("identity"));
                match unavailable_relay {
                    Some(addr) => {
                        cmd.arg("--relay").arg(format!("http://{addr}"));
                    }
                    None => {
                        cmd.arg("--no-relay");
                    }
                }
                if OWNED {
                    cmd.args(["--backend", "noq"]);
                }
            }
            "relay" => {
                cmd.args(["--addr", "127.0.0.1:0"]);
            }
            "server" => {
                cmd.args([
                    "--relay-addr",
                    "127.0.0.1:0",
                    "--http-addr",
                    "127.0.0.1:0",
                    "--directory",
                ])
                .arg(self.0.join("records"));
            }
            _ => panic!("unknown fixture role"),
        }
        if OWNED && ROLE != "agent" {
            cmd.args([
                "--relay-backend",
                "noq",
                "--development-open-relay",
                "--relay-key-file",
            ])
            .arg(self.0.join("identity"));
        }
        cmd.env("RDS_LOG_FORMAT", "text")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.0.join("stderr")).unwrap());
        cmd
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.0.join("stderr")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn address(line: &str) -> std::net::SocketAddr {
    let from = line.find("127.0.0.1:").unwrap();
    line[from..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ':'))
        .collect::<String>()
        .parse()
        .unwrap()
}
async fn wire(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_string(&mut reply))
        .await
        .unwrap()
        .unwrap();
    reply
}
async fn exit(process: &mut Process) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "owned child did not exit");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn daemon_admin_preflight_refuses_public_binds_and_missing_credentials_before_state() {
    for pair in [false, true] {
        let fixture = Fixture::new();
        let mut cmd = fixture.command();
        cmd.args([
            "--admin-addr",
            if pair { "0.0.0.0:0" } else { "127.0.0.1:0" },
        ]);
        if pair {
            cmd.arg("--admin-token-file").arg(fixture.0.join("token"));
        }
        let mut child = Process(cmd.spawn().unwrap());
        assert!(!exit(&mut child).await.success());
        assert!(!fixture.0.join("identity").exists());
        assert!(!fixture.0.join("records").exists());
        let credential = std::fs::read_to_string(fixture.0.join("token")).unwrap();
        assert!(!fixture.log().contains(&credential));
    }
}

#[tokio::test]
async fn daemon_admin_authentication_source_coverage_and_signal_cleanup() {
    check_authentication_and_signal_cleanup(None).await;
}

async fn check_authentication_and_signal_cleanup(unavailable_relay: Option<std::net::SocketAddr>) {
    let fixture = Fixture::new();
    let mut child = Process(
        fixture
            .command_with_relay(unavailable_relay)
            .args(["--admin-addr", "127.0.0.1:0", "--admin-token-file"])
            .arg(fixture.0.join("token"))
            .spawn()
            .unwrap(),
    );
    let addr = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "startup failed: {}",
                fixture.log()
            );
            if let Some(line) = fixture
                .log()
                .lines()
                .find(|line| line.contains("admin metrics listening"))
            {
                break address(line);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("admin startup timed out: {error}; {}", fixture.log()));
    let denied = wire(
        addr,
        "GET /metrics HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n",
    )
    .await;
    assert!(denied.starts_with("HTTP/1.1 401"));
    let credential = std::fs::read_to_string(fixture.0.join("token")).unwrap();
    let query = format!(
        "GET /metrics HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {credential}\r\n\r\n"
    );
    if ROLE == "server" {
        let line = fixture.log();
        let directory = address(
            line.lines()
                .find(|line| line.contains("discovery directory listening"))
                .unwrap(),
        );
        assert!(
            wire(
                directory,
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        assert!(
            wire(
                directory,
                "GET /v1/metrics HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .await
            .starts_with("HTTP/1.1 404")
        );
    }
    let metrics = wire(addr, &query).await;
    assert!(metrics.starts_with("HTTP/1.1 200"), "{metrics}");
    assert!(metrics.contains("rds_admin_unauthorized_total 1\n"));
    if ROLE == "agent" {
        assert!(metrics.contains("rds_agent_connections_active 0\n"));
        assert!(metrics.contains("rds_net_selected_path_known 0\n"));
    } else if OWNED {
        assert!(metrics.contains("rds_relay_metrics_available 1\n"));
        assert!(metrics.contains("rds_relay_forwarded_datagrams_total 0\n"));
    } else {
        assert!(metrics.contains("rds_relay_metrics_available 0\n"));
        assert!(!metrics.contains("rds_relay_forwarded_datagrams_total"));
    }
    if ROLE == "server" {
        assert!(metrics.contains("rds_directory_requests_total 2\n"));
    }
    assert!(!metrics.contains(&credential));
    assert!(!fixture.log().contains(&credential));
    // Signal only this owned child, leaving unrelated agents/services alone.
    let pid = rustix::process::Pid::from_raw(child.0.id().try_into().unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    assert!(exit(&mut child).await.success(), "{}", fixture.log());
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn occupied_admin_port_fails_before_creating_product_state() {
    let fixture = Fixture::new();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Process(
        fixture
            .command()
            .arg("--admin-addr")
            .arg(occupied.local_addr().unwrap().to_string())
            .arg("--admin-token-file")
            .arg(fixture.0.join("token"))
            .spawn()
            .unwrap(),
    );
    assert!(!exit(&mut child).await.success());
    assert!(!fixture.0.join("identity").exists());
    assert!(!fixture.0.join("records").exists());
}
