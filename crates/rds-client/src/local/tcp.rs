//! Explicit FIN on the local wire keeps half-close independent of cancellation.
use std::io;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};

use rds_core::local::{TCP_CHUNK, TcpFrame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid local TCP body")
}

async fn encode(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> io::Result<()> {
    let mut buffer = vec![0; TCP_CHUNK];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            rds_core::write_frame(writer, &TcpFrame::Finish).await?;
            // Do not OS-half-close the IPC socket: its EOF means caller loss.
            return Ok(());
        }
        rds_core::write_frame(writer, &TcpFrame::Data(buffer[..read].to_vec())).await?;
    }
}

async fn decode(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> io::Result<()> {
    loop {
        match rds_core::read_frame::<_, TcpFrame>(reader).await? {
            TcpFrame::Data(bytes) if !bytes.is_empty() && bytes.len() <= TCP_CHUNK => {
                writer.write_all(&bytes).await?
            }
            TcpFrame::Data(_) => return Err(invalid()),
            TcpFrame::Finish => {
                writer.shutdown().await?;
                return Ok(());
            }
        }
    }
}

/// After an upload FIN, keep observing IPC EOF while the download is pending.
/// A clean FIN in both directions preserves buffered QUIC writes. Any protocol
/// error, caller drop or task cancellation leaves the owner's reset guard armed.
pub(super) async fn serve(
    stream: &mut UnixStream,
    send: &mut rds_net::SendStream,
    recv: &mut rds_net::RecvStream,
) -> io::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let mut upload = Box::pin(decode(&mut reader, send));
    let mut download = Box::pin(encode(recv, &mut writer));
    tokio::select! {
        result = &mut upload => {
            result?;
            drop(upload);
            let mut extra = [0];
            tokio::select! {
                biased;
                result = &mut download => result,
                _ = reader.read(&mut extra) => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "local TCP caller ended")),
            }
        }
        result = &mut download => { result?; upload.await }
    }
}

/// Bounded AsyncRead/AsyncWrite adapter. `shutdown()` sends a byte-direction FIN;
/// Drop cancels the owned pump and closes IPC, including when the peer is silent.
/// No secret byte buffers are exposed through Debug or logs.
pub struct TcpStream {
    stream: DuplexStream,
    task: Option<JoinHandle<()>>,
    shutdown: Option<oneshot::Receiver<Result<(), io::ErrorKind>>>,
    shutdown_result: Option<Result<(), io::ErrorKind>>,
    failed: Arc<AtomicBool>,
}

impl TcpStream {
    pub(super) fn new(mut ipc: UnixStream) -> Self {
        let (stream, mut body) = tokio::io::duplex(TCP_CHUNK);
        let (finished, shutdown) = oneshot::channel();
        let failed = Arc::new(AtomicBool::new(false));
        let pump_failed = failed.clone();
        let task = tokio::spawn(async move {
            let (mut local_read, mut local_write) = tokio::io::split(&mut body);
            let (mut ipc_read, mut ipc_write) = ipc.split();
            let upload = async {
                let result = encode(&mut local_read, &mut ipc_write).await;
                let _ = finished.send(result.as_ref().map(|_| ()).map_err(|e| e.kind()));
                result
            };
            if tokio::try_join!(upload, decode(&mut ipc_read, &mut local_write)).is_err() {
                pump_failed.store(true, Ordering::Release);
            }
        });
        Self {
            stream,
            task: Some(task),
            shutdown: Some(shutdown),
            shutdown_result: None,
            failed,
        }
    }

    /// Explicit cancellation with pump join; Drop remains cancellation-safe.
    pub async fn close(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = Pin::new(&mut this.stream).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Ok(())))
            && buffer.filled().len() == before
            && buffer.remaining() != 0
            && this.failed.load(Ordering::Acquire)
        {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        result
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_shutdown(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        if let Some(receiver) = &mut this.shutdown {
            let result = match Pin::new(receiver).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(result) => result.unwrap_or(Err(io::ErrorKind::ConnectionAborted)),
            };
            this.shutdown = None;
            this.shutdown_result = Some(result);
        }
        // Invariant: the receiver is removed only after recording its result.
        Poll::Ready(
            this.shutdown_result
                .unwrap_or(Err(io::ErrorKind::ConnectionAborted))
                .map_err(Into::into),
        )
    }
}
