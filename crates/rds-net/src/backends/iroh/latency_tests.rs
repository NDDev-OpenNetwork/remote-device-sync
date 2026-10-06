//! Real Iroh actor over a bounded controllable custom link and UDP fallback.
use iroh::TransportAddr;
use iroh::endpoint::transports::{
    CustomEndpoint, CustomSender, CustomTransport, PathSelection, PathSelectionContext,
    PathSelector, RecvInfo, Transmit,
};
use iroh_base::CustomAddr;
use std::collections::HashMap;
use std::io::{self, IoSliceMut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug)]
struct Packet {
    from: CustomAddr,
    data: Vec<u8>,
}
type Fabric = Arc<Mutex<HashMap<CustomAddr, mpsc::Sender<Packet>>>>;
#[derive(Debug)]
struct Link {
    addr: CustomAddr,
    fabric: Fabric,
    drop_packets: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
impl CustomTransport for Link {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let (tx, rx) = mpsc::channel(64);
        self.fabric.lock().unwrap().insert(self.addr.clone(), tx);
        Ok(Box::new(LinkEndpoint {
            sender: Arc::new(LinkSender {
                addr: self.addr.clone(),
                fabric: self.fabric.clone(),
                drop_packets: self.drop_packets.clone(),
                dropped: self.dropped.clone(),
            }),
            rx,
            addresses: n0_watcher::Watchable::new(vec![self.addr.clone()]),
        }))
    }
}
#[derive(Debug)]
struct LinkEndpoint {
    sender: Arc<LinkSender>,
    rx: mpsc::Receiver<Packet>,
    addresses: n0_watcher::Watchable<Vec<CustomAddr>>,
}
impl Drop for LinkEndpoint {
    fn drop(&mut self) {
        self.sender.fabric.lock().unwrap().remove(&self.sender.addr);
    }
}
impl CustomEndpoint for LinkEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.addresses.watch()
    }
    fn create_sender(&self) -> Arc<dyn CustomSender> {
        self.sender.clone()
    }
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        metas: &mut [noq::udp::RecvMeta],
        infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(packet)) => {
                assert!(packet.data.len() <= bufs[0].len());
                bufs[0][..packet.data.len()].copy_from_slice(&packet.data);
                let mut meta = noq::udp::RecvMeta::default();
                meta.len = packet.data.len();
                meta.stride = packet.data.len();
                metas[0] = meta;
                infos[0] = RecvInfo::new(packet.from, Some(self.sender.addr.clone()));
                Poll::Ready(Ok(1))
            }
            Poll::Ready(None) => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}
#[derive(Debug)]
struct LinkSender {
    addr: CustomAddr,
    fabric: Fabric,
    drop_packets: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
impl CustomSender for LinkSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == self.addr.id()
    }
    fn poll_send(
        &self,
        _cx: &mut Context<'_>,
        dst: &CustomAddr,
        src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        if self.drop_packets.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Ok(()));
        }
        let sender = self
            .fabric
            .lock()
            .unwrap()
            .get(dst)
            .cloned()
            .ok_or(io::ErrorKind::NotConnected);
        let result = sender.and_then(|sender| {
            sender
                .try_send(Packet {
                    from: src.unwrap_or(&self.addr).clone(),
                    data: transmit.contents.to_vec(),
                })
                .map_err(|_| io::ErrorKind::Other)
        });
        Poll::Ready(result.map_err(Into::into))
    }
}

#[derive(Debug)]
struct SetupSelector(Arc<AtomicBool>);
impl PathSelector for SetupSelector {
    fn refresh_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        if self.0.load(Ordering::Acquire) {
            return super::latency::LatencySelector.select(ctx);
        }
        // Establish the failure precondition deterministically. On a busy CI
        // runner the genuinely faster UDP path may otherwise win before loss
        // is injected, leaving this a test of setup timing rather than recovery.
        let mut choice = PathSelection::none();
        if let Some(path) = ctx.paths().find(|path| {
            matches!(
                path.network_path().remote(),
                iroh::endpoint::transports::Addr::Custom(_)
            )
        }) {
            choice.set(&path);
        }
        choice
    }
}

