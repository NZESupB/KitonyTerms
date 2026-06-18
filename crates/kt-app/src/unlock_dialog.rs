//! 主密码解锁 / 首次设置对话框。
//!
//! Master-password unlock / first-run setup dialog.
//!
//! - 首次运行(vault 文件不存在):要求输入并确认主密码 → 创建 vault。
//! - 后续运行(vault 已存在):要求输入主密码 → 解锁。
//! - 解锁前可跳过(Skip):仍可连接,但不能读写保存的密码。

use eframe::egui;

use crate::store::UnlockOutcome;

/// 对话框输出的动作。
/// Action emitted by the dialog.
pub enum UnlockAction {
    /// 还在输入,没有动作。
    /// Still open, no decision yet.
    Pending,
    /// 用户跳过解锁(可继续用,仅无密码存取)。
    /// User skipped unlocking.
    Skipped,
    /// 用户提交了主密码(由调用方据此尝试 create/unlock)。
    /// User submitted a master password; caller should create/unlock.
    Submit(String),
}

/// 对话框状态。
/// Dialog state.
pub struct UnlockDialog {
    pub open: bool,
    is_setup: bool,
    password: String,
    confirm: String,
    error: Option<String>,
}

impl UnlockDialog {
    pub fn new_setup() -> Self {
        Self {
            open: true,
            is_setup: true,
            password: String::new(),
            confirm: String::new(),
            error: None,
        }
    }

    pub fn new_unlock() -> Self {
        Self {
            open: true,
            is_setup: false,
            password: String::new(),
            confirm: String::new(),
            error: None,
        }
    }

    /// 渲染一帧并返回动作。`failed` 用于把上一次失败结果显示进去。
    /// Render one frame and return the action.
    pub fn show(&mut self, ctx: &egui::Context) -> UnlockAction {
        if !self.open {
            return UnlockAction::Pending;
        }

        let mut action = UnlockAction::Pending;
        let mut open = self.open;
        let title = if self.is_setup {
            "Set Master Password / 设置主密码"
        } else {
            "Unlock / 解锁"
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if self.is_setup {
                    ui.label(
                        "Choose a master password to encrypt saved passwords. \
                         This cannot be recovered if lost.\n\
                         设置一个主密码用于加密保存的密码。该密码丢失后无法找回。",
                    );
                } else {
                    ui.label("Enter your master password to unlock saved passwords.\n输入主密码以解锁已保存的密码。");
                }
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("Master password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .hint_text("••••••••"),
                    );
                });
                if self.is_setup {
                    ui.horizontal(|ui| {
                        ui.label("Confirm");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.confirm)
                                .password(true)
                                .hint_text("••••••••"),
                        );
                    });
                }

                if let Some(err) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), err);
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let enabled = !self.password.is_empty()
                        && (!self.is_setup || self.password == self.confirm);
                    if ui.add_enabled(enabled, egui::Button::new("OK")).clicked() {
                        action = UnlockAction::Submit(self.password.clone());
                    }
                    // 跳过:解锁步骤才允许(首次设置也可跳过,延迟到首次保存密码)。
                    if ui.button("Skip / 跳过").clicked() {
                        action = UnlockAction::Skipped;
                    }
                });
            });

        if !open {
            action = UnlockAction::Skipped;
        }

        match &action {
            UnlockAction::Submit(_) | UnlockAction::Skipped => self.open = false,
            UnlockAction::Pending => self.open = open,
        }
        action
    }

    /// 把一次失败的解锁结果转成可显示的错误文案。
    /// Convert a failed unlock outcome into display text.
    pub fn set_failure(&mut self, outcome: &UnlockOutcome) {
        self.error = Some(match outcome {
            UnlockOutcome::BadPassword => "Wrong password / 密码错误".to_string(),
            UnlockOutcome::Error(e) => format!("Error / 错误: {e}"),
            UnlockOutcome::NeedSetup => "No vault yet — set a password / 尚未设置主密码".to_string(),
            UnlockOutcome::Ok => return,
        });
        self.open = true;
    }
}
