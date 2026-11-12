//! Czy SimplySign pamięta PIN w tym procesie.
//!
//! Przy opcji SimplySign Desktop „Pamiętaj kod PIN dla aplikacji korzystających z CSP/KSP”
//! biblioteka SimplySign CSP trzyma PIN w pamięci procesu, który podpisywał — kolejne podpisy
//! idą bez pytania, dopóki użytkownik nie kliknie „Rozłącz” w SimplySign Desktop (przy „Zapomnij
//! kod PIN po odłączeniu”) albo nie zamknie programu. Ani magazyn certyfikatów, ani PC/SC tego
//! nie pokazują, więc zapamiętujemy sami: czas pierwszego udanego podpisu kluczem SimplySign.
//! Znacznik kasujemy, gdy SimplySign Desktop przestaje działać (znika jego kanał
//! `\\.\pipe\SimplySign-…`) albo gdy użytkownik potwierdzi, że się rozłączył.

use serde::Serialize;
use std::sync::atomic::{AtomicI64, Ordering};

static PIN_SINCE: AtomicI64 = AtomicI64::new(0);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimplySignStatus {
    /// SimplySign Desktop jest uruchomiony.
    pub running: bool,
    /// Od kiedy (ms od epoki) PIN jest prawdopodobnie zapamiętany w tym procesie; brak — nie jest.
    pub pin_since: Option<i64>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Wywoływane po udanym podpisie kluczem SimplySign (pierwszy podpis ustala czas).
pub(crate) fn mark_signed() {
    let _ = PIN_SINCE.compare_exchange(0, now_ms(), Ordering::SeqCst, Ordering::SeqCst);
}

/// Użytkownik rozłączył się w SimplySign Desktop.
pub fn forget() {
    PIN_SINCE.store(0, Ordering::SeqCst);
}

fn simplysign_running() -> bool {
    std::fs::read_dir(r"\\.\pipe\")
        .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("SimplySign-")))
        .unwrap_or(false)
}

pub fn status() -> SimplySignStatus {
    let running = simplysign_running();
    if !running {
        forget();
    }
    let since = PIN_SINCE.load(Ordering::SeqCst);
    SimplySignStatus { running, pin_since: (since != 0).then_some(since) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_reads_pipes() {
        let s = super::status();
        println!("SimplySign uruchomiony: {}, PIN od: {:?}", s.running, s.pin_since);
    }
}
