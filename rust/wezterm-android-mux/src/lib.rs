//! Android-owned runtime around WezTerm's remote mux client.
//!
//! The runtime mirrors a remote `wezterm-mux-server`; it never starts a local
//! PTY or local mux daemon. All access to upstream's process-global `Mux` and
//! main-thread scheduler is serialized on one dedicated thread.

use anyhow::{anyhow, bail};
use codec::Resize as MuxResize;
use config::{SshBackend, SshDomain, SshMultiplexing};
use mux::domain::{Domain, DomainState};
use mux::pane::{Pane, PaneId};
use mux::renderable::RenderableDimensions;
use mux::Mux;
use promise::spawn::Runnable;
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use termwiz::surface::SEQ_ZERO;
use wezterm_android_core::{CellSnapshot, CellStyleSnapshot, TerminalSnapshot};
use wezterm_android_ssh::{AndroidSshConfig, SshEndpoint};
use wezterm_client::domain::{ClientDomain, ClientDomainConfig};
use wezterm_client::pane::ClientPane;
use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::{
    CellAttributes, Intensity, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, TerminalSize,
};

pub const WEZTERM_REVISION: &str = "d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
static RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AndroidViewport {
    top: isize,
    live_top: isize,
    rows: usize,
    max_offset: usize,
}

fn android_viewport(
    dimensions: &RenderableDimensions,
    android_rows: usize,
    pinned_top: Option<isize>,
) -> AndroidViewport {
    let rows = android_rows.max(1);
    // Another client can resize the same remote mux window after Android has
    // sent its preferred size.  In that case render the bottom-most rows that
    // physically fit on Android, rather than clipping the remote viewport's
    // bottom (and therefore its prompt) below the Surface.
    let hidden_remote_rows = dimensions.viewport_rows.saturating_sub(rows);
    let live_top = dimensions
        .physical_top
        .saturating_add(isize::try_from(hidden_remote_rows).unwrap_or(isize::MAX));
    let max_offset =
        usize::try_from(live_top.saturating_sub(dimensions.scrollback_top)).unwrap_or(0);
    let oldest_top = live_top.saturating_sub(isize::try_from(max_offset).unwrap_or(isize::MAX));
    // While the user browses history the viewport is pinned to an absolute
    // physical row: newly arriving output appends below without shifting the
    // visible content. `None` keeps the live bottom alignment.
    let top = match pinned_top {
        Some(top) => top.clamp(oldest_top, live_top),
        None => live_top,
    };
    AndroidViewport {
        top,
        live_top,
        rows,
        max_offset,
    }
}

/// Pinned wire version, checked before attaching any pane.
pub const fn codec_version() -> usize {
    codec::CODEC_VERSION
}

#[derive(Debug, Clone)]
pub struct MuxEndpoint {
    ssh: AndroidSshConfig,
    remote_wezterm_path: String,
}

