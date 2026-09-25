//! Shared length-prefixed owned-relay control codec (`rds-relay/0`).
//! Deadlines belong to callers so one budget can cover locks and full I/O.
use rds_core::relay::RelayControl;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Existing owned-relay control bound, independent of service/media frames.
pub const MAX_CONTROL_FRAME: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("relay control I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("relay control codec: {0}")]
    Codec(#[from] postcard::Error),
    #[error("relay control frame length {0} is outside 1..={MAX_CONTROL_FRAME}")]
    Length(usize),
    #[error("relay control frame has trailing bytes")]
    Trailing,
}

pub async fn read_control(
    recv: &mut (impl AsyncRead + Unpin),
) -> Result<RelayControl, ControlError> {
    let n = recv.read_u32().await? as usize;
    if n == 0 || n > MAX_CONTROL_FRAME {
        return Err(ControlError::Length(n));
    }
    let mut body = vec![0; n];
    recv.read_exact(&mut body).await?;
    let (control, tail) = postcard::take_from_bytes(&body)?;
    if !tail.is_empty() {
        return Err(ControlError::Trailing);
    }
    Ok(control)
}

pub async fn write_control(
    send: &mut (impl AsyncWrite + Unpin),
    control: &RelayControl,
) -> Result<(), ControlError> {
    let body = postcard::to_stdvec(control)?;
    if body.is_empty() || body.len() > MAX_CONTROL_FRAME {
        return Err(ControlError::Length(body.len()));
    }
    send.write_all(&(body.len() as u32).to_be_bytes()).await?;
    send.write_all(&body).await?;
    send.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn all_controls_roundtrip_as_coalesced_frames() {
        let controls = [
            RelayControl::Register,
            RelayControl::Registered,
            RelayControl::Drain,
            RelayControl::PeerGone { peer: [7; 32] },
            RelayControl::Ping { seq: 1 },
            RelayControl::Pong { seq: 1 },
            RelayControl::Health {
                endpoints: 3,
                draining: false,
            },
        ];
        let mut wire = Vec::new();
        for control in &controls {
            write_control(&mut wire, control).await.unwrap();
        }
        let mut reader = wire.as_slice();
        for control in controls {
            assert_eq!(
                format!("{:?}", read_control(&mut reader).await.unwrap()),
                format!("{control:?}")
            );
        }
        assert!(reader.is_empty());
    }
    #[tokio::test]
    async fn invalid_lengths_truncation_and_trailing_bytes_are_rejected() {
        for n in [0, 4097, u32::MAX] {
            let prefix = n.to_be_bytes();
            assert!(matches!(
                read_control(&mut prefix.as_slice()).await,
                Err(ControlError::Length(_))
            ));
        }
        let mut wire = Vec::new();
        write_control(&mut wire, &RelayControl::Drain)
            .await
            .unwrap();
        for end in 0..wire.len() {
            assert!(read_control(&mut &wire[..end]).await.is_err());
        }
        wire[3] += 1;
        wire.push(0);
        assert!(matches!(
            read_control(&mut wire.as_slice()).await,
            Err(ControlError::Trailing)
        ));
    }
    #[tokio::test]
    async fn fragmented_header_and_body_preserve_frame_boundaries() {
        let mut wire = Vec::new();
        write_control(&mut wire, &RelayControl::PeerGone { peer: [7; 32] })
            .await
            .unwrap();
        let (mut send, mut recv) = tokio::io::duplex(1);
        let producer = tokio::spawn(async move {
            for byte in wire {
                send.write_all(&[byte]).await.unwrap();
            }
        });
        assert!(
            matches!(read_control(&mut recv).await.unwrap(),RelayControl::PeerGone{peer} if peer==[7;32])
        );
        producer.await.unwrap();
    }
}
