#![cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cli_uses_running_identity_without_creating_its_own_key() {
    let path = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("rds-cli-local-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    let config = path.join("fresh-config");
    let endpoint = rds_net::bind_endpoint(rds_net::EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut server = rds_client::local::Server::start(
        Some(
            rds_client::local::Prepared::bind(path.join("control"))
                .await
                .unwrap(),
        ),
        endpoint.clone(),
        None,
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rds"))
            .env("XDG_CONFIG_HOME", &config)
            .env("RDS_LOG_FORMAT", "json")
            .env("RUST_LOG", "off")
            .arg("session")
            .arg("--control-dir")
            .arg(path.join("control"))
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["list", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: rds_core::local::Snapshot = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot.endpoint, endpoint.id().to_string());
    assert!(snapshot.sessions.is_empty());
    assert!(
        !config.exists(),
        "keyless managed CLI created a config/key directory"
    );
    for args in [
        vec!["ping"],
        vec!["use", "not-a-session"],
        vec!["ssh", "--bind", "0.0.0.0:2222"],
        vec!["--key-file", "unwanted-key", "list"],
    ] {
        let output = run(&args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    assert!(!config.exists());
    server.close().await.unwrap();
    endpoint.close().await;
}
