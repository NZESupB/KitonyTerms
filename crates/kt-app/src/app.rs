//! eframe 应用主体 —— 持有 [`SessionManager`]、持久化 [`Store`]、
//! 各标签页终端状态、连接对话框与渲染/输入循环。
//!
//! The eframe application: owns the [`SessionManager`], the persistence [`Store`],
//! per-tab terminal state, the connect dialog, and the render/input loop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use kt_config::{AuthMethod, ConnectParams, SessionProfile};
use kt_core::session::{AuthProviderFactory, SessionId};
use kt_core::ssh::{AcceptAllVerifier, AuthProvider, HostKeyVerifier, PtySize};
use kt_core::term::GridSnapshot;
use kt_core::{FromCore, SessionManager, ToCore};

use crate::connect_dialog::{ConnectForm, ConnectOutcome};
use crate::input::events_to_bytes;
use crate::store::{Store, UnlockOutcome};
use crate::terminal_view::TerminalView;
use crate::unlock_dialog::{UnlockAction, UnlockDialog};

/// 连接对话框预先填入的机密(按键为 SessionId),供 GUI 的 AuthProvider 读取,
/// 避免握手过程中弹阻塞对话框。
///
/// Secrets supplied up front by the connect dialog, keyed by session.
/// The GUI's [`AuthProvider`] reads from here instead of prompting mid-handshake.
type SecretStore = Arc<Mutex<HashMap<SessionId, SessionSecrets>>>;

#[derive(Default, Clone)]
struct SessionSecrets {
    password: Option<String>,
    key_passphrase: Option<String>,
}

/// 单个标签页的 UI 与终端状态。
/// Per-tab UI + terminal state.
struct Tab {
    id: SessionId,
    title: String,
    snapshot: Option<GridSnapshot>,
    view: TerminalView,
    status: TabStatus,
    /// 上次已通知 core 的 (cols, rows),避免频繁 resize。
    /// Last (cols, rows) we told the core, to avoid spamming resizes.
    last_grid: (u16, u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TabStatus {
    Connecting,
    Connected,
    Closed(Option<String>),
}

/// 应用启动状态。
/// App startup phase.
enum StartState {
    /// 尚未决定:显示解锁/设置对话框。
    /// Not decided yet: showing the unlock/setup dialog.
    Pending(UnlockDialog),
    /// 已跳过(锁定但可用)或已解锁 —— 进入主界面。
    /// Skipped (locked but usable) or unlocked — proceed to main UI.
    Ready,
}

pub struct KitonyApp {
    mgr: SessionManager,
    store: Store,
    secrets: SecretStore,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    connect_form: ConnectForm,
    font_size: f32,
    start: StartState,
    /// 待保存到侧栏的会话(连接成功后由对话框产出)。
    /// A session pending persistence after a successful connect.
    pending_save: Option<(SessionProfile, bool /* save_password */)>,
}

impl KitonyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 安装 CJK 回退字体,修复界面中文乱码(豆腐块)。
        // Install CJK fallback font to fix garbled Chinese UI text.
        crate::fonts::install_cjk_fallback(&cc.egui_ctx);

        let secrets: SecretStore = Arc::new(Mutex::new(HashMap::new()));

        // 持久化层(加载 config.toml;vault 暂未解锁)。
        // Persistence layer (loads config.toml; vault not yet unlocked).
        let store = Store::load().unwrap_or_else(|e| {
            tracing::error!("failed to load config store: {e}");
            // 兜底:用临时空配置继续启动,避免直接崩。
            Store::load().unwrap_or_else(|_| {
                panic!("cannot start: no config dir available")
            })
        });

        // 首次运行 → 设置主密码;否则 → 解锁。
        // First run → set master password; otherwise → unlock.
        let start = if store.vault_exists() {
            StartState::Pending(UnlockDialog::new_unlock())
        } else {
            StartState::Pending(UnlockDialog::new_setup())
        };

        // MVP:信任所有主机密钥(TOFU)。真正的 known_hosts 留待后续阶段。
        // MVP: trust-on-first-use accept-all verifier. A real known_hosts store
        // lands in a later phase.
        let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
        let factory: Arc<dyn AuthProviderFactory> =
            Arc::new(GuiAuthFactory { secrets: secrets.clone() });

        let mgr = SessionManager::spawn(verifier, factory).expect("spawn core runtime");

        Self {
            mgr,
            store,
            secrets,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            connect_form: ConnectForm::default(),
            font_size: 14.0,
            start,
            pending_save: None,
        }
    }

