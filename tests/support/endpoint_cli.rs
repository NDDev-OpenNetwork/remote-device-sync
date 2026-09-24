// Shared binary-level endpoint configuration contract. Including targets supply
// BINARY and TAIL; no dependency on another binary's build artifacts.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rds-endpoint-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str], scratch: &Scratch) -> std::process::Output {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        Command::new(BINARY)
            .arg("--key-file")
            .arg(scratch.0.join("endpoint.key"))
            .args(args)
            .args(TAIL)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "invalid configuration did not fail before startup"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    use std::io::Read;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}

#[test]
fn invalid_endpoint_flags_fail_before_creating_identity() {
    for args in [
        vec!["--backend", "unknown"],
        vec![
            "--backend",
            "iroh",
            "--bind-address",
            "127.0.0.1:0",
            "--bind-address",
            "127.0.0.2:0",
        ],
        vec!["--relay", "file:///invalid-relay"],
        vec!["--relay", "http://127.0.0.1:9", "--no-relay"],
        vec!["--backend", "noq", "--relay", "http://127.0.0.1:9"],
        vec!["--owned-relay", "invalid-owned-route"],
    ] {
        let dir = Scratch::new();
        let output = run(&args, &dir);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            !dir.0.join("endpoint.key").exists(),
            "identity was created for invalid settings: {args:?}"
        );
    }
}

#[test]
fn malformed_oversized_and_special_config_files_fail_before_identity() {
    for bytes in [
        br#"{"schema_version":2}"#.to_vec(),
        br#"{"schema_version":1,"issuer":"not an endpoint field"}"#.to_vec(),
        br#"{"schema_version":1,"relay":{"mode":"disabled","urls":[]}}"#.to_vec(),
        vec![b' '; 16 * 1024 + 1],
    ] {
        let dir = Scratch::new();
        let config = dir.0.join("endpoint.json");
        std::fs::write(&config, bytes).unwrap();
        let output = run(&["--endpoint-config", config.to_str().unwrap()], &dir);
        assert!(!output.status.success());
        assert!(!dir.0.join("endpoint.key").exists());
    }
    let dir = Scratch::new();
    let output = run(&["--endpoint-config", dir.0.to_str().unwrap()], &dir);
    assert!(!output.status.success());
    assert!(!dir.0.join("endpoint.key").exists());
}
