#![windows_subsystem = "windows"]

mod admin;
mod config;
mod history;
mod history_ui;
mod mail;
mod rich_editor;
mod rich_text;
use history_ui::HistoryView;

use eframe::egui::{self, Color32, RichText, TextEdit};
use mail::{Delivery, Draft};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::mpsc, time::Duration};

const ACCENT: Color32 = Color32::from_rgb(28, 91, 122);
const MUTED: Color32 = Color32::from_rgb(99, 113, 127);

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct Defaults {
    from_email: String,
    from_name: String,
    reply_to: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            from_email: String::new(),
            from_name: String::new(),
            reply_to: String::new(),
        }
    }
}

fn settings_path() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| {
            PathBuf::from(p)
                .join("MandrillSender")
                .join("settings.json")
        })
        .ok_or_else(|| "Windows settings folder is unavailable.".to_owned())
}

fn load_defaults() -> Result<Defaults, String> {
    let path = settings_path()?;
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            "Saved sender settings could not be read. Enter your From address again.".to_owned()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Defaults::default()),
        Err(_) => Err(
            "Saved sender settings could not be opened. Enter your From address again.".to_owned(),
        ),
    }
}

fn save_defaults(draft: &Draft) -> Result<(), String> {
    let path = settings_path()?;
    let defaults = Defaults {
        from_email: draft.from_email.trim().to_owned(),
        from_name: draft.from_name.trim().to_owned(),
        reply_to: draft.reply_to.trim().to_owned(),
    };
    let bytes = serde_json::to_vec_pretty(&defaults).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}

enum Completed {
    Send(mail::SendOutcome),
    Ping(Result<(), String>),
}

struct SenderApp {
    draft: Draft,
    receiver: Option<mpsc::Receiver<Completed>>,
    sending: bool,
    attempted: bool,
    notice: String,
    error: bool,
    results: Vec<Delivery>,
    show_results: bool,
    history_view: HistoryView,
    editor: rich_editor::Editor,
    about_open: bool,
    account: config::Account,
    admin: admin::AdminView,
}

