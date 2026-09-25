#![cfg(unix)]
#[path = "../../rds-ssh/tests/support/mod.rs"]
mod ssh_fixture;

use rds_client::local::{Client, Prepared, Server};
use rds_net::{Endpoint, EndpointConfig, Ticket, bind_endpoint};
use ssh_fixture::{Case, Observed};
use std::{
    fs::File,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{process::Command, task::JoinSet};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        Self::under(std::path::Path::new("/tmp"))
    }
    fn under(base: &std::path::Path) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = base.canonicalize().unwrap().join(format!(
            "rds-ssh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn write(&self, name: &str, content: &[u8]) -> PathBuf {
        use std::io::Write as _;
        let path = self.0.join(name);
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(content).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    root: Scratch,
    local: Endpoint,
    remote: Endpoint,
    manager: Server,
    agent: Arc<rds_agent::Agent>,
    tasks: JoinSet<()>,
    target: String,
    observed: Arc<Observed>,
    logs: AtomicUsize,
}
impl Fixture {
    async fn new(case: Case) -> Self {
        Self::new_backend(case, rds_net::Backend::Iroh).await
    }
    async fn new_backend(case: Case, backend: rds_net::Backend) -> Self {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = tcp.local_addr().unwrap().to_string();
        let observed = Arc::new(Observed::default());
        let observed_server = observed.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut workers = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = workers.join_next(), if !workers.is_empty() => {}
                    accepted = tcp.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let handler = ssh_fixture::Handler { case, observed: observed_server.clone() };
                        workers.spawn(async move {
                            if let Ok(session) = russh::server::run_stream(ssh_fixture::server_config(), stream, handler).await { let _ = session.await; }
                        });
                    }
                }
            }
        });
        Self::with_target(target, tasks, observed, backend).await
    }
    async fn with_target(
        target: String,
        mut tasks: JoinSet<()>,
        observed: Arc<Observed>,
        backend: rds_net::Backend,
    ) -> Self {
        let root = Scratch::new();
        root.write(
            "host.pub",
            ssh_fixture::key(1)
                .public_key()
                .to_openssh()
                .unwrap()
                .as_bytes(),
        );
        root.write(
            "identity",
            ssh_fixture::key(2)
                .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        );
        let config = || EndpointConfig {
            backend,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let local = bind_endpoint(config()).await.unwrap();
        let remote = bind_endpoint(config()).await.unwrap();
        let manager = Server::start(
            Some(Prepared::bind(root.0.join("control")).await.unwrap()),
            local.clone(),
            None,
        );
        let mut policy = rds_agent::AgentPolicy::ssh_only(
            target.parse::<rds_core::TcpTarget>().unwrap().into_parts(),
        );
        policy.allow.insert(local.id());
        let agent = Arc::new(rds_agent::Agent::new(remote.clone(), policy));
        let service = agent.clone();
        tasks.spawn(async move {
            service.run().await.unwrap();
        });
        Self {
            root,
            local,
            remote,
            manager,
            agent,
            tasks,
            target,
            observed,
            logs: AtomicUsize::new(0),
        }
    }
    fn command(&self) -> Command {
        let mut command = self.base_command("fixture");
        command.arg("--identity").arg(self.root.0.join("identity"));
        command
    }
    fn base_command(&self, user: &str) -> Command {
        self.build_command(user, true)
    }
    fn console_command(&self) -> Command {
        let mut command = self.build_command("fixture", false);
        command.arg("--identity").arg(self.root.0.join("identity"));
        command
    }
    fn build_command(&self, user: &str, file: bool) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rds"));
        cmd.kill_on_drop(true)
            .env("XDG_CONFIG_HOME", self.root.0.join("empty-config"))
            .env("RDS_LOG_FORMAT", if file { "json" } else { "text" })
            .env("RUST_LOG", "off")
            .arg("--control-dir")
            .arg(self.root.0.join("control"))
            .arg("ssh")
            .arg(Ticket::of(&self.remote).to_string())
            .args(["--user", user, "--remote", &self.target])
            .arg("--host-key")
            .arg(self.root.0.join("host.pub"));
        if user == "fixture" {
            cmd.args(["--exec", "fixture"]);
        }
        if file {
            cmd.arg("--log-file").arg(self.root.0.join(format!(
                "log-{}.jsonl",
                self.logs.fetch_add(1, Ordering::Relaxed)
            )));
        }
        cmd
    }
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.agent.active_streams() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn close(mut self) {
        self.manager.close().await.unwrap();
        self.remote.close().await;
        self.local.close().await;
        self.tasks.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_cli_preserves_status_and_reuses_the_agent_connection() {
    let fixture = Fixture::new(Case::EarlyExit).await;
    for _ in 0..2 {
        let output = tokio::time::timeout(Duration::from_secs(10), fixture.command().output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"after-status\n");
        assert!(
            output.stderr.starts_with(b"stderr\n")
                || output.stderr.windows(7).any(|w| w == b"stderr\n")
        );
    }
    fixture.idle().await;
    assert!(!fixture.root.0.join("empty-config").exists());
    let snapshot = Client::new(fixture.root.0.join("control"))
        .snapshot()
        .await
        .unwrap();
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].peer, fixture.remote.id().to_string());
    assert_eq!(fixture.observed.exec.load(Ordering::SeqCst), 2);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_stderr_cannot_impersonate_export_telemetry() {
    use std::os::unix::fs::MetadataExt as _;
    let fixture = Fixture::new(Case::SpoofTelemetry).await;
    let output = tokio::time::timeout(Duration::from_secs(10), fixture.command().output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stderr, ssh_fixture::SPOOF);
    let path = fixture.root.0.join("log-0.jsonl");
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(!log.contains("PRIVATE_SENTINEL"));
    assert!(log.contains("ssh_connect") && log.contains("ssh_session"));
    assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    fixture.idle().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_json_without_a_separate_log_fails_before_connecting() {
    let fixture = Fixture::new(Case::Hold).await;
    let output = fixture
        .console_command()
        .env("RDS_LOG_FORMAT", "json")
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--log-file"));
    assert!(
        Client::new(fixture.root.0.join("control"))
            .snapshot()
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipe_input_and_output_exceed_windows_without_stalling() {
    use tokio::io::AsyncWriteExt as _;
    let fixture = Fixture::new(Case::Echo).await;
    let bytes = "рds 🦀\n".repeat(50000).into_bytes();
    let mut child = fixture
        .command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (_, output) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(
            async {
                stdin.write_all(&bytes).await.unwrap();
                drop(stdin);
            },
            child.wait_with_output()
        )
    })
    .await
    .unwrap();
    let output = output.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, bytes);
    fixture.idle().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_host_key_and_unsafe_private_files_never_authenticate() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new(Case::Echo).await;
    let public = fixture.root.0.join("host.pub");
    std::fs::write(
        &public,
        ssh_fixture::key(3).public_key().to_openssh().unwrap(),
    )
    .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(5), fixture.command().output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.observed.auth.load(Ordering::SeqCst), 0);
    std::fs::write(
        &public,
        ssh_fixture::key(1).public_key().to_openssh().unwrap(),
    )
    .unwrap();
    let identity = fixture.root.0.join("identity");
    std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = tokio::time::timeout(Duration::from_secs(3), fixture.command().output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    std::fs::remove_file(&identity).unwrap();
    symlink(&public, &identity).unwrap();
    let output = tokio::time::timeout(Duration::from_secs(3), fixture.command().output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    std::fs::remove_file(&identity).unwrap();
    // Apple targets do not expose rustix::fs::mkfifoat. Exercise special-file
    // rejection on every Unix target, with the blocking FIFO case on Linux.
    let socket = std::os::unix::net::UnixListener::bind(&identity).unwrap();
    let output = tokio::time::timeout(Duration::from_secs(3), fixture.command().output())
        .await
        .expect("private-key socket blocked")
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    drop(socket);
    std::fs::remove_file(&identity).unwrap();
    #[cfg(target_os = "linux")]
    {
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &identity,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(3), fixture.command().output())
            .await
            .expect("private-key FIFO blocked")
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
    }
    assert_eq!(fixture.observed.auth.load(Ordering::SeqCst), 0);
    fixture.idle().await;
    fixture.close().await;
}

