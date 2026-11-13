use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, fs::File, io::Read, path::PathBuf, time::Duration};

const API_ROOT: &str = "https://mandrillapp.com/api/1.4";
// Leaves room for base64 and both bodies under Mandrill's 10 MB JSON limit.
pub const MAX_ATTACHMENTS: u64 = 6_000_000;
const MAX_BODY: usize = 1_000_000;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Draft {
    pub from_email: String,
    pub from_name: String,
    pub reply_to: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<PathBuf>,
    #[serde(default)]
    pub formatting: Vec<crate::rich_text::Span>,
    #[serde(default)]
    pub inline_images: Vec<crate::rich_text::InlineImage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Recipient {
    pub email: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Delivery {
    pub email: String,
    pub status: String,
    #[serde(default)]
    pub reject_reason: Option<String>,
    #[serde(default)]
    pub queued_reason: Option<String>,
}

impl Delivery {
    pub fn accepted(&self) -> bool {
        matches!(self.status.as_str(), "sent" | "queued" | "scheduled")
    }

    pub fn description(&self) -> String {
        let label = match self.status.as_str() {
            "sent" => "Sent",
            "queued" => "Queued",
            "scheduled" => "Scheduled",
            "rejected" => "Rejected",
            "invalid" => "Invalid address",
            _ => "Unknown status",
        };
        match self.reject_reason.as_ref().or(self.queued_reason.as_ref()) {
            Some(reason) if !reason.is_empty() => format!("{label} ({reason})"),
            _ => label.to_owned(),
        }
    }
}

pub fn address(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.contains(['\r', '\n'])
        || !email_address::EmailAddress::is_valid(value)
    {
        return Err(format!("Enter a valid email address in {label}."));
    }
    Ok(value.to_owned())
}

pub fn recipients(draft: &Draft) -> Result<Vec<Recipient>, String> {
    let mut recipients = Vec::new();
    let mut seen = HashMap::new();
    for (label, kind, input) in [
        ("To", "to", &draft.to),
        ("CC", "cc", &draft.cc),
        ("BCC", "bcc", &draft.bcc),
    ] {
        for part in input
            .split([',', ';', '\n', '\r'])
            .filter(|s| !s.trim().is_empty())
        {
            let email = address(part, label)?;
            let key = email.to_lowercase();
            if let Some(previous) = seen.get(&key) {
                if *previous != label {
                    return Err(format!(
                        "{email} appears in both {previous} and {label}. Keep it in just one field."
                    ));
                }
                continue;
            }
            seen.insert(key, label);
            recipients.push(Recipient {
                email,
                kind: kind.to_owned(),
            });
        }
    }
    if recipients.is_empty() {
        return Err("Enter at least one recipient in To, CC or BCC.".to_owned());
    }
    Ok(recipients)
}

pub fn validate(draft: &Draft, domains: &[String]) -> Result<Vec<Recipient>, String> {
    let from = address(&draft.from_email, "From")?;
    let (local, domain) = from.rsplit_once('@').ok_or("Enter a valid From address.")?;
    if local.contains('@')
        || !domains
            .iter()
            .any(|allowed| domain.eq_ignore_ascii_case(allowed))
    {
        return Err("Choose a From domain configured in Admin.".to_owned());
    }
    if !draft.reply_to.trim().is_empty() {
        address(&draft.reply_to, "Reply to")?;
    }
    if draft.from_name.contains(['\r', '\n']) || draft.subject.contains(['\r', '\n']) {
        return Err("Sender name and subject must each be a single line.".to_owned());
    }
    if draft.subject.trim().is_empty() {
        return Err("Add a subject before sending.".to_owned());
    }
    if draft.body.trim().is_empty() {
        return Err("Write a message before sending.".to_owned());
    }
    if draft.body.len() > MAX_BODY {
        return Err("The message body is too large (maximum 1 MB).".to_owned());
    }
    recipients(draft)
}

pub fn payload(draft: &Draft, api_key: &str, domains: &[String]) -> Result<Value, String> {
    let recipients = validate(draft, domains)?;
    let mut attachments = Vec::new();
    let mut total = 0u64;
    for path in &draft.attachments {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "An attachment has an unreadable file name.".to_owned())?;
        if name.contains(['\r', '\n']) {
            return Err("An attachment file name contains a line break.".to_owned());
        }
        let file = File::open(path).map_err(|e| format!("Cannot open {name}: {e}"))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Cannot read {name}: {e}"))?;
        if !metadata.is_file() {
            return Err(format!("{name} is not a regular file."));
        }
        if metadata.len() > MAX_ATTACHMENTS - total {
            return Err(
                "Attachments and embedded images exceed 6 MB in total. Remove a file or use smaller files.".to_owned(),
            );
        }
        // Bounded even if a file grows after metadata is read.
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENTS - total + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read {name}: {e}"))?;
        total += bytes.len() as u64;
        if total > MAX_ATTACHMENTS {
            return Err(
                "Attachments and embedded images exceed 6 MB in total. Remove a file or use smaller files.".to_owned(),
            );
        }
        attachments.push(json!({
            "type": mime_guess::from_path(path).first_or_octet_stream().to_string(),
            "name": name,
            "content": STANDARD.encode(bytes),
        }));
    }
    let rendered = crate::rich_text::render(draft)?;
    let mut images = Vec::new();
    let mut seen_images = std::collections::HashSet::new();
    for image in &draft.inline_images {
        if !rendered.image_ids.contains(&image.cid) {
            continue;
        }
        if !seen_images.insert(image.cid.clone()) {
            return Err("Duplicate embedded image identifier.".into());
        }
        let file = File::open(&image.path)
            .map_err(|e| format!("Cannot open embedded image {}: {e}", image.name))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_ATTACHMENTS - total {
            return Err(
                "Attachments and embedded images must be regular files totalling at most 6 MB."
                    .into(),
            );
        }
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENTS - total + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        if total > MAX_ATTACHMENTS {
            return Err("Attachments and embedded images exceed 6 MB in total.".into());
        }
        let mime = crate::rich_text::image_mime(&bytes)?;
        images.push(json!({"type":mime,"name":image.cid,"content":STANDARD.encode(bytes)}));
    }
    let mut message = json!({
        "from_email": draft.from_email.trim(),
        "from_name": draft.from_name.trim(),
        "to": recipients,
        "subject": draft.subject.trim(),
        "text": rendered.text,
        "html": rendered.html,
        "preserve_recipients": true,
        "attachments": attachments,
        "images": images,
    });
    if !draft.reply_to.trim().is_empty() {
        message["headers"] = json!({"Reply-To": draft.reply_to.trim()});
    }
    let payload = json!({ "key": api_key.trim(), "message": message, "async": false });
    // Mandrill also limits the API JSON request to 10 MB, below the 25 MB MIME limit.
    if serde_json::to_vec(&payload)
        .map_err(|e| e.to_string())?
        .len()
        >= 9_500_000
    {
        return Err("This email is too large for Mandrill's API. Reduce its images, attachments or formatted text.".into());
    }
    Ok(payload)
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialise the secure connection.".to_owned())
}

fn request(endpoint: &str, body: &Value) -> Result<Value, String> {
    let response = client()?.post(format!("{API_ROOT}/{endpoint}"))
        .json(body).send().map_err(|_| {
            "No confirmed response from Mandrill. Check your connection and office IP access. If sending, check Mandrill Activity before retrying: the email may already have been accepted.".to_owned()
        })?;
    let status = response.status();
    let value: Value = response.json().map_err(|_| {
        "Mandrill returned an unreadable response. If sending, check Mandrill Activity before retrying.".to_owned()
    })?;
    if !status.is_success() || value.get("status").and_then(Value::as_str) == Some("error") {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The request could not be completed.");
        let key = body.get("key").and_then(Value::as_str).unwrap_or("");
        let message = if key.is_empty() {
            message.to_owned()
        } else {
            message.replace(key, "[hidden]")
        };
        return Err(format!(
            "Mandrill: {message}\nIf sending, check Mandrill Activity before retrying."
        ));
    }
    Ok(value)
}

pub fn interpret(value: Value, expected: &[Recipient]) -> Result<Vec<Delivery>, String> {
    let rows: Vec<Delivery> = serde_json::from_value(value).map_err(|_| {
        "Unexpected send response. Check Mandrill Activity before retrying.".to_owned()
    })?;
    let mut returned = HashMap::new();
    for row in &rows {
        if !matches!(
            row.status.as_str(),
            "sent" | "queued" | "scheduled" | "rejected" | "invalid"
        ) || returned.insert(row.email.to_lowercase(), ()).is_some()
        {
            return Err(
                "Unrecognised send results. Check Mandrill Activity before retrying.".to_owned(),
            );
        }
    }
    if rows.len() != expected.len()
        || expected
            .iter()
            .any(|r| !returned.contains_key(&r.email.to_lowercase()))
    {
        return Err("Incomplete send results. Some recipients may already have been accepted. Check Mandrill Activity before retrying.".to_owned());
    }
    Ok(rows)
}

pub struct SendOutcome {
    pub result: Result<Vec<Delivery>, String>,
    pub history_warning: Option<String>,
}

pub fn send(draft: Draft, account: crate::config::Account) -> SendOutcome {
    let result = send_recorded(draft, account);
    match result {
        Ok(outcome) => outcome,
        Err(error) => SendOutcome {
            result: Err(error),
            history_warning: None,
        },
    }
}

fn send_recorded(draft: Draft, account: crate::config::Account) -> Result<SendOutcome, String> {
    let key = crate::config::validate_key(&account.api_key)?;
    let recipients = validate(&draft, &account.domains)?;
    let body = payload(&draft, &key, &account.domains)?;
    let mut attempt = crate::history::Attempt::begin(&draft, &body)
        .map_err(|e| format!("Email was not submitted because history could not be saved: {e}"))?;
    let result = request("messages/send", &body).and_then(|value| interpret(value, &recipients));
    let history_warning = attempt.finish(&result).err()
        .map(|e| format!("The email is in History, but its final status could not be saved: {e}. Check Mandrill Activity before retrying."));
    Ok(SendOutcome {
        result,
        history_warning,
    })
}

pub fn ping(key: &str) -> Result<(), String> {
    let key = crate::config::validate_key(key)?;
    let result = request("users/ping", &json!({"key": key}))?;
    if result.as_str() == Some("PONG!") {
        Ok(())
    } else {
        Err("Unexpected connection-check response from Mandrill.".to_owned())
    }
}

#[derive(Clone, Deserialize)]
pub struct SenderDomain {
    pub domain: String,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub valid_signing: bool,
}

impl SenderDomain {
    pub fn verified_for_signing(&self) -> bool {
        self.valid_signing
            && self
                .verified_at
                .as_ref()
                .is_some_and(|value| !value.is_empty())
    }
}

pub fn domains(key: &str) -> Result<Vec<SenderDomain>, String> {
    let key = crate::config::validate_key(key)?;
    let value = request("senders/domains", &json!({"key": key}))?;
    let mut rows: Vec<SenderDomain> = serde_json::from_value(value)
        .map_err(|_| "Mandrill returned an unexpected domain list.".to_owned())?;
    for row in &mut rows {
        let parsed = crate::config::parse_domains(&row.domain)?;
        if parsed.len() != 1 {
            return Err("Mandrill returned an invalid domain entry.".into());
        }
        row.domain = parsed[0].clone();
    }
    rows.sort_by(|a, b| a.domain.cmp(&b.domain));
    rows.dedup_by(|a, b| a.domain == b.domain);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_domains() -> Vec<String> {
        vec!["example.com".into(), "example.org".into()]
    }

    fn draft() -> Draft {
        Draft {
            from_email: "sender@example.com".into(),
            from_name: "Sender".into(),
            to: "one@example.com; two@example.com\nthree@example.com,ONE@example.com".into(),
            cc: "copy@example.com".into(),
            bcc: "hidden@example.com".into(),
            subject: "Test subject".into(),
            body: "Line one\nLine two — £10".into(),
            ..Default::default()
        }
    }

    #[test]
    fn custom_sender_and_multiple_recipient_types() {
        let mut d = draft();
        d.reply_to = "replies@example.com".into();
        let p = payload(&d, "fake-test-key", &test_domains()).unwrap();
        assert_eq!(p["message"]["from_email"], "sender@example.com");
        assert_eq!(p["message"]["to"].as_array().unwrap().len(), 5);
        assert_eq!(p["message"]["to"][3]["type"], "cc");
        assert_eq!(p["message"]["to"][4]["type"], "bcc");
        assert_eq!(p["message"]["preserve_recipients"], true);
        assert_eq!(p["message"]["headers"]["Reply-To"], "replies@example.com");
        assert_eq!(p["message"]["text"], d.body);
    }

    #[test]
    fn sender_domains_allow_custom_local_names_but_no_other_domain() {
        let mut d = draft();
        for domain in test_domains() {
            d.from_email = format!("customer.service+orders@{domain}");
            assert!(validate(&d, &test_domains()).is_ok());
        }
        d.from_email = "email@example.net".into();
        assert!(validate(&d, &test_domains()).is_err());
        d.from_email = "email@EXAMPLE.COM".into();
        assert!(validate(&d, &test_domains()).is_ok());
    }

    #[test]
    fn sends_formatted_html_and_inline_image_bytes_as_cid_parts() {
        use crate::rich_text::{InlineImage, Span, Style};
        let path =
            std::env::temp_dir().join(format!("sender-inline-test-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 90, 120, 255]))
            .save(&path)
            .unwrap();
        let mut d = draft();
        d.body = "Hello [Image]".into();
        d.formatting = vec![
            Span {
                start: 0,
                end: 5,
                style: Style {
                    bold: true,
                    ..Default::default()
                },
            },
            Span {
                start: 6,
                end: 13,
                style: Style {
                    image: "test-image".into(),
                    ..Default::default()
                },
            },
        ];
        d.inline_images = vec![InlineImage {
            cid: "test-image".into(),
            name: "picture.png".into(),
            path: path.clone(),
            width: 200,
        }];
        let p = payload(&d, "fake-key", &test_domains()).unwrap();
        assert!(
            p["message"]["html"]
                .as_str()
                .unwrap()
                .contains("<strong>Hello</strong>")
        );
        assert!(
            p["message"]["html"]
                .as_str()
                .unwrap()
                .contains("cid:test-image")
        );
        assert_eq!(p["message"]["text"], "Hello [Image: picture.png]");
        assert_eq!(p["message"]["images"][0]["name"], "test-image");
        assert_eq!(p["message"]["images"][0]["type"], "image/png");
        assert_eq!(
            STANDARD
                .decode(p["message"]["images"][0]["content"].as_str().unwrap())
                .unwrap(),
            std::fs::read(&path).unwrap()
        );
        std::fs::write(&path, b"not an image").unwrap();
        assert!(payload(&d, "fake-key", &test_domains()).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(payload(&d, "fake-key", &test_domains()).is_err());
    }

    #[test]
    fn reads_history_written_before_rich_formatting_was_added() {
        let mut value = serde_json::to_value(draft()).unwrap();
        value.as_object_mut().unwrap().remove("formatting");
        value.as_object_mut().unwrap().remove("inline_images");
        let restored: Draft = serde_json::from_value(value).unwrap();
        assert!(restored.formatting.is_empty());
        assert!(restored.inline_images.is_empty());
        assert!(payload(&restored, "test", &test_domains()).is_ok());
    }

    #[test]
    fn prevents_cross_field_duplicates_and_header_injection() {
        let mut d = draft();
        d.bcc = "ONE@example.com".into();
        assert!(
            validate(&d, &test_domains())
                .unwrap_err()
                .contains("both To and BCC")
        );
        d.bcc.clear();
        d.from_email = "sender@example.com\r\nBcc: intruder@example.com".into();
        assert!(validate(&d, &test_domains()).is_err());
        d.from_email = "sender@example.com".into();
        d.subject = "Hello\nInjected".into();
        assert!(validate(&d, &test_domains()).is_err());
    }

    #[test]
    fn rejects_invalid_or_empty_recipients_and_accepts_bcc_only() {
        let mut d = draft();
        d.to = "invalid address".into();
        assert!(validate(&d, &test_domains()).is_err());
        d.to.clear();
        d.cc.clear();
        assert_eq!(validate(&d, &test_domains()).unwrap()[0].kind, "bcc");
        d.bcc.clear();
        assert!(validate(&d, &test_domains()).is_err());
    }

    #[test]
    fn attachments_encode_real_bytes_and_missing_files_fail() {
        let dir = std::env::temp_dir().join(format!("mandrill-sender-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.txt");
        std::fs::write(&path, b"attachment\x00bytes").unwrap();
        let mut d = draft();
        d.attachments.push(path.clone());
        let p = payload(&d, "test", &test_domains()).unwrap();
        let attachment = &p["message"]["attachments"][0];
        assert_eq!(attachment["type"], "text/plain");
        assert_eq!(
            STANDARD
                .decode(attachment["content"].as_str().unwrap())
                .unwrap(),
            b"attachment\x00bytes"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(payload(&d, "test", &test_domains()).is_err());
        File::create(&path)
            .unwrap()
            .set_len(MAX_ATTACHMENTS + 1)
            .unwrap();
        assert!(
            payload(&d, "test", &test_domains())
                .unwrap_err()
                .contains("6 MB")
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn mixed_results_preserve_rejections_and_queued_status() {
        let expected = recipients(&Draft {
            to: "one@example.com,two@example.com".into(),
            ..Default::default()
        })
        .unwrap();
        let rows = interpret(
            json!([
                {"email":"one@example.com","status":"queued","queued_reason":"attachments"},
                {"email":"two@example.com","status":"rejected","reject_reason":"unsigned"}
            ]),
            &expected,
        )
        .unwrap();
        assert!(rows[0].accepted());
        assert!(!rows[1].accepted());
        assert_eq!(rows[0].description(), "Queued (attachments)");
        assert!(interpret(json!([]), &expected).is_err());
        assert!(
            interpret(
                json!([{"email":"one@example.com","status":"delivered"}]),
                &expected
            )
            .is_err()
        );
    }
}
