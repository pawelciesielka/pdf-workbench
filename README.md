# PDF Workbench

Desktopowa aplikacja (Windows) do przeglądania PDF-ów i rozmawiania o nich z Claude, z klikalnymi Cytatami do stron.
Słownik pojęć: [CONTEXT.md](CONTEXT.md). Decyzje architektoniczne: [docs/adr](docs/adr).

## Wymagania

- Node.js 22+, Rust (rustup, toolchain MSVC), Visual Studio Build Tools z modułem C++.
- `src-tauri/pdfium/pdfium.dll`: PDFium (bblanchon/pdfium-binaries, `chromium/8125`, win-x64) zgodny z `pdfium-render` 0.9.5.
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
- Edycja treści stron (ADR 0004): Obiekty strony (zaznacz, przesuń, zmień rozmiar, usuń), edycja tekstu w miejscu z Czcionką zastępczą, nowy tekst w treści strony.
- Edycja przez czat: podświetlanie, obrót i nawigacja od razu; usuwanie i przenoszenie stron, Redakcja i zamiana tekstu jako Propozycje zmiany do zatwierdzenia.
- Przestrzenie robocze: Rozmowy o kilku Dokumentach naraz.
- OCR skanów (wbudowany w Windows), Redakcja i Korekta (ADR 0003), Porównanie wersji z podsumowaniem w czacie.
- Podpis elektroniczny PAdES (ADR 0005) certyfikatem z magazynu Windows, np. kwalifikowanym z SimplySign Desktop: widoczny lub niewidoczny, ze znacznikiem czasu Certum. Długoterminowa weryfikacja (PAdES-LTA), sprawdzanie zaufania (łańcuch, unieważnienie, unijna lista zaufanych) i podpisywanie wielu plików naraz. Podpisane dokumenty mają zablokowaną edycję, którą można świadomie odblokować.

## Struktura

- `src/`: UI (React + pdf.js)
  - `components/`: widok Dokumentu, podgląd z edytorami, miniatury, czat, Porównanie, Przestrzenie robocze
  - `lib/editing.ts`: edycje na bajtach PDF (strony, OCR, Redakcja/Korekta, Porównanie)
- `src-tauri/src/`: backend (Rust)
  - `lib.rs`: komendy: Dokumenty i Wersje, zapis, Rozmowy, Przestrzenie robocze, ustawienia
  - `chat.rs`: Claude API: streaming, Cytaty, narzędzia edycji, tryb „wybrane strony”, koszt
  - `editor.rs`: PDFium: Obiekty strony, edycja i zamiana tekstu, Czcionka zastępcza
  - `pdfops.rs`: lopdf: obrót stron, zamiana strony w obraz, niewidoczna warstwa tekstu OCR
  - `signing.rs`: podpis PAdES przez CryptoAPI, odczyt i sprawdzanie podpisów
    - `signing/ltv.rs`: DSS (certyfikaty, CRL) do długoterminowej weryfikacji
    - `signing/trust.rs`: zaufanie: łańcuch Windows, unieważnienie, unijna lista zaufanych
  - `ocr.rs`: Windows.Media.Ocr
  - `db.rs`: SQLite (migracje), indeks FTS5
  - `keys.rs`: klucz API w Menedżerze poświadczeń Windows
