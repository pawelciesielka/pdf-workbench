"""Tworzy prosty, kilkustronicowy PDF testowy (umowa) bez zewnętrznych bibliotek."""
import sys

pages = [
    ["UMOWA SWIADCZENIA USLUG nr 12/2026", "", "Zawarta w dniu 3 marca 2026 r. w Warszawie pomiedzy:",
     "Alfa Sp. z o.o. (Zleceniodawca), reprezentowana przez Jana Kowalskiego,",
     "a Beta Consulting S.A. (Wykonawca), reprezentowana przez Anne Nowak.", "",
     "Par. 1 Przedmiot umowy", "Wykonawca wdrozy system obiegu dokumentow w siedzibie Zleceniodawcy."],
    ["Par. 2 Terminy", "", "Rozpoczecie prac: 1 kwietnia 2026 r.", "Odbior koncowy: do 30 wrzesnia 2026 r.",
     "Opoznienie powyzej 14 dni uprawnia Zleceniodawce do odstapienia od umowy.", "",
     "Par. 3 Wynagrodzenie", "Wynagrodzenie ryczaltowe wynosi 240 000 zl netto,",
     "platne w trzech ratach po 80 000 zl, w terminie 21 dni od daty faktury."],
    ["Par. 4 Kary umowne", "", "Za kazdy dzien opoznienia Wykonawca zaplaci kare 0,2% wynagrodzenia,",
     "lacznie nie wiecej niz 10% wynagrodzenia ryczaltowego.", "",
     "Par. 5 Poufnosc", "Strony zachowaja w tajemnicy informacje uzyskane w zwiazku z umowa",
     "przez 5 lat od jej zakonczenia.", "", "Podpisy stron: ____________   ____________"],
]

objs = []
def add(o):
    objs.append(o)
    return len(objs)

font = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
pages_id = len(objs) + 1 + 2 * len(pages)  # zarezerwowane po stronach
kids = []
for lines in pages:
    ops = ["BT", "/F1 13 Tf", "16 TL", "72 760 Td"]
    for ln in lines:
        ops.append("(" + ln.replace("\\", "\\\\").replace("(", r"\(").replace(")", r"\)") + ") Tj T*")
    ops.append("ET")
    stream = "\n".join(ops).encode("latin-1")
    content = add(b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream")
    page = add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 %d 0 R >> >> /Contents %d 0 R >>" % (pages_id, font, content))
    kids.append(page)
assert add(b"<< /Type /Pages /Kids [" + b" ".join(b"%d 0 R" % k for k in kids) + b"] /Count %d >>" % len(kids)) == pages_id
catalog = add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages_id)

out = bytearray(b"%PDF-1.4\n")
offsets = []
for i, o in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for off in offsets:
    out += b"%010d 00000 n \n" % off
out += b"trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, catalog, xref)
open(sys.argv[1], "wb").write(out)
print("written", sys.argv[1], len(out), "bytes")
