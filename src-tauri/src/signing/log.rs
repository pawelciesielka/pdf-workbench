//! Dziennik podpisów: każdy podpis dopisuje wpis do `dziennik-podpisow.md` w folderze wybranym
//! w ustawieniach (data, pliki, podpisujący, poziom, skrót SHA-256).

use super::{certs_with_key, Document};
use crate::Res;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

pub const FILE_NAME: &str = "dziennik-podpisow.md";

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Plik, który podpisywano.
    pub source: String,
    /// Plik zapisany z podpisem (ten sam przy nadpisywaniu).
    pub output: String,
    pub cert_id: String,
    /// Czas podpisu w formacie PDF („D:20261106143205+01'00'”).
    pub signing_time: String,
    pub level: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub warning: Option<String>,
}

/// „D:20261106143205+01'00'” → „2026-11-06 14:32:05 (UTC+01:00)”.
fn pdf_time(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 14 {
        return s.to_string();
    }
    let n = |a: usize, b: usize| &digits[a..b];
    let tz = d[14..].replace('\'', "");
    let tz = match tz.as_str() {
        "" => String::new(),
        "Z" => " (UTC)".into(),
        t if t.len() >= 5 => format!(" (UTC{}:{})", &t[..3], &t[3..5]),
        t => format!(" (UTC{t})"),
    };
    format!("{}-{}-{} {}:{}:{}{tz}", n(0, 4), n(4, 6), n(6, 8), n(8, 10), n(10, 12), n(12, 14))
}

fn level_label(level: &str) -> String {
    if level == "B-LTA" { "PAdES-LTA".into() } else { format!("PAdES-{level}") }
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

fn entry_md(e: &LogEntry, signer: &str) -> String {
    let mut s = format!("\n## {} — {}\n\n", pdf_time(&e.signing_time), file_name(&e.output));
    if e.source.eq_ignore_ascii_case(&e.output) {
        s += &format!("- Plik (nadpisany): `{}`\n", e.output);
    } else {
        s += &format!("- Plik podpisany: `{}`\n- Oryginał: `{}`\n", e.output, e.source);
    }
    s += &format!("- Podpisujący: {signer}\n");
    if let Some(l) = &e.level {
        s += &format!("- Poziom: {}\n", level_label(l));
    }
    if let Some(r) = e.reason.as_deref().filter(|r| !r.is_empty()) {
        s += &format!("- Powód: {r}\n");
    }
    if let Some(l) = e.location.as_deref().filter(|l| !l.is_empty()) {
        s += &format!("- Miejsce: {l}\n");
    }
    match std::fs::read(&e.output) {
        Ok(bytes) => {
            let pages = Document::load_mem(&bytes).map(|d| d.get_pages().len()).ok();
            s += &format!(
                "- Rozmiar: {} B{}\n- SHA-256: `{}`\n",
                bytes.len(),
                pages.map(|p| format!(", stron: {p}")).unwrap_or_default(),
                hex::encode(Sha256::digest(&bytes))
            );
        }
        Err(err) => s += &format!("- Nie można odczytać pliku: {err}\n"),
    }
    if let Some(w) = &e.warning {
        s += &format!("- Uwaga: {w}\n");
    }
    s
}

/// Dopisuje wpisy do dziennika w folderze `dir` (tworzy plik z nagłówkiem, jeśli go nie ma).
pub fn append(dir: &str, entries: &[LogEntry]) -> Res<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let dir = Path::new(dir);
    if !dir.is_dir() {
        return Err(format!("Folder dziennika podpisów nie istnieje: {}", dir.display()));
    }
    let certs = certs_with_key().unwrap_or_default();
    let mut text = String::new();
    for e in entries {
        let signer = certs
            .iter()
            .find(|c| c.id() == e.cert_id)
            .map(|c| format!("{} (wystawca: {})", c.name(false), c.name(true)))
            .unwrap_or_else(|| "nieznany certyfikat".into());
        text += &entry_md(e, &signer);
    }
    let path = dir.join(FILE_NAME);
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Nie można zapisać dziennika podpisów {}: {e}", path.display()))?;
    if new {
        text = format!("# Dziennik podpisów\n{text}");
    }
    f.write_all(text.as_bytes()).map_err(|e| format!("Nie można zapisać dziennika podpisów: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(pdf_time("D:20261106143205+01'00'"), "2026-11-06 14:32:05 (UTC+01:00)");
        assert_eq!(pdf_time("D:20261106143205Z"), "2026-11-06 14:32:05 (UTC)");
    }

    #[test]
    fn appends_entries() {
        let dir = std::env::temp_dir().join(format!("pdfwb-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("umowa.pdf");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../samples/umowa-testowa.pdf"), &pdf).unwrap();
        let p = pdf.to_string_lossy().to_string();
        let e = LogEntry {
            source: p.clone(),
            output: p.clone(),
            cert_id: "brak".into(),
            signing_time: "D:20261112204105+01'00'".into(),
            level: Some("B-T".into()),
            reason: Some("Akceptacja".into()),
            location: None,
            warning: None,
        };
        let d = dir.to_string_lossy().to_string();
        append(&d, &[e.clone()]).unwrap();
        append(&d, &[LogEntry { output: dir.join("umowa (podpisany).pdf").to_string_lossy().to_string(), ..e }]).unwrap();
        let log = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(log.starts_with("# Dziennik podpisów\n"));
        assert_eq!(log.matches("# Dziennik").count(), 1);
        assert_eq!(log.matches("\n## 2026-11-12 20:41:05 (UTC+01:00)").count(), 2);
        assert!(log.contains("- Plik (nadpisany): `") && log.contains("- Oryginał: `"));
        assert!(log.contains("- Poziom: PAdES-B-T") && log.contains("- Powód: Akceptacja") && log.contains("SHA-256: `"));
        println!("{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
