# PDF Workbench

Aplikacja desktopowa do przeglądania plików PDF i rozmawiania o ich treści z AI, a w kolejnych etapach do ich edycji.

## Język

### Dokumenty

**Dokument**:
PDF o określonej treści, rozpoznawany po zawartości, a nie po ścieżce czy nazwie pliku; plik ze zmienioną treścią to nowy Dokument.
_Unikaj_: plik, PDF (w znaczeniu domenowym)

**Poprzednia wersja**:
Dokument, który wcześniej był pod tą samą ścieżką co obecny, zanim zmieniono treść pliku; jego Rozmowy są dostępne tylko do odczytu.
_Unikaj_: stara wersja, historia pliku

**Przestrzeń robocza**:
Nazwany zbiór Dokumentów, o których można prowadzić wspólne Rozmowy; Rozmowa zapamiętuje skład Dokumentów z chwili jej rozpoczęcia.
_Unikaj_: projekt, folder, workspace

**Wersja z edycji**:
Poprzednia wersja powstała dlatego, że użytkownik zapisał zmiany w aplikacji; pamięta, z którego Dokumentu powstała i kiedy.
_Unikaj_: rewizja, kopia

### Edycja

**Edycja**:
Niezapisana zmiana Dokumentu w aplikacji (Adnotacja, operacja na stronach, zmiana Obiektu strony, wypełnienie formularza, OCR, Redakcja, Korekta); staje się nowym Dokumentem dopiero po zapisie.
_Unikaj_: modyfikacja, zmiana pliku

**Adnotacja**:
Element dodany na wierzch strony (podświetlenie, notatka tekstowa, rysunek, obraz), który nie zmienia treści strony.
_Unikaj_: komentarz, znacznik

**Redakcja**:
Trwałe usunięcie wskazanych fragmentów z Dokumentu: strona jest zastępowana obrazem z zaczernionymi miejscami, a pozostały tekst odtwarza OCR.
_Unikaj_: zaczernienie, anonimizacja, maskowanie

**Korekta**:
Zastąpienie wskazanego fragmentu tekstu nowym tekstem w tym samym miejscu strony. Gdy fragmentu nie da się zmienić w treści strony (skan, tekst rozbity na części), strona staje się obrazem jak przy Redakcji.
_Unikaj_: poprawka

**Obiekt strony**:
Element treści strony (fragment tekstu, obraz, kształt), który można zaznaczyć, przesunąć, przeskalować, usunąć, a w przypadku tekstu także przepisać. W odróżnieniu od Adnotacji należy do samej strony.
_Unikaj_: element, warstwa

**Czcionka zastępcza**:
Podobna czcionka systemowa użyta w zmienionym tekście, gdy czcionka Dokumentu nie ma potrzebnych znaków (np. polskich liter).
_Unikaj_: font fallback

**OCR**:
Rozpoznanie tekstu na stronie bez warstwy tekstu (skan) i dodanie niewidocznego tekstu, dzięki któremu strona jest przeszukiwalna.
_Unikaj_: skanowanie, rozpoznawanie

**Porównanie**:
Zestawienie tekstu dwóch Dokumentów słowo po słowie, ze wskazaniem stron, na których coś dodano, usunięto lub zmieniono.
_Unikaj_: diff, różnice

**Propozycja zmiany**:
Edycja zaproponowana przez AI w Rozmowie, która czeka na „Zastosuj” użytkownika (usunięcie lub przestawienie stron, Redakcja, zamiana tekstu).
_Unikaj_: sugestia, akcja AI

### Rozmowy

**Rozmowa**:
Zapamiętany czat z AI przypięty do jednego Dokumentu; Dokument może mieć wiele Rozmów.
_Unikaj_: czat, sesja, wątek

**Cytat**:
Wskazany fragment Dokumentu (strona i tekst), na którym opiera się odpowiedź w Rozmowie.
_Unikaj_: źródło, odnośnik, przypis

**Odpowiedź spoza dokumentu**:
Część odpowiedzi oparta na ogólnej wiedzy modelu, a nie na treści Dokumentu, zawsze tak oznaczona.
_Unikaj_: halucynacja, wiedza ogólna

**Szybka akcja**:
Gotowe, jednym kliknięciem uruchamiane pytanie do Dokumentu (np. „Streść”, „Wypisz kluczowe daty i kwoty”).
_Unikaj_: szablon, prompt, makro

**Zapytanie o zaznaczenie**:
Pytanie w Rozmowie dotyczące fragmentu tekstu zaznaczonego przez użytkownika w podglądzie.
_Unikaj_: kontekst, wklejka