impl MuxEndpoint {
    pub fn new(
        host: impl Into<String>,
        user: impl Into<String>,
        port: u16,
        app_files_dir: impl Into<PathBuf>,
        identity_file: Option<PathBuf>,
        remote_wezterm_path: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let endpoint = SshEndpoint::new(host, user, port)?;
        let mut ssh = AndroidSshConfig::new(endpoint, app_files_dir)?;
        if let Some(identity) = identity_file {
            ssh = ssh.with_identity_file(identity)?;
        }
        let remote_wezterm_path = remote_wezterm_path.into();
        if remote_wezterm_path.is_empty()
            || !remote_wezterm_path
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '+'))
        {
            bail!("remote WezTerm path contains unsupported shell characters");
        }
        Ok(Self {
            ssh,
            remote_wezterm_path,
        })
    }

    fn to_domain(&self) -> anyhow::Result<SshDomain> {
        let ssh_option = self.ssh.to_config_map()?.into_iter().collect();
        Ok(SshDomain {
            name: "android-sshmux".to_string(),
            // Keep port in ssh_option. This avoids upstream's host:port
            // splitter misinterpreting a raw IPv6 address.
            remote_address: self.ssh.endpoint().host().to_string(),
            username: Some(self.ssh.endpoint().user().to_string()),
            no_agent_auth: true,
            remote_wezterm_path: Some(self.remote_wezterm_path.clone()),
            ssh_backend: Some(SshBackend::LibSsh),
            multiplexing: SshMultiplexing::WezTerm,
            ssh_option,
            ..SshDomain::default()
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MuxTabInfo {
    pub tab_id: usize,
    pub remote_tab_id: usize,
    pub pane_id: usize,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxEvent {
    Connecting,
    Attached {
        tab_count: usize,
        codec_version: usize,
    },
    TabsChanged {
        tabs: Vec<MuxTabInfo>,
    },
    Detached,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct MuxViewSnapshot {
    pub terminal: TerminalSnapshot,
    pub tabs: Vec<MuxTabInfo>,
}

enum RuntimeMessage {
    Run(Runnable),
    AttachFinished(Result<(), String>),
    SpawnFinished(Result<PaneId, String>),
    Command(RuntimeCommand),
    Stop,
}

enum RuntimeCommand {
    Snapshot(
        Option<isize>,
        Sender<Result<Option<MuxViewSnapshot>, String>>,
    ),
    Write(Vec<u8>, Sender<Result<(), String>>),
    MouseWheel {
        column: usize,
        row: usize,
        delta: isize,
        reply: Sender<Result<(), String>>,
    },
    Resize(TerminalSize, Sender<Result<(), String>>),
    Tabs(Sender<Result<Vec<MuxTabInfo>, String>>),
    ActivateRelative(isize, Sender<Result<(), String>>),
    Activate(usize, Sender<Result<(), String>>),
    SpawnTab(Sender<Result<(), String>>),
    CloseActiveTab(Sender<Result<(), String>>),
    Detach(Sender<Result<(), String>>),
}

struct RuntimeState {
    mux: Arc<Mux>,
    domain: Arc<ClientDomain>,
    active_pane_id: Option<PaneId>,
    preferred_remote_tab_id: Option<usize>,
    last_emitted_tabs: Option<Vec<MuxTabInfo>>,
    size: TerminalSize,
    event_tx: Sender<MuxEvent>,
    runtime_tx: Sender<RuntimeMessage>,
    pending_spawn_reply: Option<Sender<Result<(), String>>>,
}

/// JNI-facing handle. Dropping it synchronously tears down only the local
/// client runtime; the remote panes survive because the domain detaches first.
pub struct AndroidMuxSession {
    runtime_tx: Sender<RuntimeMessage>,
    event_tx: Sender<MuxEvent>,
    events: Receiver<MuxEvent>,
    stopped: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl AndroidMuxSession {
    pub fn start(
        endpoint: MuxEndpoint,
        size: TerminalSize,
        preferred_remote_tab_id: Option<usize>,
    ) -> anyhow::Result<Self> {
        if RUNTIME_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            bail!("an Android SSHMUX runtime is already active");
        }

        let (runtime_tx, runtime_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let thread_tx = runtime_tx.clone();
        let session_event_tx = event_tx.clone();
        let join = std::thread::Builder::new()
            .name("wezterm-android-mux".into())
            .spawn(move || {
                run_runtime(
                    endpoint,
                    size,
                    preferred_remote_tab_id,
                    thread_tx,
                    runtime_rx,
                    event_tx,
                    stopped_tx,
                )
            })
            .map_err(|error| {
                RUNTIME_ACTIVE.store(false, Ordering::SeqCst);
                error
            })?;

        Ok(Self {
            runtime_tx,
            event_tx: session_event_tx,
            events: event_rx,
            stopped: stopped_rx,
            join: Some(join),
        })
    }

    pub fn try_next_event(&self) -> Option<MuxEvent> {
        self.events.try_recv().ok()
    }

    /// Reports a transport failure discovered by the JNI-facing snapshot
    /// path. Upstream can move a ClientDomain out of Attached without emitting
    /// an Android wrapper event, so the UI must not infer health solely from
    /// the continued existence of this session handle.
    pub fn report_transport_failure(&self, message: String) {
        let _ = self.event_tx.send(MuxEvent::Error { message });
    }

    pub fn snapshot(&self) -> anyhow::Result<Option<MuxViewSnapshot>> {
        self.snapshot_with_viewport_top(None)
    }

    pub fn snapshot_with_viewport_top(
        &self,
        pinned_top: Option<isize>,
    ) -> anyhow::Result<Option<MuxViewSnapshot>> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Snapshot(
                pinned_top, tx,
            )))?;
        recv_result(rx, "snapshot")
    }

    pub fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Write(
                bytes.to_vec(),
                tx,
            )))?;
        recv_result(rx, "write")
    }

    /// Sends a terminal mouse-wheel event at the touched cell. Positive
    /// deltas scroll down and negative deltas scroll up. The remote WezTerm
    /// terminal decides whether to encode mouse reporting, use alternate-
    /// screen cursor keys, or ignore the event on a normal shell screen.
    pub fn mouse_wheel(&self, column: usize, row: usize, delta: isize) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::MouseWheel {
                column,
                row,
                delta,
                reply: tx,
            }))?;
        recv_result(rx, "mouse wheel")
    }

    pub fn resize(&self, size: TerminalSize) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Resize(size, tx)))?;
        recv_result(rx, "resize")
    }

    pub fn tabs(&self) -> anyhow::Result<Vec<MuxTabInfo>> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Tabs(tx)))?;
        recv_result(rx, "list tabs")
    }

    pub fn activate_relative(&self, delta: isize) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::ActivateRelative(
                delta, tx,
            )))?;
        recv_result(rx, "activate tab")
    }

    pub fn activate(&self, remote_tab_id: usize) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Activate(
                remote_tab_id,
                tx,
            )))?;
        recv_result(rx, "activate saved tab")
    }

    pub fn spawn_tab(&self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::SpawnTab(tx)))?;
        recv_result(rx, "spawn tab")
    }

    /// Explicitly kills every pane in the selected remote tab. The Android UI
    /// must obtain user confirmation before calling this destructive action.
    pub fn close_active_tab(&self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::CloseActiveTab(tx)))?;
        recv_result(rx, "close tab")
    }

    /// Safely detaches without sending KillPane RPCs for mirrored panes.
    pub fn detach(&self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.runtime_tx
            .send(RuntimeMessage::Command(RuntimeCommand::Detach(tx)))?;
        recv_result(rx, "detach")
    }
}

