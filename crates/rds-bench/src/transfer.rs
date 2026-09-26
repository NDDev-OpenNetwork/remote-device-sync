//! Benchmark-only receipt: big-endian u64 byte count, BLAKE3 digest, EOF.
//! This is the local TCP target's protocol, not an RDS wire extension.

use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const CHUNK_BYTES: usize = 64 * 1024;

pub(crate) struct Measurement {
    pub bytes: u64,
    pub elapsed: Duration,
}

/// Time payload generation, hashing, upload and receiver completion together.
/// The caller owns the absolute operation deadline and both streams.
pub(crate) async fn send_verified(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    total: u64,
) -> anyhow::Result<Measurement> {
    ensure!(total > 0, "transfer payload must be nonempty");
    let mut generator = blake3::Hasher::new();
    generator.update(b"rds-bench-transfer/v1");
    let mut generator = generator.finalize_xof();
    let mut digest = blake3::Hasher::new();
    let mut chunk = vec![0; CHUNK_BYTES];
    let started = Instant::now();
    let mut written = 0;
    while written < total {
        let n = (total - written).min(CHUNK_BYTES as u64) as usize;
        // Position-dependent data also exposes duplicated/reordered chunks.
        generator.fill(&mut chunk[..n]);
        digest.update(&chunk[..n]);
        send.write_all(&chunk[..n]).await.context("write payload")?;
        written += n as u64;
    }
    send.shutdown().await.context("finish payload")?;
    let mut receipt = [0; 40];
    recv.read_exact(&mut receipt)
        .await
        .context("read receiver receipt")?;
    ensure!(
        receipt[..8] == total.to_be_bytes(),
        "receiver byte count mismatch"
    );
    ensure!(
        digest.finalize().eq(&receipt[8..]),
        "receiver digest mismatch"
    );
    ensure!(
        recv.read(&mut [0]).await.context("read receipt EOF")? == 0,
        "trailing receiver receipt data"
    );
    Ok(Measurement {
        bytes: total,
        elapsed: started.elapsed(),
    })
}

/// Acknowledge only after all payload bytes and FIN have been consumed.
pub(crate) async fn receive(
    socket: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> anyhow::Result<()> {
    let mut buffer = vec![0; CHUNK_BYTES];
    let mut received = 0u64;
    let mut digest = blake3::Hasher::new();
    loop {
        let n = socket.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        received = received
            .checked_add(n as u64)
            .context("receiver count overflow")?;
        digest.update(&buffer[..n]);
    }
    socket.write_all(&received.to_be_bytes()).await?;
    socket.write_all(digest.finalize().as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_send_window_cannot_complete_before_receiver_ack() {
        let total = CHUNK_BYTES as u64 * 2 + 17;
        let (sender, mut receiver) = tokio::io::duplex(total as usize + 1);
        let (mut read, mut write) = tokio::io::split(sender);
        let work = send_verified(&mut write, &mut read, total);
        tokio::pin!(work);
        // The entire body fits the transport buffer. A sender-only timer
        // would already finish, while the receiver has consumed nothing.
        tokio::select! {
            biased;
            result = &mut work => panic!("completed without receipt: {:?}", result.err()),
            () = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
        let (sent, received) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(work, receive(&mut receiver))
        })
        .await
        .unwrap();
        received.unwrap();
        let sent = sent.unwrap();
        assert_eq!(sent.bytes, total);
        assert!(sent.elapsed >= Duration::from_millis(20));
    }

    #[tokio::test]
    async fn invalid_or_missing_receipts_never_produce_throughput() {
        for fault in [
            "count",
            "digest",
            "truncated",
            "trailing",
            "missing",
            "corrupt_body",
            "short_body",
            "reordered_body",
        ] {
            let (sender, mut receiver) = tokio::io::duplex(1024);
            let (mut read, mut write) = tokio::io::split(sender);
            let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(send_verified(&mut write, &mut read, 257), async {
                    let mut body = Vec::new();
                    receiver.read_to_end(&mut body).await.unwrap();
                    assert_eq!(body.len(), 257);
                    match fault {
                        "corrupt_body" => body[0] ^= 1,
                        "short_body" => {
                            body.pop();
                        }
                        "reordered_body" => body.rotate_left(1),
                        _ => {}
                    }
                    let mut receipt = (body.len() as u64).to_be_bytes().to_vec();
                    receipt.extend_from_slice(blake3::hash(&body).as_bytes());
                    match fault {
                        "count" => receipt[7] ^= 1,
                        "digest" => receipt[8] ^= 1,
                        "truncated" => {
                            receipt.pop();
                        }
                        "trailing" => receipt.push(0),
                        "missing" => receipt.clear(),
                        "corrupt_body" | "short_body" | "reordered_body" => {}
                        _ => unreachable!(),
                    }
                    receiver.write_all(&receipt).await.unwrap();
                    receiver.shutdown().await.unwrap();
                })
            })
            .await
            .unwrap();
            assert!(result.is_err(), "accepted {fault} receipt");
        }
    }

    #[tokio::test]
    async fn missing_receipt_or_eof_remains_subject_to_the_operation_deadline() {
        for ack_before_stall in [false, true] {
            let (sender, mut receiver) = tokio::io::duplex(1024);
            let (mut read, mut write) = tokio::io::split(sender);
            let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(
                    tokio::time::timeout(
                        Duration::from_millis(20),
                        send_verified(&mut write, &mut read, 257)
                    ),
                    async {
                        let mut body = Vec::new();
                        receiver.read_to_end(&mut body).await.unwrap();
                        if ack_before_stall {
                            receiver
                                .write_all(&(body.len() as u64).to_be_bytes())
                                .await
                                .unwrap();
                            receiver
                                .write_all(blake3::hash(&body).as_bytes())
                                .await
                                .unwrap();
                        }
                        // Keep the stream alive without EOF until the deadline.
                    }
                )
            })
            .await
            .unwrap();
            assert!(result.is_err());
        }
    }
}
