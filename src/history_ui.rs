use crate::{history, mail::Draft};
use eframe::egui::{self, RichText};
use std::path::PathBuf;

const PAGE_SIZE: usize = 30;

#[derive(Default)]
pub struct HistoryView {
    pub open: bool,
    paths: Vec<PathBuf>,
    records: Vec<history::Record>,
    page: usize,
    selected: Option<history::Record>,
    error: String,
    preview: crate::rich_editor::Preview,
}

impl HistoryView {
    pub fn reload(&mut self) {
        match history::list() {
            Ok(paths) => {
                self.paths = paths;
                self.page = self
                    .page
                    .min(self.paths.len().saturating_sub(1) / PAGE_SIZE);
                self.load_page();
            }
            Err(error) => self.error = format!("Could not load history: {error}"),
        }
    }

    fn load_page(&mut self) {
        self.records.clear();
        self.error.clear();
        for path in self
            .paths
            .iter()
            .skip(self.page * PAGE_SIZE)
            .take(PAGE_SIZE)
        {
            match history::read(path) {
                Ok(record) => self.records.push(record),
                Err(_) => self.error = "One or more history entries could not be read.".into(),
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Draft> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut copy = None;
        egui::Window::new("Email history").open(&mut open).default_size([850.0, 560.0]).resizable(true).show(ctx, |ui| {
            ui.label("Saved on this PC, including copies of attachments. Status is recorded at send time.");
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() { self.reload(); }
                if ui.add_enabled(self.page > 0, egui::Button::new("Newer")).clicked() {
                    self.page -= 1;
                    self.load_page();
                }
                if ui.add_enabled((self.page + 1) * PAGE_SIZE < self.paths.len(), egui::Button::new("Older")).clicked() {
                    self.page += 1;
                    self.load_page();
                }
                ui.label(format!("{} email(s) · Page {}", self.paths.len(), self.page + 1));
            });
            if !self.error.is_empty() { ui.colored_label(egui::Color32::DARK_RED, &self.error); }
            ui.separator();
            if self.paths.is_empty() {
                ui.add_space(20.0);
                ui.label("No emails sent yet. Your sent emails and send attempts will appear here.");
            } else {
                ui.columns(2, |columns| {
                    egui::ScrollArea::vertical().id_salt("history_list").max_height(450.0).show(&mut columns[0], |ui| {
                        for record in &self.records {
                            let selected = self.selected.as_ref().is_some_and(|r| r.created == record.created && r.draft.subject == record.draft.subject);
                            let label = format!("{}\n{}\n{}", record.created, record.draft.subject, record.status);
                            if ui.add(egui::Button::new(label).selected(selected).wrap()).clicked() {
                                self.selected = Some(record.clone());
                            }
                        }
                    });
                    egui::ScrollArea::vertical().id_salt("history_detail").max_height(450.0).show(&mut columns[1], |ui| {
                        if let Some(record) = &self.selected {
                            ui.heading(&record.draft.subject);
                            ui.label(&record.created);
                            ui.label(RichText::new(&record.status).strong());
                            ui.separator();
                            ui.label(format!("From: {} <{}>", record.draft.from_name, record.draft.from_email));
                            for (label, value) in [("To", &record.draft.to), ("CC", &record.draft.cc), ("BCC", &record.draft.bcc), ("Reply to", &record.draft.reply_to)] {
                                if !value.is_empty() { ui.label(format!("{label}: {value}")); }
                            }
                            ui.add_space(5.0);
                            self.preview.show(ui, &record.draft);
                            for row in &record.deliveries {
                                ui.label(format!("{} — {}", row.email, row.description()));
                            }
                            if let Some(error) = &record.error { ui.colored_label(egui::Color32::DARK_RED, error); }
                            if !record.draft.attachments.is_empty() { ui.label(RichText::new("Saved attachments").strong()); }
                            for path in &record.draft.attachments {
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(name.as_ref());
                                    if ui.small_button("Save a copy…").clicked()
                                        && let Some(destination) = rfd::FileDialog::new().set_file_name(name.as_ref()).save_file()
                                        && let Err(error) = std::fs::copy(path, destination)
                                    { self.error = format!("Could not save attachment: {error}"); }
                                });
                            }
                            for image in &record.draft.inline_images {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!("Embedded: {}", image.name));
                                    if ui.small_button("Save image…").clicked()
                                        && let Some(destination) = rfd::FileDialog::new().set_file_name(&image.name).save_file()
                                        && let Err(error) = std::fs::copy(&image.path, destination)
                                    { self.error = format!("Could not save image: {error}"); }
                                });
                            }
                            ui.add_space(8.0);
                            if ui.button("Copy into a new email").clicked() { copy = Some(record.draft.clone()); }
                            ui.small("This creates a draft. Nothing is sent until you click Send email.");
                        } else {
                            ui.label("Select an email to view its message, attachments and send results.");
                        }
                    });
                });
            }
        });
        self.open = open;
        copy
    }
}