    /// 处理启动期的解锁/设置对话框。
    /// Drive the startup unlock/setup dialog.
    fn handle_start_dialog(&mut self, ctx: &egui::Context) {
        let StartState::Pending(dialog) = &mut self.start else {
            return;
        };
        match dialog.show(ctx) {
            UnlockAction::Pending => {
                ctx.request_repaint();
            }
            UnlockAction::Skipped => {
                tracing::info!("master password skipped — secrets read/write disabled");
                self.start = StartState::Ready;
            }
            UnlockAction::Submit(password) => {
                let outcome = if self.store.vault_exists() {
                    self.store.unlock(&password)
                } else {
                    self.store.create_vault(&password)
                };
                match outcome {
                    UnlockOutcome::Ok => {
                        tracing::info!("vault unlocked");
                        self.start = StartState::Ready;
                    }
                    other => dialog.set_failure(&other),
                }
            }
        }
    }

    fn start_connection(
        &mut self,
        params: ConnectParams,
        password: Option<String>,
        save_name: Option<String>,
    ) {
        let id = SessionId(self.next_id);
        self.next_id += 1;

        let vault_id = params.effective_vault_id();

        // 若已解锁且提供了密码,写入 vault(供下次重连复用)。
        // If unlocked and a password was provided, persist it to the vault.
        if let Some(pw) = password.as_ref() {
            if self.store.is_unlocked() {
                if let Err(e) = self.store.set_secret(&vault_id, pw) {
                    tracing::warn!("failed to save password to vault: {e}");
                }
            }
        }

        if password.is_some() {
            self.secrets.lock().unwrap().insert(
                id,
                SessionSecrets {
                    password,
                    key_passphrase: None,
                },
            );
        }

        let title = format!("{}@{}", params.user, params.host);
        let pty = PtySize { cols: 80, rows: 24 };

        // 排程:连接成功后若用户要求保存会话,写入 TOML。
        // Schedule: persist the session to TOML after connect if the user asked.
        if let Some(name) = save_name {
            self.pending_save = Some((
                SessionProfile {
                    name,
                    params: params.clone(),
                },
                true,
            ));
        }

        self.mgr.send(ToCore::Connect {
            id,
            params: Box::new(params),
            pty,
        });

        self.tabs.push(Tab {
            id,
            title,
            snapshot: None,
            view: TerminalView::new(self.font_size),
            status: TabStatus::Connecting,
            last_grid: (0, 0),
        });
        self.active = self.tabs.len() - 1;
    }

    /// 连接成功后,把排程中的会话写入 TOML。
    /// Persist the scheduled session to TOML after a successful connect.
    fn flush_pending_save(&mut self) {
        if let Some((profile, save_password)) = self.pending_save.take() {
            if let Err(e) = self.store.save_session(profile) {
                tracing::warn!("failed to save session: {e}");
            }
            let _ = save_password; // 密码已在 start_connection 中写入 vault
        }
    }

    /// 排空 core 的所有待处理事件到标签页状态。
    /// Drain all pending events from the core into tab state.
    fn pump_core_events(&mut self) {
        while let Some(ev) = self.mgr.try_recv() {
            match ev {
                FromCore::Connected { id } => {
                    if let Some(t) = self.tab_mut(id) {
                        t.status = TabStatus::Connected;
                    }
                    self.flush_pending_save();
                }
                FromCore::Render { id, snapshot } => {
                    if let Some(t) = self.tab_mut(id) {
                        t.snapshot = Some(*snapshot);
                    }
                }
                FromCore::Title { id, title } => {
                    if let Some(t) = self.tab_mut(id) {
                        if !title.is_empty() {
                            t.title = title;
                        }
                    }
                }
                FromCore::Bell { .. } => {
                    // 可用于让标签闪烁;暂忽略。
                    // Could flash the tab; ignored for now.
                }
                FromCore::Closed { id, error } => {
                    if let Some(t) = self.tab_mut(id) {
                        t.status = TabStatus::Closed(error);
                    }
                    self.secrets.lock().unwrap().remove(&id);
                }
            }
        }
    }