async fn endpoint(link: Link, armed: Arc<AtomicBool>) -> iroh::Endpoint {
    let mut transport = iroh::endpoint::QuicTransportConfig::builder()
        .congestion_controller_factory(crate::ack_progress::factory(
            crate::CongestionControl::Cubic.factory(),
            Arc::new(|| tokio::time::Instant::now().into_std()),
        ));
    transport = transport
        .initial_mtu(1200)
        .min_mtu(1200)
        .mtu_discovery_config(None)
        .enable_segmentation_offload(false);
    iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .add_custom_transport(Arc::new(link))
        .path_selector(Arc::new(SetupSelector(armed)))
        .transport_config(transport.build())
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_iroh_actor_retires_blackholed_preferred_link_and_delivers_pending_stream_bytes() {
    tokio::time::timeout(Duration::from_secs(15),async {
        let fabric=Fabric::default();
        let armed=Arc::new(AtomicBool::new(false));
        let dropping=Arc::new(AtomicBool::new(false));let dropped=Arc::new(AtomicU64::new(0));
        let a_addr=CustomAddr::from_parts(0x72647374657374,b"a");let b_addr=CustomAddr::from_parts(0x72647374657374,b"b");
        let a=endpoint(Link {addr:a_addr,fabric:fabric.clone(),drop_packets:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicU64::new(0))},armed.clone()).await;
        let b=endpoint(Link {addr:b_addr.clone(),fabric,drop_packets:dropping.clone(),dropped:dropped.clone()},armed.clone()).await;
        let target=iroh::EndpointAddr::from_parts(b.id(),[TransportAddr::Custom(b_addr)]);
        let (client,server)=tokio::join!(a.connect(target,rds_core::ALPN),async {b.accept().await.unwrap().await});
        let (client,server)=(client.unwrap(),server.unwrap());
        let (mut request,mut reply)=client.open_bi().await.unwrap();request.write_all(b"warm").await.unwrap();
        let (mut send,mut recv)=server.accept_bi().await.unwrap();let mut warm=[0;4];recv.read_exact(&mut warm).await.unwrap();
        send.write_all(b"ok").await.unwrap();let mut ack=[0;2];reply.read_exact(&mut ack).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5),async {
            loop {
                let paths=server.paths();
                if paths.iter().any(|p|p.is_ip()) && paths.iter().any(|p|p.is_selected() && matches!(p.remote_addr(),TransportAddr::Custom(_))) { break }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("fixture did not establish its UDP standby while the custom link was preferred");
        let selected=server.paths().iter().find(|path|path.is_selected()).unwrap().id();
        let before=server.paths().get(selected).unwrap().stats().frame_tx.stream;
        dropping.store(true,Ordering::Release);let started=Instant::now();send.write_all(b"pending reliable bytes").await.unwrap();
        tokio::time::timeout(Duration::from_secs(1),async {
            while dropped.load(Ordering::Relaxed)==0 || server.paths().get(selected).unwrap().stats().frame_tx.stream<=before {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("fixture did not emit and drop the already-pending stream frame");
        armed.store(true,Ordering::Release);
        let mut body=[0;22];let result=tokio::time::timeout(Duration::from_secs(3),reply.read_exact(&mut body)).await;
        let elapsed=started.elapsed().as_millis();let injected=dropped.load(Ordering::Relaxed);
        tokio::join!(a.close(),b.close());
        assert!(injected>0,"blackhole fixture did not drop a packet");
        result.expect("Iroh left the existing reliable stream stalled after its preferred path blackholed").unwrap();
        assert_eq!(&body,b"pending reliable bytes");assert!(elapsed<3000);
    }).await.expect("isolated Iroh blackhole fixture did not terminate");
}
