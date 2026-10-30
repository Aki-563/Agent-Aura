//! Saves the model provider settings between runs. The API key is encrypted with Windows DPAPI
//! (`CryptProtectData`), so the file is useless on another account or another machine; the other
//! settings (protocol, base URL, model) are plain. Stored as `provider.json` under the data root.
//! The key is never written to logs, traces or the database.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

/// Extra bytes mixed into the encryption so another program that can call DPAPI as this user
/// cannot decrypt the blob without knowing the application's tag.
const ENTROPY: &[u8] = b"AgentAura.provider.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedProvider {
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    /// DPAPI-encrypted key, base64. Empty when the provider needs no key.
    #[serde(default)]
    pub api_key_encrypted: String,
}

fn path(data_root: &std::path::Path) -> std::path::PathBuf {
    data_root.join("provider.json")
}

fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr() as *mut u8 }
}

pub fn encrypt(secret: &str) -> Result<String, String> {
    if secret.is_empty() {
        return Ok(String::new());
    }
    let input = blob(secret.as_bytes());
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe { CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), 0, &mut output) };
    if ok == 0 || output.pbData.is_null() {
        return Err("Windows could not encrypt the API key".into());
    }
    let encrypted = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData as _) };
    Ok(BASE64.encode(encrypted))
}

pub fn decrypt(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let bytes = BASE64.decode(encoded).map_err(|_| "the saved API key is damaged".to_owned())?;
    let input = blob(&bytes);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), 0, &mut output) };
    if ok == 0 || output.pbData.is_null() {
        return Err("the saved API key cannot be read by this Windows account".into());
    }
    let plain = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    // Wipe the decrypted copy Windows allocated before handing the memory back.
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        LocalFree(output.pbData as _);
    }
    String::from_utf8(plain).map_err(|_| "the saved API key is damaged".to_owned())
}

pub fn save(data_root: &std::path::Path, protocol: &str, base_url: &str, model: &str, api_key: &str) -> Result<(), String> {
    let saved = SavedProvider { protocol: protocol.to_owned(), base_url: base_url.to_owned(), model: model.to_owned(), api_key_encrypted: encrypt(api_key)? };
    let text = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_root).map_err(|e| e.to_string())?;
    // Write to a temporary file and rename, so a crash cannot leave half a file behind.
    let target = path(data_root);
    let temporary = target.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|e| format!("could not save the provider settings: {e}"))?;
    std::fs::rename(&temporary, &target).map_err(|e| format!("could not save the provider settings: {e}"))
}

/// The saved provider with its key decrypted, or None when nothing is saved or it cannot be read.
pub fn load(data_root: &std::path::Path) -> Option<(SavedProvider, String)> {
    let saved: SavedProvider = serde_json::from_str(&std::fs::read_to_string(path(data_root)).ok()?).ok()?;
    let key = decrypt(&saved.api_key_encrypted).ok()?;
    Some((saved, key))
}

pub fn clear(data_root: &std::path::Path) {
    let _ = std::fs::remove_file(path(data_root));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_round_trips_and_is_not_stored_in_the_clear() {
        let dir = std::env::temp_dir().join(format!("aura-vault-test-{}", std::process::id()));
        let secret = "sk-test-1234567890-secret";
        save(&dir, "open_ai_responses", "https://api.openai.com/v1", "gpt-test", secret).unwrap();
        let file = std::fs::read_to_string(dir.join("provider.json")).unwrap();
        assert!(!file.contains(secret) && !file.contains("1234567890"), "the key must not be readable in the file");
        assert!(file.contains("gpt-test"), "the model name is stored plainly");
        let (saved, key) = load(&dir).unwrap();
        assert_eq!(key, secret);
        assert_eq!(saved.base_url, "https://api.openai.com/v1");
        // A damaged or foreign blob is refused, not guessed at.
        assert!(decrypt("not-base64!!").is_err());
        assert!(decrypt(&BASE64.encode(b"garbage bytes")).is_err());
        clear(&dir);
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_provider_without_a_key_is_saved_too() {
        let dir = std::env::temp_dir().join(format!("aura-vault-test2-{}", std::process::id()));
        save(&dir, "open_ai_compatible_chat", "http://192.168.6.70:1234/v1", "gui-owl", "").unwrap();
        let (saved, key) = load(&dir).unwrap();
        assert_eq!((saved.model.as_str(), key.as_str()), ("gui-owl", ""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