impl Drop for AndroidMuxSession {
    fn drop(&mut self) {
        let _ = self.runtime_tx.send(RuntimeMessage::Stop);
        // Do not join after the acknowledgement. The old runtime deliberately
        // keeps its scheduler receiver alive until a subsequent runtime swaps
        // in fresh global promise schedulers; joining here would deadlock that
        // hand-off. Dropping JoinHandle safely detaches the short-lived drain
        // thread.
        let _ = self.stopped.recv_timeout(COMMAND_TIMEOUT);
        self.join.take();
    }
}

fn recv_result<T>(rx: Receiver<Result<T, String>>, operation: &str) -> anyhow::Result<T> {
    match rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(anyhow!(error)),
        Err(RecvTimeoutError::Timeout) => bail!("SSHMUX {operation} timed out"),
        Err(RecvTimeoutError::Disconnected) => bail!("SSHMUX runtime stopped during {operation}"),
    }
}

fn run_runtime(
    endpoint: MuxEndpoint,
    size: TerminalSize,
    preferred_remote_tab_id: Option<usize>,
    runtime_tx: Sender<RuntimeMessage>,
    runtime_rx: Receiver<RuntimeMessage>,
    event_tx: Sender<MuxEvent>,
    stopped_tx: Sender<()>,
) {
    let _ = event_tx.send(MuxEvent::Connecting);
    let mut cleaned_up = false;
    let outcome = catch_unwind(AssertUnwindSafe(|| -> anyhow::Result<()> {
        log::info!("SSHMUX runtime: configuring upstream main thread");
        config::designate_this_as_the_main_thread();

        log::info!("SSHMUX runtime: installing promise schedulers");
        let high_tx = runtime_tx.clone();
        let low_tx = runtime_tx.clone();
        promise::spawn::set_schedulers(
            Box::new(move |runnable| {
                let _ = high_tx.send(RuntimeMessage::Run(runnable));
            }),
            Box::new(move |runnable| {
                let _ = low_tx.send(RuntimeMessage::Run(runnable));
            }),
        );

        log::info!("SSHMUX runtime: creating process-global mux");
        let mux = Arc::new(Mux::new(None));
        Mux::set_mux(&mux);
        log::info!("SSHMUX runtime: creating SSH client domain");
        let domain = Arc::new(ClientDomain::new(ClientDomainConfig::Ssh(
            endpoint.to_domain()?,
        )));
        let domain_trait: Arc<dyn Domain> = domain.clone();
        mux.add_domain(&domain_trait);

        let mut state = RuntimeState {
            mux,
            domain,
            active_pane_id: None,
            preferred_remote_tab_id,
            last_emitted_tabs: None,
            size,
            event_tx: event_tx.clone(),
            runtime_tx: runtime_tx.clone(),
            pending_spawn_reply: None,
        };

        log::info!("SSHMUX runtime: scheduling remote attachment");
        schedule_attach(&state);

        let mut requested_stop = false;
        while let Ok(message) = runtime_rx.recv() {
            match message {
                RuntimeMessage::Run(runnable) => {
                    runnable.run();
                }
                RuntimeMessage::AttachFinished(result) => state.finish_attach(result),
                RuntimeMessage::SpawnFinished(result) => state.finish_spawn(result),
                RuntimeMessage::Command(command) => state.handle_command(command),
                RuntimeMessage::Stop => {
                    if state.domain.state() == DomainState::Attached {
                        let _ = state.domain.detach();
                    }
                    requested_stop = true;
                    break;
                }
            }
        }

        if requested_stop {
            // `promise::spawn` calls its scheduler while holding a process-wide
            // mutex. If the old receiver disappears first, a late Runnable can
            // fail to send and recursively wake while that mutex is held. The
            // mutex then remains deadlocked and a same-process reattach blocks
            // forever in set_schedulers. Clear the old mux, acknowledge Stop,
            // and retain only this receiver until the next set_schedulers call
            // drops both old Sender closures.
            Mux::shutdown();
            RUNTIME_ACTIVE.store(false, Ordering::SeqCst);
            cleaned_up = true;
            drop(state);
            drop(runtime_tx);
            let _ = stopped_tx.send(());
            while let Ok(message) = runtime_rx.recv() {
                // Dropping pending work outside the scheduler mutex safely
                // cancels it. Other command variants also release their reply
                // channels so stale callers cannot hang.
                drop(message);
            }
        }
        Ok(())
    }));

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::error!("SSHMUX runtime failed: {error:#}");
            let _ = event_tx.send(MuxEvent::Error {
                message: format!("{error:#}"),
            });
        }
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            let message = format!("SSHMUX runtime panicked: {detail}");
            log::error!("{message}");
            let _ = event_tx.send(MuxEvent::Error { message });
        }
    }
    if !cleaned_up {
        Mux::shutdown();
        RUNTIME_ACTIVE.store(false, Ordering::SeqCst);
        let _ = stopped_tx.send(());
    }
}

