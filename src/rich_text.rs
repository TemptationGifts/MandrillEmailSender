use crate::mail::Draft;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub size: u16,
    pub color: [u8; 3],
    pub link: String,
    pub image: String,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            size: 16,
            color: [30, 35, 40],
            link: String::new(),
            image: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InlineImage {
    pub cid: String,
    pub name: String,
    pub path: PathBuf,
    pub width: u32,
}

pub fn expand(text: &str, spans: &[Span]) -> Vec<Style> {
    let mut styles = vec![Style::default(); text.chars().count()];
    for span in spans {
        let end = span.end.min(styles.len());
        if span.start < end {
            styles[span.start..end].fill(span.style.clone());
        }
    }
    styles
}

pub fn compress(styles: &[Style]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, style) in styles.iter().enumerate() {
        if let Some(last) = spans.last_mut()
            && last.style == *style
            && last.end == i
        {
            last.end += 1;
        } else {
            spans.push(Span {
                start: i,
                end: i + 1,
                style: style.clone(),
            });
        }
    }
    spans
}

// Reconcile a text edit using Unicode character positions, matching egui cursors.
pub fn edited_styles(before: &str, after: &str, styles: &[Style], active: &Style) -> Vec<Style> {
    let old: Vec<char> = before.chars().collect();
    let new: Vec<char> = after.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut previous = styles.to_vec();
    previous.resize(old.len(), Style::default());
    let mut inserted = active.clone();
    inserted.image.clear();
    let mut result = previous[..prefix].to_vec();
    result.extend(vec![inserted; new.len() - prefix - suffix]);
    result.extend_from_slice(&previous[old.len() - suffix..]);
    // Editing inside an image label turns that label into ordinary text, never
    // silently sends an image whose visible marker the user has changed.
    let mut touched = HashSet::new();
    for style in &previous[prefix..old.len() - suffix] {
        if !style.image.is_empty() {
            touched.insert(style.image.clone());
        }
    }
    if prefix > 0
        && prefix < previous.len()
        && previous[prefix - 1].image == previous[prefix].image
        && !previous[prefix].image.is_empty()
    {
        touched.insert(previous[prefix].image.clone());
    }
    for style in &mut result {
        if touched.contains(&style.image) {
            *style = Style::default();
        }
    }
    result
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn valid_link(value: &str) -> bool {
    if value.contains(['\r', '\n']) {
        return false;
    }
    reqwest::Url::parse(value).is_ok_and(|url| match url.scheme() {
        "http" | "https" => url.host_str().is_some(),
        "mailto" => !url.path().is_empty(),
        _ => false,
    })
}

pub struct Rendered {
    pub html: String,
    pub text: String,
    pub image_ids: HashSet<String>,
}

pub fn render(draft: &Draft) -> Result<Rendered, String> {
    let chars: Vec<char> = draft.body.chars().collect();
    for span in &draft.formatting {
        if span.start >= span.end || span.end > chars.len() {
            return Err(
                "The saved text formatting is invalid. Recreate this message before sending."
                    .into(),
            );
        }
    }
    let spans = compress(&expand(&draft.body, &draft.formatting));
    let mut html = String::from(
        "<!doctype html><html><body><div style=\"font-family:Arial,Helvetica,sans-serif;font-size:16px;line-height:1.5;white-space:pre-wrap;overflow-wrap:break-word;\">",
    );
    let mut text = String::new();
    let mut image_ids = HashSet::new();
    for span in spans {
        let style = &span.style;
        let value: String = chars[span.start..span.end].iter().collect();
        if !style.image.is_empty() {
            let image = draft
                .inline_images
                .iter()
                .find(|i| i.cid == style.image)
                .ok_or("An embedded image is missing. Remove its label or insert it again.")?;
            if image.cid.is_empty()
                || !image
                    .cid
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("An embedded image has an invalid identifier.".into());
            }
            html.push_str(&format!("<img src=\"cid:{}\" alt=\"{}\" width=\"{}\" style=\"display:block;max-width:100%;height:auto;border:0;\">", image.cid, escape(&image.name), image.width.clamp(40, 1200)));
            text.push_str(&format!("[Image: {}]", image.name));
            image_ids.insert(image.cid.clone());
            continue;
        }
        if !style.link.is_empty() && !valid_link(&style.link) {
            return Err("A link must start with https://, http:// or mailto:.".into());
        }
        let [r, g, b] = style.color;
        let size = style.size.clamp(10, 48);
        html.push_str(&format!(
            "<span style=\"font-size:{size}px;color:#{r:02x}{g:02x}{b:02x};\">"
        ));
        if !style.link.is_empty() {
            html.push_str(&format!("<a href=\"{}\">", escape(&style.link)));
        }
        if style.bold {
            html.push_str("<strong>");
        }
        if style.italic {
            html.push_str("<em>");
        }
        if style.underline {
            html.push_str("<u>");
        }
        html.push_str(&escape(&value).replace('\n', "<br>"));
        if style.underline {
            html.push_str("</u>");
        }
        if style.italic {
            html.push_str("</em>");
        }
        if style.bold {
            html.push_str("</strong>");
        }
        if !style.link.is_empty() {
            html.push_str("</a>");
        }
        html.push_str("</span>");
        text.push_str(&value);
        if !style.link.is_empty() && value != style.link {
            text.push_str(&format!(" ({})", style.link));
        }
    }
    html.push_str("</div></body></html>");
    Ok(Rendered {
        html,
        text,
        image_ids,
    })
}