impl SenderApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        rich_editor::install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.visuals.panel_fill = Color32::from_rgb(247, 249, 251);
        style.visuals.selection.bg_fill = ACCENT;
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        cc.egui_ctx.set_style(style);
        let (defaults, mut notice) = match load_defaults() {
            Ok(value) => (
                value,
                "Ready. Enter your From address and compose an email.".to_owned(),
            ),
            Err(message) => (Defaults::default(), message),
        };
        let (account, admin_error) = match config::load() {
            Ok(value) => (value, false),
            Err(error) => {
                notice = error;
                (config::Account::default(), true)
            }
        };
        if account.api_key.is_empty() && !admin_error {
            notice = "Open Admin to set your Mandrill API key and sender domains.".into();
        }
        Self {
            draft: Draft {
                from_email: defaults.from_email,
                from_name: defaults.from_name,
                reply_to: defaults.reply_to,
                ..Default::default()
            },
            receiver: None,
            sending: false,
            attempted: false,
            notice,
            error: admin_error,
            results: Vec::new(),
            show_results: false,
            history_view: HistoryView::default(),
            editor: rich_editor::Editor::default(),
            about_open: false,
            account,
            admin: admin::AdminView::default(),
        }
    }

    fn fail(&mut self, text: String) {
        self.notice = text;
        self.error = true;
    }

    fn send(&mut self, ctx: &egui::Context) {
        if self.receiver.is_some() || self.attempted {
            return;
        }
        if let Err(message) = config::validate_key(&self.account.api_key) {
            self.fail(message);
            return;
        }
        if let Err(message) = mail::validate(&self.draft, &self.account.domains) {
            self.fail(message);
            return;
        }
        if let Err(message) = save_defaults(&self.draft) {
            self.fail(format!("Could not save sender settings: {message}"));
            return;
        }
        let draft = self.draft.clone();
        let account = self.account.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.sending = true;
        self.attempted = true;
        self.error = false;
        self.results.clear();
        self.notice = "Sending to Mandrill…".into();
        std::thread::spawn(move || {
            let result = mail::send(draft, account);
            let _ = tx.send(Completed::Send(result));
            ctx.request_repaint();
        });
    }

    fn ping(&mut self, ctx: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        let ctx = ctx.clone();
        let key = self.account.api_key.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.sending = false;
        self.notice = "Checking Mandrill connection…".into();
        self.error = false;
        std::thread::spawn(move || {
            let _ = tx.send(Completed::Ping(mail::ping(&key)));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(completed) => {
                self.receiver = None;
                self.sending = false;
                match completed {
                    Completed::Ping(Ok(())) => {
                        self.notice =
                            "Connected to Mandrill. API key accepted; no email sent.".into();
                        self.error = false;
                    }
                    Completed::Ping(Err(message)) => self.fail(message),
                    Completed::Send(outcome) => {
                        match outcome.result {
                            Err(message) => self.fail(message),
                            Ok(results) => {
                                let accepted = results.iter().filter(|r| r.accepted()).count();
                                let failed = results.len() - accepted;
                                self.notice = if failed == 0 {
                                    format!(
                                        "Mandrill accepted all {accepted} recipient(s). See results for sent / queued status."
                                    )
                                } else {
                                    format!(
                                        "{accepted} recipient(s) accepted; {failed} failed. Review results before retrying."
                                    )
                                };
                                self.error = failed != 0;
                                self.results = results;
                                self.show_results = true;
                            }
                        }
                        if let Some(warning) = outcome.history_warning {
                            self.notice.push_str(&format!("\n{warning}"));
                            self.error = true;
                        }
                        self.history_view.reload();
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.sending = false;
                self.fail("The request ended without a result. If sending, check Mandrill Activity before retrying.".into());
            }
        }
    }

    fn has_message(&self) -> bool {
        !self.draft.to.is_empty()
            || !self.draft.cc.is_empty()
            || !self.draft.bcc.is_empty()
            || !self.draft.subject.is_empty()
            || !self.draft.body.is_empty()
            || !self.draft.attachments.is_empty()
    }

    fn confirm(title: &str, message: &str) -> bool {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(message)
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    fn new_email(&mut self) {
        let all_accepted = !self.results.is_empty() && self.results.iter().all(|r| r.accepted());
        if self.has_message()
            && !all_accepted
            && !Self::confirm(
                "New email",
                "Clear the current message and start a new email?",
            )
        {
            return;
        }
        self.draft = Draft {
            from_email: self.draft.from_email.clone(),
            from_name: self.draft.from_name.clone(),
            reply_to: self.draft.reply_to.clone(),
            ..Default::default()
        };
        self.attempted = false;
        self.results.clear();
        self.show_results = false;
        self.error = false;
        self.notice = "Ready to compose a new email.".into();
        self.editor = rich_editor::Editor::default();
    }

    fn compose(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("addresses").num_columns(2).spacing([16.0, 9.0]).show(ui, |ui| {
            sender_field(ui, &mut self.draft.from_email, &self.account.domains);
            field(ui, "Sender name", &mut self.draft.from_name, "Optional display name", false);
            field(ui, "Reply to", &mut self.draft.reply_to, "Optional — defaults to the From address", false);
            ui.label("");
            ui.label(RichText::new("From must use a sending domain verified in your Mandrill account.").size(12.0).color(MUTED));
            ui.end_row();
            field(ui, "To", &mut self.draft.to, "alice@example.com; bob@example.com", true);
            field(ui, "CC", &mut self.draft.cc, "Optional copy recipients", true);
            field(ui, "BCC", &mut self.draft.bcc, "Optional hidden recipients", true);
            ui.label("");
            ui.label(RichText::new("Separate addresses with commas, semicolons or new lines. Use email addresses only.").size(12.0).color(MUTED));
            ui.end_row();
            field(ui, "Subject", &mut self.draft.subject, "Email subject", false);
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Message").strong());
        self.editor.show(ui, &mut self.draft);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Attachments").strong());
            if ui.button("Add files…").clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .set_title("Choose attachments")
                    .pick_files()
            {
                for file in files {
                    if !self.draft.attachments.contains(&file) {
                        self.draft.attachments.push(file);
                    }
                }
            }
            ui.label(
                RichText::new("6 MB total, including embedded images")
                    .size(12.0)
                    .color(MUTED),
            );
        });
        if self.draft.attachments.is_empty() {
            ui.label(RichText::new("No files attached").size(13.0).color(MUTED));
        }
        let mut remove = None;
        for (index, path) in self.draft.attachments.iter().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    let size = match path.metadata() {
                        Ok(m) => format!("{:.1} KB", m.len() as f64 / 1024.0),
                        Err(_) => "file unavailable".to_owned(),
                    };
                    ui.label(format!("{name}   ({size})"))
                        .on_hover_text(path.display().to_string());
                });
            });
        }
        if let Some(index) = remove {
            self.draft.attachments.remove(index);
        }
    }
}

fn sender_field(ui: &mut egui::Ui, value: &mut String, domains: &[String]) {
    let label = ui.label(RichText::new("From").strong());
    let (local, saved_domain) = value.rsplit_once('@').unwrap_or((value.as_str(), ""));
    let mut local = local.to_owned();
    let mut domain = domains
        .iter()
        .map(String::as_str)
        .find(|allowed| saved_domain.eq_ignore_ascii_case(allowed))
        .unwrap_or("");
    let mut changed = false;
    ui.horizontal(|ui| {
        let local_width = (ui.available_width() - 270.0).max(80.0);
        changed |= ui
            .add(
                TextEdit::singleline(&mut local)
                    .hint_text("email")
                    .desired_width(local_width)
                    .font(egui::TextStyle::Body),
            )
            .labelled_by(label.id)
            .changed();
        ui.label("@");
        egui::ComboBox::from_id_salt("sender_domain")
            .width(230.0)
            .selected_text(if domain.is_empty() {
                "Choose domain"
            } else {
                domain
            })
            .show_ui(ui, |ui| {
                for allowed in domains {
                    changed |= ui
                        .selectable_value(&mut domain, allowed.as_str(), allowed)
                        .changed();
                }
            });
    });
    // Keep saved addresses intact until edited, including old history entries.
    // Unsupported domains require an explicit selection before sending.
    if changed {
        *value = format!("{local}@{domain}");
    }
    ui.end_row();
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, multiline: bool) {
    let label_response = ui.label(RichText::new(label).strong());
    let edit = if multiline {
        TextEdit::multiline(value).desired_rows(2)
    } else {
        TextEdit::singleline(value)
    };
    ui.add(
        edit.hint_text(hint)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Body),
    )
    .labelled_by(label_response.id);
    ui.end_row();
}

