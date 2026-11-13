use crate::{
    config::{self, Account},
    mail::{self, SenderDomain},
};
use eframe::egui::{self, Color32, TextEdit};
use std::{sync::mpsc, time::Duration};

enum Completed {
    Key(Result<(), String>),
    Domains(Result<Vec<SenderDomain>, String>),
}

#[derive(Default)]
pub struct AdminView {
    open: bool,
    current: Account,
    replacement_key: String,
    domains_text: String,
    receiver: Option<mpsc::Receiver<Completed>>,
    domain_results: Vec<SenderDomain>,
    status: String,
    error: bool,
}

impl AdminView {
    pub fn open(&mut self, account: &Account) {
        *self = Self {
            open: true,
            current: account.clone(),
            domains_text: account.domains.join("\n"),
            ..Default::default()
        };
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn key(&self) -> Result<String, String> {
        config::validate_key(if self.replacement_key.trim().is_empty() {
            &self.current.api_key
        } else {
            &self.replacement_key
        })
    }

    fn start(&mut self, ctx: &egui::Context, fetch_domains: bool) {
        if self.receiver.is_some() {
            return;
        }
        let key = match self.key() {
            Ok(key) => key,
            Err(error) => {
                self.status = error;
                self.error = true;
                return;
            }
        };
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.error = false;
        self.status = if fetch_domains {
            "Loading sender domains…"
        } else {
            "Testing API key…"
        }
        .into();
        std::thread::spawn(move || {
            let result = if fetch_domains {
                Completed::Domains(mail::domains(&key))
            } else {
                Completed::Key(mail::ping(&key))
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver = None;
                match result {
                    Completed::Key(Ok(())) => {
                        self.status = "API key accepted. No email was sent.".into();
                        self.error = false;
                    }
                    Completed::Key(Err(error)) | Completed::Domains(Err(error)) => {
                        self.status = error;
                        self.error = true;
                    }
                    Completed::Domains(Ok(domains)) => {
                        let ready: Vec<&str> = domains
                            .iter()
                            .filter(|d| d.verified_for_signing())
                            .map(|d| d.domain.as_str())
                            .collect();
                        if ready.is_empty() {
                            self.status="No verified domains with valid signing were returned. The current list has been kept; check domain setup in Mandrill.".into();
                            self.error = true;
                        } else {
                            self.domains_text = ready.join("\n");
                            self.status = format!(
                                "Loaded {} verified domain(s). Edit the list if needed, then Save settings.",
                                ready.len()
                            );
                            self.error = false;
                        }
                        self.domain_results = domains;
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(120)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.status = "The check ended without a result. Try again.".into();
                self.error = true;
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, allow_changes: bool) -> Option<Account> {
        self.poll(ctx);
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut saved = None;
        let mut cancel = false;
        let busy = self.receiver.is_some();
        egui::Window::new("Admin settings").open(&mut open).default_width(600.0).resizable(true).show(ctx,|ui|{
            ui.label("API key and sender domains for this Windows user.");
            ui.small("These settings apply to this profile, including when the app is opened from the shared folder.");
            ui.separator();
            ui.add_enabled_ui(allow_changes && !busy,|ui|{
                ui.label("Mandrill API key");
                let hint=if self.current.api_key.is_empty(){"Enter your organisation's Mandrill API key"}else{"A key is configured. Enter a replacement to change it"};
                if ui.add(TextEdit::singleline(&mut self.replacement_key).password(true).hint_text(hint).desired_width(f32::INFINITY)).changed(){
                    self.domain_results.clear();
                    if self.replacement_key.trim().is_empty(){self.domains_text=self.current.domains.join("\n");}
                    else{self.domains_text.clear();}
                    self.status="Load or enter the sender domains for this key before saving.".into();self.error=false;
                }
                ui.horizontal(|ui|{
                    if ui.button("Test key").clicked(){self.start(ctx,false);}
                    if ui.button("Load domains from API key").clicked(){self.start(ctx,true);}
                });
                ui.add_space(6.0);
                ui.label("Domains shown in the From dropdown");
                ui.small("One domain per line. You can add, remove or reorder these manually.");
                ui.add(TextEdit::multiline(&mut self.domains_text).desired_rows(6).desired_width(f32::INFINITY).hint_text("example.com\nexample.org"));
                ui.small("Loading uses verified domains with valid signing. Manual entries still need to be set up in Mandrill.");
            });
            if !self.domain_results.is_empty(){
                egui::CollapsingHeader::new("Domains returned by Mandrill").show(ui,|ui|{
                    for domain in &self.domain_results {
                        ui.label(format!("{} — {}",domain.domain,if domain.verified_for_signing(){"verified, signing valid"}else{"not verified or signing unavailable"}));
                    }
                });
            }
            if busy{ui.spinner();}
            if !self.status.is_empty(){ui.colored_label(if self.error{Color32::DARK_RED}else{Color32::from_rgb(60,90,100)},&self.status);}
            ui.separator();
            ui.horizontal(|ui|{
                if ui.add_enabled(allow_changes && !busy,egui::Button::new("Save settings")).clicked(){
                    let result=self.key().and_then(|key|config::parse_domains(&self.domains_text).map(|domains|Account{api_key:key,domains}))
                        .and_then(|account|config::save(&account).map(|()|account));
                    match result {Ok(account)=>saved=Some(account),Err(error)=>{self.status=error;self.error=true;}}
                }
                if ui.button("Cancel").clicked(){cancel=true;}
                ui.small("The saved key is encrypted using your Windows account.");
            });
        });
        self.open = open && !cancel && saved.is_none();
        if !self.open {
            self.replacement_key.clear();
            self.current.api_key.clear();
            self.receiver = None;
        }
        saved
    }
}
