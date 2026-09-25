//! Explicit local infrastructure qualification. No real credentials, hosts,
//! datasets or human alert destinations. Requires Docker Compose and pre-pulled
//! pinned images. See docs/observability.md for the two build/run commands.

#![cfg(target_os = "linux")]

#[path = "pipeline/admin.rs"]
mod admin;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde_json::{Value, json};

struct Stack {
    root: PathBuf,
    config: PathBuf,
    project: String,
    password: String,
}

impl Stack {
    fn new() -> Self {
        let suffix = format!("{:016x}", rand::random::<u64>());
        let root = std::env::temp_dir().join(format!("rds-observe-{suffix}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        }
        #[cfg(not(unix))]
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("logs")).unwrap();
        let password = format!("Rds!9\"\\{:032x}", rand::random::<u128>());
        let secrets = root.join("secrets");
        fs::create_dir(&secrets).unwrap();
        let authorization = format!(
            "Basic {}",
            data_encoding::BASE64.encode(format!("fixture@example.invalid:{password}").as_bytes())
        );
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options
            .open(secrets.join("authorization"))
            .unwrap()
            .write_all(authorization.as_bytes())
            .unwrap();
        Self {
            root,
            config: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ops/observability"),
            project: format!("rds-observe-{suffix}"),
            password,
        }
    }

    fn compose(&self, args: &[&str]) -> Output {
        Command::new("docker")
            .args(["compose", "--project-name", &self.project, "--env-file"])
            .arg(self.config.join("images.env"))
            .arg("-f")
            .arg(self.config.join("compose.yaml"))
            .arg("-f")
            .arg(self.config.join("compose.smoke.yaml"))
            .env("RDS_OO_USER", "fixture@example.invalid")
            .env("RDS_OO_PASSWORD", &self.password)
            .env("RDS_OBSERVE_LOG_DIR", self.root.join("logs"))
            .env("RDS_VECTOR_SECRET_DIR", self.root.join("secrets"))
            .args(args)
            .output()
            .expect("Docker Compose is required for this ignored test")
    }

    fn checked(&self, args: &[&str]) -> String {
        let result = self.compose(args);
        assert!(
            result.status.success(),
            "Docker operation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // Only this randomly named fixture project and its newly created data.
        let output = self.compose(&["down", "--volumes", "--timeout", "5"]);
        if !output.status.success() {
            eprintln!("fixture cleanup failed for project {}", self.project);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
}

async fn post(client: &Client, stack: &Stack, base: &str, path: &str, body: Value) -> Value {
    let response = client
        .post(format!("{base}{path}"))
        .basic_auth("fixture@example.invalid", Some(&stack.password))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "API {path} failed: {status} {text}");
    serde_json::from_str(&text).unwrap()
}

async fn search(client: &Client, stack: &Stack, base: &str, sql: &str) -> Value {
    post(
        client,
        stack,
        base,
        "/api/default/_search",
        json!({"query": {
            "sql": sql, "start_time": now_us() - 600_000_000,
            "end_time": now_us() + 60_000_000, "from": 0, "size": 1000
        }}),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "starts isolated local Vector/OpenObserve containers; see docs/observability.md"]
async fn vector_openobserve_logs_metrics_alerts_and_restart() {
    let fixture = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/fixture");
    assert!(
        fixture.is_file(),
        "first run cargo build -p rds-observe --example fixture"
    );
    let stack = Stack::new();
    let output = Command::new(&fixture).output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "logging must not contaminate stdout"
    );
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(!text.contains("PRIVATE_SENTINEL"));
    let records: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(records.len(), 7);
    let first_run = records[0]["run_id"].as_str().unwrap().to_owned();
    let path = stack.root.join("logs/rds.jsonl");
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
    // Defense in depth: collection rejects raw text/unknown schemas and strips
    // unknown fields even from an otherwise valid schema-1 record.
    writeln!(file, "PRIVATE_SENTINEL arbitrary failure").unwrap();
    let mut extended = records[0].clone();
    extended["sequence"] = json!(100);
    extended["unexpected_private_field"] = json!("PRIVATE_SENTINEL");
    writeln!(file, "{extended}").unwrap();
    extended["schema_version"] = json!(999);
    writeln!(file, "{extended}").unwrap();
    file.sync_all().unwrap();

    stack.checked(&[
        "run",
        "--rm",
        "--no-deps",
        "vector",
        "validate",
        "--dangerously-allow-env-var-interpolation",
        "--no-environment",
        "/etc/vector/vector.toml",
    ]);
    stack.checked(&[
        "run",
        "--rm",
        "--no-deps",
        "vector",
        "test",
        "--dangerously-allow-env-var-interpolation",
        "/etc/vector/vector.toml",
    ]);
    stack.checked(&["up", "-d", "--pull", "never"]);
    let port = stack.checked(&["port", "openobserve", "5080"]);
    assert!(
        port.trim().starts_with("127.0.0.1:"),
        "fixture API must publish on loopback"
    );
    let mut base = format!("http://{}", port.trim());
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(response) = client.get(format!("{base}/healthz")).send().await
            && response.status().is_success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "OpenObserve did not become healthy"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    println!("fixture backend healthy");

    // Wait for the stream to exist before querying it (absence is an API error).
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let response = client
            .get(format!("{base}/api/default/streams"))
            .basic_auth("fixture@example.invalid", Some(&stack.password))
            .send()
            .await
            .unwrap();
        let body = response.text().await.unwrap();
        if body.contains("rds_events") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Vector did not ingest the fixture"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let rows = search(
        &client,
        &stack,
        &base,
        "SELECT * FROM rds_events ORDER BY sequence",
    )
    .await;
    let hits = rows["hits"].as_array().unwrap();
    let unique: std::collections::BTreeSet<_> = hits
        .iter()
        .map(|row| {
            (
                row["run_id"].as_str().unwrap(),
                row["sequence"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(unique.len(), 8, "bad text/schema must be dropped");
    assert!(!rows.to_string().contains("PRIVATE_SENTINEL"));
    assert!(!rows.to_string().contains("unexpected_private_field"));
    assert!(hits.iter().any(|r| r["event"] == "process_failed"));
    assert!(hits.iter().any(|r| r["session_id"] == 7));
    println!("logs: schema, projection, correlation, invalid-record rejection passed");

    for metric in [
        "rds_observed_operations_total",
        "rds_observed_operation_duration_seconds_count",
        "rds_observed_operation_duration_seconds_sum",
    ] {
        let deadline = Instant::now() + Duration::from_secs(30);
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
                && series.len() == 2
            {
                for row in series {
                    let value = row["value"][1].as_str().unwrap().parse::<f64>().unwrap();
                    let expected = if metric.ends_with("_sum") {
                        records
                            .iter()
                            .find(|record| {
                                record["event"] == "operation_completed"
                                    && record["outcome"] == row["metric"]["outcome"]
                            })
                            .unwrap()["elapsed_us"]
                            .as_f64()
                            .unwrap()
                            / 1_000_000.0
                    } else {
                        1.0
                    };
                    assert!(
                        (value - expected).abs() < 1e-9,
                        "wrong metric units/count: {row}"
                    );
                    assert!(row["metric"].get("run_id").is_none());
                    assert!(row["metric"].get("session_id").is_none());
                }
                break;
            }
            assert!(
                Instant::now() < deadline,
                "remote-write metrics missing: {response}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    println!(
        "metrics: remote-write received exact controlled ok/error counters and histogram counts"
    );

    admin::check(&client, &stack, &base).await;

    let alerts: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(stack.config.join("alerts.json")).unwrap())
            .unwrap();
    for alert in &alerts {
        let result = search(
            &client,
            &stack,
            &base,
            alert["query_condition"]["sql"].as_str().unwrap(),
        )
        .await;
        let matches = result["hits"].as_array().unwrap().len();
        assert_eq!(matches, usize::from(alert["name"] == "rds_process_failure"));
    }

    // Deliberately synthesized records test minimum volume and the exact 20%
    // boundary; these are not measurements of real network connects.
    let sample_run = format!("{:032x}", rand::random::<u128>());
    let mut sample = records
        .iter()
        .find(|r| r["event"] == "operation_completed")
        .unwrap()
        .clone();
    sample["service"] = json!("rds-cli");
    sample["run_id"] = json!(sample_run);
    for sequence in 0..9 {
        sample["sequence"] = json!(sequence);
        sample["outcome"] = json!(if sequence < 2 { "error" } else { "ok" });
        writeln!(file, "{sample}").unwrap();
    }
    file.sync_all().unwrap();
    let count_sql =
        format!("SELECT DISTINCT sequence FROM rds_events WHERE run_id = '{sample_run}'");
    let deadline = Instant::now() + Duration::from_secs(30);
    while search(&client, &stack, &base, &count_sql).await["hits"]
        .as_array()
        .unwrap()
        .len()
        != 9
    {
        assert!(Instant::now() < deadline, "nine-attempt fixture missing");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let connect_sql = alerts[2]["query_condition"]["sql"].as_str().unwrap();
    assert!(
        search(&client, &stack, &base, connect_sql).await["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sample["sequence"] = json!(9);
    writeln!(file, "{sample}").unwrap();
    let mut loss = records[0].clone();
    loss["sequence"] = json!(101);
    loss["telemetry_dropped_total"] = json!(5);
    writeln!(file, "{loss}").unwrap();
    file.sync_all().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result = search(&client, &stack, &base, connect_sql).await;
        if let Some(row) = result["hits"].as_array().unwrap().first() {
            assert_eq!(row["attempts"], 10);
            assert_eq!(row["failures"], 2);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "20 percent error boundary missing"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let loss_rows = search(
        &client,
        &stack,
        &base,
        alerts[1]["query_condition"]["sql"].as_str().unwrap(),
    )
    .await;
    assert_eq!(loss_rows["hits"][0]["dropped"], 5);
    post(
        &client,
        &stack,
        &base,
        "/api/default/alerts/templates",
        json!({
            "name": "rds_fixture", "type": "http",
            "body": "{\"rds_alert_fixture\":\"delivered\",\"name\":\"{alert_name}\"}"
        }),
    )
    .await;
    post(
        &client,
        &stack,
        &base,
        "/api/default/alerts/destinations",
        json!({
            "name": "rds_fixture", "type": "http", "url": "http://127.0.0.1:8687/",
            "method": "post", "template": "rds_fixture",
            "headers": {"Content-Type": "application/json"}, "skip_tls_verify": false
        }),
    )
    .await;
    let mut alert = alerts[0].clone();
    alert["org_id"] = json!("default");
    alert["destinations"] = json!(["rds_fixture"]);
    alert["enabled"] = json!(true);
    post(&client, &stack, &base, "/api/v2/default/alerts", alert).await;
    let deadline = Instant::now() + Duration::from_secs(100);
    loop {
        let logs = stack.checked(&["logs", "--no-color", "receiver"]);
        if logs.contains("\"rds_alert_fixture\":\"delivered\"") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scheduled alert never reached the local fixture"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!("alerts: SQL predicates and scheduled local webhook delivery passed");

    // Recover all unique records after collector/backend restart. This allows
    // replay from source checkpoints; it is not an isolated disk-buffer or
    // power-loss durability proof.
    stack.checked(&["stop", "--timeout", "5", "openobserve"]);
    let output = Command::new(&fixture).output().unwrap();
    assert!(output.status.success());
    let second_text = String::from_utf8(output.stderr).unwrap();
    let second: Value = serde_json::from_str(second_text.lines().next().unwrap()).unwrap();
    assert_ne!(second["run_id"], first_run);
    file.write_all(second_text.as_bytes()).unwrap();
    file.sync_all().unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    stack.checked(&["restart", "--timeout", "5", "vector"]);
    stack.checked(&["start", "openobserve"]);
    // Docker can allocate a different ephemeral host port when restarting.
    let port = stack.checked(&["port", "openobserve", "5080"]);
    assert!(port.trim().starts_with("127.0.0.1:"));
    base = format!("http://{}", port.trim());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(response) = client.get(format!("{base}/healthz")).send().await
            && response.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "backend failed to restart");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let response = search(
            &client,
            &stack,
            &base,
            &format!(
                "SELECT DISTINCT sequence FROM rds_events WHERE run_id = '{}'",
                second["run_id"].as_str().unwrap()
            ),
        )
        .await;
        if response["hits"].as_array().unwrap().len() == 7 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "buffered log run did not recover"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    println!(
        "outage: product logging completed; all seven unique records recovered after collector/backend restart"
    );
}
