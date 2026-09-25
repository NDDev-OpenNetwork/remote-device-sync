#![cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cli_uses_running_identity_without_creating_its_own_key() {
    let path = std::path::Path::new("/tmp")
        .canonicalize()
        .unwrap()
        .join(format!("rds-c-{}", std::process::id()));
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
    let key = config.join("remote-device-sync/endpoint.key");
    let directory = rds_client::local::control_dir_for_key(&key).unwrap();
    let endpoint = rds_net::bind_endpoint(rds_net::EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut server = rds_client::local::Server::start(
        Some(rds_client::local::Prepared::bind(&directory).await.unwrap()),
        endpoint.clone(),
        None,
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rds"))
            .env("XDG_CONFIG_HOME", &config)
            .env("RDS_LOG_FORMAT", "json")
            .env("RUST_LOG", "off")
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["session", "list", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: rds_core::local::Snapshot = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot.endpoint, endpoint.id().to_string());
    assert!(snapshot.sessions.is_empty());
    assert!(!key.exists(), "keyless managed CLI created a key");
    // Real CLI processes reuse one authenticated remote connection. This peer
    // implements only Ping/Info, so accidental TCP or command execution fails.
    let remote = rds_net::bind_endpoint(rds_net::EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peer = remote.clone();
    let observed = accepted.clone();
    let expected = endpoint.id();
    let service = tokio::spawn(async move {
        while let Some(incoming) = peer.accept().await {
            let conn = incoming.await.unwrap();
            if observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                assert_eq!(conn.remote_id(), expected);
            }
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let request: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
                match request {
                    rds_core::StreamHello::Ping { nonce } => {
                        rds_core::write_frame(&mut send, &rds_core::HelloAck::Ok)
                            .await
                            .unwrap();
                        send.write_all(&nonce.to_be_bytes()).await.unwrap();
                    }
                    rds_core::StreamHello::Info => {
                        rds_core::write_frame(
                            &mut send,
                            &rds_core::HelloAck::Info(rds_core::AgentInfo {
                                protocol: 0,
                                version: "fixture".into(),
                                hostname: None,
                                services: vec![rds_core::ServiceKind::Ping],
                                desktop: None,
                            }),
                        )
                        .await
                        .unwrap();
                    }
                    _ => panic!("unexpected fixture service"),
                }
                send.finish().unwrap();
            }
        }
    });
    let target = rds_net::Ticket::of(&remote).to_string();
    let first = run(&["ping", &target, "-c", "1"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let before = rds_client::local::Client::new(&directory)
        .snapshot()
        .await
        .unwrap();
    let mut processes = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let config = config.clone();
        let target = target.clone();
        processes.spawn_blocking(move || {
            Command::new(env!("CARGO_BIN_EXE_rds"))
                .env("XDG_CONFIG_HOME", config)
                .args(["info", &target])
                .output()
                .unwrap()
        });
    }
    while let Some(result) = processes.join_next().await {
        let output = result.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("fixture"));
    }
    let after = rds_client::local::Client::new(&directory)
        .snapshot()
        .await
        .unwrap();
    assert_eq!(before.generation, after.generation);
    assert_eq!(before.sessions[0].id, after.sessions[0].id);
    assert_eq!(accepted.load(std::sync::atomic::Ordering::Relaxed), 1);
    for args in [
        vec![
            "session",
            "ping",
            "--session",
            "00000000000000000000000000000000",
        ],
        vec!["session", "use", "not-a-session"],
        vec!["session", "ssh", "--bind", "0.0.0.0:2222"],
        vec!["session", "--key-file", "unwanted-key", "list"],
        vec!["--no-relay", "ticket"],
        vec!["--direct", "session", "list"],
        vec!["ssh", "unused-peer", "--bind", "0.0.0.0:2222"],
        vec!["send", "unused-peer", "unread-file"],
    ] {
        let output = run(&args);
        assert!(!output.status.success(), "unexpected success: {args:?}");
        assert!(output.stdout.is_empty());
    }
    let ticket = run(&["ticket"]);
    assert!(ticket.status.success());
    assert_eq!(
        rds_net::parse_target(std::str::from_utf8(&ticket.stdout).unwrap().trim())
            .unwrap()
            .id,
        endpoint.id()
    );
    assert!(!key.exists());
    server.close().await.unwrap();
    // Missing manager fails closed, even with a perfectly usable default path.
    let missing = run(&["ticket"]);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(!key.exists());
    let owner = rds_net::acquire_key(&key).unwrap();
    let output = run(&["--direct", "--no-relay", "ticket"]);
    assert!(!output.status.success());
    assert_eq!(
        output.status.code(),
        Some(1),
        "must fail at runtime, not argument parsing"
    );
    assert!(output.stdout.is_empty());
    let reader = run(&["id"]);
    assert!(reader.status.success());
    assert_eq!(
        String::from_utf8(reader.stdout).unwrap().trim(),
        owner.secret_key().public().to_string()
    );
    drop(owner);
    let direct = run(&["--direct", "--no-relay", "ping", &target, "-c", "1"]);
    assert!(
        direct.status.success(),
        "{}",
        String::from_utf8_lossy(&direct.stderr)
    );
    assert!(String::from_utf8_lossy(&direct.stdout).contains("pong seq=0"));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::Relaxed), 2);
    endpoint.close().await;
    remote.close().await;
    service.await.unwrap();
}
