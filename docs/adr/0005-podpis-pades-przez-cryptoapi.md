# 5. Podpis PAdES przez CryptoAPI Windows i blokada Podpisanych dokumentów

Data: 2026-11-06

## Kontekst

Użytkownik podpisuje dokumenty kwalifikowanym certyfikatem Certum w SimplySign (podpis w chmurze, odblokowany tokenem w SimplySign Desktop). Dostarczany program SimplySign SmartSign jest dla niego zbyt słaby, więc chce podpisywać PDF-y w PDF Workbench. SimplySign Desktop rejestruje certyfikaty w osobistym magazynie certyfikatów Windows jako kartę wirtualną, więc każdy program może nimi podpisywać przez standardowe API Windows. Druga potrzeba: edycja Podpisanego dokumentu po cichu unieważnia podpis, a dotychczasowy zapis (lopdf, PDFium, pdf.js) przepisuje plik.

## Decyzja

- **Podpis przez CryptoAPI**, bez PKCS#11 i bez bibliotek od Certum. Certyfikat wybierany jest z magazynu „Osobisty” bieżącego użytkownika. Klucz pobiera `CryptAcquireCertificatePrivateKey` (preferowany CNG), a podpis CMS buduje `CryptMsg*`. Sam podpis wykonuje dostawca klucza (u użytkownika SimplySign), więc klucz prywatny nigdy nie trafia do programu.
- **Format PAdES (ETSI.CAdES.detached)**: SHA-256, podpisany atrybut signing-certificate-v2, dołączone certyfikaty pośrednie i opcjonalny znacznik czasu RFC 3161 z `time.certum.pl` (`CryptRetrieveTimeStamp`). Powstaje poziom PAdES-B-T, a bez znacznika czasu PAdES-B-B.
- **PAdES-LTA** (opcja, domyślnie włączona przy znaczniku czasu): po podpisie dopisywany jest słownik /DSS z certyfikatami łańcuchów podpisującego i serwera znaczników czasu oraz listami unieważnień (CRL pobierane przez `CryptRetrieveObjectByUrlW` z rozszerzenia CRL Distribution Points), a potem znacznik czasu całego dokumentu (/DocTimeStamp, ETSI.RFC3161). Gdy którejś listy nie da się pobrać, podpis zostaje na poziomie B-T, a użytkownik dostaje ostrzeżenie.
- **Aktualizacja przyrostowa pisana ręcznie** (`Update`): oryginalne bajty pliku zostają nietknięte, a na końcu dopisywane są zmienione i nowe obiekty oraz xref w tym samym formacie co poprzedni (tablica albo strumień). Dzięki temu kolejne podpisy, DSS i znacznik czasu dokumentu nie unieważniają wcześniejszych podpisów. Miejsce na podpis CMS ma 30 000 bajtów.
- **Widoczny podpis** ma warstwy wyglądu Acrobata (n0–n4 w formie /FRM). Adobe Reader podmienia warstwę n1 na ikonę ważności (zielony znaczek, „?” albo „✖”), a n4 na tekst stanu w górnym pasku, np. „Podpis jest prawidłowy”. Inne przeglądarki pokazują tylko warstwę n2: ramkę i wiersze tekstu w standardowej czcionce Helvetica z kodowaniem /Differences dla polskich liter, bez osadzania czcionki.
- **Wykrywanie i blokada**: przy wczytaniu Dokumentu odczytywane są wszystkie /ByteRange. Każdy podpis jest sprawdzany matematycznie (CryptMsg), a znacznik czasu dokumentu przez `CryptVerifyTimeStampSignature`. Gdy choć jeden jest nienaruszony, edycja jest zablokowana: w UI, w `applyEdit` i w narzędziach czatu. Użytkownik może ją świadomie odblokować z ostrzeżeniem, a przy zapisie zmian dostaje drugie potwierdzenie. Zapis bez zmian zapisuje dokładnie bajty z dysku.
- **Zaufanie** (sprawdzane w tle po wczytaniu):
  - łańcuch Windows z kontrolą unieważnienia, w chwili wskazanej przez znacznik czasu;
  - zaufany czas ze znacznika czasu;
  - unijna lista zaufanych (EUTL): z listy list UE (LOTL) pobierana jest lista kraju wystawcy, np. PL, i przechowywana w danych aplikacji przez 3 dni. Wystawca certyfikatu musi być usługą „CA/QC” o statusie „granted”, z pasującą nazwą i poprawnym podpisem certyfikatu, a sam certyfikat musi mieć rozszerzenie QcStatements. Wtedy podpis jest oznaczany jako kwalifikowany.
- **Wiele plików naraz**: klucz pobierany jest raz, a podpisane kopie trafiają obok oryginałów jako „… (podpisany).pdf”, bez nadpisywania. Miejsce widocznego podpisu użytkownik wskazuje w głównym edytorze: kolejne pliki otwierają się w kartach, a „to samo miejsce w pozostałych plikach” kończy wskazywanie. Domyślne miejsce to prawy dolny róg ostatniej strony. Postęp przekazuje kanał Tauri.

## Rozważane alternatywy

- **PKCS#11 z bibliotek SimplySign**: zależność od konkretnego dostawcy i ścieżek instalacji. CryptoAPI działa z każdym certyfikatem w Windows (karty, tokeny, SimplySign).
- **Gotowa biblioteka PAdES** (np. pyHanko, DSS): wymagałaby Pythona albo Javy w instalatorze.
- **lopdf `IncrementalDocument`**: brak kontroli nad miejscem na /Contents i /ByteRange oraz nad formatem xref.
- **Odpowiedzi OCSP w DSS**: wymagałyby własnego klienta OCSP. CRL wystarczają do weryfikacji długoterminowej i pobiera je Windows.

## Konsekwencje

- Podpis XML listy zaufanych nie jest sprawdzany. Listy pobierane są przez HTTPS z oficjalnych adresów z LOTL. Sprawdzenie zaufania wymaga sieci (bez niej używana jest ostatnia kopia list).
- Darmowy serwer znaczników czasu Certum (`time.certum.pl`) nie jest kwalifikowaną usługą na liście zaufanych. Adobe Reader może oznaczyć znacznik czasu jako „tożsamość nieznana”, choć jest nienaruszony.
- Do DSS trafiają tylko CRL, bez OCSP. Certyfikat, którego wystawca publikuje unieważnienia wyłącznie przez OCSP, nie dostanie poziomu LTA.
- Na stronie obróconej (/Rotate) widoczny podpis nie jest obracany razem ze stroną.
- Plików zaszyfrowanych (z hasłem) nie można podpisać.
- Testy jednostkowe podpisują przykładowe pliki kluczem RSA istniejącym tylko w pamięci, bez zmian w magazynie certyfikatów. Wyniki są dodatkowo sprawdzane przez OpenSSL i pyHanko, a wygląd i poprawność struktury w Acrobacie. Test LTA z realistycznym łańcuchem używa testowego CA z OpenSSL (PFX importowany bez utrwalania klucza i CRL udostępniany lokalnie). Certyfikat z podpisem własnym w pełnym LTA jest odrzucany przez Acrobat jako niepoprawny urząd, więc do takich prób nie nadaje się.
