//! Headless SSH+VT smoke test — the Phase 1 verification target.
//!
//! Connects to a real SSH server, opens a PTY shell, and drives the full
//! `kt-core` pipeline (russh → `TermEngine` → `GridSnapshot`). The snapshot
//! is painted to the **alternate screen** using the host terminal, so this
//! genuinely exercises our terminal engine (not just byte passthrough) while
//! still being usable for `top`, `vim`, `tmux`, etc.
//!
//! Usage:
//!     cargo run -p kt-core --example headless -- [user@]host[:port]
//!
//! Auth: tries `~/.ssh/config` for defaults, then public key (the IdentityFile
//! from ssh_config, or ~/.ssh/id_ed25519 / id_rsa), then keyboard-interactive,
//! then password. Secrets are prompted on the terminal (never stored here).
//!
//! Keys: type normally; press Ctrl-] to quit.

use std::io::{Read, Write};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    cursor,
    style::{Color as CtColor, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
    QueueableCommand,
};

use kt_config::{AuthMethod, ConnectParams, Paths};
use kt_core::session::{AuthProviderFactory, SessionId};
use kt_core::ssh::{AuthProvider, HostKeyDecision, HostKeyVerifier, PtySize};
use kt_core::term::{CursorShape, GridSnapshot};
use kt_core::{FromCore, SessionManager, ToCore};
use russh::keys::PublicKey;

fn main() {
    let target = match std::env::args().nth(1) {
        Some(t) => t,
        None => {
            eprintln!("usage: headless [user@]host[:port]   (Ctrl-] to quit)");
            std::process::exit(2);
        }
    };

    if let Err(e) = run(&target) {
        // Make sure we leave the terminal in a sane state on error.
        let _ = terminal::disable_raw_mode();
        let mut out = std::io::stdout();
        let _ = out.queue(terminal::LeaveAlternateScreen);
        let _ = out.flush();
        eprintln!("\nerror: {e}");
        std::process::exit(1);
    }
}

fn run(target: &str) -> anyhow::Result<()> {
    let params = build_params(target)?;
    eprintln!(
        "connecting to {}@{}:{} …",
        params.user, params.host, params.port
    );

    // Host-key verifier: TOFU prompt on the terminal.
    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(PromptVerifier);
    let auth_factory: Arc<dyn AuthProviderFactory> = Arc::new(PromptAuthFactory);

    let mut mgr = SessionManager::spawn(verifier, auth_factory)?;
    let id = SessionId(1);

    // Determine the initial PTY size from the real terminal.
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(params),
        pty: PtySize { cols, rows },
    });

    // Spawn a thread to read raw stdin and forward keystrokes into the core.
    let (input_quit_tx, input_quit_rx) = std_mpsc::channel::<()>();
    let stdin_handle = spawn_stdin_forwarder(mgr_sender(&mgr), id, input_quit_rx);

    // Enter raw mode + alternate screen for rendering.
    terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    out.queue(terminal::EnterAlternateScreen)?
        .queue(cursor::Hide)?;
    out.flush()?;

    let result = event_loop(&mut mgr, &mut out);

    // Restore terminal.
    let _ = out.queue(cursor::Show);
    let _ = out.queue(terminal::LeaveAlternateScreen);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();

    // Stop stdin thread.
    let _ = input_quit_tx.send(());
    let _ = stdin_handle.join();

    result
}

/// Pull events from the core and paint snapshots until the session closes or
/// the user quits.
fn event_loop(mgr: &mut SessionManager, out: &mut std::io::Stdout) -> anyhow::Result<()> {
    loop {
        match mgr.blocking_recv() {
            Some(FromCore::Connected { .. }) => {
                // nothing; first Render will paint.
            }
            Some(FromCore::Render { snapshot, .. }) => {
                paint(out, &snapshot)?;
            }
            Some(FromCore::Title { title, .. }) => {
                let _ = out.queue(terminal::SetTitle(&title));
            }
            Some(FromCore::Bell { .. }) => {
                // Audible bell via host terminal.
                let _ = out.write_all(b"\x07");
                let _ = out.flush();
            }
            Some(FromCore::Closed { error, .. }) => {
                if let Some(e) = error {
                    anyhow::bail!("session closed: {e}");
                }
                return Ok(());
            }
            None => return Ok(()), // core stopped
        }
    }
}

