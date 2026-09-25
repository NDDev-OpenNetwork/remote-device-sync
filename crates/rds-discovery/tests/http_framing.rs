//! One exchange per connection must have one unambiguous bounded body length.
use rds_discovery::http::{self, MAX_BODY, MAX_HEAD};

async fn request(bytes: &[u8]) -> Result<Option<http::Request>, rds_discovery::DiscoveryError> {
    http::read_request(&mut std::io::Cursor::new(bytes)).await
}
async fn response(bytes: &[u8]) -> Result<http::Response, rds_discovery::DiscoveryError> {
    http::read_response(&mut std::io::Cursor::new(bytes)).await
}

#[tokio::test]
async fn duplicate_lengths_are_rejected_in_both_directions() {
    for fields in [
        "Content-Length: 0\r\nContent-Length: 0",
        "Content-Length: 9\r\ncontent-length: 0",
        "Content-Length: 0, 0",
        "Content-Length: +0",
        "Content-Length:",
    ] {
        assert!(
            request(
                format!("GET /v1/health HTTP/1.1\r\nHost: localhost\r\n{fields}\r\n\r\n")
                    .as_bytes()
            )
            .await
            .is_err(),
            "request accepted {fields:?}"
        );
        assert!(
            response(format!("HTTP/1.1 200 OK\r\n{fields}\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "response accepted {fields:?}"
        );
    }
}

#[tokio::test]
async fn transfer_encoding_is_refused_in_responses_as_well_as_requests() {
    for fields in [
        "Transfer-Encoding: chunked",
        "transfer-encoding: identity\r\nContent-Length: 0",
        "Content-Length: 0\r\nTransfer-Encoding: chunked",
    ] {
        assert!(
            request(format!("GET / HTTP/1.1\r\nHost: localhost\r\n{fields}\r\n\r\n").as_bytes())
                .await
                .is_err()
        );
        assert!(
            response(format!("HTTP/1.1 200 OK\r\n{fields}\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "response accepted {fields:?}"
        );
    }
}

#[tokio::test]
async fn malformed_start_lines_and_headers_are_not_normalized() {
    for line in [
        "GET / HTTP/1.9",
        "GET / HTTP/1.x",
        " / HTTP/1.1",
        "GET /\tbad HTTP/1.1",
        "GET /?query HTTP/1.1",
        "GET /#fragment HTTP/1.1",
    ] {
        assert!(
            request(format!("{line}\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "accepted {line:?}"
        );
    }
    for line in [
        "HTTP/1.9 200 OK",
        "HTTP/1.1 +200 OK",
        "HTTP/1.1 0200 OK",
        "HTTP/1.1 99 OK",
        "HTTP/1.1 600 OK",
        "HTTP/1.1 200",
        "HTTP/1.1 200 bad\0reason",
    ] {
        assert!(
            response(format!("{line}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "accepted {line:?}"
        );
    }
    for field in [
        "Content-Length : 0",
        " Content-Length: 0",
        "\tContent-Length: 0",
        ": value",
        "Bad Name: x",
        "X: bad\0value",
        "X: bad\nvalue",
        "X: bad\rvalue",
        "X: bad\u{7f}value",
        "X: non-ascii-\u{e9}",
        "Content-Encoding: gzip",
        "Expect: 100-continue",
    ] {
        assert!(
            request(format!("GET / HTTP/1.1\r\nHost: localhost\r\n{field}\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "request accepted {field:?}"
        );
        assert!(
            response(format!("HTTP/1.1 200 OK\r\n{field}\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "response accepted {field:?}"
        );
    }
}

#[tokio::test]
async fn host_and_explicit_response_framing_are_required() {
    for fields in [
        "",
        "Host:",
        "Host: localhost\r\nHost: localhost",
        "Host: user@localhost",
        "Host: local host",
        "Host: [::1",
        "Host: localhost:99999",
    ] {
        assert!(
            request(format!("GET / HTTP/1.1\r\n{fields}\r\n\r\n").as_bytes())
                .await
                .is_err(),
            "accepted host {fields:?}"
        );
    }
    assert!(response(b"HTTP/1.1 200 OK\r\n\r\n").await.is_err());
    assert!(
        response(b"HTTP/1.1 100 Continue\r\nContent-Length: 0\r\n\r\n")
            .await
            .is_err()
    );
    assert!(
        request(b"GET /v1/health HTTP/1.0\r\n\r\n")
            .await
            .unwrap()
            .unwrap()
            .body
            .is_empty()
    );
    assert!(
        request(b"GET / HTTP/1.1\r\nHost: [::1]:1234\r\nContent-Length:\t0 \t\r\n\r\n")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn head_and_body_limits_are_exact_including_the_terminal_crlf() {
    for prefix in [
        "GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nX: ",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX: ",
    ] {
        let bytes = format!(
            "{prefix}{}\r\n\r\n",
            "x".repeat(MAX_HEAD - prefix.len() - 4)
        );
        assert_eq!(bytes.len(), MAX_HEAD);
        if prefix.starts_with("GET") {
            assert!(request(bytes.as_bytes()).await.is_ok());
        } else {
            assert!(response(bytes.as_bytes()).await.is_ok());
        }
        let excessive = bytes.replacen("X: ", "X: xx", 1);
        // Exactly MAX_HEAD + 2; also exercise the old +1 terminator bypass.
        for oversized in [&excessive[..], &bytes.replacen("X: ", "X: x", 1)] {
            if prefix.starts_with("GET") {
                assert!(request(oversized.as_bytes()).await.is_err());
            } else {
                assert!(response(oversized.as_bytes()).await.is_err());
            }
        }
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    assert!(response(head.as_bytes()).await.is_err());
    let head = format!(
        "PUT /v1/records HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    assert!(request(head.as_bytes()).await.is_err());
    assert!(
        response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn writers_refuse_invalid_fields_and_limits_before_emitting_bytes() {
    for (method, path) in [
        ("GET\r\nX: injected", "/"),
        ("GET", "/\r\nX: injected"),
        ("", "/"),
        ("GET", "/?query"),
    ] {
        let mut out = Vec::new();
        assert!(
            http::write_request(&mut out, method, path, &[])
                .await
                .is_err()
        );
        assert!(out.is_empty());
    }
    let mut out = Vec::new();
    assert!(
        http::write_request(&mut out, "PUT", "/v1/records", &vec![0; MAX_BODY + 1])
            .await
            .is_err()
    );
    assert!(out.is_empty());
    for resp in [
        http::Response {
            status: 200,
            body: vec![0; MAX_BODY + 1],
        },
        http::Response::text(99, ""),
        http::Response::text(204, "forbidden body"),
        http::Response::text(304, "forbidden body"),
    ] {
        let mut out = Vec::new();
        assert!(http::write_response(&mut out, &resp).await.is_err());
        assert!(out.is_empty());
    }
}

#[tokio::test]
async fn ambiguous_request_cannot_commit_an_otherwise_valid_signed_publication() {
    use rds_discovery::{
        EndpointRecord, MemoryStore, RecordStore, Service,
        service::{self, ServiceConfig},
    };
    use tokio::{io::AsyncWriteExt, net::TcpStream};
    let store = std::sync::Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig::open_ephemeral(),
    )
    .await
    .unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[134; 32]);
    let record = EndpointRecord::publish(
        &key,
        1,
        vec!["127.0.0.1:4000".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        std::time::Duration::from_secs(300),
    )
    .unwrap();
    let body = serde_json::to_vec(&record).unwrap();
    let mut stream = TcpStream::connect(dir.addr()).await.unwrap();
    stream.write_all(format!("PUT /v1/records HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    let reply = http::read_response(&mut stream).await.unwrap();
    assert_eq!(reply.status, 400);
    assert!(store.is_empty());
}

#[tokio::test]
async fn client_refuses_ambiguous_framing_even_with_a_valid_record_body() {
    use rds_discovery::{EndpointKey, EndpointRecord, Service, client::Client};
    use tokio::{io::AsyncWriteExt, net::TcpListener};
    let key = ed25519_dalek::SigningKey::from_bytes(&[135; 32]);
    let record = EndpointRecord::publish(
        &key,
        1,
        vec!["127.0.0.1:4001".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        std::time::Duration::from_secs(300),
    )
    .unwrap();
    let body = serde_json::to_vec(&record).unwrap();
    for extra in [
        "Content-Length: 0\r\n",
        "Transfer-Encoding: chunked\r\n",
        "",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(listener.local_addr().unwrap());
        let body = &body;
        let endpoint = EndpointKey(key.verifying_key().to_bytes());
        let (fetched, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(client.fetch(&endpoint), async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let req = http::read_request(&mut stream).await.unwrap().unwrap();
                assert_eq!(req.method, "GET");
                let mut wire = format!(
                    "HTTP/1.1 200 OK\r\n{extra}Content-Length: {}\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                wire.extend_from_slice(body);
                stream.write_all(&wire).await.unwrap();
            })
        })
        .await
        .unwrap();
        if extra.is_empty() {
            assert_eq!(fetched.unwrap().payload, record.payload);
        } else {
            assert!(fetched.is_err(), "client accepted {extra:?}");
        }
    }
}

#[tokio::test]
async fn fragmented_maximum_binary_body_is_preserved_in_both_directions() {
    use tokio::io::AsyncWriteExt;
    let body: Vec<u8> = (0..MAX_BODY).map(|n| (n % 256) as u8).collect();
    for is_request in [true, false] {
        let mut wire = Vec::new();
        if is_request {
            http::write_request(&mut wire, "PUT", "/v1/records", &body)
                .await
                .unwrap();
        } else {
            http::write_response(
                &mut wire,
                &http::Response {
                    status: 200,
                    body: body.clone(),
                },
            )
            .await
            .unwrap();
        }
        // Seven bytes forces splits inside header names, CRLF and binary data.
        let (mut tx, mut rx) = tokio::io::duplex(7);
        let ((), received) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                async {
                    tx.write_all(&wire).await.unwrap();
                    tx.shutdown().await.unwrap();
                },
                async {
                    if is_request {
                        http::read_request(&mut rx).await.unwrap().unwrap().body
                    } else {
                        http::read_response(&mut rx).await.unwrap().body
                    }
                }
            )
        })
        .await
        .unwrap();
        assert_eq!(received, body);
    }
}

#[tokio::test]
async fn bodyless_statuses_and_truncated_messages_follow_the_profile() {
    for status in [204, 304] {
        let mut wire = Vec::new();
        http::write_response(&mut wire, &http::Response::text(status, ""))
            .await
            .unwrap();
        assert!(
            !String::from_utf8(wire.clone())
                .unwrap()
                .contains("Content-Length")
        );
        let parsed = response(&wire).await.unwrap();
        assert_eq!(parsed.status, status);
        assert!(parsed.body.is_empty());
    }
    assert!(
        response(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await
            .is_err()
    );
    // A 304 length describes the selected representation, never a message body.
    assert!(
        response(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 100\r\n\r\n")
            .await
            .unwrap()
            .body
            .is_empty()
    );
    assert!(request(b"").await.unwrap().is_none());
    assert!(response(b"").await.is_err());
    for wire in [
        &b"GET / HTTP/1.1\r\nHost: localhost\r\n\r"[..],
        &b"PUT / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\nx"[..],
    ] {
        assert!(request(wire).await.is_err());
    }
    assert!(
        response(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn directory_closes_after_one_exchange_and_head_has_no_body() {
    use rds_discovery::{
        MemoryStore,
        service::{self, ServiceConfig},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        std::sync::Arc::new(MemoryStore::default()),
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    for method in ["GET", "HEAD"] {
        let mut stream = TcpStream::connect(dir.addr()).await.unwrap();
        let first = format!("{method} /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n");
        // The second request is never routed and receives no second response.
        stream
            .write_all(
                format!("{first}GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut wire = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read_to_end(&mut wire),
        )
        .await
        .unwrap()
        .unwrap();
        let text = String::from_utf8(wire.clone()).unwrap();
        assert_eq!(text.matches("HTTP/1.1 ").count(), 1);
        let parsed = response(&wire).await.unwrap();
        if method == "HEAD" {
            assert_eq!(parsed.status, 405);
            assert!(text.ends_with("\r\n\r\n"));
            assert!(parsed.body.is_empty());
        } else {
            assert_eq!(parsed.status, 200);
        }
    }
}
