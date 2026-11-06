# 5. Podpis PAdES przez CryptoAPI Windows i blokada Podpisanych dokumentów

Data: 2026-11-06

## Kontekst

Użytkownik podpisuje dokumenty kwalifikowanym certyfikatem Certum w SimplySign (podpis w chmurze, odblokowany tokenem w SimplySign Desktop). Dostarczany program SimplySign SmartSign jest dla niego zbyt słaby, więc chce podpisywać PDF-y w PDF Workbench. SimplySign Desktop rejestruje certyfikaty w osobistym magazynie certyfikatów Windows jako kartę wirtualną, więc każdy program może nimi podpisywać przez standardowe API Windows. Druga potrzeba: edycja Podpisanego dokumentu po cichu unieważnia podpis, a dotychczasowy zapis (lopdf, PDFium, pdf.js) przepisuje plik.

## Decyzja

- **Podpis przez CryptoAPI**, bez PKCS#11 i bez bibliotek od Certum. Certyfikat wybierany jest z magazynu „Osobisty” bieżącego użytkownika. Klucz pobiera `CryptAcquireCertificatePrivateKey` (preferowany CNG), a podpis CMS buduje `CryptMsg*`. Sam podpis wykonuje dostawca klucza (u użytkownika SimplySign), więc klucz prywatny nigdy nie trafia do programu.
- **Format PAdES (ETSI.CAdES.detached)**: SHA-256, podpisany atrybut signing-certificate-v2, dołączone certyfikaty pośrednie i opcjonalny znacznik czasu RFC 3161 z `time.certum.pl` (`CryptRetrieveTimeStamp`). Powstaje poziom PAdES-B-T, a bez znacznika czasu PAdES-B-B.
- **Aktualizacja przyrostowa pisana ręcznie**: oryginalne bajty pliku zostają nietknięte, a na końcu dopisywane są słownik podpisu, widżet pola, zmieniona strona, AcroForm lub katalog oraz xref w tym samym formacie co poprzedni (tablica albo strumień). Dzięki temu kolejne podpisy nie unieważniają wcześniejszych. Miejsce na podpis CMS ma 30 000 bajtów.
- **Widoczny podpis** ma warstwy wyglądu Acrobata (n0–n4 w formie /FRM). Adobe Reader podmienia warstwę n1 na ikonę ważności (zielony znaczek, „?” albo „✖”), a n4 na tekst stanu w górnym pasku, np. „Podpis jest prawidłowy”. Inne przeglądarki pokazują tylko warstwę n2: ramkę i wiersze tekstu w standardowej czcionce Helvetica z kodowaniem /Differences dla polskich liter, bez osadzania czcionki.
- **Wykrywanie i blokada**: przy wczytaniu Dokumentu odczytywane są wszystkie /ByteRange, a każdy podpis jest sprawdzany matematycznie (CryptMsg, bez łańcucha zaufania). Gdy choć jeden jest nienaruszony, edycja jest zablokowana: w UI, w `applyEdit` i w narzędziach czatu. Użytkownik może ją świadomie odblokować z ostrzeżeniem, a przy zapisie zmian dostaje drugie potwierdzenie. Zapis bez zmian zapisuje dokładnie bajty z dysku.

## Rozważane alternatywy

- **PKCS#11 z bibliotek SimplySign**: zależność od konkretnego dostawcy i ścieżek instalacji. CryptoAPI działa z każdym certyfikatem w Windows (karty, tokeny, SimplySign).
- **Gotowa biblioteka PAdES** (np. pyHanko, DSS): wymagałaby Pythona albo Javy w instalatorze.
- **lopdf `IncrementalDocument`**: brak kontroli nad miejscem na /Contents i /ByteRange oraz nad formatem xref.

## Konsekwencje

- Program sprawdza tylko integralność podpisu. Zaufanie do certyfikatu (łańcuch, listy zaufania UE, unieważnienie) weryfikuje odbiorca, np. w Adobe Acrobat Reader albo na gov.pl.
- Nie ma jeszcze PAdES-LT/LTA (dane do długoterminowej weryfikacji w słowniku DSS) ani podpisywania wielu plików naraz.
- Na stronie obróconej (/Rotate) widoczny podpis nie jest obracany razem ze stroną.
- Plików zaszyfrowanych (z hasłem) nie można podpisać.
- Test jednostkowy podpisuje przykładowe pliki kluczem RSA istniejącym tylko w pamięci i certyfikatem z podpisem własnym. Nie zmienia to magazynu certyfikatów, a wynik jest dodatkowo sprawdzany przez OpenSSL.
