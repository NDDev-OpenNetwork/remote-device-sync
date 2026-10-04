//! Verify the actual UDP wire boundary, not only configuration setters.
use rds_net::{Backend, EndpointConfig, Packetization, TransportAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::UdpSocket;

struct Proxy {
    address: std::net::SocketAddr,
    maximum: Arc<AtomicUsize>,
    bytes: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Proxy {
    async fn new(upstream: std::net::SocketAddr) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let maximum = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        let max = maximum.clone();
        let total = bytes.clone();
        let task = tokio::spawn(async move {
            let mut client = None;
            let mut packet = vec![0; 65536];
            loop {
                let (length, from) = socket.recv_from(&mut packet).await.unwrap();
                max.fetch_max(length, Ordering::Relaxed);
                total.fetch_add(length as u64, Ordering::Relaxed);
                let destination = if from == upstream {
                    client.unwrap()
                } else {
                    client = Some(from);
                    upstream
                };
                socket
                    .send_to(&packet[..length], destination)
                    .await
                    .unwrap();
            }
        });
        Self {
            address,
            maximum,
            bytes,
            task,
        }
    }
}

async fn conservative_transfer(backend: Backend) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let config = EndpointConfig {
            backend,
            packetization: Packetization::Conservative,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            max_multipath_paths: Some(1),
            observed_address_reports: false,
            ..Default::default()
        };
        let a = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let b = rds_net::bind_endpoint(config).await.unwrap();
        let mut address = b.addr();
        let upstream = address
            .addrs
            .iter()
            .find_map(|address| match address {
                TransportAddr::Ip(ip) => Some(*ip),
                _ => None,
            })
            .unwrap();
        let proxy = Proxy::new(upstream).await;
        address.addrs = [TransportAddr::Ip(proxy.address)].into_iter().collect();
        let (client, server) = tokio::join!(a.connect(address, rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        let (client, server) = (client.unwrap(), server.unwrap());
        let data: Vec<_> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
        let received = data.clone();
        let echo = tokio::spawn(async move {
            let (mut send, mut recv) = server.accept_bi().await.unwrap();
            let mut body = vec![0; received.len()];
            recv.read_exact(&mut body).await.unwrap();
            assert_eq!(body, received);
            send.write_all(&body).await.unwrap();
            send.finish().unwrap();
            let _ = server.accept_bi().await;
        });
        let (mut send, mut recv) = client.open_bi().await.unwrap();
        send.write_all(&data).await.unwrap();
        send.finish().unwrap();
        let mut body = vec![0; data.len()];
        recv.read_exact(&mut body).await.unwrap();
        assert_eq!(body, data);
        assert!(proxy.bytes.load(Ordering::Relaxed) >= (data.len() * 2) as u64);
        assert!(proxy.maximum.load(Ordering::Relaxed) <= 1200);
        client.close(0u32.into(), b"done");
        echo.await.unwrap();
        a.close().await;
        b.close().await;
    })
    .await
    .expect("conservative packetization did not deliver the echoed payload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_conservative_packets_never_exceed_1200_bytes() {
    conservative_transfer(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_conservative_packets_never_exceed_1200_bytes() {
    conservative_transfer(Backend::Noq).await;
}