    fn tab_mut(&mut self, id: SessionId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let id = self.tabs[index].id;
        self.mgr.send(ToCore::Disconnect { id });
        self.tabs.remove(index);
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
    }

    /// 左侧栏:保存的会话列表 + 重连/删除。
    /// Left sidebar: saved-session list with reconnect/delete.
    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sessions")
            .resizable(true)
            .default_width(200.0)
            .width_range(120.0..=360.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Sessions");
                    if ui.button("➕").on_hover_text("New / 新建").clicked() {
                        self.connect_form.open();
                    }
                    if self.store.is_unlocked() {
                        ui
                            .label(egui::RichText::new("🔓"))
                            .on_hover_text("Vault unlocked / 已解锁");
                    } else {
                        ui
                            .label(egui::RichText::new("🔒"))
                            .on_hover_text("Vault locked / 未解锁");
                    }
                });
                ui.separator();

                // 收集本帧要执行的动作(避免在迭代中借用 self)。
                // Collect actions to run after iterating (avoid borrow conflicts).
                enum SideAction {
                    Reconnect(String),
                    Delete(String),
                }
                let mut action: Option<SideAction> = None;

                let names: Vec<String> = self
                    .store
                    .saved_sessions()
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                if names.is_empty() {
                    ui.label(
                        egui::RichText::new("No saved sessions.\n无已保存会话。")
                            .color(egui::Color32::from_gray(130))
                            .small(),
                    );
                }
                for name in names {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(false, &name)
                            .on_hover_text("Click to reconnect / 点击重连")
                            .clicked()
                        {
                            action = Some(SideAction::Reconnect(name.clone()));
                        }
                        if ui.small_button("🗑").on_hover_text("Delete / 删除").clicked() {
                            action = Some(SideAction::Delete(name.clone()));
                        }
                    });
                }

                if let Some(a) = action {
                    match a {
                        SideAction::Reconnect(name) => {
                            if let Some(profile) = self.store.session_named(&name) {
                                let pw = self.store.get_secret(&profile.params.effective_vault_id());
                                self.connect_form.open_with(&profile, pw);
                            }
                        }
                        SideAction::Delete(name) => {
                            if let Err(e) = self.store.delete_session(&name) {
                                tracing::warn!("failed to delete session: {e}");
                            }
                        }
                    }
                }
            });
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ New").clicked() {
                    self.connect_form.open();
                }
                ui.separator();

                let mut to_close: Option<usize> = None;
                for i in 0..self.tabs.len() {
                    let selected = i == self.active;
                    let label = {
                        let t = &self.tabs[i];
                        let marker = match &t.status {
                            TabStatus::Connecting => "… ",
                            TabStatus::Connected => "",
                            TabStatus::Closed(_) => "✖ ",
                        };
                        format!("{marker}{}", t.title)
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        self.active = i;
                    }
                    if ui.small_button("×").clicked() {
                        to_close = Some(i);
                    }
                    ui.separator();
                }
                if let Some(i) = to_close {
                    self.close_tab(i);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("A+").clicked() {
                        self.font_size = (self.font_size + 1.0).min(40.0);
                        self.apply_font_size();
                    }
                    if ui.button("A−").clicked() {
                        self.font_size = (self.font_size - 1.0).max(6.0);
                        self.apply_font_size();
                    }
                });
            });
        });
    }

    fn apply_font_size(&mut self) {
        for t in &mut self.tabs {
            t.view.font_size = self.font_size;
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0x1a, 0x1b, 0x26)))
            .show(ctx, |ui| {
                if self.tabs.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("No sessions.\nClick ➕ New to connect.")
                                .size(16.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                    });
                    return;
                }

                let active = self.active.min(self.tabs.len() - 1);
                let id = self.tabs[active].id;

                // 已关闭的会话显示错误。
                // Closed sessions show their error.
                if let TabStatus::Closed(err) = &self.tabs[active].status {
                    let msg = err
                        .clone()
                        .unwrap_or_else(|| "Session closed.".to_string());
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(msg)
                                .size(15.0)
                                .color(egui::Color32::from_rgb(0xf7, 0x76, 0x8e)),
                        );
                    });
                    return;
                }

                let avail = ui.available_size();

                // 按可用区域计算网格尺寸;变化时通知 core。
                // Compute the grid size for the available area and notify core
                // if it changed.
                let (cols, rows) = self.tabs[active].view.grid_size_for(ctx, avail);
                if (cols, rows) != self.tabs[active].last_grid && cols > 0 && rows > 0 {
                    self.tabs[active].last_grid = (cols, rows);
                    self.mgr.send(ToCore::Resize { id, cols, rows });
                }

                // 分配终端区域并捕获焦点/按键。
                // Allocate the terminal area and capture focus/keys.
                let (rect, response) =
                    ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
                let has_focus = response.has_focus() || response.clicked();
                if response.clicked() {
                    response.request_focus();
                }

                // 把键盘输入喂给当前会话。
                // Feed keyboard input to the active session.
                if response.has_focus() {
                    let bytes = ui.input(|i| events_to_bytes(&i.events));
                    if !bytes.is_empty() {
                        self.mgr.send(ToCore::Input { id, data: bytes });
                    }
                    // 鼠标滚轮翻 scrollback。
                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                    if scroll.abs() > 0.0 {
                        let lines = (scroll / 20.0).round() as i32;
                        if lines != 0 {
                            self.mgr.send(ToCore::Scroll { id, delta: lines });
                        }
                    }
                }

                // 绘制快照。
                if let Some(snap) = self.tabs[active].snapshot.clone() {
                    self.tabs[active].view.paint(ui, rect, &snap, has_focus);
                }
            });
    }
}

