use serde::Serialize;
use smol::channel::{Receiver, TryRecvError as SmolTryRecvError};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{channel, Receiver as ThreadReceiver, TryRecvError as ThreadTryRecvError};
use wezterm_ssh::{
    AuthenticationEvent, Child, ConfigMap, HostVerificationEvent, MasterPty, Session, SessionEvent,
    SshChildProcess, SshPty,
};

pub use wezterm_ssh::PtySize;

pub const WEZTERM_REVISION: &str = "d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshEndpoint {
    host: String,
    user: String,
    port: u16,
}

impl SshEndpoint {
    pub fn new(
        host: impl Into<String>,
        user: impl Into<String>,
        port: u16,
    ) -> Result<Self, ConfigError> {
        let host = host.into();
        let user = user.into();
        validate_host(&host)?;
        validate_user(&user)?;
        if port == 0 {
            return Err(ConfigError::InvalidPort);
        }
        Ok(Self { host, user, port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidSshConfig {
    endpoint: SshEndpoint,
    ssh_dir: PathBuf,
    identity_file: Option<PathBuf>,
}

impl AndroidSshConfig {
    /// Builds a connection description rooted below Android's app-private
    /// `filesDir`. No default desktop SSH config or credential path is used.
    pub fn new(
        endpoint: SshEndpoint,
        app_files_dir: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let app_files_dir = app_files_dir.into();
        validate_absolute_normal_path(&app_files_dir)
            .map_err(|_| ConfigError::InvalidAppFilesDirectory(app_files_dir.clone()))?;
        if app_files_dir.parent().is_none() {
            return Err(ConfigError::InvalidAppFilesDirectory(app_files_dir));
        }

        Ok(Self {
            endpoint,
            ssh_dir: app_files_dir.join("ssh"),
            identity_file: None,
        })
    }

    /// Selects a key that has already been imported into this app's private
    /// `filesDir/ssh/` tree. Symlink/canonical-path checks belong to the future
    /// Android importer, immediately before it writes the key.
    pub fn with_identity_file(mut self, path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        validate_absolute_normal_path(&path)
            .map_err(|_| ConfigError::IdentityOutsidePrivateStorage(path.clone()))?;
        if !path.starts_with(&self.ssh_dir) || path == self.ssh_dir {
            return Err(ConfigError::IdentityOutsidePrivateStorage(path));
        }
        self.identity_file = Some(path);
        Ok(self)
    }

    pub fn endpoint(&self) -> &SshEndpoint {
        &self.endpoint
    }

    pub fn ssh_dir(&self) -> &Path {
        &self.ssh_dir
    }

    pub fn known_hosts_file(&self) -> PathBuf {
        self.ssh_dir.join("known_hosts")
    }

    pub fn to_config_map(&self) -> Result<ConfigMap, ConfigError> {
        let ssh_dir = path_text(&self.ssh_dir)?;
        let known_hosts = path_text(&self.known_hosts_file())?;
        let disabled_global_hosts = path_text(&self.ssh_dir.join("global_known_hosts.disabled"))?;
        let disabled_agent = path_text(&self.ssh_dir.join("agent.disabled"))?;

        let mut map = ConfigMap::new();
        map.insert("hostname".into(), self.endpoint.host.clone());
        map.insert("user".into(), self.endpoint.user.clone());
        map.insert("port".into(), self.endpoint.port.to_string());
        map.insert("wezterm_ssh_backend".into(), "libssh".into());
        map.insert("wezterm_ssh_dir".into(), ssh_dir);
        map.insert("wezterm_ssh_process_config".into(), "false".into());
        map.insert("proxycommand".into(), "none".into());
        map.insert("forwardagent".into(), "no".into());
        map.insert("identitiesonly".into(), "yes".into());
        map.insert("identityagent".into(), disabled_agent);
        map.insert("userknownhostsfile".into(), known_hosts);
        map.insert("globalknownhostsfile".into(), disabled_global_hosts);
        map.insert("serveraliveinterval".into(), "30".into());

        if let Some(identity) = &self.identity_file {
            map.insert("identityfile".into(), path_text(identity)?);
        }
        Ok(map)
    }

    pub fn connect(&self) -> Result<AndroidSshSession, SessionControlError> {
        let config = self.to_config_map()?;
        let (session, events) = Session::connect(config)
            .map_err(|error| SessionControlError::Start(error.to_string()))?;
        Ok(AndroidSshSession {
            session,
            events,
            pending_host_verification: None,
            pending_authentication: None,
            authenticated: false,
            pty_open: None,
            pty: None,
            pending_output: VecDeque::new(),
            pty_notice: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthenticationPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Banner {
        message: Option<String>,
    },
    VerifyHost {
        message: String,
    },
    Authenticate {
        username: String,
        instructions: String,
        prompts: Vec<AuthenticationPrompt>,
    },
    HostVerificationFailed {
        remote_address: String,
        fingerprint: String,
        known_hosts_file: Option<PathBuf>,
    },
    Authenticated,
    PtyReady,
    PtyExited {
        exit_code: Option<u32>,
    },
    PtyError {
        message: String,
    },
    Error {
        message: String,
    },
}

struct PendingAuthentication {
    upstream: AuthenticationEvent,
    answer_count: usize,
    event: ClientEvent,
}

struct PendingHostVerification {
    upstream: HostVerificationEvent,
    event: ClientEvent,
}

enum PtyOpenResult {
    Ready {
        master: SshPty,
        writer: Box<dyn Write + Send>,
        output: ThreadReceiver<PtyOutput>,
        child: SshChildProcess,
    },
    Error(String),
}

enum PtyOutput {
    Bytes(Vec<u8>),
    Eof,
    Error(String),
}

struct ActivePty {
    master: SshPty,
    writer: Box<dyn Write + Send>,
    output: ThreadReceiver<PtyOutput>,
    child: SshChildProcess,
}

/// Owns the transport thread and protocol session independently from any
/// Android `Surface`. UI code polls events and answers challenges explicitly.
pub struct AndroidSshSession {
    session: Session,
    events: Receiver<SessionEvent>,
    pending_host_verification: Option<PendingHostVerification>,
    pending_authentication: Option<PendingAuthentication>,
    authenticated: bool,
    pty_open: Option<ThreadReceiver<PtyOpenResult>>,
    pty: Option<ActivePty>,
    pending_output: VecDeque<u8>,
    pty_notice: Option<ClientEvent>,
}

impl AndroidSshSession {
    pub fn session(&self) -> Session {
        self.session.clone()
    }

    pub fn try_next_event(&mut self) -> Result<Option<ClientEvent>, SessionControlError> {
        if self.pending_host_verification.is_some() || self.pending_authentication.is_some() {
            return Err(SessionControlError::PendingChallenge);
        }

        let event = match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(SmolTryRecvError::Empty) => None,
            Err(SmolTryRecvError::Closed) => return Err(SessionControlError::EventChannelClosed),
        };

        let Some(event) = event else {
            return self.poll_pty_event();
        };

        Ok(Some(match event {
            SessionEvent::Banner(message) => ClientEvent::Banner { message },
            SessionEvent::HostVerify(event) => {
                let result = ClientEvent::VerifyHost {
                    message: event.message.clone(),
                };
                self.pending_host_verification = Some(PendingHostVerification {
                    upstream: event,
                    event: result.clone(),
                });
                result
            }
            SessionEvent::Authenticate(event) => {
                let prompts = event
                    .prompts
                    .iter()
                    .map(|prompt| AuthenticationPrompt {
                        text: prompt.prompt.clone(),
                        echo: prompt.echo,
                    })
                    .collect::<Vec<_>>();
                let result = ClientEvent::Authenticate {
                    username: event.username.clone(),
                    instructions: event.instructions.clone(),
                    prompts,
                };
                self.pending_authentication = Some(PendingAuthentication {
                    answer_count: event.prompts.len(),
                    upstream: event,
                    event: result.clone(),
                });
                result
            }
            SessionEvent::HostVerificationFailed(event) => ClientEvent::HostVerificationFailed {
                remote_address: event.remote_address,
                fingerprint: event.key,
                known_hosts_file: event.file,
            },
            SessionEvent::Authenticated => {
                self.authenticated = true;
                ClientEvent::Authenticated
            }
            SessionEvent::Error(message) => ClientEvent::Error { message },
        }))
    }

    /// Opens a single remote shell without blocking Android's UI thread.
    pub fn open_pty(&mut self, size: PtySize) -> Result<(), SessionControlError> {
        if !self.authenticated {
            return Err(SessionControlError::NotAuthenticated);
        }
        if self.pty_open.is_some() || self.pty.is_some() {
            return Err(SessionControlError::PtyAlreadyOpen);
        }

        let session = self.session.clone();
        let (result_tx, result_rx) = channel();
        std::thread::Builder::new()
            .name("wezterm-android-open-pty".into())
            .spawn(move || {
                let result = (|| -> Result<PtyOpenResult, String> {
                    let (master, child) =
                        smol::block_on(session.request_pty("xterm-256color", size, None, None))
                            .map_err(|error| format!("request remote PTY: {error:#}"))?;
                    let mut reader = master
                        .try_clone_reader()
                        .map_err(|error| format!("clone remote PTY reader: {error:#}"))?;
                    let writer = master
                        .take_writer()
                        .map_err(|error| format!("take remote PTY writer: {error:#}"))?;
                    let (output_tx, output_rx) = channel();
                    std::thread::Builder::new()
                        .name("wezterm-android-pty-reader".into())
                        .spawn(move || {
                            let mut buffer = vec![0_u8; 16 * 1024];
                            loop {
                                match reader.read(&mut buffer) {
                                    Ok(0) => {
                                        let _ = output_tx.send(PtyOutput::Eof);
                                        break;
                                    }
                                    Ok(count) => {
                                        if output_tx
                                            .send(PtyOutput::Bytes(buffer[..count].to_vec()))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(error)
                                        if error.kind() == std::io::ErrorKind::Interrupted => {}
                                    Err(error) => {
                                        let _ = output_tx.send(PtyOutput::Error(error.to_string()));
                                        break;
                                    }
                                }
                            }
                        })
                        .map_err(|error| format!("spawn remote PTY reader: {error}"))?;

                    Ok(PtyOpenResult::Ready {
                        master,
                        writer,
                        output: output_rx,
                        child,
                    })
                })()
                .unwrap_or_else(PtyOpenResult::Error);
                let _ = result_tx.send(result);
            })
            .map_err(|error| SessionControlError::Pty(format!("spawn PTY opener: {error}")))?;
        self.pty_open = Some(result_rx);
        Ok(())
    }

    /// Drains a bounded amount of raw output for the WezTerm parser. Escape
    /// sequences are deliberately preserved.
    pub fn drain_pty_output(&mut self, limit: usize) -> Vec<u8> {
        if limit == 0 {
            return Vec::new();
        }
        let mut drained = Vec::with_capacity(limit.min(16 * 1024));
        while drained.len() < limit {
            let Some(byte) = self.pending_output.pop_front() else {
                break;
            };
            drained.push(byte);
        }

        while drained.len() < limit {
            let message = match self.pty.as_ref().map(|pty| pty.output.try_recv()) {
                Some(Ok(message)) => message,
                Some(Err(ThreadTryRecvError::Empty)) | None => break,
                Some(Err(ThreadTryRecvError::Disconnected)) => {
                    self.pty_notice
                        .get_or_insert(ClientEvent::PtyExited { exit_code: None });
                    break;
                }
            };
            match message {
                PtyOutput::Bytes(bytes) => {
                    let remaining = limit - drained.len();
                    let split = bytes.len().min(remaining);
                    drained.extend_from_slice(&bytes[..split]);
                    self.pending_output.extend(&bytes[split..]);
                }
                PtyOutput::Eof => {
                    let exit_code = self
                        .pty
                        .as_mut()
                        .and_then(|pty| pty.child.try_wait().ok().flatten())
                        .map(|status| status.exit_code());
                    self.pty_notice
                        .get_or_insert(ClientEvent::PtyExited { exit_code });
                    break;
                }
                PtyOutput::Error(message) => {
                    self.pty_notice
                        .get_or_insert(ClientEvent::PtyError { message });
                    break;
                }
            }
        }
        drained
    }

    pub fn write_pty(&mut self, bytes: &[u8]) -> Result<(), SessionControlError> {
        let pty = self.pty.as_mut().ok_or(SessionControlError::PtyNotReady)?;
        pty.writer
            .write_all(bytes)
            .and_then(|()| pty.writer.flush())
            .map_err(|error| SessionControlError::Pty(format!("write remote PTY: {error}")))
    }

    pub fn resize_pty(&mut self, size: PtySize) -> Result<(), SessionControlError> {
        let pty = self.pty.as_ref().ok_or(SessionControlError::PtyNotReady)?;
        pty.master
            .resize(size)
            .map_err(|error| SessionControlError::Pty(format!("resize remote PTY: {error:#}")))
    }

    pub fn pty_is_ready(&self) -> bool {
        self.pty.is_some()
    }

    fn poll_pty_event(&mut self) -> Result<Option<ClientEvent>, SessionControlError> {
        if let Some(notice) = self.pty_notice.take() {
            return Ok(Some(notice));
        }
        let Some(open) = self.pty_open.as_ref() else {
            return Ok(None);
        };
        match open.try_recv() {
            Ok(PtyOpenResult::Ready {
                master,
                writer,
                output,
                child,
            }) => {
                self.pty_open = None;
                self.pty = Some(ActivePty {
                    master,
                    writer,
                    output,
                    child,
                });
                Ok(Some(ClientEvent::PtyReady))
            }
            Ok(PtyOpenResult::Error(message)) => {
                self.pty_open = None;
                Ok(Some(ClientEvent::PtyError { message }))
            }
            Err(ThreadTryRecvError::Empty) => Ok(None),
            Err(ThreadTryRecvError::Disconnected) => {
                self.pty_open = None;
                Ok(Some(ClientEvent::PtyError {
                    message: "remote PTY opener exited without a result".into(),
                }))
            }
        }
    }

    /// Returns a copy of the currently unanswered challenge. This lets a
    /// recreated Android Activity restore the dialog without restarting the
    /// network session.
    pub fn pending_event(&self) -> Option<ClientEvent> {
        self.pending_host_verification
            .as_ref()
            .map(|pending| pending.event.clone())
            .or_else(|| {
                self.pending_authentication
                    .as_ref()
                    .map(|pending| pending.event.clone())
            })
    }

    pub fn answer_host_verification(&mut self, trust: bool) -> Result<(), SessionControlError> {
        let challenge = self
            .pending_host_verification
            .take()
            .ok_or(SessionControlError::NoHostVerificationChallenge)?;
        challenge
            .upstream
            .try_answer(trust)
            .map_err(|error| SessionControlError::Reply(error.to_string()))
    }

    pub fn answer_authentication(
        &mut self,
        answers: Vec<String>,
    ) -> Result<(), SessionControlError> {
        let expected = self
            .pending_authentication
            .as_ref()
            .ok_or(SessionControlError::NoAuthenticationChallenge)?
            .answer_count;
        if answers.len() != expected {
            return Err(SessionControlError::WrongAnswerCount {
                expected,
                actual: answers.len(),
            });
        }
        let challenge = self.pending_authentication.take().unwrap();
        challenge
            .upstream
            .try_answer(answers)
            .map_err(|error| SessionControlError::Reply(error.to_string()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("SSH host must be an IP address or an ASCII DNS name")]
    InvalidHost,
    #[error("SSH username must be non-empty and contain no whitespace or control characters")]
    InvalidUser,
    #[error("SSH port must be in 1..=65535")]
    InvalidPort,
    #[error("Android app files directory must be an absolute normalized non-root path: {0:?}")]
    InvalidAppFilesDirectory(PathBuf),
    #[error(
        "identity must be an absolute normalized path below the app-private SSH directory: {0:?}"
    )]
    IdentityOutsidePrivateStorage(PathBuf),
    #[error("Android SSH path is not valid UTF-8: {0:?}")]
    NonUtf8Path(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum SessionControlError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unable to start SSH session: {0}")]
    Start(String),
    #[error("an SSH challenge must be answered before polling another event")]
    PendingChallenge,
    #[error("SSH event channel closed")]
    EventChannelClosed,
    #[error("SSH session is not authenticated")]
    NotAuthenticated,
    #[error("a remote PTY is already opening or active")]
    PtyAlreadyOpen,
    #[error("remote PTY is not ready")]
    PtyNotReady,
    #[error("remote PTY error: {0}")]
    Pty(String),
    #[error("there is no pending host-verification challenge")]
    NoHostVerificationChallenge,
    #[error("there is no pending authentication challenge")]
    NoAuthenticationChallenge,
    #[error("authentication expected {expected} answers but received {actual}")]
    WrongAnswerCount { expected: usize, actual: usize },
    #[error("unable to send SSH challenge response: {0}")]
    Reply(String),
}

fn validate_host(host: &str) -> Result<(), ConfigError> {
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return Err(ConfigError::InvalidHost);
    }
    let valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    valid.then_some(()).ok_or(ConfigError::InvalidHost)
}

fn validate_user(user: &str) -> Result<(), ConfigError> {
    if user.is_empty() || user.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        Err(ConfigError::InvalidUser)
    } else {
        Ok(())
    }
}

fn validate_absolute_normal_path(path: &Path) -> Result<(), ()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        Err(())
    } else {
        Ok(())
    }
}

fn path_text(path: &Path) -> Result<String, ConfigError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| ConfigError::NonUtf8Path(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> SshEndpoint {
        SshEndpoint::new("test.example", "robot", 2222).unwrap()
    }

    #[test]
    fn validates_endpoint_without_reinterpreting_ipv6() {
        assert_eq!(
            SshEndpoint::new("2001:db8::5", "user", 22).unwrap().host(),
            "2001:db8::5"
        );
        assert!(matches!(
            SshEndpoint::new("bad host", "user", 22),
            Err(ConfigError::InvalidHost)
        ));
        assert!(matches!(
            SshEndpoint::new("host", "bad user", 22),
            Err(ConfigError::InvalidUser)
        ));
        assert!(matches!(
            SshEndpoint::new("host", "user", 0),
            Err(ConfigError::InvalidPort)
        ));
    }

    #[test]
    fn produces_isolated_password_capable_config_without_default_identity() {
        let config = AndroidSshConfig::new(endpoint(), "/data/user/0/app/files").unwrap();
        let map = config.to_config_map().unwrap();

        assert_eq!(map.len(), 13);
        assert_eq!(map["hostname"], "test.example");
        assert_eq!(map["user"], "robot");
        assert_eq!(map["port"], "2222");
        assert_eq!(map["wezterm_ssh_backend"], "libssh");
        assert_eq!(map["wezterm_ssh_process_config"], "false");
        assert_eq!(map["wezterm_ssh_dir"], "/data/user/0/app/files/ssh");
        assert_eq!(map["proxycommand"], "none");
        assert_eq!(map["forwardagent"], "no");
        assert_eq!(map["identitiesonly"], "yes");
        assert_eq!(
            map["identityagent"],
            "/data/user/0/app/files/ssh/agent.disabled"
        );
        assert_eq!(
            map["userknownhostsfile"],
            "/data/user/0/app/files/ssh/known_hosts"
        );
        assert_eq!(
            map["globalknownhostsfile"],
            "/data/user/0/app/files/ssh/global_known_hosts.disabled"
        );
        assert_eq!(map["serveraliveinterval"], "30");
        assert!(!map.contains_key("identityfile"));
        assert!(!map.contains_key("password"));
    }

    #[test]
    fn accepts_only_an_imported_private_identity_path() {
        let config = AndroidSshConfig::new(endpoint(), "/data/user/0/app/files")
            .unwrap()
            .with_identity_file("/data/user/0/app/files/ssh/identities/id_ed25519")
            .unwrap();
        assert_eq!(
            config.to_config_map().unwrap()["identityfile"],
            "/data/user/0/app/files/ssh/identities/id_ed25519"
        );

        let outside = AndroidSshConfig::new(endpoint(), "/data/user/0/app/files")
            .unwrap()
            .with_identity_file("/sdcard/Download/id_ed25519");
        assert!(matches!(
            outside,
            Err(ConfigError::IdentityOutsidePrivateStorage(_))
        ));

        let traversal = AndroidSshConfig::new(endpoint(), "/data/user/0/app/files")
            .unwrap()
            .with_identity_file("/data/user/0/app/files/ssh/../leak");
        assert!(matches!(
            traversal,
            Err(ConfigError::IdentityOutsidePrivateStorage(_))
        ));
    }

    #[test]
    fn refuses_relative_or_root_storage() {
        assert!(matches!(
            AndroidSshConfig::new(endpoint(), "relative/files"),
            Err(ConfigError::InvalidAppFilesDirectory(_))
        ));
        assert!(matches!(
            AndroidSshConfig::new(endpoint(), "/"),
            Err(ConfigError::InvalidAppFilesDirectory(_))
        ));
    }

    #[test]
    fn client_events_have_a_stable_tagged_json_shape() {
        let event = ClientEvent::Authenticate {
            username: "robot".into(),
            instructions: "second factor".into(),
            prompts: vec![AuthenticationPrompt {
                text: "Code: ".into(),
                echo: false,
            }],
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"authenticate","username":"robot","instructions":"second factor","prompts":[{"text":"Code: ","echo":false}]}"#
        );
        assert_eq!(
            serde_json::to_string(&ClientEvent::Authenticated).unwrap(),
            r#"{"type":"authenticated"}"#
        );
    }
}
