# 0002. Dokument rozpoznawany po treści (SHA-256), nie po ścieżce

Status: przyjęta (2026-11-04)

## Kontekst

Rozmowy są przypięte do Dokumentu. Użytkownicy przenoszą pliki i zmieniają im nazwy, a czasem nadpisują je nową wersją o tej samej nazwie. Cytaty w Rozmowie wskazują strony i tekst, więc mają sens tylko dla tej samej treści.

## Decyzja

Tożsamością Dokumentu jest skrót SHA-256 zawartości pliku. Aplikacja zapamiętuje dodatkowo, pod jakimi ścieżkami widziała dany Dokument. Gdy pod znaną ścieżką pojawia się inna treść, wcześniejsze Dokumenty z tej ścieżki są pokazywane jako **Poprzednia wersja**, a ich Rozmowy tylko do odczytu.

## Alternatywy

- **Ścieżka pliku**: prostsza, ale przeniesienie pliku gubi Rozmowy, a podmiana treści po cichu unieważnia Cytaty.
- **Ścieżka + data modyfikacji**: kruche (kopiowanie zmienia datę) i nie wykrywa przenosin.

## Konsekwencje

- Otwarcie pliku wymaga przeczytania go w całości, żeby policzyć skrót (dla typowych PDF-ów to milisekundy).
- Dwie identyczne kopie pliku to ten sam Dokument ze wspólnymi Rozmowami.
- Zmiana tej decyzji później oznacza migrację danych, stąd ten zapis.
