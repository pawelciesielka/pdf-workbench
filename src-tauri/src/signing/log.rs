//! Dziennik podpisów: każdy podpis dopisuje do `dziennik-podpisow.md` w folderze wybranym
//! w ustawieniach jedną linię — czas podpisu i ścieżkę podpisanego pliku.

use crate::Res;
use serde::Deserialize;
use std::io::Write;
use std::path::Path;

pub const FILE_NAME: &str = "dziennik-podpisow.md";

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Plik zapisany z podpisem.
    pub output: String,
    /// Czas podpisu w formacie PDF („D:20261106143205+01'00'”).
    pub signing_time: String,
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

/// Dopisuje wpisy do dziennika w folderze `dir` (tworzy plik, jeśli go nie ma).
pub fn append(dir: &str, entries: &[LogEntry]) -> Res<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let dir = Path::new(dir);
    if !dir.is_dir() {
        return Err(format!("Folder dziennika podpisów nie istnieje: {}", dir.display()));
    }
    let text: String = entries.iter().map(|e| format!("{} {}\n", pdf_time(&e.signing_time), e.output)).collect();
    let path = dir.join(FILE_NAME);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("Nie można zapisać dziennika podpisów {}: {e}", path.display()))
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
    fn appends_lines() {
        let dir = std::env::temp_dir().join(format!("pdfwb-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        let e = |out: &str| LogEntry { output: out.into(), signing_time: "D:20261112223036+01'00'".into() };
        append(&d, &[e(r"C:\a\umowa (podpisany).pdf")]).unwrap();
        append(&d, &[e(r"C:\a\umowa.pdf")]).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(),
            "2026-11-12 22:30:36 (UTC+01:00) C:\\a\\umowa (podpisany).pdf\n2026-11-12 22:30:36 (UTC+01:00) C:\\a\\umowa.pdf\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