/// Render a grid snapshot onto the alternate screen.
fn paint(out: &mut std::io::Stdout, snap: &GridSnapshot) -> std::io::Result<()> {
    out.queue(cursor::MoveTo(0, 0))?;
    out.queue(terminal::Clear(ClearType::All))?;

    let mut last_fg: Option<(u8, u8, u8)> = None;
    let mut last_bg: Option<(u8, u8, u8)> = None;

    for row in 0..snap.rows {
        out.queue(cursor::MoveTo(0, row as u16))?;
        for col in 0..snap.cols {
            let Some(cell) = snap.cell(row, col) else {
                continue;
            };
            if cell.attrs.wide_spacer {
                continue;
            }

            let fg = (cell.fg.r, cell.fg.g, cell.fg.b);
            let bg = (cell.bg.r, cell.bg.g, cell.bg.b);
            if last_fg != Some(fg) {
                out.queue(SetForegroundColor(CtColor::Rgb {
                    r: fg.0,
                    g: fg.1,
                    b: fg.2,
                }))?;
                last_fg = Some(fg);
            }
            if last_bg != Some(bg) {
                out.queue(SetBackgroundColor(CtColor::Rgb {
                    r: bg.0,
                    g: bg.1,
                    b: bg.2,
                }))?;
                last_bg = Some(bg);
            }
            if cell.attrs.bold {
                out.queue(SetAttribute(crossterm::style::Attribute::Bold))?;
            }
            if cell.attrs.underline {
                out.queue(SetAttribute(crossterm::style::Attribute::Underlined))?;
            }

            out.queue(Print(cell.c))?;

            if cell.attrs.bold || cell.attrs.underline {
                out.queue(SetAttribute(crossterm::style::Attribute::Reset))?;
                last_fg = None;
                last_bg = None;
            }
        }
    }

    out.queue(ResetColor)?;

    // Position the host cursor where the terminal cursor is.
    if !matches!(snap.cursor.shape, CursorShape::Hidden) {
        out.queue(cursor::MoveTo(
            snap.cursor.column as u16,
            snap.cursor.line as u16,
        ))?;
        out.queue(cursor::Show)?;
    } else {
        out.queue(cursor::Hide)?;
    }

    out.flush()?;
    Ok(())
}

