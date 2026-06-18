//! Session orchestration and the UI⇄core message protocol.
//!
//! A [`SessionManager`] owns the tokio runtime and one task per SSH session.
//! Callers (the GUI or the headless example) talk to it exclusively through
//! channels:
//!
//! * [`ToCore`] — commands sent *into* the core (connect, input, resize, …)
//! * [`FromCore`] — events emitted *out* of the core (data ready, auth prompt,
//!   closed, …)
//!
//! This keeps all blocking/async SSH I/O off the UI thread. The UI just pumps
//! messages and repaints from [`GridSnapshot`]s.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use kt_config::ConnectParams;

use crate::ssh::{AuthProvider, HostKeyVerifier, PtySize, SshShell};
use crate::term::{GridSnapshot, TermEngine, TermEvent};

/// Opaque session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// Commands sent from the UI into the core.
#[derive(Debug)]
pub enum ToCore {
    /// Open a new connection under `id`.
    Connect {
        id: SessionId,
        params: Box<ConnectParams>,
        pty: PtySize,
    },
    /// Keyboard input bytes for a session's PTY.
    Input { id: SessionId, data: Vec<u8> },
    /// New terminal size (columns, rows).
    Resize {
        id: SessionId,
        cols: u16,
        rows: u16,
    },
    /// Scroll the viewport by `delta` lines (positive = into history).
    Scroll { id: SessionId, delta: i32 },
    /// Close / disconnect a session.
    Disconnect { id: SessionId },
}

/// Events emitted from the core out to the UI.
#[derive(Debug)]
pub enum FromCore {
    /// Connection + auth + shell are up.
    Connected { id: SessionId },
    /// New rendered grid available.
    Render {
        id: SessionId,
        snapshot: Box<GridSnapshot>,
    },
    /// Title changed (OSC).
    Title { id: SessionId, title: String },
    /// Terminal bell.
    Bell { id: SessionId },
    /// Session ended. `error` is `None` for a clean exit.
    Closed {
        id: SessionId,
        error: Option<String>,
    },
}

/// Factory that produces a fresh [`AuthProvider`] per connection.
///
/// Auth providers are `Send` but generally not `Sync` (they may prompt), so
/// each session gets its own. The factory itself must be shareable.
pub trait AuthProviderFactory: Send + Sync {
    fn create(&self, id: SessionId, params: &ConnectParams) -> Box<dyn AuthProvider>;
}

/// Owns the runtime and live sessions.
pub struct SessionManager {
    to_core_tx: mpsc::UnboundedSender<ToCore>,
    from_core_rx: mpsc::UnboundedReceiver<FromCore>,
    _runtime: tokio::runtime::Runtime,
}

impl SessionManager {
    /// Spawn the core on a dedicated multi-threaded runtime.
    pub fn spawn(
        verifier: Arc<dyn HostKeyVerifier>,
        auth_factory: Arc<dyn AuthProviderFactory>,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("kt-core")
            .build()?;

        let (to_core_tx, to_core_rx) = mpsc::unbounded_channel::<ToCore>();
        let (from_core_tx, from_core_rx) = mpsc::unbounded_channel::<FromCore>();

        runtime.spawn(core_loop(to_core_rx, from_core_tx, verifier, auth_factory));

        Ok(Self {
            to_core_tx,
            from_core_rx,
            _runtime: runtime,
        })
    }

    /// Send a command into the core.
    pub fn send(&self, msg: ToCore) {
        // Errors only occur if the core loop has stopped; ignore for now.
        let _ = self.to_core_tx.send(msg);
    }

    /// Clone the raw command sender. Useful for forwarding input from a
    /// separate thread (e.g. a stdin reader) without borrowing the manager.
    pub fn raw_sender(&self) -> mpsc::UnboundedSender<ToCore> {
        self.to_core_tx.clone()
    }

    /// Non-blocking poll for the next event from the core.
    pub fn try_recv(&mut self) -> Option<FromCore> {
        self.from_core_rx.try_recv().ok()
    }

    /// Blocking receive (used by the headless example).
    pub fn blocking_recv(&mut self) -> Option<FromCore> {
        self.from_core_rx.blocking_recv()
    }
}

