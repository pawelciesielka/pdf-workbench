//! Klucz Anthropic API: Menedżer poświadczeń Windows, z awaryjnym odczytem z ANTHROPIC_API_KEY.

const SERVICE: &str = "pdf-workbench";
const ACCOUNT: &str = "anthropic-api-key";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
}

pub fn store(key: &str) -> Result<(), String> {
    entry()?.set_password(key).map_err(|e| e.to_string())
}

pub fn clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Dowolny sekret aplikacji w Menedżerze poświadczeń (np. tokeny Google); None — brak.
pub fn get_named(account: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, account).ok()?.get_password().ok().filter(|s| !s.is_empty())
}

pub fn set_named(account: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?.set_password(value).map_err(|e| e.to_string())
}

pub fn clear_named(account: &str) -> Result<(), String> {
    match keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Zwraca (klucz, źródło) — źródło: "keyring" albo "env".
pub fn load() -> Option<(String, &'static str)> {
    if let Ok(e) = entry() {
        if let Ok(k) = e.get_password() {
            if !k.trim().is_empty() {
                return Some((k, "keyring"));
            }
        }
    }
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(|k| (k, "env"))
}
