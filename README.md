# PDF Workbench

Desktopowa aplikacja (Windows) do przeglądania PDF-ów i rozmawiania o nich z Claude, z klikalnymi Cytatami do stron.
Słownik pojęć: [CONTEXT.md](CONTEXT.md). Decyzje architektoniczne: [docs/adr](docs/adr).

## Wymagania

- Node.js 22+, Rust (rustup, toolchain MSVC), Visual Studio Build Tools z modułem C++.
- Klucz Anthropic API (wpisywany w Ustawieniach aplikacji albo w zmiennej `ANTHROPIC_API_KEY`).

## Uruchomienie

```bash
npm install
npm run tauri dev
```

Instalator (NSIS) trafia do `src-tauri/target/release/bundle/nsis/`:

```bash
npm run tauri build
```

PDF testowy: `python scripts/make-sample-pdf.py samples/umowa-testowa.pdf`.

## Struktura

- `src/`: UI (React + pdf.js): karty, podgląd, miniatury, wyszukiwanie, czat.
- `src-tauri/src/`: backend (Rust)
  - `lib.rs`: komendy: Dokumenty, Rozmowy, ustawienia
  - `chat.rs`: Claude API: streaming, Cytaty, tryb „wybrane strony” dla dużych PDF-ów, koszt
  - `db.rs`: SQLite i indeks FTS5
  - `keys.rs`: klucz API w Menedżerze poświadczeń Windows
