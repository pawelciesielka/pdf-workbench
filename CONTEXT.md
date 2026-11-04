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

**Przestrzeń robocza** (poza MVP):
Nazwany zbiór Dokumentów, o których można prowadzić wspólne Rozmowy.
_Unikaj_: projekt, folder, workspace

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