impl eframe::App for SenderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.receiver.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.fail("Please wait for the current request to finish before closing.".into());
            } else if self.has_message()
                && !self.results.iter().any(|r| r.accepted())
                && !Self::confirm(
                    "Close Mandrill Sender",
                    "Close and discard this message? Drafts are not saved.",
                )
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                let _ = save_defaults(&self.draft);
            }
        }
        let busy = self.receiver.is_some();
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(20))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Mandrill Sender")
                                .size(25.0)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.label(
                            RichText::new("Compose and send an email")
                                .size(13.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("About").clicked() {
                            self.about_open = true;
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Admin")).clicked() {
                            self.admin.open(&self.account);
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("History"))
                            .clicked()
                        {
                            self.history_view.reload();
                            self.history_view.open = true;
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Check connection"))
                            .clicked()
                        {
                            self.ping(ctx);
                        }
                    });
                });
            });
        egui::TopBottomPanel::bottom("footer").frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(16)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let label = if self.sending { "Sending…" } else { "Send email" };
                if ui.add_enabled(!busy && !self.attempted && !self.admin.is_open() && !self.account.api_key.is_empty(), egui::Button::new(RichText::new(label).color(Color32::WHITE))
                    .fill(ACCENT).min_size(egui::vec2(130.0, 38.0))).clicked() { self.send(ctx); }
                if ui.add_enabled(!busy, egui::Button::new("New email")).clicked() { self.new_email(); }
                if self.attempted && !busy && ui.button("Edit & send again").clicked()
                    && Self::confirm("Prepare another send", "Some recipients may already have received this email. Check Mandrill Activity and remove successful recipients before retrying. Enable sending again?")
                {
                    self.attempted = false;
                    self.notice = "Ready to edit. Remove any recipients who already received the email before sending again.".into();
                }
                if !self.results.is_empty() && ui.button("View results").clicked() { self.show_results = true; }
                if busy { ui.spinner(); }
            });
            let color = if self.error { Color32::from_rgb(169, 51, 48) } else { MUTED };
            ui.label(RichText::new(&self.notice).size(13.0).color(color));
        });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(247, 249, 251))
                    .inner_margin(20),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| self.compose(ui));
                });
            });
        if !busy && let Some(draft) = self.history_view.show(ctx) {
            if !self.has_message()
                || Self::confirm(
                    "Load previous email",
                    "Replace the current message with a copy of this saved email? Nothing will be sent yet.",
                )
            {
                self.draft = draft;
                self.editor = rich_editor::Editor::default();
                self.attempted = false;
                self.results.clear();
                self.show_results = false;
                self.history_view.open = false;
                self.notice =
                    "Copied from history. Review the recipients and message before sending.".into();
                self.error = false;
            }
        }
        if self.show_results {
            egui::Window::new("Send results").open(&mut self.show_results).default_width(540.0).resizable(true).show(ctx, |ui| {
                ui.label("Sent and queued are Mandrill statuses, not confirmation of inbox delivery.");
                if self.results.iter().any(|r| !r.accepted()) {
                    ui.colored_label(Color32::from_rgb(169, 51, 48), "Retry only failed recipients to avoid duplicate emails.");
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
                    for row in &self.results {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&row.email).strong());
                            ui.label(row.description());
                        });
                    }
                });
            });
        }
        if let Some(account) = self.admin.show(ctx, !busy) {
            self.account = account;
            self.notice = "Admin settings saved for this Windows user.".into();
            self.error = false;
        }
        if self.about_open {
            egui::Window::new("About Mandrill Sender")
                .open(&mut self.about_open)
                .default_width(560.0)
                .show(ctx, |ui| {
                    ui.label("Mandrill Sender 0.3 — standalone Windows email sender");
                    ui.label("Includes Lato fonts, licensed under the SIL Open Font License.");
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.label(include_str!("../assets/OFL.txt"));
                        });
                });
        }
    }
}

fn main() -> eframe::Result {
    // Read-only authentication smoke test; never invokes messages/send.
    if std::env::args().any(|arg| arg == "--check-connection") {
        match config::load().and_then(|account| mail::ping(&account.api_key)) {
            Ok(()) => std::process::exit(0),
            Err(_) => std::process::exit(1),
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mandrill Sender")
            .with_inner_size([900.0, 900.0])
            .with_min_inner_size([720.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Mandrill Sender",
        options,
        Box::new(|cc| Ok(Box::new(SenderApp::new(cc)))),
    )
}
