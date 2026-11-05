# 4. Edycja obiektów i tekstu przez PDFium

Data: 2026-11-05

## Kontekst

Użytkownik chce edytować treść stron: przesuwać, skalować i usuwać obiekty (bloki tekstu, obrazy, kształty), poprawiać tekst w miejscu i dodawać nowy tekst w treści strony (nie jako Adnotację). pdf.js tylko wyświetla i dodaje Adnotacje, a lopdf daje dostęp do surowych strumieni treści bez modelu obiektów, czcionek i kodowania. Dotychczasowa Korekta (ADR 0003) zamienia stronę w obraz, przez co strona traci jakość wektorową.

## Decyzja

- Edycją treści stron zajmuje się **PDFium** (silnik PDF z Chrome) przez crate `pdfium-render`. Biblioteka `pdfium.dll` (kompilacja bblanchon/pdfium-binaries zgodna z wersją API crate'a) jest dołączona do instalatora jako zasób i ładowana dynamicznie przy starcie.
- Operacje są bezstanowe: UI wysyła bajty Dokumentu i listę operacji `move`, `resize`, `delete`, `setText` lub `addText`, a w odpowiedzi dostaje nowe bajty. Dzięki temu cofanie i ponawianie oraz zapis działają tak samo jak przy pozostałych edycjach.
- **Brakujące znaki w czcionce.** Czcionki osadzone w podzbiorze mają tylko znaki użyte w pliku. Jeśli nowy tekst zawiera znak, którego nie ma wśród znaków tej czcionki w Dokumencie, cały obiekt tekstowy jest zastępowany nowym w podobnej czcionce systemowej (Arial, Times New Roman albo Courier New, z zachowaniem pogrubienia i kursywy) w tym samym miejscu, a użytkownik dostaje komunikat. Po każdej podmianie tekst jest odczytywany ponownie jako zabezpieczenie.
- **Korekta.** Na stronach z warstwą tekstu Korekta najpierw podmienia tekst w treści strony. Korekta przez obraz (ADR 0003) zostaje jako metoda zapasowa, gdy fragment obejmuje kilka obiektów tekstowych albo strona jest skanem. Redakcja dalej rasteryzuje stronę.
- Czat dostaje narzędzie `replace_text`, które działa jako Propozycja zmiany.

## Konsekwencje

- Instalator jest większy o około 7,5 MB. Przy aktualizacji `pdfium-render` trzeba też dobrać odpowiedni plik `pdfium.dll`.
- Tekst nie przepływa do następnej linii: edytuje się pojedynczy obiekt tekstowy, czyli zwykle wiersz lub jego fragment. Dłuższy tekst może nachodzić na sąsiedni.
- Gdy użyta zostanie czcionka zapasowa, cały plik czcionki systemowej trafia do PDF, co dodaje kilkaset KB.
- PDFium zapisuje zmienione strony z nowo wygenerowanym strumieniem treści, więc tekst z kerningiem (TJ) po edycji traci kerning.