fn pty() -> (File, File) {
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave = rustix::fs::open(
        name.as_c_str(),
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    rustix::termios::tcsetwinsize(
        &slave,
        rustix::termios::Winsize {
            ws_col: 80,
            ws_row: 24,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .unwrap();
    (master.into(), slave.into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tty_modes_and_shared_descriptor_flags_restore_on_rejection_and_signal() {
    for case in [Case::RejectPty, Case::Hold, Case::Resize] {
        let fixture = Fixture::new(case).await;
        let (_master, slave) = pty();
        let saved = rustix::termios::tcgetattr(&slave).unwrap();
        let flags = rustix::fs::fcntl_getfl(&slave).unwrap();
        let mut child = fixture
            .console_command()
            .env("TERM", "xterm-256color")
            .arg("--pty")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap())
            .spawn()
            .unwrap();
        if !matches!(case, Case::RejectPty) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while fixture.observed.exec.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !rustix::termios::tcgetattr(&slave)
                    .unwrap()
                    .local_modes
                    .contains(rustix::termios::LocalModes::ICANON)
            );
            let pid = rustix::process::Pid::from_raw(child.id().unwrap() as i32).unwrap();
            if matches!(case, Case::Resize) {
                rustix::termios::tcsetwinsize(
                    &slave,
                    rustix::termios::Winsize {
                        ws_col: 120,
                        ws_row: 40,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    },
                )
                .unwrap();
                rustix::process::kill_process(pid, rustix::process::Signal::WINCH).unwrap();
            } else {
                rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
            }
        }
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(match case {
                Case::Resize => 0,
                Case::Hold => 143,
                _ => 1,
            })
        );
        let restored = rustix::termios::tcgetattr(&slave).unwrap();
        // XNU's ttioctl sets PENDIN when restoring ICANON without flushing
        // pending input (bsd/kern/tty.c). It is kernel queue state, not a
        // terminal mode that tcsetattr can restore byte-for-byte. Keep every
        // other mode, control character and descriptor flag comparison exact.
        #[cfg(target_os = "macos")]
        assert_eq!(
            restored.local_modes & !rustix::termios::LocalModes::PENDIN,
            saved.local_modes & !rustix::termios::LocalModes::PENDIN
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(restored.local_modes, saved.local_modes);
        assert_eq!(restored.input_modes, saved.input_modes);
        assert_eq!(restored.output_modes, saved.output_modes);
        assert_eq!(restored.control_modes, saved.control_modes);
        use rustix::termios::SpecialCodeIndex as Code;
        for code in [
            Code::VINTR,
            Code::VQUIT,
            Code::VERASE,
            Code::VKILL,
            Code::VEOF,
            Code::VTIME,
            Code::VMIN,
            Code::VSTART,
            Code::VSTOP,
            Code::VSUSP,
            Code::VEOL,
            Code::VREPRINT,
            Code::VDISCARD,
            Code::VWERASE,
            Code::VLNEXT,
            Code::VEOL2,
        ] {
            assert_eq!(restored.special_codes[code], saved.special_codes[code]);
        }
        let restored_flags = rustix::fs::fcntl_getfl(&slave).unwrap();
        // XNU exposes the read-only FWASWRITTEN bit through F_GETFL after
        // output (bsd/sys/fcntl.h); F_SETFL cannot clear this kernel history.
        // All other bits, particularly shared O_NONBLOCK, remain exact.
        #[cfg(target_os = "macos")]
        assert_eq!(
            restored_flags.bits() & !0x0001_0000,
            flags.bits() & !0x0001_0000
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(restored_flags, flags);
        fixture.idle().await;
        fixture.close().await;
    }
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_ssh_runs_over_owned_transport_manager() {
    let fixture = Fixture::new_backend(Case::EarlyExit, rds_net::Backend::Noq).await;
    let output = tokio::time::timeout(Duration::from_secs(10), fixture.command().output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"after-status\n");
    fixture.idle().await;
    fixture.close().await;
}

// Qualification-only use of installed reference executables. RDS product code
// never invokes these. Every key/socket/config is synthetic and private to this
// test; sshd runs once per accepted loopback connection in inetd mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires installed OpenSSH sshd, ssh-agent, ssh-add and an unlocked local test account"]
async fn openssh_interop_key_agent_exec_and_os_pty() {
    use std::os::fd::OwnedFd;
    let root = Scratch::new();
    // OpenSSH StrictModes checks every authorized_keys ancestor up to the
    // account home. A sticky /tmp parent is intentionally not exempted there.
    let authorized = Scratch::under(&PathBuf::from(
        std::env::var_os("HOME").expect("local account home"),
    ));
    let host = root.write(
        "host",
        ssh_fixture::key(1)
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    );
    let auth = authorized.write(
        "authorized_keys",
        ssh_fixture::key(2)
            .public_key()
            .to_openssh()
            .unwrap()
            .as_bytes(),
    );
    let config = root.write("sshd_config", format!(
        "HostKey {}\nAuthorizedKeysFile {}\nStrictModes yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAuthenticationMethods publickey\nUsePAM no\nPermitRootLogin prohibit-password\nAllowAgentForwarding no\nAllowTcpForwarding no\nX11Forwarding no\nPrintMotd no\nPrintLastLog no\nLogLevel VERBOSE\n",
        host.display(),auth.display()).as_bytes());
    let check = Command::new("/usr/sbin/sshd")
        .arg("-t")
        .arg("-f")
        .arg(&config)
        .output()
        .await
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let user = std::env::var("USER").expect("local test account USER");
    assert_ne!(user, "fixture");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap().to_string();
    let errors = root.write("sshd-errors", b"");
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {
                Some(_) = children.join_next(), if !children.is_empty() => {},
                accepted = listener.accept() => {
                    let (stream, _) = accepted.unwrap();
                    let stream = stream.into_std().unwrap(); stream.set_nonblocking(false).unwrap();
                    let fd: OwnedFd = stream.into();
                    let stdout = rustix::io::dup(&fd).unwrap();
                    let mut child = Command::new("/usr/sbin/sshd").kill_on_drop(true).arg("-i").arg("-e").arg("-f").arg(&config)
                        .stdin(fd).stdout(stdout).stderr(std::fs::OpenOptions::new().append(true).open(&errors).unwrap()).spawn().unwrap();
                    children.spawn(async move { child.wait().await.unwrap(); });
                }
            }
        }
    });
    let fixture = Fixture::with_target(
        target,
        tasks,
        Arc::new(Observed::default()),
        rds_net::Backend::Iroh,
    )
    .await;
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        fixture
            .base_command(&user)
            .env("RDS_LOG_FORMAT", "text")
            .arg("--identity")
            .arg(fixture.root.0.join("identity"))
            .args([
                "--exec",
                "printf 'stdout-interop'; printf 'stderr-interop' >&2; exit 23",
            ])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        std::fs::read_to_string(root.0.join("sshd-errors")).unwrap()
    );
    assert_eq!(output.stdout, b"stdout-interop");
    assert!(String::from_utf8_lossy(&output.stderr).contains("stderr-interop"));

    let socket = root.0.join("agent.sock");
    let mut agent = Command::new("ssh-agent")
        .kill_on_drop(true)
        .arg("-D")
        .arg("-a")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let added = Command::new("ssh-add")
        .env("SSH_AUTH_SOCK", &socket)
        .arg(fixture.root.0.join("identity"))
        .output()
        .await
        .unwrap();
    assert!(added.status.success());
    let public = root.write(
        "agent.pub",
        ssh_fixture::key(2)
            .public_key()
            .to_openssh()
            .unwrap()
            .as_bytes(),
    );
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        fixture
            .base_command(&user)
            .env("SSH_AUTH_SOCK", &socket)
            .arg("--agent-key")
            .arg(public)
            .args(["--exec", "printf 'agent-interop'"])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"agent-interop");
    agent.kill().await.unwrap();
    agent.wait().await.unwrap();

    let (master, slave) = pty();
    let original = rustix::termios::tcgetattr(&slave).unwrap();
    let mut child = fixture
        .base_command(&user)
        .env("TERM", "xterm-256color")
        .arg("--identity")
        .arg(fixture.root.0.join("identity"))
        .args([
            "--pty",
            "--exec",
            "test -t 0 && stty size; printf 'REAL-PTY\\n'; exit 17",
        ])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap())
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .unwrap()
        .unwrap();
    rustix::fs::fcntl_setfl(
        &master,
        rustix::fs::fcntl_getfl(&master).unwrap() | rustix::fs::OFlags::NONBLOCK,
    )
    .unwrap();
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    loop {
        match rustix::io::read(&master, &mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::IO) => break,
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(
        status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("24 80"));
    assert!(text.contains("REAL-PTY"));
    assert_eq!(
        rustix::termios::tcgetattr(&slave).unwrap().local_modes,
        original.local_modes
    );
    fixture.idle().await;
    fixture.close().await;
}
