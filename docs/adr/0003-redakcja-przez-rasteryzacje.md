# 0003. Redakcja i Korekta przez zamianę strony w obraz

Status: przyjęta (2026-11-04)

## Kontekst

Czarny prostokąt narysowany na PDF-ie nie usuwa tekstu, obrazów ani adnotacji pod spodem; da się je skopiować lub wyciągnąć narzędziami. Prawdziwe usuwanie fragmentów ze strumienia treści PDF (tekst, wektory, obrazy częściowo zakryte) jest złożone i zawodne bez dużego silnika PDF.

## Decyzja

Strona objęta Redakcją lub Korektą jest renderowana do obrazu (ok. 200 DPI), zaczerniane (lub zakrywane i opisywane nowym tekstem) są piksele, a strona w pliku zostaje zastąpiona tym obrazem bez starych strumieni treści i adnotacji. Następnie OCR dodaje niewidoczny tekst, żeby reszta strony była przeszukiwalna. Przy zapisie plik jest przepisywany w całości (bez historii przyrostowych zapisów), a przy zapisie z Redakcją aplikacja nie tworzy kopii zapasowej z oryginałem.

## Alternatywy

- **Prostokąt jako adnotacja lub nakładka**: wygląda na zaczernienie, ale nie usuwa danych; odrzucone jako niebezpieczne.
- **Edycja strumienia treści (np. PDFium)**: zachowuje wektorową jakość, ale wymaga dużej biblioteki i nadal łatwo coś przeoczyć (np. tekst w obrazach).

## Konsekwencje

- Zredagowane strony są obrazami: większy plik, tekst tylko z OCR (z możliwymi błędami).
- Lokalny indeks i stare Rozmowy (o poprzedniej wersji) nadal zawierają niezaczerniony tekst; to dane tylko na tym komputerze.
