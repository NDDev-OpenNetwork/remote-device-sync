//! Linux-only qualification of the real local admin listener and the optional
//! Vector fragment, using the same OpenObserve backend as the logging fixture.

use std::process::Command;
use std::time::{Duration, Instant};

use super::{Client, Stack, Value};
use rds_observe::admin::{Prepared, Server, Snapshot, Token};

struct Collector(String);
impl Drop for Collector {
    fn drop(&mut self) {
        let result = Command::new("docker")
            .args(["rm", "--force", &self.0])
            .output();
        if !matches!(result, Ok(output) if output.status.success()) {
            eprintln!("could not remove owned admin collector fixture");
        }
    }
}

pub(super) async fn check(client: &Client, stack: &Stack, base: &str) {
    let token_path = stack.root.join("secrets/admin_token");
    Token::create(&token_path).unwrap();
    let prepared = Prepared::bind(
        "127.0.0.1:0".parse().unwrap(),
        Token::load(&token_path).unwrap(),
    )
    .await
    .unwrap();
    let mut admin = Server::start(Some(prepared), || {
        Snapshot::from([
            ("rds_directory_puts_ok_total", 42),
            ("rds_net_bytes_sent_total{via=\"direct\"}", 123),
            ("rds_net_bytes_sent_total{via=\"relay\"}", 456),
            ("rds_relay_metrics_available", 0),
        ])
    });
    let url = format!("http://{}/metrics", admin.addr().unwrap());
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);

    // A configured proxy must not receive the local bearer credential. Force
    // a source-specific proxy and clear bypasses, retaining the shipped disable.
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fragment = stack.root.join("vector-admin.toml");
    let config = std::fs::read_to_string(stack.config.join("vector-admin.toml")).unwrap();
    assert_eq!(config.matches("proxy.enabled = false").count(), 1);
    std::fs::write(
        &fragment,
        config.replace(
            "proxy.enabled = false",
            &format!(
                "proxy.enabled = false\nproxy.http = \"http://{}\"\nproxy.no_proxy = []",
                proxy.local_addr().unwrap()
            ),
        ),
    )
    .unwrap();

    let images = std::fs::read_to_string(stack.config.join("images.env")).unwrap();
    let vector = images
        .lines()
        .find_map(|line| line.strip_prefix("RDS_VECTOR_IMAGE="))
        .unwrap();
    let collector = Collector(format!("{}-admin", stack.project));
    let state = stack.root.join("admin-vector-state");
    std::fs::create_dir(&state).unwrap();
    let mut command = Command::new("docker");
    command
        .args([
            "run",
            "--detach",
            "--pull",
            "never",
            "--name",
            &collector.0,
            "--network",
            "host",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--read-only",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=67108864",
            "--memory",
            "512m",
            "--cpus",
            "1",
            "--pids-limit",
            "128",
            "--user",
        ])
        .arg(format!(
            "{}:{}",
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw()
        ))
        .arg("--label")
        .arg(format!("rds.observability.fixture={}", stack.project))
        .arg("--volume")
        .arg(format!("{}:/fixture/state:rw", state.display()));
    for (source, destination) in [
        (stack.config.join("vector.toml"), "/etc/vector/vector.toml"),
        (fragment, "/etc/vector/admin.toml"),
        (stack.root.join("secrets"), "/fixture/secrets"),
    ] {
        command.args([
            "--volume",
            &format!("{}:{destination}:ro", source.display()),
        ]);
    }
    for value in [
        "RDS_VECTOR_STATE=/fixture/state".to_owned(),
        "RDS_VECTOR_SECRETS=/fixture/secrets".into(),
        "RDS_VECTOR_LOCAL_METRICS_ADDR=127.0.0.1:0".into(),
        "RDS_LOG_GLOB=/fixture/no-logs/*.jsonl".into(),
        "RDS_OBSERVE_NODE=fixture-admin".into(),
        "RDS_OBSERVE_INSTANCE=primary".into(),
        "RDS_ADMIN_SERVICE=rds-server".into(),
        "RDS_OO_ORG=default".into(),
        format!("RDS_ADMIN_URL={url}"),
        format!("RDS_OO_URL={base}"),
    ] {
        command.args(["--env", &value]);
    }
    let started = command
        .arg(vector)
        .args([
            "--config",
            "/etc/vector/vector.toml",
            "--config",
            "/etc/vector/admin.toml",
            "--dangerously-allow-env-var-interpolation",
        ])
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "admin collector failed to start: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(45);
    for (metric, expected) in [
        ("rds_directory_puts_ok_total", vec![42.0]),
        ("rds_net_bytes_sent_total", vec![123.0, 456.0]),
        ("rds_relay_metrics_available", vec![0.0]),
    ] {
        loop {
            let response: Value = client
                .get(format!(
                    "{base}/api/default/prometheus/api/v1/query?query={metric}"
                ))
                .basic_auth("fixture@example.invalid", Some(&stack.password))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(series) = response["data"]["result"].as_array()
                && series.len() == expected.len()
            {
                let mut actual = Vec::new();
                for row in series {
                    actual.push(row["value"][1].as_str().unwrap().parse::<f64>().unwrap());
                    let tags = row["metric"].as_object().unwrap();
                    assert_eq!(tags["node"], "fixture-admin");
                    assert_eq!(tags["instance"], "primary");
                    assert_eq!(tags["service"], "rds-server");
                    assert!(
                        tags.keys()
                            .all(|name| ["__name__", "node", "instance", "service", "via"]
                                .contains(&name.as_str())),
                        "private/unbounded label: {tags:?}"
                    );
                }
                actual.sort_by(f64::total_cmp);
                assert_eq!(actual, expected);
                break;
            }
            let state = Command::new("docker")
                .args(["inspect", "--format", "{{.State.Running}}", &collector.0])
                .output()
                .unwrap();
            if Instant::now() >= deadline || !state.status.success() || state.stdout != b"true\n" {
                let output = Command::new("docker")
                    .args(["logs", "--tail", "30", &collector.0])
                    .output()
                    .unwrap();
                panic!(
                    "admin scrape did not reach backend: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    assert!(admin.snapshot()["rds_admin_scrapes_total"] >= 1);
    assert_eq!(admin.snapshot()["rds_admin_snapshot_errors_total"], 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), proxy.accept())
            .await
            .is_err(),
        "admin bearer scrape traversed a proxy"
    );
    drop(collector);
    admin.close().await.unwrap();
    println!(
        "admin: authenticated Rust scrape, disabled proxy, merged Vector config, exact remote-write values and private label projection passed"
    );
}
