mod support;
use rds_ssh::{Error, Exit, Pty, Request, Session, Size, Terminal};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

fn request(pty: bool) -> Request {
    Request {
        command: Some("fixture".into()),
        terminal: pty.then(|| Terminal {
            term: "xterm-256color".into(),
            size: Size {
                columns: 80,
                rows: 24,
            },
            modes: vec![(Pty::IUTF8, 1)],
        }),
    }
}
fn resize() -> watch::Receiver<Size> {
    watch::channel(Size {
        columns: 80,
        rows: 24,
    })
    .1
}

#[tokio::test]
async fn strict_host_key_is_verified_before_authentication() {
    let (stream, observed, task) = fixture(Case::Echo);
    let mut wrong = config();
    wrong.host_key = key(3).public_key().clone();
    assert!(matches!(
        Session::connect(stream, wrong).await,
        Err(Error::HostKey)
    ));
    finished(task).await;
    assert_eq!(observed.auth.load(Ordering::SeqCst), 0);
    assert_eq!(observed.exec.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authentication_rejection_closes_transport_without_exec() {
    let (stream, observed, task) = fixture(Case::RejectAuth);
    assert!(matches!(
        Session::connect(stream, config()).await,
        Err(Error::Authentication)
    ));
    finished(task).await;
    assert_eq!(observed.exec.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplex_output_larger_than_window_and_utf8_are_lossless() {
    let (stream, _, task) = fixture(Case::Echo);
    let session = Session::connect(stream, config()).await.unwrap();
    let bytes = "terminal: русский 日本語 🦀\n".repeat(25000).into_bytes();
    let mut output = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        session.run(
            request(false),
            &bytes[..],
            &mut output,
            tokio::io::sink(),
            resize(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(result, Exit::Status(0)));
    assert_eq!(output, bytes);
    finished(task).await;
}

#[tokio::test]
async fn status_does_not_truncate_trailing_output_or_wait_for_stdin() {
    let (stream, observed, task) = fixture(Case::EarlyExit);
    let session = Session::connect(stream, config()).await.unwrap();
    let (input, _hold_stdin_open) = tokio::io::duplex(8);
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let result = session
        .run(request(false), input, &mut output, &mut error, resize())
        .await
        .unwrap();
    assert!(matches!(result, Exit::Status(23)));
    assert_eq!(output, b"after-status\n");
    assert_eq!(error, b"stderr\n");
    finished(task).await;
    assert_eq!(observed.exec.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_exit_status_is_failure_and_is_not_replayed() {
    let (stream, observed, task) = fixture(Case::MissingStatus);
    let session = Session::connect(stream, config()).await.unwrap();
    let (input, _hold) = tokio::io::duplex(8);
    assert!(matches!(
        session
            .run(
                request(false),
                input,
                tokio::io::sink(),
                tokio::io::sink(),
                resize()
            )
            .await,
        Err(Error::MissingStatus)
    ));
    finished(task).await;
    assert_eq!(observed.exec.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pty_and_exec_require_positive_acknowledgement() {
    for (case, expected_exec) in [(Case::RejectPty, 0), (Case::RejectExec, 1)] {
        let (stream, observed, task) = fixture(case);
        let session = Session::connect(stream, config()).await.unwrap();
        assert!(matches!(
            session
                .run(
                    request(true),
                    tokio::io::empty(),
                    tokio::io::sink(),
                    tokio::io::sink(),
                    resize()
                )
                .await,
            Err(Error::Rejected(_))
        ));
        finished(task).await;
        assert_eq!(observed.pty.load(Ordering::SeqCst), 1);
        assert_eq!(observed.exec.load(Ordering::SeqCst), expected_exec);
    }
}

#[tokio::test]
async fn pty_resize_reaches_server_while_stdin_is_idle() {
    let (stream, observed, task) = fixture(Case::Resize);
    let session = Session::connect(stream, config()).await.unwrap();
    let (input, _hold) = tokio::io::duplex(8);
    let (tx, rx) = watch::channel(Size {
        columns: 80,
        rows: 24,
    });
    tx.send_replace(Size {
        columns: 120,
        rows: 40,
    });
    assert!(matches!(
        session
            .run(
                request(true),
                input,
                tokio::io::sink(),
                tokio::io::sink(),
                rx
            )
            .await
            .unwrap(),
        Exit::Status(0)
    ));
    finished(task).await;
    assert_eq!(observed.resized.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_during_banner_and_kex_closes_real_stream() {
    for banner in [false, true] {
        let (client, mut remote) = tokio::io::duplex(1024);
        let task = tokio::spawn(Session::connect(client, config()));
        let mut id = vec![0; 128];
        let count = remote.read(&mut id).await.unwrap();
        assert!(count > 0);
        if banner {
            remote.write_all(b"SSH-2.0-fixture\r\n").await.unwrap();
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(matches!(task.await, Err(e) if e.is_cancelled()));
        let mut remainder = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), remote.read_to_end(&mut remainder))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn cancellation_while_output_blocked_releases_transport() {
    let (stream, observed, server) = fixture(Case::Echo);
    let session = Session::connect(stream, config()).await.unwrap();
    let (output, _hold) = tokio::io::duplex(1);
    let task = tokio::spawn(session.run(
        request(false),
        std::io::Cursor::new(vec![b'x'; 1024 * 1024]),
        output,
        tokio::io::sink(),
        resize(),
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while observed.exec.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    finished(server).await;
}

#[tokio::test]
async fn unsolicited_agent_forwarding_channel_is_rejected() {
    let (stream, remote) = tokio::io::duplex(4096);
    let client = tokio::spawn(Session::connect(stream, config()));
    let handler = Handler {
        case: Case::Hold,
        observed: std::sync::Arc::new(Observed::default()),
    };
    let server = russh::server::run_stream(server_config(), remote, handler)
        .await
        .unwrap();
    let session = client.await.unwrap().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), server.handle().channel_open_agent())
            .await
            .unwrap()
            .is_err()
    );
    session.close().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap();
}

#[tokio::test]
async fn remote_early_exit_preserves_output_even_when_stdin_is_still_sending() {
    for _ in 0..4 {
        let (stream, _, task) = fixture(Case::EarlyExit);
        let session = Session::connect(stream, config()).await.unwrap();
        let mut output = Vec::new();
        let result = session
            .run(
                request(false),
                std::io::Cursor::new(vec![b'x'; 1024 * 1024]),
                &mut output,
                tokio::io::sink(),
                resize(),
            )
            .await
            .unwrap();
        assert!(matches!(result, Exit::Status(23)));
        assert_eq!(output, b"after-status\n");
        finished(task).await;
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_banner_deadline_releases_the_transport() {
    let (stream, mut remote) = tokio::io::duplex(1024);
    assert!(matches!(
        Session::connect(stream, config()).await,
        Err(Error::Timeout)
    ));
    let mut bytes = Vec::new();
    remote.read_to_end(&mut bytes).await.unwrap();
    assert!(bytes.starts_with(b"SSH-2.0-"));
}

#[tokio::test(start_paused = true)]
async fn responsive_idle_shell_survives_more_than_inactivity_timeout() {
    let (stream, observed, server) = fixture(Case::Hold);
    let session = Session::connect(stream, config()).await.unwrap();
    let (input, _hold) = tokio::io::duplex(8);
    let task = tokio::spawn(session.run(
        request(true),
        input,
        tokio::io::sink(),
        tokio::io::sink(),
        resize(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while observed.exec.load(Ordering::SeqCst) == 0 {
            assert!(!task.is_finished(), "shell setup failed");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(240)).await;
    assert!(
        !task.is_finished(),
        "idle responsive shell was disconnected"
    );
    task.abort();
    let _ = task.await;
    finished(server).await;
}