pub fn image_mime(bytes: &[u8]) -> Result<&'static str, String> {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => Ok("image/png"),
        Ok(image::ImageFormat::Jpeg) => Ok("image/jpeg"),
        Ok(image::ImageFormat::Gif) => Ok("image/gif"),
        _ => Err("Insert a PNG, JPEG or GIF image.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_unicode_is_escaped_and_links_are_safe() {
        let mut draft = Draft {
            body: "£10 <sale>\nClick".into(),
            ..Default::default()
        };
        draft.formatting = vec![
            Span {
                start: 0,
                end: 3,
                style: Style {
                    bold: true,
                    italic: true,
                    underline: true,
                    ..Default::default()
                },
            },
            Span {
                start: 11,
                end: 16,
                style: Style {
                    link: "https://example.com/?a=1&b=2".into(),
                    ..Default::default()
                },
            },
        ];
        let out = render(&draft).unwrap();
        assert!(out.html.contains("<strong><em><u>£10</u></em></strong>"));
        assert!(out.html.contains("&lt;sale&gt;<br>"));
        assert!(out.html.contains("a=1&amp;b=2"));
        draft.formatting[1].style.link = "javascript:alert(1)".into();
        assert!(render(&draft).is_err());
    }

    #[test]
    fn unicode_edits_preserve_surrounding_styles() {
        let bold = Style {
            bold: true,
            ..Default::default()
        };
        let old = vec![bold.clone(), bold.clone(), Style::default()];
        let result = edited_styles("£é!", "£newé!", &old, &Style::default());
        assert!(result[0].bold);
        assert!(!result[1].bold);
        assert!(result[4].bold);
        assert_eq!(result.len(), 6);
    }

    #[test]
    fn image_is_embedded_by_content_id_and_has_text_fallback() {
        let draft = Draft {
            body: "[Image]".into(),
            formatting: vec![Span {
                start: 0,
                end: 7,
                style: Style {
                    image: "pic-1".into(),
                    ..Default::default()
                },
            }],
            inline_images: vec![InlineImage {
                cid: "pic-1".into(),
                name: "logo.png".into(),
                path: "unused".into(),
                width: 420,
            }],
            ..Default::default()
        };
        let out = render(&draft).unwrap();
        assert!(out.html.contains("src=\"cid:pic-1\""));
        assert_eq!(out.text, "[Image: logo.png]");
        assert!(out.image_ids.contains("pic-1"));
    }
}
