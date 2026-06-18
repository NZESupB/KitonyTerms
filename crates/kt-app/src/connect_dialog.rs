//! The "New Connection" dialog: collects host/port/user/auth into a
//! [`ConnectParams`], plus an optional password supplied up front (the MVP
//! avoids mid-handshake async prompts by collecting the password here).

use eframe::egui;
use kt_config::{AuthMethod, ConnectParams};
use std::path::PathBuf;

/// Which auth method the user picked in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthChoice {
    Password,
    PublicKey,
    Agent,
}

/// Mutable form state for the connect dialog.
pub struct ConnectForm {
    pub open: bool,
    host: String,
    port: String,
    user: String,
    auth_choice: AuthChoice,
    password: String,
    key_path: String,
    save_session: bool,
    session_name: String,
    error: Option<String>,
}

impl Default for ConnectForm {
    fn default() -> Self {
        Self {
            open: false,
            host: String::new(),
            port: "22".to_string(),
            // 默认使用 root —— 服务器连接最常见的初始用户。
            // Default to root — the most common initial user for server connections.
            user: "root".to_string(),
            auth_choice: AuthChoice::Password,
            password: String::new(),
            key_path: String::new(),
            save_session: false,
            session_name: String::new(),
            error: None,
        }
    }
}

/// Result of showing the dialog for one frame.
pub enum ConnectOutcome {
    /// Still open, nothing decided.
    Pending,
    /// User cancelled.
    Cancelled,
    /// User submitted a valid connection.
    Connect {
        params: ConnectParams,
        password: Option<String>,
        save: Option<String>,
    },
}

impl ConnectForm {
    pub fn open(&mut self) {
        self.reset_to_defaults();
        self.open = true;
        self.error = None;
    }

    /// 用一个已保存会话的字段预填表单(用于侧栏"重连")。
    /// `stored_password` 为已解锁 vault 里的读到的密码(如有)。
    ///
    /// Pre-fill the form from a saved session (sidebar "reconnect").
    /// `stored_password` is the password read from the unlocked vault, if any.
    pub fn open_with(
        &mut self,
        profile: &kt_config::SessionProfile,
        stored_password: Option<String>,
    ) {
        self.host = profile.params.host.clone();
        self.port = profile.params.port.to_string();
        self.user = profile.params.user.clone();
        // 按会话里首个认证方法推断单选。
        // Infer the radio choice from the session's first auth method.
        self.auth_choice = match profile.params.auth.first() {
            Some(AuthMethod::PublicKey { .. }) => AuthChoice::PublicKey,
            Some(AuthMethod::Agent) => AuthChoice::Agent,
            _ => AuthChoice::Password,
        };
        if let Some(AuthMethod::PublicKey { key_path }) = profile.params.auth.first() {
            self.key_path = key_path.display().to_string();
        } else {
            self.key_path.clear();
        }
        self.password = stored_password.unwrap_or_default();
        self.save_session = false;
        self.session_name = String::new();
        self.open = true;
        self.error = None;
    }

    fn reset_to_defaults(&mut self) {
        self.host.clear();
        self.port = "22".to_string();
        self.user = "root".to_string();
        self.auth_choice = AuthChoice::Password;
        self.password.clear();
        self.key_path.clear();
        self.save_session = false;
        self.session_name.clear();
        self.error = None;
    }

    /// Render the dialog. Returns the outcome for this frame.
    pub fn show(&mut self, ctx: &egui::Context) -> ConnectOutcome {
        if !self.open {
            return ConnectOutcome::Pending;
        }

        let mut outcome = ConnectOutcome::Pending;
        let mut open = self.open;

        egui::Window::new("New Connection")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("connect_grid")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Host");
                        ui.text_edit_singleline(&mut self.host);
                        ui.end_row();

                        ui.label("Port");
                        ui.text_edit_singleline(&mut self.port);
                        ui.end_row();

                        ui.label("User");
                        ui.text_edit_singleline(&mut self.user);
                        ui.end_row();

                        ui.label("Auth");
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut self.auth_choice,
                                AuthChoice::Password,
                                "Password",
                            );
                            ui.selectable_value(
                                &mut self.auth_choice,
                                AuthChoice::PublicKey,
                                "Public Key",
                            );
                            ui.selectable_value(
                                &mut self.auth_choice,
                                AuthChoice::Agent,
                                "Agent",
                            );
                        });
                        ui.end_row();

                        match self.auth_choice {
                            AuthChoice::Password => {
                                ui.label("Password");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.password).password(true),
                                );
                                ui.end_row();
                            }
                            AuthChoice::PublicKey => {
                                ui.label("Key file");
                                ui.horizontal(|ui| {
                                    ui.text_edit_singleline(&mut self.key_path);
                                    if ui.button("Browse…").clicked() {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .set_title("Select private key")
                                            .pick_file()
                                        {
                                            self.key_path = path.display().to_string();
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                            AuthChoice::Agent => {}
                        }

                        ui.label("Save session");
                        ui.checkbox(&mut self.save_session, "");
                        ui.end_row();

                        if self.save_session {
                            ui.label("Name");
                            ui.text_edit_singleline(&mut self.session_name);
                            ui.end_row();
                        }
                    });

                if let Some(err) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), err);
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Connect").clicked() {
                        match self.build() {
                            Ok((params, password, save)) => {
                                outcome = ConnectOutcome::Connect {
                                    params,
                                    password,
                                    save,
                                };
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = ConnectOutcome::Cancelled;
                    }
                });
            });

        // Window close button.
        if !open {
            outcome = ConnectOutcome::Cancelled;
        }

        match &outcome {
            ConnectOutcome::Connect { .. } | ConnectOutcome::Cancelled => {
                self.open = false;
            }
            ConnectOutcome::Pending => {
                self.open = open;
            }
        }
        outcome
    }

    /// Validate the form and build a [`ConnectParams`].
    fn build(&self) -> Result<(ConnectParams, Option<String>, Option<String>), String> {
        if self.host.trim().is_empty() {
            return Err("Host is required".into());
        }
        if self.user.trim().is_empty() {
            return Err("User is required".into());
        }
        let port: u16 = self
            .port
            .trim()
            .parse()
            .map_err(|_| "Port must be a number".to_string())?;

        let (auth, password) = match self.auth_choice {
            AuthChoice::Password => {
                if self.password.is_empty() {
                    return Err("Password is required".into());
                }
                (vec![AuthMethod::Password], Some(self.password.clone()))
            }
            AuthChoice::PublicKey => {
                if self.key_path.trim().is_empty() {
                    return Err("Key file is required".into());
                }
                (
                    vec![AuthMethod::PublicKey {
                        key_path: PathBuf::from(self.key_path.trim()),
                    }],
                    None,
                )
            }
            AuthChoice::Agent => (vec![AuthMethod::Agent], None),
        };

        let params = ConnectParams {
            host: self.host.trim().to_string(),
            port,
            user: self.user.trim().to_string(),
            auth,
            vault_id: None,
        };

        let save = if self.save_session && !self.session_name.trim().is_empty() {
            Some(self.session_name.trim().to_string())
        } else {
            None
        };

        Ok((params, password, save))
    }
}