fn schedule_attach(state: &RuntimeState) {
    let domain: Arc<dyn Domain> = state.domain.clone();
    let runtime_tx = state.runtime_tx.clone();
    promise::spawn::spawn(async move {
        let result = domain
            .attach(None)
            .await
            .map_err(|error| format!("{error:#}"));
        let _ = runtime_tx.send(RuntimeMessage::AttachFinished(result));
    })
    .detach();
}

impl RuntimeState {
    fn finish_attach(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if let Some(remote_tab_id) = self.preferred_remote_tab_id.take() {
                    if let Some(target) = self
                        .tab_handles()
                        .into_iter()
                        .find(|tab| tab.info.remote_tab_id == remote_tab_id)
                    {
                        self.active_pane_id = Some(target.info.pane_id);
                    }
                }
                self.ensure_active_pane();
                if let Some(pane) = self.active_pane() {
                    let _ = pane.resize(self.size);
                    pane.advise_focus();
                }
                let tabs: Vec<_> = self.tab_handles().into_iter().map(|tab| tab.info).collect();
                let _ = self.event_tx.send(MuxEvent::Attached {
                    tab_count: tabs.len(),
                    codec_version: codec_version(),
                });
                self.emit_tabs_changed_if_needed(&tabs);
            }
            Err(message) => {
                let _ = self.event_tx.send(MuxEvent::Error { message });
            }
        }
    }

    fn finish_spawn(&mut self, result: Result<PaneId, String>) {
        let response = match result {
            Ok(pane_id) => {
                self.active_pane_id = Some(pane_id);
                if let Err(error) = self.focus_active_pane() {
                    Err(format!("{error:#}"))
                } else {
                    self.emit_tabs_changed();
                    Ok(())
                }
            }
            Err(error) => Err(error),
        };
        if let Some(reply) = self.pending_spawn_reply.take() {
            let _ = reply.send(response);
        }
    }

    fn handle_command(&mut self, command: RuntimeCommand) {
        match command {
            RuntimeCommand::Snapshot(pinned_top, reply) => {
                let result = self.snapshot(pinned_top).map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::Write(bytes, reply) => {
                let result = self.write(&bytes).map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::MouseWheel {
                column,
                row,
                delta,
                reply,
            } => {
                let result = self.mouse_wheel(column, row, delta).map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::Resize(size, reply) => {
                self.size = size;
                let result = self.resize_active().map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::Tabs(reply) => {
                self.ensure_active_pane();
                let tabs = self.tab_handles().into_iter().map(|tab| tab.info).collect();
                let _ = reply.send(Ok(tabs));
            }
            RuntimeCommand::ActivateRelative(delta, reply) => {
                let result = self.activate_relative(delta).map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::Activate(remote_tab_id, reply) => {
                let result = self.activate(remote_tab_id).map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::SpawnTab(reply) => self.spawn_tab(reply),
            RuntimeCommand::CloseActiveTab(reply) => {
                let result = self.close_active_tab().map_err(display_error);
                let _ = reply.send(result);
            }
            RuntimeCommand::Detach(reply) => {
                let result = self.domain.detach().map_err(display_error);
                if result.is_ok() {
                    self.active_pane_id = None;
                    let _ = self.event_tx.send(MuxEvent::Detached);
                }
                let _ = reply.send(result);
            }
        }
    }

    fn ensure_attached(&self) -> anyhow::Result<()> {
        if self.domain.state() != DomainState::Attached {
            bail!("SSHMUX domain is not attached");
        }
        Ok(())
    }

    fn ensure_active_pane(&mut self) {
        let valid = self
            .active_pane_id
            .and_then(|id| self.mux.get_pane(id))
            .is_some_and(|pane| pane.domain_id() == self.domain.domain_id());
        if !valid {
            self.active_pane_id = self.tab_handles().first().map(|tab| tab.info.pane_id);
        }
    }

    fn active_pane(&self) -> Option<Arc<dyn Pane>> {
        self.active_pane_id
            .and_then(|pane_id| self.mux.get_pane(pane_id))
            .filter(|pane| pane.domain_id() == self.domain.domain_id())
    }

    fn focus_active_pane(&self) -> anyhow::Result<()> {
        let pane = self
            .active_pane()
            .ok_or_else(|| anyhow!("no active remote pane"))?;
        self.mux.focus_pane_and_containing_tab(pane.pane_id())?;
        pane.resize(self.size)?;
        pane.advise_focus();
        Ok(())
    }

    fn resize_active(&mut self) -> anyhow::Result<()> {
        self.ensure_attached()?;
        self.ensure_active_pane();
        let Some(pane_id) = self.active_pane_id else {
            return Ok(());
        };
        let (_, _, tab_id) = self
            .mux
            .resolve_pane_id(pane_id)
            .ok_or_else(|| anyhow!("active pane no longer belongs to a tab"))?;
        self.mux
            .get_tab(tab_id)
            .ok_or_else(|| anyhow!("active tab disappeared"))?
            .resize(self.size);

        // ClientPane::resize deliberately sends its RPC from a detached
        // promise.  During Android attachment the initial full-height resize
        // and the keyboard-induced resize can therefore overlap.  If the
        // earlier RPC completes last, the server is left taller than the
        // visible Surface even though the local Tab already caches the right
        // size, so repeating Tab::resize becomes a no-op.  Queue an explicit
        // RPC after the normal local-tree update to make the latest Android
        // Surface size authoritative.
        let pane = self
            .active_pane()
            .ok_or_else(|| anyhow!("active pane disappeared during resize"))?;
        let client_pane = pane
            .downcast_ref::<ClientPane>()
            .ok_or_else(|| anyhow!("active SSHMUX pane is not a ClientPane"))?;
        let request = MuxResize {
            containing_tab_id: client_pane.remote_tab_id,
            pane_id: client_pane.remote_pane_id,
            size: self.size,
        };
        let client = ClientDomain::get_client_inner_for_domain(self.domain.domain_id())?;
        promise::spawn::spawn(async move {
            if let Err(error) = client.client.resize(request).await {
                log::warn!("explicit Android SSHMUX resize failed: {error:#}");
            }
        })
        .detach();
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.ensure_attached()?;
        self.ensure_active_pane();
        let pane = self
            .active_pane()
            .ok_or_else(|| anyhow!("no active remote pane"))?;
        let mut writer = pane.writer();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    fn mouse_wheel(&mut self, column: usize, row: usize, delta: isize) -> anyhow::Result<()> {
        self.ensure_attached()?;
        if delta == 0 {
            return Ok(());
        }
        self.ensure_active_pane();
        let pane = self
            .active_pane()
            .ok_or_else(|| anyhow!("no active remote pane"))?;
        pane.mouse_event(mouse_wheel_event(column, row, delta))
    }

    fn activate_relative(&mut self, delta: isize) -> anyhow::Result<()> {
        self.ensure_attached()?;
        self.ensure_active_pane();
        let tabs = self.tab_handles();
        if tabs.is_empty() {
            bail!("remote mux has no tabs");
        }
        let current = tabs
            .iter()
            .position(|tab| Some(tab.info.pane_id) == self.active_pane_id)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(tabs.len() as isize) as usize;
        self.active_pane_id = Some(tabs[next].info.pane_id);
        self.focus_active_pane()?;
        self.emit_tabs_changed();
        Ok(())
    }

    fn activate(&mut self, remote_tab_id: usize) -> anyhow::Result<()> {
        self.ensure_attached()?;
        let target = self
            .tab_handles()
            .into_iter()
            .find(|tab| tab.info.remote_tab_id == remote_tab_id)
            .ok_or_else(|| anyhow!("saved remote tab {remote_tab_id} is no longer available"))?;
        self.active_pane_id = Some(target.info.pane_id);
        self.focus_active_pane()?;
        self.emit_tabs_changed();
        Ok(())
    }

    fn spawn_tab(&mut self, reply: Sender<Result<(), String>>) {
        if let Err(error) = self.ensure_attached() {
            let _ = reply.send(Err(display_error(error)));
            return;
        }
        if self.pending_spawn_reply.is_some() {
            let _ = reply.send(Err("a remote tab spawn is already in progress".into()));
            return;
        }

        let window_id = self
            .active_pane_id
            .and_then(|pane_id| self.mux.resolve_pane_id(pane_id))
            .map(|(_, window_id, _)| window_id)
            .unwrap_or_else(|| {
                let window = self.mux.new_empty_window(Some("default".into()), None);
                *window
            });
        let domain: Arc<dyn Domain> = self.domain.clone();
        let size = self.size;
        let runtime_tx = self.runtime_tx.clone();
        self.pending_spawn_reply = Some(reply);
        promise::spawn::spawn(async move {
            let result = domain
                .spawn(size, None, None, window_id)
                .await
                .and_then(|tab| {
                    tab.get_active_pane()
                        .map(|pane| pane.pane_id())
                        .ok_or_else(|| anyhow!("new remote tab has no pane"))
                })
                .map_err(|error| format!("{error:#}"));
            let _ = runtime_tx.send(RuntimeMessage::SpawnFinished(result));
        })
        .detach();
    }

    fn close_active_tab(&mut self) -> anyhow::Result<()> {
        self.ensure_attached()?;
        self.ensure_active_pane();
        let pane_id = self
            .active_pane_id
            .ok_or_else(|| anyhow!("no active remote pane"))?;
        let (_, _, tab_id) = self
            .mux
            .resolve_pane_id(pane_id)
            .ok_or_else(|| anyhow!("active pane no longer belongs to a tab"))?;
        self.mux
            .remove_tab(tab_id)
            .ok_or_else(|| anyhow!("active tab already disappeared"))?;
        self.active_pane_id = None;
        self.ensure_active_pane();
        if self.active_pane_id.is_some() {
            self.focus_active_pane()?;
        }
        self.emit_tabs_changed();
        Ok(())
    }

    fn snapshot(&mut self, pinned_top: Option<isize>) -> anyhow::Result<Option<MuxViewSnapshot>> {
        self.ensure_attached()?;
        self.ensure_active_pane();
        let Some(pane) = self.active_pane() else {
            return Ok(None);
        };
        let dimensions = pane.get_dimensions();
        let viewport = android_viewport(&dimensions, self.size.rows, pinned_top);
        let range = viewport.top..viewport.top + viewport.rows as isize;
        // This call drives ClientRenderable's adaptive remote poll.
        let _ = pane.get_changed_since(range.clone(), SEQ_ZERO);
        let (first_row, lines) = pane.get_lines(range);
        let viewport_offset = usize::try_from(viewport.live_top.saturating_sub(viewport.top))
            .unwrap_or(0)
            .min(viewport.max_offset);
        let cursor = pane.get_cursor_position();
        let palette = pane.palette();
        let default_style = snapshot_attributes(&CellAttributes::blank(), &palette);
        let columns = self.size.cols.max(1);
        let mut cells = Vec::new();
        let mut wrapped_rows: Vec<bool> = lines
            .iter()
            .map(|line| line.last_cell_was_wrapped())
            .collect();
        wrapped_rows.resize(viewport.rows, false);
        for (row, line) in lines.iter().enumerate() {
            for cell in line.visible_cells() {
                if cell.cell_index() >= columns {
                    continue;
                }
                let text = cell.str();
                let style = snapshot_attributes(cell.attrs(), &palette);
                if text.trim().is_empty() && style == default_style {
                    continue;
                }
                cells.push(CellSnapshot {
                    row,
                    column: cell.cell_index(),
                    text: text.to_owned(),
                    width: cell.width(),
                    style,
                });
            }
        }
        let cursor_row = if cursor.y >= first_row && cursor.y < first_row + viewport.rows as isize {
            usize::try_from(cursor.y - first_row).unwrap_or(viewport.rows)
        } else {
            viewport.rows
        };
        // ClientRenderable updates its pane title as part of the adaptive
        // remote poll driven above. Compare the accompanying tab metadata on
        // every rendered snapshot so OSC title changes reach Android without
        // flooding the UI with unchanged 100 ms poll results.
        let tabs: Vec<_> = self.tab_handles().into_iter().map(|tab| tab.info).collect();
        self.emit_tabs_changed_if_needed(&tabs);
        Ok(Some(MuxViewSnapshot {
            terminal: TerminalSnapshot {
                columns,
                rows: viewport.rows,
                cursor_column: cursor.x.min(columns.saturating_sub(1)),
                cursor_row,
                cells,
                viewport_offset,
                max_viewport_offset: viewport.max_offset,
                viewport_top: viewport.top,
                wrapped_rows,
            },
            tabs,
        }))
    }

    fn emit_tabs_changed(&mut self) {
        self.ensure_active_pane();
        let tabs: Vec<_> = self.tab_handles().into_iter().map(|tab| tab.info).collect();
        self.emit_tabs_changed_if_needed(&tabs);
    }

    fn emit_tabs_changed_if_needed(&mut self, tabs: &[MuxTabInfo]) {
        if !tabs_differ(self.last_emitted_tabs.as_deref(), tabs) {
            return;
        }
        self.last_emitted_tabs = Some(tabs.to_vec());
        let _ = self.event_tx.send(MuxEvent::TabsChanged {
            tabs: tabs.to_vec(),
        });
    }

    fn tab_handles(&self) -> Vec<TabHandle> {
        let mut window_ids = self.mux.iter_windows();
        window_ids.sort_unstable();
        let mut result = Vec::new();
        for window_id in window_ids {
            let Some(window) = self.mux.get_window(window_id) else {
                continue;
            };
            for tab in window.iter_tabs() {
                let pane = tab
                    .get_active_pane()
                    .filter(|pane| pane.domain_id() == self.domain.domain_id())
                    .or_else(|| {
                        tab.iter_panes_ignoring_zoom()
                            .into_iter()
                            .map(|positioned| positioned.pane)
                            .find(|pane| pane.domain_id() == self.domain.domain_id())
                    });
                let Some(pane) = pane else {
                    continue;
                };
                let Some(client_pane) = pane.downcast_ref::<ClientPane>() else {
                    continue;
                };
                let title = match tab.get_title() {
                    title if !title.is_empty() => title,
                    _ => pane.get_title(),
                };
                result.push(TabHandle {
                    info: MuxTabInfo {
                        tab_id: tab.tab_id(),
                        remote_tab_id: client_pane.remote_tab_id,
                        pane_id: pane.pane_id(),
                        title,
                        active: self.active_pane_id == Some(pane.pane_id()),
                    },
                });
            }
        }
        result
    }
}

struct TabHandle {
    info: MuxTabInfo,
}

fn tabs_differ(previous: Option<&[MuxTabInfo]>, current: &[MuxTabInfo]) -> bool {
    previous != Some(current)
}

fn snapshot_attributes(attrs: &CellAttributes, palette: &ColorPalette) -> CellStyleSnapshot {
    let mut foreground = attrs.foreground();
    let mut background = attrs.background();
    if attrs.intensity() == Intensity::Bold {
        if let ColorAttribute::PaletteIndex(index) = foreground {
            if index < 8 {
                foreground = ColorAttribute::PaletteIndex(index + 8);
            }
        }
    }
    if attrs.reverse() {
        std::mem::swap(&mut foreground, &mut background);
    }
    CellStyleSnapshot {
        foreground_rgba: color_to_rgba(palette.resolve_fg(foreground)),
        background_rgba: color_to_rgba(palette.resolve_bg(background)),
        intensity: attrs.intensity() as u8,
        underline: attrs.underline() as u8,
        italic: attrs.italic(),
        strikethrough: attrs.strikethrough(),
        invisible: attrs.invisible(),
    }
}

fn color_to_rgba(color: SrgbaTuple) -> [u8; 4] {
    let (red, green, blue, alpha) = color.to_srgb_u8();
    [red, green, blue, alpha]
}

fn display_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

fn mouse_wheel_event(column: usize, row: usize, delta: isize) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Press,
        x: column,
        y: i64::try_from(row).unwrap_or(i64::MAX),
        x_pixel_offset: 0,
        y_pixel_offset: 0,
        button: if delta > 0 {
            MouseButton::WheelDown(delta.unsigned_abs())
        } else {
            MouseButton::WheelUp(delta.unsigned_abs())
        },
        modifiers: KeyModifiers::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_a_nonzero_mux_codec() {
        assert!(codec_version() > 0);
        assert_eq!(WEZTERM_REVISION.len(), 40);
    }

    #[test]
    fn endpoint_keeps_android_ssh_isolation_options() {
        let endpoint = MuxEndpoint::new(
            "host.example",
            "user",
            2222,
            "/data/user/0/example/files",
            Some(PathBuf::from(
                "/data/user/0/example/files/ssh/identities/debug",
            )),
            "/opt/wezterm/bin/wezterm",
        )
        .unwrap();
        let domain = endpoint.to_domain().unwrap();
        assert_eq!(domain.remote_address, "host.example");
        assert_eq!(domain.ssh_option["port"], "2222");
        assert_eq!(domain.ssh_option["wezterm_ssh_process_config"], "false");
        assert_eq!(domain.ssh_option["identitiesonly"], "yes");
        assert_eq!(
            domain.remote_wezterm_path.as_deref(),
            Some("/opt/wezterm/bin/wezterm")
        );
    }

    #[test]
    fn endpoint_rejects_shell_metacharacters_in_remote_program_path() {
        for path in [
            "/opt/wezterm;touch/tmp/injected",
            "/opt/wezterm $(id)",
            "'/opt/wezterm'",
            "",
        ] {
            assert!(
                MuxEndpoint::new(
                    "host.example",
                    "user",
                    22,
                    "/data/user/0/example/files",
                    None,
                    path,
                )
                .is_err(),
                "unsafe remote path unexpectedly accepted: {path:?}",
            );
        }
    }

    #[test]
    fn android_viewport_stays_bottom_aligned_when_another_client_is_taller() {
        let dimensions = RenderableDimensions {
            cols: 101,
            viewport_rows: 33,
            scrollback_rows: 120,
            physical_top: 68,
            scrollback_top: -20,
            ..RenderableDimensions::default()
        };

        let live = android_viewport(&dimensions, 17, None);
        assert_eq!(live.live_top, 84);
        assert_eq!(live.top, 84);
        assert_eq!(live.rows, 17);
        assert_eq!(live.max_offset, 104);

        let history = android_viewport(&dimensions, 17, Some(72));
        assert_eq!(history.live_top, 84);
        assert_eq!(history.top, 72);

        let clamped_old = android_viewport(&dimensions, 17, Some(isize::MIN));
        assert_eq!(clamped_old.top, dimensions.scrollback_top);

        let clamped_new = android_viewport(&dimensions, 17, Some(isize::MAX));
        assert_eq!(clamped_new.top, live.live_top);
    }

    #[test]
    fn reported_transport_failure_is_delivered_as_one_mux_event() {
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let session = AndroidMuxSession {
            runtime_tx,
            event_tx,
            events: event_rx,
            stopped: stopped_rx,
            join: None,
        };

        session.report_transport_failure("domain is not attached".to_string());
        assert_eq!(
            session.try_next_event(),
            Some(MuxEvent::Error {
                message: "domain is not attached".to_string(),
            }),
        );
        assert_eq!(session.try_next_event(), None);

        // Let Drop finish immediately and verify that it still requests the
        // normal runtime shutdown after the externally reported error.
        stopped_tx.send(()).unwrap();
        drop(session);
        assert!(matches!(runtime_rx.try_recv(), Ok(RuntimeMessage::Stop)));
    }

    #[test]
    fn touch_scroll_delta_maps_to_terminal_wheel_direction() {
        assert_eq!(mouse_wheel_event(4, 7, 3).button, MouseButton::WheelDown(3),);
        assert_eq!(mouse_wheel_event(4, 7, -2).button, MouseButton::WheelUp(2),);
        let event = mouse_wheel_event(4, 7, 1);
        assert_eq!((event.x, event.y), (4, 7));
        assert_eq!(event.kind, MouseEventKind::Press);
    }

    #[test]
    fn tab_snapshot_detects_dynamic_title_changes_without_repeating_unchanged_state() {
        let original = vec![MuxTabInfo {
            tab_id: 1,
            remote_tab_id: 3,
            pane_id: 2,
            title: "shell".to_string(),
            active: true,
        }];
        assert!(tabs_differ(None, &original));
        assert!(!tabs_differ(Some(&original), &original));

        let renamed = vec![MuxTabInfo {
            title: "nvim · main.rs".to_string(),
            ..original[0].clone()
        }];
        assert!(tabs_differ(Some(&original), &renamed));
    }
}