/// Read raw bytes from stdin and forward them to the core as PTY input.
/// Quits the whole app when Ctrl-] (0x1d) is seen.
fn spawn_stdin_forwarder(
    sender: SessionSender,
    id: SessionId,
    quit_rx: std_mpsc::Receiver<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            if quit_rx.try_recv().is_ok() {
                return;
            }
            // Non-blocking-ish: rely on small reads. crossterm raw mode makes
            // stdin byte-oriented.
            match stdin.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    let data = &buf[..n];
                    if data.contains(&0x1d) {
                        // Ctrl-]: quit.
                        sender.send(ToCore::Disconnect { id });
                        return;
                    }
                    sender.send(ToCore::Input {
                        id,
                        data: data.to_vec(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    })
}

/// A cheap clone of the core's input sender for the stdin thread.
#[derive(Clone)]
struct SessionSender {
    tx: tokio::sync::mpsc::UnboundedSender<ToCore>,
}
impl SessionSender {
    fn send(&self, msg: ToCore) {
        let _ = self.tx.send(msg);
    }
}

// SessionManager exposes its raw command sender so the stdin thread can
// forward keystrokes without borrowing the manager.
fn mgr_sender(mgr: &SessionManager) -> SessionSender {
    SessionSender {
        tx: mgr.raw_sender(),
    }
}

/// Parse `[user@]host[:port]` and merge `~/.ssh/config` + default key files.
fn build_params(target: &str) -> anyhow::Result<ConnectParams> {
    let (user_part, host_part) = match target.split_once('@') {
        Some((u, h)) => (Some(u.to_string()), h.to_string()),
        None => (None, target.to_string()),
    };
    let (host, port) = match host_part.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => {
            (h.to_string(), p.parse::<u16>().unwrap())
        }
        _ => (host_part, 22),
    };

    // 未在 user@host 中指定用户时,默认使用 root。
    // Default to root when no user is given in user@host form.
    let user = user_part.unwrap_or_else(|| "root".to_string());

    let mut params = ConnectParams {
        host: host.clone(),
        port,
        user,
        auth: Vec::new(),
        vault_id: None,
    };

    // Merge ~/.ssh/config (best-effort).
    if let Some(home) = dirs_home() {
        let cfg_path = home.join(".ssh").join("config");
        if let Ok(Some(host_cfg)) = kt_config::lookup_ssh_config(&cfg_path, &host) {
            params.merge_ssh_config(&host_cfg);
        }
        // Add default identity files if none specified.
        if !params
            .auth
            .iter()
            .any(|a| matches!(a, AuthMethod::PublicKey { .. }))
        {
            for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
                let key = home.join(".ssh").join(name);
                if key.exists() {
                    params.auth.push(AuthMethod::PublicKey { key_path: key });
                }
            }
        }
    }

    // Always end with interactive fallbacks.
    params.auth.push(AuthMethod::KeyboardInteractive);
    params.auth.push(AuthMethod::Password);

    // Touch Paths so the dependency is exercised (config dir discovery).
    if let Ok(p) = Paths::discover() {
        tracing::debug!("config dir: {}", p.config_dir().display());
    }

    Ok(params)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

// ---- Auth + host-key prompts (terminal-based) ----

struct PromptVerifier;
impl HostKeyVerifier for PromptVerifier {
    fn verify(
        &self,
        host: &str,
        port: u16,
        _key: &PublicKey,
        fingerprint: &str,
    ) -> HostKeyDecision {
        eprintln!("\nThe authenticity of host '{host}:{port}' can't be established.");
        eprintln!("Key fingerprint is {fingerprint}.");
        eprint!("Are you sure you want to continue connecting (yes/no)? ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok() {
            let ans = line.trim().to_lowercase();
            if ans == "yes" || ans == "y" {
                return HostKeyDecision::Accept;
            }
        }
        HostKeyDecision::Reject
    }
}

struct PromptAuthFactory;
impl AuthProviderFactory for PromptAuthFactory {
    fn create(&self, _id: SessionId, _params: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(PromptAuth)
    }
}

struct PromptAuth;
impl AuthProvider for PromptAuth {
    fn password(&mut self, user: &str, host: &str) -> Option<String> {
        rpassword::prompt_password(format!("{user}@{host}'s password: ")).ok()
    }

    fn key_passphrase(&mut self, key_path: &str) -> Option<String> {
        rpassword::prompt_password(format!("Enter passphrase for key '{key_path}': ")).ok()
    }

    fn keyboard_interactive(
        &mut self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        if !name.is_empty() {
            eprintln!("{name}");
        }
        if !instructions.is_empty() {
            eprintln!("{instructions}");
        }
        let mut answers = Vec::with_capacity(prompts.len());
        for (prompt, echo) in prompts {
            if *echo {
                eprint!("{prompt}");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok()?;
                answers.push(line.trim_end_matches(['\r', '\n']).to_string());
            } else {
                let ans = rpassword::prompt_password(prompt).ok()?;
                answers.push(ans);
            }
        }
        Some(answers)
    }
}
