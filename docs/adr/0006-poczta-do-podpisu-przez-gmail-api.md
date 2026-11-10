# 6. Poczta do podpisu przez Gmail API i szkice odpowiedzi

Data: 2026-11-10

## Kontekst

Użytkownik często dostaje maile z PDF-ami do podpisu. Musi wtedy zapisać załączniki, podpisać je, a potem odpowiedzieć wszystkim z podpisanymi plikami. Pocztę firmową (Google Workspace) czyta w Gmailu w przeglądarce i w Thunderbirdzie.

## Decyzja

- **Gmail API zamiast wtyczki do Thunderbirda.** Działa niezależnie od programu pocztowego, a szkice i etykiety synchronizują się do obu (Thunderbird przez IMAP). Pozwala też podpisać załączniki z wielu maili naraz, jednym pobraniem klucza (podpisywanie wielu plików z ADR 0005).
- **Logowanie jednym kliknięciem przez wbudowany klient OAuth** typu „Aplikacja komputerowa” wydawcy programu. Dane klienta nie trafiają do repozytorium: `build.rs` wkompilowuje je ze zmiennych `PDFWB_GOOGLE_CLIENT_ID`/`PDFWB_GOOGLE_CLIENT_SECRET` albo z pliku `%USERPROFILE%\.tauri\pdf-workbench-google.json`, a skrypt wydania bez nich przerywa pracę. Google traktuje sekret klienta desktopowego jako niepoufny, a bezpieczeństwo zapewniają PKCE i przekierowanie na 127.0.0.1. W opcjach zaawansowanych można wczytać własnego klienta, który ma wtedy pierwszeństwo.
- Logowanie przez przekierowanie na 127.0.0.1 z losowym portem, z PKCE i parametrem `state`. Zakres to `gmail.modify`: czytanie, szkice i etykiety, bez trwałego usuwania. Token odświeżania (i ewentualny własny sekret) trzymany jest w Menedżerze poświadczeń Windows, a token dostępu tylko w pamięci.
- **Szkice zamiast wysyłki.** Program tworzy szkic „Odpowiedz wszystkim” w wątku maila (threadId, In-Reply-To, References) z cytatem oryginału i podpisanymi plikami. Wysyła zawsze użytkownik. Adresaci: Reply-To albo nadawca + Do, w DW pierwotne DW, bez własnego adresu i bez powtórzeń.
- **Etykieta „Podpisane”** trafia na mail po utworzeniu szkicu. Domyślne wyszukiwanie (`in:inbox has:attachment filename:pdf -label:podpisane newer_than:60d`) pomija takie maile, więc lista działa bez filtrów w Gmailu. Wyszukiwanie można zmienić, np. na etykietę nadawaną przez filtr.
- Załączniki trafiają do `Dokumenty\PDF Workbench\Do podpisu\RRRR-MM-DD temat\`, a podpisane kopie obok nich.

## Konsekwencje

- Wydawca raz konfiguruje projekt Google Cloud. Ekran zgody „Wewnętrzny” pozwala logować się bez weryfikacji Google, ale tylko kontom z tej samej organizacji Workspace. Udostępnienie innym wymaga typu „Zewnętrzny” i weryfikacji Google, bo `gmail.modify` jest zakresem z ograniczeniami (ocena bezpieczeństwa CASA). Do tego czasu działa tryb testowy dla maks. 100 dodanych użytkowników, a jego tokeny wygasają po 7 dniach.
- Szkic z API nie ma podpisu Gmaila (stopki). Treść odpowiedzi jest konfigurowalna w opcjach Poczty.
- Wiadomości powyżej ok. 3 MB idą przez endpoint wysyłania plików (`uploadType=multipart`).
- Testy jednostkowe sprawdzają adresatów, MIME i kodowanie nagłówków. Połączenie z prawdziwym kontem testuje użytkownik.
