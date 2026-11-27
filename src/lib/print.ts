// Drukowanie Dokumentu: pdf.js renderuje strony (z niezapisanymi Adnotacjami i polami formularza) do obrazów,
// które trafiają do ukrytego kontenera widocznego tylko przy wydruku, po czym otwieramy systemowe okno drukowania.
import { pdfjsLib, type PDFDocumentProxy } from "./pdf";

const PRINT_DPI = 200;

export async function printDocument(doc: PDFDocumentProxy, onProgress?: (done: number, total: number) => void): Promise<void> {
  const root = document.createElement("div");
  root.id = "print-root";
  const urls: string[] = [];
  const storage = doc.annotationStorage.print;
  try {
    // Rozmiar strony wydruku ustawiamy według pierwszej strony, żeby przeglądarka nie dokładała marginesów.
    const first = await doc.getPage(1);
    const [, , w0, h0] = first.view;
    const rot0 = first.rotate % 180 !== 0;
    const style = document.createElement("style");
    style.textContent = `@page { size: ${rot0 ? h0 : w0}pt ${rot0 ? w0 : h0}pt; margin: 0; }`;
    root.appendChild(style);

    for (let i = 1; i <= doc.numPages; i++) {
      onProgress?.(i - 1, doc.numPages);
      const page = await doc.getPage(i);
      const viewport = page.getViewport({ scale: PRINT_DPI / 72 });
      const canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width);
      canvas.height = Math.floor(viewport.height);
      const ctx = canvas.getContext("2d")!;
      ctx.fillStyle = "#fff";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      await page.render({
        canvas,
        canvasContext: ctx,
        viewport,
        intent: "print",
        annotationMode: pdfjsLib.AnnotationMode.ENABLE_STORAGE,
        printAnnotationStorage: storage,
      }).promise;
      const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
      canvas.width = canvas.height = 0;
      page.cleanup();
      if (!blob) throw new Error(`Nie udało się przygotować strony ${i} do druku.`);
      const url = URL.createObjectURL(blob);
      urls.push(url);
      const img = document.createElement("img");
      img.src = url;
      const sheet = document.createElement("div");
      sheet.className = "print-page";
      sheet.appendChild(img);
      root.appendChild(sheet);
    }
    document.body.appendChild(root);
    await Promise.all(Array.from(root.querySelectorAll("img"), (img) => img.decode().catch(() => undefined)));
    document.body.classList.add("printing");
    // W Chromium print() blokuje do zamknięcia okna, ale sprzątamy dopiero po „afterprint” na wypadek, gdyby nie blokowało.
    await new Promise<void>((resolve) => {
      window.addEventListener("afterprint", () => resolve(), { once: true });
      window.print();
    });
  } finally {
    document.body.classList.remove("printing");
    root.remove();
    urls.forEach((u) => URL.revokeObjectURL(u));
  }
}