impl eframe::App for KitonyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 启动期:先处理解锁/设置对话框,解决前不进入主界面。
        // Startup: handle the unlock/setup dialog first; main UI waits until resolved.
        if matches!(self.start, StartState::Pending(_)) {
            self.handle_start_dialog(ctx);
            return;
        }

        self.pump_core_events();

        // 连接对话框。
        // Connect dialog.
        match self.connect_form.show(ctx) {
            ConnectOutcome::Connect {
                params,
                password,
                save,
            } => {
                self.start_connection(params, password, save);
            }
            ConnectOutcome::Cancelled | ConnectOutcome::Pending => {}
        }

        self.side_panel(ctx);
        self.top_bar(ctx);
        self.central(ctx);

        // 任一会话存活时持续重绘,保证输出持续流入。
        // Keep animating while any session is live so output keeps flowing.
        let any_live = self
            .tabs
            .iter()
            .any(|t| !matches!(t.status, TabStatus::Closed(_)));
        if any_live {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
}

// ---- GUI 认证提供者:读取预先提供的机密,不做阻塞式弹窗。 ----
// ---- GUI auth provider: reads pre-supplied secrets, no blocking prompts. ----

struct GuiAuthFactory {
    secrets: SecretStore,
}

impl AuthProviderFactory for GuiAuthFactory {
    fn create(&self, id: SessionId, _params: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(GuiAuth {
            id,
            secrets: self.secrets.clone(),
        })
    }
}

struct GuiAuth {
    id: SessionId,
    secrets: SecretStore,
}

impl AuthProvider for GuiAuth {
    fn password(&mut self, _user: &str, _host: &str) -> Option<String> {
        self.secrets
            .lock()
            .unwrap()
            .get(&self.id)
            .and_then(|s| s.password.clone())
    }

    fn key_passphrase(&mut self, _key_path: &str) -> Option<String> {
        self.secrets
            .lock()
            .unwrap()
            .get(&self.id)
            .and_then(|s| s.key_passphrase.clone())
    }

    fn keyboard_interactive(
        &mut self,
        _name: &str,
        _instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        // 单个非回显提示(常见的 "Password:") 复用密码;更复杂的情况需要真正的对话框。
        // Reuse the password for a single non-echo prompt (common "Password:"
        // keyboard-interactive). Anything more complex needs a real dialog.
        let pw = self
            .secrets
            .lock()
            .unwrap()
            .get(&self.id)
            .and_then(|s| s.password.clone());
        match (prompts.len(), pw) {
            (1, Some(pw)) if !prompts[0].1 => Some(vec![pw]),
            (0, _) => Some(vec![]),
            _ => None,
        }
    }
}

// 显式标注:AuthMethod 仅在 open_with 推断单选时用到,避免未使用告警。
// Explicit reference to AuthMethod (used only in open_with); kept to document intent.
#[allow(dead_code)]
fn _auth_method_is_referenced(_m: AuthMethod) {}
