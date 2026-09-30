//! X11 CLIPBOARD owner using ICCCM TARGETS/TIMESTAMP and INCR transfers.
//! No external clipboard helper. One worker/window per controlling session.
use crate::DesktopError;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc, oneshot};
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{Event, xproto::*},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
static WORKERS: Semaphore = Semaphore::const_new(4);
struct Job {
    text: String,
    reply: oneshot::Sender<Result<(), DesktopError>>,
}
pub(crate) struct Worker {
    jobs: mpsc::Sender<Job>,
    stopped: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Worker {
    pub(crate) fn new(display: u32) -> Self {
        let (jobs, mut rx) = mpsc::channel::<Job>(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let task = tokio::task::spawn_blocking(move || {
            let Ok(_permit) = WORKERS.try_acquire() else {
                return;
            };
            let mut clipboard = match Clipboard::new(display) {
                Ok(c) => c,
                Err(e) => {
                    if let Some(job) = rx.blocking_recv() {
                        let _ = job.reply.send(Err(e));
                    }
                    return;
                }
            };
            while !stop.load(Ordering::Acquire) {
                match rx.try_recv() {
                    Ok(job) => {
                        if !job.reply.is_closed() {
                            let result = clipboard.publish(job.text);
                            let _ = job.reply.send(result);
                        }
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                    Err(mpsc::error::TryRecvError::Empty) => {}
                }
                if let Err(error) = clipboard.pump() {
                    tracing::warn!(%error,"clipboard selection worker ended");
                    break;
                }
                let mut fds = [rustix::event::PollFd::new(
                    clipboard.conn.stream(),
                    rustix::event::PollFlags::IN,
                )];
                let timeout = rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 10_000_000,
                };
                if rustix::event::poll(&mut fds, Some(&timeout)).is_err() {
                    break;
                }
            }
        });
        Self {
            jobs,
            stopped,
            task,
        }
    }
    pub(crate) async fn publish(&mut self, text: String) -> Result<(), DesktopError> {
        let (reply, rx) = oneshot::channel();
        self.jobs
            .send(Job { text, reply })
            .await
            .map_err(|_| error("clipboard worker unavailable"))?;
        rx.await
            .map_err(|_| error("clipboard worker unavailable"))?
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.task.abort();
    }
}
fn error(e: impl std::fmt::Display) -> DesktopError {
    DesktopError::Input(format!("X11 clipboard: {e}"))
}
struct Transfer {
    window: Window,
    property: Atom,
    kind: Atom,
    bytes: Arc<[u8]>,
    offset: usize,
    started: Instant,
}
struct Clipboard {
    conn: RustConnection,
    window: Window,
    clipboard: Atom,
    utf8: Atom,
    targets: Atom,
    timestamp: Atom,
    incr: Atom,
    marker: Atom,
    time: u32,
    text: Arc<[u8]>,
    transfers: Vec<Transfer>,
    chunk: usize,
}
impl Clipboard {
    fn new(display: u32) -> Result<Self, DesktopError> {
        let (conn, _) = RustConnection::connect(None).map_err(error)?;
        let root = conn
            .setup()
            .roots
            .get(display as usize)
            .ok_or_else(|| error("display unavailable"))?
            .root;
        let window = conn.generate_id().map_err(error)?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(error)?
        .check()
        .map_err(error)?;
        let atom = |name: &[u8]| {
            conn.intern_atom(false, name)
                .map_err(error)?
                .reply()
                .map(|r| r.atom)
                .map_err(error)
        };
        let clipboard = atom(b"CLIPBOARD")?;
        let utf8 = atom(b"UTF8_STRING")?;
        let targets = atom(b"TARGETS")?;
        let timestamp = atom(b"TIMESTAMP")?;
        let incr = atom(b"INCR")?;
        let marker = atom(b"RDS_CLIPBOARD_TIME")?;
        let chunk = conn
            .maximum_request_bytes()
            .saturating_sub(128)
            .min(32 * 1024);
        Ok(Self {
            conn,
            window,
            clipboard,
            utf8,
            targets,
            timestamp,
            incr,
            marker,
            time: 0,
            text: Arc::from([]),
            transfers: Vec::new(),
            chunk,
        })
    }
    fn publish(&mut self, text: String) -> Result<(), DesktopError> {
        if text.len() > super::MAX_TEXT_BYTES {
            return Err(error("text exceeds 1 MiB"));
        }
        self.conn
            .change_property8(
                PropMode::REPLACE,
                self.window,
                self.marker,
                AtomEnum::INTEGER,
                &[0],
            )
            .map_err(error)?
            .check()
            .map_err(error)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if Instant::now() >= deadline {
                return Err(error("server timestamp unavailable"));
            }
            match self.conn.poll_for_event().map_err(error)? {
                Some(Event::PropertyNotify(e))
                    if e.window == self.window && e.atom == self.marker =>
                {
                    self.time = e.time;
                    break;
                }
                Some(event) => self.event(event)?,
                None => {
                    let mut fds = [rustix::event::PollFd::new(
                        self.conn.stream(),
                        rustix::event::PollFlags::IN,
                    )];
                    let timeout = rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: 10_000_000,
                    };
                    rustix::event::poll(&mut fds, Some(&timeout)).map_err(error)?;
                }
            }
        }
        self.conn
            .set_selection_owner(self.window, self.clipboard, self.time)
            .map_err(error)?
            .check()
            .map_err(error)?;
        if self
            .conn
            .get_selection_owner(self.clipboard)
            .map_err(error)?
            .reply()
            .map_err(error)?
            .owner
            != self.window
        {
            return Err(error("selection ownership refused"));
        }
        self.text = Arc::from(text.into_bytes());
        tracing::info!(bytes = self.text.len(), "remote text clipboard published");
        Ok(())
    }
    fn pump(&mut self) -> Result<(), DesktopError> {
        self.transfers
            .retain(|t| t.started.elapsed() < Duration::from_secs(5));
        for _ in 0..64 {
            match self.conn.poll_for_event().map_err(error)? {
                Some(e) => self.event(e)?,
                None => break,
            }
        }
        self.conn.flush().map_err(error)
    }
    fn event(&mut self, event: Event) -> Result<(), DesktopError> {
        match event {
            Event::SelectionRequest(e) => self.request(e),
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                let Some(index) = self
                    .transfers
                    .iter()
                    .position(|t| t.window == e.window && t.property == e.atom)
                else {
                    return Ok(());
                };
                let t = &mut self.transfers[index];
                let end = (t.offset + self.chunk).min(t.bytes.len());
                let result = self
                    .conn
                    .change_property8(
                        PropMode::REPLACE,
                        t.window,
                        t.property,
                        t.kind,
                        &t.bytes[t.offset..end],
                    )
                    .map_err(error)?
                    .check();
                let done = end == t.offset;
                t.offset = end;
                if result.is_err() || done {
                    self.transfers.swap_remove(index);
                }
                Ok(())
            }
            Event::SelectionClear(e) if e.selection == self.clipboard => {
                self.text = Arc::from([]);
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn request(&mut self, e: SelectionRequestEvent) -> Result<(), DesktopError> {
        let property = if e.property == 0 {
            e.target
        } else {
            e.property
        };
        let mut accepted = false;
        if e.owner == self.window
            && e.selection == self.clipboard
            && (e.time == 0 || e.time.wrapping_sub(self.time) < u32::MAX / 2)
        {
            if e.target == self.targets {
                accepted = self
                    .conn
                    .change_property32(
                        PropMode::REPLACE,
                        e.requestor,
                        property,
                        AtomEnum::ATOM,
                        &[
                            self.targets,
                            self.timestamp,
                            self.utf8,
                            AtomEnum::STRING.into(),
                        ],
                    )
                    .map_err(error)?
                    .check()
                    .is_ok();
            } else if e.target == self.timestamp {
                accepted = self
                    .conn
                    .change_property32(
                        PropMode::REPLACE,
                        e.requestor,
                        property,
                        AtomEnum::INTEGER,
                        &[self.time],
                    )
                    .map_err(error)?
                    .check()
                    .is_ok();
            } else if e.target == self.utf8
                || (e.target == u32::from(AtomEnum::STRING) && self.text.is_ascii())
            {
                if self.text.len() <= self.chunk {
                    accepted = self
                        .conn
                        .change_property8(
                            PropMode::REPLACE,
                            e.requestor,
                            property,
                            e.target,
                            &self.text,
                        )
                        .map_err(error)?
                        .check()
                        .is_ok();
                } else if self.transfers.len() < 8
                    && !self
                        .transfers
                        .iter()
                        .any(|t| t.window == e.requestor && t.property == property)
                {
                    if self
                        .conn
                        .change_window_attributes(
                            e.requestor,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        )
                        .map_err(error)?
                        .check()
                        .is_ok()
                    {
                        accepted = self
                            .conn
                            .change_property32(
                                PropMode::REPLACE,
                                e.requestor,
                                property,
                                self.incr,
                                &[self.text.len() as u32],
                            )
                            .map_err(error)?
                            .check()
                            .is_ok();
                        if accepted {
                            self.transfers.push(Transfer {
                                window: e.requestor,
                                property,
                                kind: e.target,
                                bytes: self.text.clone(),
                                offset: 0,
                                started: Instant::now(),
                            });
                        }
                    }
                }
            }
        }
        let reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if accepted { property } else { 0 },
        };
        // A requestor may disappear; that is one failed transfer, not loss of
        // the session's selection owner. All retained transfers are bounded.
        let _ = self
            .conn
            .send_event(false, e.requestor, EventMask::NO_EVENT, reply)
            .map_err(error)?
            .check();
        self.conn.flush().map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn receive(
        conn: &RustConnection,
        window: Window,
        selection: Atom,
        utf8: Atom,
        property: Atom,
    ) -> Vec<u8> {
        conn.convert_selection(window, selection, utf8, property, 0u32)
            .unwrap()
            .check()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "selection owner failed to reply");
            if let Some(Event::SelectionNotify(event)) = conn.poll_for_event().unwrap() {
                assert_ne!(event.property, 0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let incr = conn
            .intern_atom(false, b"INCR")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let first = conn
            .get_property(
                false,
                window,
                property,
                AtomEnum::ANY,
                0,
                super::super::MAX_TEXT_BYTES as u32,
            )
            .unwrap()
            .reply()
            .unwrap();
        if first.type_ != incr {
            return first.value;
        }
        while conn.poll_for_event().unwrap().is_some() {}
        conn.delete_property(window, property)
            .unwrap()
            .check()
            .unwrap();
        let mut result = Vec::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "INCR transfer failed to terminate"
            );
            if let Some(Event::PropertyNotify(event)) = conn.poll_for_event().unwrap()
                && event.window == window
                && event.atom == property
                && event.state == Property::NEW_VALUE
            {
                let part = conn
                    .get_property(
                        true,
                        window,
                        property,
                        utf8,
                        0,
                        super::super::MAX_TEXT_BYTES as u32,
                    )
                    .unwrap()
                    .reply()
                    .unwrap();
                if part.value.is_empty() {
                    break;
                }
                result.extend_from_slice(&part.value);
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        result
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a dedicated Xvfb server; owns its clipboard"]
    async fn native_clipboard_serves_utf8_and_large_incr_without_a_helper() {
        let (conn, _) = RustConnection::connect(None).expect("dedicated X11 server required");
        let window = conn.generate_id().unwrap();
        conn.create_window(
            0,
            window,
            conn.setup().roots[0].root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .unwrap()
        .check()
        .unwrap();
        let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        let (selection, utf8, property) = (
            atom(b"CLIPBOARD"),
            atom(b"UTF8_STRING"),
            atom(b"RDS_TEST_CLIP"),
        );
        let mut owner = Worker::new(0);
        for text in ["Привет 🖥️\nHello".to_owned(), "Привет 🖥️\n".repeat(8000)]
        {
            owner.publish(text.clone()).await.unwrap();
            assert_eq!(
                receive(&conn, window, selection, utf8, property).await,
                text.as_bytes()
            );
        }
        drop(owner);
    }
}
