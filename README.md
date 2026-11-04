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

PDF testowe: `python scripts/make-sample-pdf.py samples/umowa-testowa.pdf`; `samples/skan-testowy.pdf` to ta sama umowa bez warstwy tekstu (do OCR).

## Funkcje

- Podgląd: karty, miniatury, wyszukiwanie, powiększanie, tryb ciemny, przeciągnij i upuść, skróty.
- Czat z Claude z klikalnymi Cytatami, Szybkie akcje, Zapytanie o zaznaczenie, licznik kosztu.
- Edycja: Adnotacje (podświetlenie, notatka, rysunek, obraz), formularze, operacje na stronach (obrót, usuwanie, przeciąganie, wyodrębnianie, wstawianie PDF), cofnij/ponów, zapis z kopią zapasową.
- Edycja przez czat: podświetlanie, obrót i nawigacja od razu; usuwanie i przenoszenie stron oraz Redakcja jako Propozycje zmiany do zatwierdzenia.
- Przestrzenie robocze: Rozmowy o kilku Dokumentach naraz.
- OCR skanów (wbudowany w Windows), Redakcja i Korekta (ADR 0003), Porównanie wersji z podsumowaniem w czacie.

## Struktura

- `src/`: UI (React + pdf.js)
  - `components/`: widok Dokumentu, podgląd z edytorami, miniatury, czat, Porównanie, Przestrzenie robocze
  - `lib/editing.ts`: edycje na bajtach PDF (strony, OCR, Redakcja/Korekta, Porównanie)
- `src-tauri/src/`: backend (Rust)
  - `lib.rs`: komendy: Dokumenty i Wersje, zapis, Rozmowy, Przestrzenie robocze, ustawienia
  - `chat.rs`: Claude API: streaming, Cytaty, narzędzia edycji, tryb „wybrane strony”, koszt
  - `pdfops.rs`: lopdf: obrót stron, zamiana strony w obraz, niewidoczna warstwa tekstu OCR
  - `ocr.rs`: Windows.Media.Ocr
  - `db.rs`: SQLite (migracje), indeks FTS5
  - `keys.rs`: klucz API w Menedżerze poświadczeń Windows