/// Top-level core dispatch loop: routes [`ToCore`] commands to per-session tasks.
async fn core_loop(
    mut rx: mpsc::UnboundedReceiver<ToCore>,
    tx: mpsc::UnboundedSender<FromCore>,
    verifier: Arc<dyn HostKeyVerifier>,
    auth_factory: Arc<dyn AuthProviderFactory>,
) {
    // Per-session input/control senders.
    let mut sessions: HashMap<SessionId, SessionHandles> = HashMap::new();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            ToCore::Connect { id, params, pty } => {
                let (input_tx, input_rx) = mpsc::unbounded_channel::<SessionCmd>();
                sessions.insert(id, SessionHandles { cmd_tx: input_tx });

                let provider = auth_factory.create(id, &params);
                let task = SessionTask {
                    id,
                    params: *params,
                    pty,
                    verifier: verifier.clone(),
                    provider,
                    out: tx.clone(),
                    cmd_rx: input_rx,
                };
                tokio::spawn(task.run());
            }
            ToCore::Input { id, data } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Input(data));
                }
            }
            ToCore::Resize { id, cols, rows } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Resize { cols, rows });
                }
            }
            ToCore::Scroll { id, delta } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Scroll(delta));
                }
            }
            ToCore::Disconnect { id } => {
                if let Some(h) = sessions.remove(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Disconnect);
                }
            }
        }
    }
}

/// Control messages routed to a single session task.
enum SessionCmd {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Scroll(i32),
    Disconnect,
}

struct SessionHandles {
    cmd_tx: mpsc::UnboundedSender<SessionCmd>,
}

/// All state for one session's task.
struct SessionTask {
    id: SessionId,
    params: ConnectParams,
    pty: PtySize,
    verifier: Arc<dyn HostKeyVerifier>,
    provider: Box<dyn AuthProvider>,
    out: mpsc::UnboundedSender<FromCore>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
}

impl SessionTask {
    async fn run(mut self) {
        let id = self.id;

        // Connect + authenticate + open shell.
        let mut shell = match SshShell::open(
            &self.params,
            self.pty,
            self.verifier.clone(),
            self.provider.as_mut(),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                let _ = self.out.send(FromCore::Closed {
                    id,
                    error: Some(e.to_string()),
                });
                return;
            }
        };

        let _ = self.out.send(FromCore::Connected { id });

        // Build the terminal engine at the negotiated size.
        let scrollback = 10_000;
        let mut term = TermEngine::new(self.pty.cols as usize, self.pty.rows as usize, scrollback);

        // Emit the initial (blank) frame.
        self.emit_render(&term);

        let mut close_error: Option<String> = None;

        loop {
            tokio::select! {
                // Control messages from the UI.
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCmd::Input(data)) => {
                            if let Err(e) = shell.write(&data).await {
                                close_error = Some(e.to_string());
                                break;
                            }
                        }
                        Some(SessionCmd::Resize { cols, rows }) => {
                            term.resize(cols as usize, rows as usize, scrollback);
                            let _ = shell.resize(cols, rows).await;
                            self.emit_render(&term);
                        }
                        Some(SessionCmd::Scroll(delta)) => {
                            term.scroll(delta);
                            self.emit_render(&term);
                        }
                        Some(SessionCmd::Disconnect) | None => {
                            let _ = shell.disconnect().await;
                            break;
                        }
                    }
                }

                // Output / channel events from the remote.
                msg = shell.next_message() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            term.advance(&data);
                            self.handle_term_events(&term);
                            self.emit_render(&term);
                        }
                        Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                            // stderr — feed it to the terminal too.
                            term.advance(&data);
                            self.emit_render(&term);
                        }
                        Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) => {
                            break;
                        }
                        Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                            if exit_status != 0 {
                                close_error = Some(format!("remote shell exited with status {exit_status}"));
                            }
                            // wait for Close/Eof to actually break.
                        }
                        Some(_) => {}
                        None => break, // channel fully closed
                    }
                }
            }
        }

        let _ = self.out.send(FromCore::Closed {
            id,
            error: close_error,
        });
    }

    /// Forward terminal events (bell/title/pty-write) to the UI / back to PTY.
    fn handle_term_events(&self, term: &TermEngine) {
        for ev in term.take_events() {
            match ev {
                TermEvent::Bell => {
                    let _ = self.out.send(FromCore::Bell { id: self.id });
                }
                TermEvent::Title(title) => {
                    let _ = self.out.send(FromCore::Title { id: self.id, title });
                }
                // PtyWrite would be written back to the shell; deferred until
                // needed (device-status responses etc.).
                TermEvent::PtyWrite(_) | TermEvent::Wakeup => {}
            }
        }
    }

    fn emit_render(&self, term: &TermEngine) {
        let snapshot = Box::new(term.snapshot());
        let _ = self.out.send(FromCore::Render {
            id: self.id,
            snapshot,
        });
    }
}
