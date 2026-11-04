# 0001. Tauri 2 + Rust zamiast Electrona

Status: przyjęta (2026-11-04)

## Kontekst

Aplikacja ma być szybka, lekka i rozwijalna, z ciężką pracą na PDF-ach (indeksowanie, a później edycja) i z czatem AI, którego klucz API nie może trafić do warstwy UI.

## Decyzja

Powłoka Tauri 2 (WebView2) z backendem w Ruście i UI w React + TypeScript. Podgląd przez pdf.js, wyszukiwanie przez SQLite FTS5, wywołania Claude API z backendu przez HTTP (nie ma oficjalnego SDK do Rusta).

## Alternatywy

- **Electron**: działa na samym Node, ale instalator ma ok. 150 MB, zużywa więcej RAM-u, a logika „po stronie serwera” i tak żyje w JavaScripcie.
- **.NET + WinUI/WPF**: najbardziej natywny, ale UI rozwija się wolniej i nie ma drogi na macOS.

## Konsekwencje

- Do budowania potrzebne są Rust i MSVC Build Tools.
- Instalator ma kilka–kilkanaście MB, aplikacja startuje szybko.
- W etapie 2 edycja PDF trafia do backendu (PDFium) jako komendy wspólne dla UI i czatu.
- W MVP tekst stron wyciąga pdf.js w UI i przekazuje go do indeksu w backendzie, żeby nie wiązać się jeszcze z PDFium.
