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

Instalator (NSIS) trafia do `src-tauri/target/release/bundle/nsis/`. Budowanie podpisuje pliki aktualizacji, więc wymaga klucza w `TAURI_SIGNING_PRIVATE_KEY` (zwykle robi to skrypt wydania poniżej):

```bash
npm run tauri build
```

## Wydanie i aktualizacje

Program po starcie sprawdza `latest.json` z najnowszego wydania na GitHubie i proponuje aktualizację; można to też zrobić ręcznie w Ustawieniach. Aktualizacja instaluje się tylko z poprawnym podpisem kluczem wydawcy (klucz publiczny jest w `tauri.conf.json`, prywatny w `%USERPROFILE%\.tauri\pdf-workbench.key` i nigdy nie trafia do repozytorium).

```bash
npm run release -- 0.2.0 "Opis zmian"
```

Skrypt ustawia wersję w `package.json`, `tauri.conf.json` i `Cargo.toml`, buduje podpisany instalator i składa w `release/v0.2.0/` instalator oraz `latest.json`. Potem: commit, push i nowe wydanie na GitHubie z tagiem `v0.2.0` i oboma plikami (jako zwykłe wydanie, nie pre-release).

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
- Poczta do podpisu (ADR 0006): maile z PDF-ami z Gmaila, podpis wszystkich załączników naraz i szkice „Odpowiedz wszystkim” z podpisanymi plikami w tym samym wątku. Logowanie jednym kliknięciem przez klienta OAuth wkompilowanego przy budowaniu z `%USERPROFILE%\.tauri\pdf-workbench-google.json` (JSON klienta „Aplikacja komputerowa” z Google Cloud, z włączonym Gmail API) albo ze zmiennych `PDFWB_GOOGLE_CLIENT_ID`/`PDFWB_GOOGLE_CLIENT_SECRET`.

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
  - `gmail.rs`: Gmail API: logowanie OAuth (pętla zwrotna + PKCE), lista maili, załączniki, szkic odpowiedzi
  - `keys.rs`: klucz API i tokeny Google w Menedżerze poświadczeń Windows

## Licencja

MIT — zobacz [LICENSE](LICENSE). Dołączona biblioteka `src-tauri/pdfium/pdfium.dll` (kompilacja [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)) ma własne licencje: [LICENSE-pdfium](src-tauri/pdfium/LICENSE-pdfium) i [LICENSE-pdfium-google](src-tauri/pdfium/LICENSE-pdfium-google). pdf.js jest na licencji Apache 2.0.
