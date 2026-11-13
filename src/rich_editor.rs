use crate::{
    mail::Draft,
    rich_text::{self, InlineImage, Span, Style},
};
use eframe::egui::{
    self, Color32, FontData, FontFamily, FontId, RichText, TextEdit, TextFormat, text::LayoutJob,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        (
            "mail-regular",
            include_bytes!("../assets/Lato-Regular.ttf").as_slice(),
        ),
        (
            "mail-bold",
            include_bytes!("../assets/Lato-Bold.ttf").as_slice(),
        ),
        (
            "mail-italic",
            include_bytes!("../assets/Lato-Italic.ttf").as_slice(),
        ),
        (
            "mail-bold-italic",
            include_bytes!("../assets/Lato-BoldItalic.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(bytes).into());
        let mut family = vec![name.to_owned()];
        family.extend(fonts.families[&FontFamily::Proportional].clone());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    ctx.set_fonts(fonts);
}

fn format(style: &Style) -> TextFormat {
    let family = match (style.bold, style.italic) {
        (true, true) => "mail-bold-italic",
        (true, false) => "mail-bold",
        (false, true) => "mail-italic",
        _ => "mail-regular",
    };
    let color = if !style.image.is_empty() || !style.link.is_empty() {
        Color32::from_rgb(28, 91, 122)
    } else {
        Color32::from_rgb(style.color[0], style.color[1], style.color[2])
    };
    TextFormat {
        font_id: FontId::new(
            style.size.clamp(10, 48) as f32,
            FontFamily::Name(family.into()),
        ),
        color,
        underline: if style.underline || !style.link.is_empty() {
            egui::Stroke::new(1.0_f32, color)
        } else {
            egui::Stroke::NONE
        },
        background: if style.image.is_empty() {
            Color32::TRANSPARENT
        } else {
            Color32::from_rgb(224, 238, 246)
        },
        ..Default::default()
    }
}

fn job(text: &str, styles: &[Style], width: f32) -> LayoutJob {
    let chars: Vec<char> = text.chars().collect();
    let mut layout = LayoutJob::default();
    layout.wrap.max_width = width;
    for span in rich_text::compress(styles) {
        let end = span.end.min(chars.len());
        if span.start < end {
            layout.append(
                &chars[span.start..end].iter().collect::<String>(),
                0.0,
                format(&span.style),
            );
        }
    }
    layout
}

#[derive(Clone)]
struct Snapshot {
    body: String,
    formatting: Vec<Span>,
    images: Vec<InlineImage>,
}

impl Snapshot {
    fn from(draft: &Draft) -> Self {
        Self {
            body: draft.body.clone(),
            formatting: draft.formatting.clone(),
            images: draft.inline_images.clone(),
        }
    }
    fn restore(self, draft: &mut Draft) {
        draft.body = self.body;
        draft.formatting = self.formatting;
        draft.inline_images = self.images;
    }
}

#[derive(Default)]
pub struct Preview {
    textures: HashMap<PathBuf, Result<egui::TextureHandle, String>>,
}

impl Preview {
    pub fn show(&mut self, ui: &mut egui::Ui, draft: &Draft) {
        let chars: Vec<char> = draft.body.chars().collect();
        let spans = rich_text::compress(&rich_text::expand(&draft.body, &draft.formatting));
        let mut layout = LayoutJob::default();
        layout.wrap.max_width = ui.available_width();
        for span in spans {
            if span.style.image.is_empty() {
                layout.append(
                    &chars[span.start..span.end].iter().collect::<String>(),
                    0.0,
                    format(&span.style),
                );
            } else {
                if !layout.text.is_empty() {
                    ui.label(std::mem::take(&mut layout));
                }
                layout.wrap.max_width = ui.available_width();
                if let Some(image) = draft
                    .inline_images
                    .iter()
                    .find(|image| image.cid == span.style.image)
                {
                    let texture = self
                        .textures
                        .entry(image.path.clone())
                        .or_insert_with(|| load_texture(ui.ctx(), &image.path));
                    match texture {
                        Ok(texture) => {
                            let width =
                                (image.width.clamp(40, 1200) as f32).min(ui.available_width());
                            let height =
                                width * texture.size()[1] as f32 / texture.size()[0] as f32;
                            ui.add(egui::Image::new((texture.id(), egui::vec2(width, height))));
                        }
                        Err(error) => {
                            ui.colored_label(Color32::DARK_RED, format!("{}: {error}", image.name));
                        }
                    }
                } else {
                    ui.colored_label(Color32::DARK_RED, "Embedded image missing");
                }
            }
        }
        if !layout.text.is_empty() {
            ui.label(layout);
        }
    }
}

fn load_texture(ctx: &egui::Context, path: &Path) -> Result<egui::TextureHandle, String> {
    let metadata = path.metadata().map_err(|e| e.to_string())?;
    if metadata.len() > crate::mail::MAX_ATTACHMENTS {
        return Err("Image exceeds 6 MB.".into());
    }
    let dimensions =
        image::image_dimensions(path).map_err(|_| "Unable to read this image.".to_owned())?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 as u64 * dimensions.1 as u64 > 24_000_000
    {
        return Err("Image is too large to preview. Resize it below 24 megapixels.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    rich_text::image_mime(&bytes)?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [decoded.width() as usize, decoded.height() as usize],
        decoded.as_raw(),
    );
    Ok(ctx.load_texture(path.to_string_lossy(), color, egui::TextureOptions::LINEAR))
}

#[derive(Default)]
pub struct Editor {
    selection: (usize, usize),
    active: Style,
    preview_mode: bool,
    preview: Preview,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    link_open: bool,
    link_url: String,
    error: String,
    pending_cursor: bool,
}

impl Editor {
    fn save_undo(&mut self, draft: &Draft) {
        self.undo.push(Snapshot::from(draft));
        if self.undo.len() > 50 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn apply(&mut self, draft: &mut Draft, change: impl Fn(&mut Style)) {
        self.save_undo(draft);
        change(&mut self.active);
        let mut styles = rich_text::expand(&draft.body, &draft.formatting);
        let start = self.selection.0.min(styles.len());
        let end = self.selection.1.min(styles.len());
        for style in &mut styles[start..end] {
            if style.image.is_empty() {
                change(style);
            }
        }
        draft.formatting = rich_text::compress(&styles);
    }

    fn insert(&mut self, draft: &mut Draft, text: &str, style: &Style) {
        self.save_undo(draft);
        let mut chars: Vec<char> = draft.body.chars().collect();
        let mut styles = rich_text::expand(&draft.body, &draft.formatting);
        let start = self.selection.0.min(chars.len());
        let end = self.selection.1.min(chars.len());
        let added = text.chars().count();
        chars.splice(start..end, text.chars());
        styles.splice(start..end, vec![style.clone(); added]);
        draft.body = chars.into_iter().collect();
        draft.formatting = rich_text::compress(&styles);
        self.selection = (start + added, start + added);
    }

    fn bullets(&mut self, draft: &mut Draft) {
        self.save_undo(draft);
        let mut chars: Vec<char> = draft.body.chars().collect();
        let mut styles = rich_text::expand(&draft.body, &draft.formatting);
        let start = self.selection.0.min(chars.len());
        let end = self.selection.1.min(chars.len());
        let line_start = chars[..start]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |p| p + 1);
        let mut starts = vec![line_start];
        for (index, c) in chars.iter().enumerate().take(end).skip(line_start) {
            if *c == '\n' && index + 1 < end {
                starts.push(index + 1);
            }
        }
        let remove = starts
            .iter()
            .all(|p| chars.get(*p) == Some(&'•') && chars.get(*p + 1) == Some(&' '));
        for p in starts.iter().rev() {
            if remove {
                chars.drain(*p..*p + 2);
                styles.drain(*p..*p + 2);
            } else {
                chars.splice(*p..*p, ['•', ' ']);
                styles.splice(*p..*p, [self.active.clone(), self.active.clone()]);
            }
        }
        draft.body = chars.into_iter().collect();
        draft.formatting = rich_text::compress(&styles);
        let position = if remove {
            end.saturating_sub(2 * starts.len())
        } else {
            end + 2 * starts.len()
        };
        self.selection = (position, position);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, draft: &mut Draft) {
        let editor_id = ui.make_persistent_id("formatted_body");
        let focused = ui.memory(|memory| memory.has_focus(editor_id));
        let mut force_cursor = std::mem::take(&mut self.pending_cursor);
        let mut undo =
            focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        let mut redo =
            focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Y));
        let bold_key =
            focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::B));
        let italic_key =
            focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::I));
        let underline_key =
            focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::U));
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.preview_mode, |ui| {
                if ui
                    .selectable_label(self.active.bold, RichText::new("B").strong())
                    .on_hover_text("Bold (Ctrl+B)")
                    .clicked()
                    || bold_key
                {
                    let enabled = !self.active.bold;
                    self.apply(draft, |s| s.bold = enabled);
                    force_cursor = true;
                }
                if ui
                    .selectable_label(self.active.italic, RichText::new("I").italics())
                    .on_hover_text("Italic (Ctrl+I)")
                    .clicked()
                    || italic_key
                {
                    let enabled = !self.active.italic;
                    self.apply(draft, |s| s.italic = enabled);
                    force_cursor = true;
                }
                if ui
                    .selectable_label(self.active.underline, RichText::new("U").underline())
                    .on_hover_text("Underline (Ctrl+U)")
                    .clicked()
                    || underline_key
                {
                    let enabled = !self.active.underline;
                    self.apply(draft, |s| s.underline = enabled);
                    force_cursor = true;
                }
                let mut size = self.active.size;
                egui::ComboBox::from_id_salt("text_size")
                    .width(62.0)
                    .selected_text(format!("{} px", size))
                    .show_ui(ui, |ui| {
                        for value in [12, 14, 16, 18, 20, 24, 28, 32, 40, 48] {
                            ui.selectable_value(&mut size, value, format!("{value} px"));
                        }
                    });
                if size != self.active.size {
                    self.apply(draft, |s| s.size = size);
                    force_cursor = true;
                }
                let mut color = self.active.color;
                if ui
                    .color_edit_button_srgb(&mut color)
                    .on_hover_text("Text colour")
                    .changed()
                {
                    self.apply(draft, |s| s.color = color);
                }
                if ui.button("Bullets").clicked() {
                    self.bullets(draft);
                    force_cursor = true;
                }
                if ui.button("Link…").clicked() {
                    self.link_url = self.active.link.clone();
                    self.link_open = true;
                }
                if ui.button("Clear format").clicked() {
                    self.apply(draft, |s| *s = Style::default());
                    force_cursor = true;
                }
                undo |= ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                    .clicked();
                redo |= ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                    .clicked();
            });
            ui.separator();
            ui.selectable_value(&mut self.preview_mode, false, "Write");
            ui.selectable_value(&mut self.preview_mode, true, "Preview");
        });
        if undo && let Some(snapshot) = self.undo.pop() {
            self.redo.push(Snapshot::from(draft));
            snapshot.restore(draft);
            self.selection = (draft.body.chars().count(), draft.body.chars().count());
            force_cursor = true;
        }
        if redo && let Some(snapshot) = self.redo.pop() {
            self.undo.push(Snapshot::from(draft));
            snapshot.restore(draft);
            self.selection = (draft.body.chars().count(), draft.body.chars().count());
            force_cursor = true;
        }
        if self.link_open {
            ui.horizontal(|ui| {
                ui.label("Link URL");
                ui.add(TextEdit::singleline(&mut self.link_url).hint_text("https://…"));
                if ui.button("Apply link").clicked() {
                    let url = self.link_url.trim().to_owned();
                    if rich_text::valid_link(&url) {
                        if self.selection.0 == self.selection.1 {
                            let style = Style {
                                link: url.clone(),
                                ..self.active.clone()
                            };
                            self.insert(draft, &url, &style);
                        } else {
                            self.apply(draft, |s| s.link = url.clone());
                        }
                        self.link_open = false;
                        self.error.clear();
                        force_cursor = true;
                    } else {
                        self.error = "Use an https://, http:// or mailto: link.".into();
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.link_open = false;
                }
            });
        }
        if self.preview_mode {
            egui::Frame::new()
                .fill(Color32::WHITE)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_min_height(190.0);
                    self.preview.show(ui, draft);
                });
        } else {
            let previous = Snapshot::from(draft);
            let styles = rich_text::expand(&previous.body, &previous.formatting);
            let active = self.active.clone();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
                let current_styles =
                    rich_text::edited_styles(&previous.body, text.as_str(), &styles, &active);
                ui.fonts_mut(|fonts| fonts.layout_job(job(text.as_str(), &current_styles, width)))
            };
            let mut output = TextEdit::multiline(&mut draft.body)
                .id(editor_id)
                .desired_width(f32::INFINITY)
                .desired_rows(10)
                .hint_text("Write your message…")
                .layouter(&mut layouter)
                .show(ui);
            if output.response.changed() {
                self.undo.push(previous.clone());
                if self.undo.len() > 50 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                draft.formatting = rich_text::compress(&rich_text::edited_styles(
                    &previous.body,
                    &draft.body,
                    &styles,
                    &self.active,
                ));
            }
            if force_cursor {
                use egui::text::{CCursor, CCursorRange};
                output.state.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(self.selection.0),
                    CCursor::new(self.selection.1),
                )));
                output.state.clear_undoer();
                output.state.store(ui.ctx(), editor_id);
                output.response.request_focus();
            } else if output.response.has_focus()
                && let Some(range) = output.cursor_range
            {
                let selection = (
                    range.primary.index.min(range.secondary.index),
                    range.primary.index.max(range.secondary.index),
                );
                if self.selection != selection {
                    let styles = rich_text::expand(&draft.body, &draft.formatting);
                    let index = if selection.0 == selection.1 {
                        selection.0.saturating_sub(1)
                    } else {
                        selection.0
                    };
                    if let Some(style) = styles.get(index) {
                        self.active = style.clone();
                        self.active.image.clear();
                    }
                }
                self.selection = selection;
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Insert image…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Insert image into email")
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif"])
                    .pick_file()
            {
                match load_texture(ui.ctx(), &path) {
                    Ok(texture) => {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let cid = format!(
                            "image-{}-{}",
                            std::process::id(),
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos()
                        );
                        let width = (texture.size()[0] as u32).clamp(40, 600);
                        let marker = format!("[Image: {name}]");
                        self.insert(
                            draft,
                            &marker,
                            &Style {
                                image: cid.clone(),
                                ..Default::default()
                            },
                        );
                        draft.inline_images.push(InlineImage {
                            cid,
                            name,
                            path: path.clone(),
                            width,
                        });
                        self.preview.textures.insert(path, Ok(texture));
                        self.error.clear();
                        self.pending_cursor = true;
                    }
                    Err(error) => self.error = error,
                }
            }
            ui.small("Select text to format it. Use Preview to see embedded images.");
        });
        let used: std::collections::HashSet<String> = draft
            .formatting
            .iter()
            .filter(|s| !s.style.image.is_empty())
            .map(|s| s.style.image.clone())
            .collect();
        let mut remove = None;
        for image in &mut draft.inline_images {
            if !used.contains(&image.cid) {
                continue;
            }
            ui.push_id(&image.cid, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&image.name);
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut image.width)
                            .range(40..=1200)
                            .suffix(" px"),
                    );
                    if ui.small_button("Remove image").clicked() {
                        remove = Some(image.cid.clone());
                    }
                });
            });
        }
        if let Some(cid) = remove {
            self.save_undo(draft);
            let styles = rich_text::expand(&draft.body, &draft.formatting);
            let (chars, styles): (String, Vec<Style>) = draft
                .body
                .chars()
                .zip(styles)
                .filter(|(_, s)| s.image != cid)
                .unzip();
            draft.body = chars;
            draft.formatting = rich_text::compress(&styles);
            draft.inline_images.retain(|i| i.cid != cid);
            self.selection = (draft.body.chars().count(), draft.body.chars().count());
            self.pending_cursor = true;
        }
        if !self.error.is_empty() {
            ui.colored_label(Color32::DARK_RED, &self.error);
        }
    }
}
