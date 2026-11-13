use crate::mail::{Delivery, Draft};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Record {
    pub created: String,
    pub draft: Draft,
    pub status: String,
    pub deliveries: Vec<Delivery>,
    pub error: Option<String>,
}

pub struct Attempt {
    pub record: Record,
    directory: PathBuf,
}

pub fn root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("MandrillSender").join("history"))
        .ok_or_else(|| "The local history folder is unavailable.".to_owned())
}

fn write_record(directory: &Path, record: &Record) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    let temp = directory.join("record.tmp");
    let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(temp, directory.join("record.json")).map_err(|e| e.to_string())
}

impl Attempt {
    pub fn begin(draft: &Draft, payload: &Value) -> Result<Self, String> {
        Self::begin_at(&root()?, draft, payload)
    }

    fn begin_at(root: &Path, draft: &Draft, payload: &Value) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory = root.join(format!("{stamp:020}-{}", std::process::id()));
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let mut saved_draft = draft.clone();
        saved_draft.attachments.clear();
        if let Some(attachments) = payload["message"]["attachments"].as_array() {
            for (i, attachment) in attachments.iter().enumerate() {
                let name = attachment["name"]
                    .as_str()
                    .ok_or("Invalid attachment name")?;
                // Only names from the validated payload are used; reject paths defensively.
                if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".."
                {
                    return Err("Invalid attachment file name.".to_owned());
                }
                let bytes = STANDARD
                    .decode(attachment["content"].as_str().ok_or("Invalid attachment")?)
                    .map_err(|e| e.to_string())?;
                let folder = directory.join("attachments").join(i.to_string());
                fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                let path = folder.join(name);
                fs::write(&path, bytes).map_err(|e| e.to_string())?;
                saved_draft.attachments.push(path);
            }
        }
        saved_draft.inline_images.retain(|image| {
            payload["message"]["images"]
                .as_array()
                .is_some_and(|images| {
                    images
                        .iter()
                        .any(|item| item["name"].as_str() == Some(image.cid.as_str()))
                })
        });
        for (i, image) in saved_draft.inline_images.iter_mut().enumerate() {
            let encoded = payload["message"]["images"]
                .as_array()
                .and_then(|images| {
                    images
                        .iter()
                        .find(|item| item["name"].as_str() == Some(image.cid.as_str()))
                })
                .and_then(|item| item["content"].as_str())
                .ok_or("Embedded image data is missing")?;
            let bytes = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
            let extension = match crate::rich_text::image_mime(&bytes)? {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                _ => "gif",
            };
            let folder = directory.join("images");
            fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            let path = folder.join(format!("image-{i}.{extension}"));
            fs::write(&path, bytes).map_err(|e| e.to_string())?;
            image.path = path;
        }
        let record = Record {
            created: chrono::Local::now()
                .format("%d %b %Y, %H:%M:%S %:z")
                .to_string(),
            draft: saved_draft,
            status: "Pending / outcome unknown — check Mandrill Activity before retrying".into(),
            deliveries: Vec::new(),
            error: None,
        };
        write_record(&directory, &record)?;
        Ok(Self { record, directory })
    }

    pub fn finish(&mut self, result: &Result<Vec<Delivery>, String>) -> Result<(), String> {
        match result {
            Ok(rows) => {
                let accepted = rows.iter().filter(|r| r.accepted()).count();
                self.record.status =
                    format!("{accepted} accepted, {} failed", rows.len() - accepted);
                self.record.deliveries = rows.clone();
            }
            Err(error) => {
                self.record.status =
                    "Failed / unconfirmed — check Mandrill Activity before retrying".into();
                self.record.error = Some(error.clone());
            }
        }
        write_record(&self.directory, &self.record)
    }
}

pub fn list() -> Result<Vec<PathBuf>, String> {
    let root = root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path().join("record.json");
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    paths.reverse();
    Ok(paths)
}

pub fn read(path: &Path) -> Result<Record, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_message_attachments_and_results_without_api_key() {
        let root =
            std::env::temp_dir().join(format!("mandrill-history-test-{}", std::process::id()));
        let mut draft = Draft {
            from_email: "sender@example.com".into(),
            to: "recipient@example.com".into(),
            subject: "Saved message".into(),
            body: "A message to revisit".into(),
            ..Default::default()
        };
        let mut image_data = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 90, 120, 255]))
            .write_to(&mut image_data, image::ImageFormat::Png)
            .unwrap();
        draft.inline_images = vec![crate::rich_text::InlineImage {
            cid: "archived-image".into(),
            name: "logo.png".into(),
            path: "original-no-longer-present.png".into(),
            width: 320,
        }];
        draft.formatting = vec![crate::rich_text::Span {
            start: 0,
            end: 1,
            style: crate::rich_text::Style {
                image: "archived-image".into(),
                ..Default::default()
            },
        }];
        let payload = serde_json::json!({"key":"never-store-this-key", "message":{"attachments":[
            {"name":"example.txt", "content":STANDARD.encode(b"saved attachment")}
        ],"images":[{"name":"archived-image","content":STANDARD.encode(image_data.get_ref())}]}});
        let mut attempt = Attempt::begin_at(&root, &draft, &payload).unwrap();
        let record_path = attempt.directory.join("record.json");
        assert!(read(&record_path).unwrap().status.contains("unknown"));
        let outcome = Ok(vec![Delivery {
            email: "recipient@example.com".into(),
            status: "queued".into(),
            reject_reason: None,
            queued_reason: Some("attachments".into()),
        }]);
        attempt.finish(&outcome).unwrap();
        let record = read(&record_path).unwrap();
        assert_eq!(record.draft.body, draft.body);
        assert_eq!(record.draft.formatting[0].style.image, "archived-image");
        assert_eq!(
            fs::read(&record.draft.inline_images[0].path).unwrap(),
            *image_data.get_ref()
        );
        assert_eq!(record.deliveries[0].status, "queued");
        assert_eq!(
            fs::read(&record.draft.attachments[0]).unwrap(),
            b"saved attachment"
        );
        assert!(
            !fs::read_to_string(&record_path)
                .unwrap()
                .contains("never-store-this-key")
        );
        // Synthetic test data only, in this test's dedicated temporary directory.
        fs::remove_dir_all(root).unwrap();
    }
}
