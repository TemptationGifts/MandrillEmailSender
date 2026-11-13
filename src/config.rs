use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

#[derive(Clone, Default)]
pub struct Account {
    pub api_key: String,
    pub domains: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredAccount {
    encrypted_api_key: String,
    domains: Vec<String>,
}

pub fn validate_key(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("Enter a valid Mandrill API key, without spaces.".into());
    }
    Ok(key.to_owned())
}

pub fn parse_domains(input: &str) -> Result<Vec<String>, String> {
    let mut domains = Vec::new();
    for value in input
        .split([',', ';', '\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let domain = value.to_ascii_lowercase();
        let valid = domain.len() <= 253
            && domain.contains('.')
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            });
        if !valid {
            return Err(format!(
                "Invalid domain: {value}. Enter domain names only, without @, https:// or paths."
            ));
        }
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.is_empty() {
        return Err("Add at least one sender domain or load domains from the API key.".into());
    }
    Ok(domains)
}

fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    if bytes.len() > u32::MAX as usize {
        return Err("Stored key is too large.".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI uses the current Windows user. Input is read-only for these calls;
    // the OS allocates the output and requires LocalFree to release it.
    unsafe {
        let result = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if result == 0 {
            return Err("Windows could not protect or unlock the saved API key. Enter the key again in Admin.".into());
        }
        let result = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}

fn path() -> Result<std::path::PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| {
            std::path::PathBuf::from(p)
                .join("MandrillSender")
                .join("admin.json")
        })
        .ok_or_else(|| "The Windows settings folder is unavailable.".into())
}

fn decode(bytes: &[u8]) -> Result<Account, String> {
    let stored: StoredAccount = serde_json::from_slice(bytes).map_err(|_| {
        "Admin settings could not be read. Re-enter the key and domains in Admin.".to_owned()
    })?;
    let encrypted = STANDARD
        .decode(stored.encrypted_api_key)
        .map_err(|_| "Saved key data is invalid.".to_owned())?;
    let key = String::from_utf8(crypt(&encrypted, false)?)
        .map_err(|_| "Saved API key could not be decoded.".to_owned())?;
    Ok(Account {
        api_key: validate_key(&key)?,
        domains: parse_domains(&stored.domains.join("\n"))?,
    })
}

pub fn load() -> Result<Account, String> {
    match fs::read(path()?) {
        Ok(bytes) => decode(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Account::default()),
        Err(_) => {
            Err("Admin settings could not be opened. Re-enter the key and domains in Admin.".into())
        }
    }
}

fn save_at(path: &Path, account: &Account) -> Result<(), String> {
    let key = validate_key(&account.api_key)?;
    let stored = StoredAccount {
        encrypted_api_key: STANDARD.encode(crypt(key.as_bytes(), true)?),
        domains: parse_domains(&account.domains.join("\n"))?,
    };
    fs::create_dir_all(path.parent().ok_or("Invalid settings path")?).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

pub fn save(account: &Account) -> Result<(), String> {
    save_at(&path()?, account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_domains_and_rejects_urls_addresses_and_empty_lists() {
        assert_eq!(
            parse_domains(" Example.com;example.com\nmail.example.org ").unwrap(),
            vec!["example.com", "mail.example.org"]
        );
        for bad in [
            "",
            "https://example.com",
            "me@example.com",
            "bad..com",
            "-bad.com",
            "a/b.com",
            "*.example.com",
        ] {
            assert!(parse_domains(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn windows_encryption_round_trips_and_saved_file_contains_no_key() {
        let path =
            std::env::temp_dir().join(format!("mandrill-admin-test-{}.json", std::process::id()));
        let account = Account {
            api_key: "synthetic-test-secret".into(),
            domains: vec!["example.com".into()],
        };
        save_at(&path, &account).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&account.api_key));
        let restored = decode(&bytes).unwrap();
        assert_eq!(restored.api_key, account.api_key);
        assert_eq!(restored.domains, account.domains);
        let replacement = Account {
            api_key: "different-test-key".into(),
            domains: vec!["second.example.com".into()],
        };
        save_at(&path, &replacement).unwrap();
        assert_eq!(
            decode(&fs::read(&path).unwrap()).unwrap().api_key,
            replacement.api_key
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_settings_fail_instead_of_using_another_account() {
        assert!(decode(br#"{"encrypted_api_key":"invalid","domains":["example.com"]}"#).is_err());
        assert!(decode(b"not-json").is_err());
    }
}
