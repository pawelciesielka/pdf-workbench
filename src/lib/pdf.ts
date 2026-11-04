// Inicjalizacja pdf.js. Komponenty przeglądarki (pdf_viewer.mjs) czytają rdzeń z globalThis.pdfjsLib,
// więc ładujemy je dynamicznie dopiero po jego ustawieniu.
import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "pdfjs-dist/web/pdf_viewer.css";
import type { PDFDocumentProxy } from "pdfjs-dist";

pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;
(globalThis as unknown as { pdfjsLib: typeof pdfjsLib }).pdfjsLib = pdfjsLib;

export type ViewerModule = typeof import("pdfjs-dist/web/pdf_viewer.mjs");

let viewerModule: Promise<ViewerModule> | null = null;
export function loadViewerModule(): Promise<ViewerModule> {
  viewerModule ??= import("pdfjs-dist/web/pdf_viewer.mjs");
  return viewerModule;
}

export function loadPdf(data: ArrayBuffer): Promise<PDFDocumentProxy> {
  return pdfjsLib.getDocument({
    data: new Uint8Array(data),
    cMapUrl: "/pdfjs/cmaps/",
    cMapPacked: true,
    standardFontDataUrl: "/pdfjs/standard_fonts/",
    wasmUrl: "/pdfjs/wasm/",
  }).promise;
}

/** Wyciąga tekst każdej strony (do lokalnego indeksu i trybu „wybrane strony”). */
export async function extractPageTexts(doc: PDFDocumentProxy): Promise<string[]> {
  const pages: string[] = [];
  for (let i = 1; i <= doc.numPages; i++) {
    const page = await doc.getPage(i);
    const content = await page.getTextContent();
    let text = "";
    for (const item of content.items) {
      if ("str" in item) {
        text += item.str;
        if (item.hasEOL) text += "\n";
        else if (item.str && !item.str.endsWith(" ")) text += " ";
      }
    }
    pages.push(text.replace(/[ \t]+/g, " ").trim());
    page.cleanup();
    // Oddajemy wątek UI co kilka stron, żeby przewijanie nie przycinało.
    if (i % 8 === 0) await new Promise((r) => setTimeout(r, 0));
  }
  return pages;
}

export { pdfjsLib };
export type { PDFDocumentProxy };
