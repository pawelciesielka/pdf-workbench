// Edycje Dokumentu wykonywane na bajtach PDF: operacje na stronach (pdf.js + backend), OCR,
// Redakcja/Korekta (zamiana strony w obraz) i Porównanie tekstu.
import { diffArrays } from "diff";
import { api, type OcrLine } from "./api";
import { loadPdf, type PDFDocumentProxy } from "./pdf";

export type Rect = [number, number, number, number]; // x0, y0, x1, y1 w przestrzeni strony PDF (bez obrotu)

export interface Mark {
  id: string;
  page: number;
  rect: Rect;
  kind: "redact" | "correct";
  text?: string;
}

const copy = (d: Uint8Array) => d.slice();
let markSeq = 0;
export const newMarkId = () => `m${++markSeq}`;

function checkPages(pages: number[], total: number) {
  const bad = pages.filter((p) => !Number.isInteger(p) || p < 1 || p > total);
  if (bad.length) throw new Error(`Dokument ma ${total} stron; nie ma stron: ${bad.join(", ")}.`);
}

const uniqSorted = (pages: number[]) => [...new Set(pages)].sort((a, b) => a - b);

export async function deletePages(doc: PDFDocumentProxy, data: Uint8Array, pages: number[]): Promise<Uint8Array> {
  const p = uniqSorted(pages);
  checkPages(p, doc.numPages);
  if (p.length >= doc.numPages) throw new Error("Nie można usunąć wszystkich stron Dokumentu.");
  return doc.extractPages([{ document: copy(data), excludePages: p.map((x) => x - 1) }]);
}

/** Przenosi strony tak, by następowały po stronie `afterPage` (0 = na początek); numery sprzed przeniesienia. */
export async function movePages(doc: PDFDocumentProxy, data: Uint8Array, pages: number[], afterPage: number): Promise<Uint8Array> {
  const n = doc.numPages;
  const moved = uniqSorted(pages);
  checkPages(moved, n);
  if (afterPage < 0 || afterPage > n) throw new Error(`Nie ma strony ${afterPage}.`);
  const remaining = Array.from({ length: n }, (_, i) => i + 1).filter((p) => !moved.includes(p));
  const idx = remaining.filter((p) => p <= afterPage).length;
  const order = [...remaining.slice(0, idx), ...moved, ...remaining.slice(idx)];
  const pageIndices = Array.from({ length: n }, (_, i) => order.indexOf(i + 1));
  return doc.extractPages([{ document: copy(data), pageIndices }]);
}

export async function extractPages(doc: PDFDocumentProxy, data: Uint8Array, pages: number[]): Promise<Uint8Array> {
  const p = uniqSorted(pages);
  checkPages(p, doc.numPages);
  return doc.extractPages([{ document: copy(data), includePages: p.map((x) => x - 1) }]);
}

/** Wstawia cały inny PDF po stronie `afterPage` (0 = na początek). */
export async function insertPdf(doc: PDFDocumentProxy, data: Uint8Array, other: Uint8Array, afterPage: number): Promise<Uint8Array> {
  return doc.extractPages([{ document: copy(data) }, { document: copy(other), insertAfter: afterPage - 1 }]);
}

export async function rotatePages(doc: PDFDocumentProxy, data: Uint8Array, pages: number[], degrees: number): Promise<Uint8Array> {
  const p = uniqSorted(pages);
  checkPages(p, doc.numPages);
  return api.rotatePages(copy(data), p, ((degrees % 360) + 360) % 360);
}

// --- Renderowanie i OCR ------------------------------------------------------------------------

async function renderUnrotated(doc: PDFDocumentProxy, pageNo: number, scale: number) {
  const page = await doc.getPage(pageNo);
  const viewport = page.getViewport({ scale, rotation: 0 });
  const canvas = document.createElement("canvas");
  canvas.width = Math.floor(viewport.width);
  canvas.height = Math.floor(viewport.height);
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  await page.render({ canvas, canvasContext: ctx, viewport }).promise;
  page.cleanup();
  return { canvas, ctx, viewport };
}

function canvasBytes(canvas: HTMLCanvasElement, type: "image/png" | "image/jpeg", quality?: number): Promise<Uint8Array> {
  return new Promise((resolve, reject) =>
    canvas.toBlob(
      (b) => (b ? b.arrayBuffer().then((a) => resolve(new Uint8Array(a)), reject) : reject(new Error("Nie można zakodować obrazu."))),
      type,
      quality,
    ),
  );
}

type Viewport = Awaited<ReturnType<typeof renderUnrotated>>["viewport"];

/** Linie OCR z pikseli obrazu do przestrzeni strony PDF. */
function linesToPdf(lines: OcrLine[], viewport: Viewport): OcrLine[] {
  return lines.map((l) => {
    const [ax, ay] = viewport.convertToPdfPoint(l.x, l.y + l.h);
    const [bx, by] = viewport.convertToPdfPoint(l.x + l.w, l.y);
    return { text: l.text, x: Math.min(ax, bx), y: Math.min(ay, by), w: Math.abs(bx - ax), h: Math.abs(by - ay) };
  });
}

async function ocrCanvas(canvas: HTMLCanvasElement, viewport: Viewport): Promise<OcrLine[]> {
  const png = await canvasBytes(canvas, "image/png");
  const result = await api.ocrImage(png);
  return linesToPdf(result.lines, viewport);
}

/** OCR wskazanych stron: dodaje niewidoczny tekst. Zwraca nowe bajty i liczbę rozpoznanych linii. */
export async function ocrPages(
  data: Uint8Array,
  pages: number[],
  onProgress?: (done: number, total: number) => void,
): Promise<{ bytes: Uint8Array; lines: number }> {
  const doc = await loadPdf(copy(data).buffer as ArrayBuffer);
  try {
    const results: { page: number; lines: OcrLine[] }[] = [];
    let count = 0;
    for (let i = 0; i < pages.length; i++) {
      onProgress?.(i, pages.length);
      const { canvas, viewport } = await renderUnrotated(doc, pages[i], 2.5);
      const lines = await ocrCanvas(canvas, viewport);
      count += lines.length;
      results.push({ page: pages[i], lines });
    }
    onProgress?.(pages.length, pages.length);
    return { bytes: await api.addTextLayer(copy(data), results), lines: count };
  } finally {
    doc.loadingTask.destroy();
  }
}

/** Strony bez warstwy tekstu (prawdopodobnie skany). */
export function scannedPages(pageTexts: string[]): number[] {
  return pageTexts.flatMap((t, i) => (t.replace(/\s/g, "").length < 20 ? [i + 1] : []));
}

function fitText(ctx: CanvasRenderingContext2D, text: string, width: number, height: number) {
  let size = Math.max(6, height * 0.78);
  ctx.font = `${size}px Arial, "Segoe UI", sans-serif`;
  while (size > 6 && ctx.measureText(text).width > width) {
    size *= 0.92;
    ctx.font = `${size}px Arial, "Segoe UI", sans-serif`;
  }
  return size;
}

/**
 * Redakcja i Korekta: strony z oznaczeniami są renderowane do obrazu (ok. 200 DPI), oznaczone miejsca
 * zaczerniane lub zakrywane nowym tekstem, strona zastępowana obrazem, a tekst odtwarzany OCR.
 */
export async function applyMarks(data: Uint8Array, marks: Mark[], onProgress?: (done: number, total: number) => void): Promise<Uint8Array> {
  const doc = await loadPdf(copy(data).buffer as ArrayBuffer);
  try {
    const pages = uniqSorted(marks.map((m) => m.page));
    const images: Uint8Array[] = [];
    const items: { page: number; offset: number; len: number; width: number; height: number }[] = [];
    const texts: { page: number; lines: OcrLine[] }[] = [];
    let offset = 0;
    for (let i = 0; i < pages.length; i++) {
      onProgress?.(i, pages.length);
      const pageNo = pages[i];
      const { canvas, ctx, viewport } = await renderUnrotated(doc, pageNo, 200 / 72);
      for (const m of marks.filter((x) => x.page === pageNo)) {
        const [x1, y1] = viewport.convertToViewportPoint(m.rect[0], m.rect[1]);
        const [x2, y2] = viewport.convertToViewportPoint(m.rect[2], m.rect[3]);
        const x = Math.min(x1, x2);
        const y = Math.min(y1, y2);
        const w = Math.abs(x2 - x1);
        const h = Math.abs(y2 - y1);
        if (m.kind === "redact") {
          ctx.fillStyle = "#000";
          ctx.fillRect(x - 1, y - 1, w + 2, h + 2);
        } else {
          ctx.fillStyle = "#fff";
          ctx.fillRect(x - 1, y - 1, w + 2, h + 2);
          ctx.fillStyle = "#000";
          ctx.textBaseline = "alphabetic";
          const size = fitText(ctx, m.text ?? "", w, h);
          ctx.fillText(m.text ?? "", x, y + h - (h - size) / 2 - size * 0.15);
        }
      }
      const jpeg = await canvasBytes(canvas, "image/jpeg", 0.9);
      images.push(jpeg);
      items.push({ page: pageNo, offset, len: jpeg.length, width: canvas.width, height: canvas.height });
      offset += jpeg.length;
      texts.push({ page: pageNo, lines: await ocrCanvas(canvas, viewport) });
    }
    onProgress?.(pages.length, pages.length);
    const body = new Uint8Array(data.length + offset);
    body.set(data, 0);
    let pos = data.length;
    for (const img of images) {
      body.set(img, pos);
      pos += img.length;
    }
    const rasterized = await api.rasterizePages(body, { pdfLen: data.length, items });
    return api.addTextLayer(rasterized, texts);
  } finally {
    doc.loadingTask.destroy();
  }
}

/** Wszystkie wystąpienia tekstu jako prostokąty w przestrzeni strony (do „Znajdź i zaczernij”). */
export async function findTextRects(doc: PDFDocumentProxy, query: string): Promise<Mark[]> {
  const q = query.trim().toLocaleLowerCase("pl");
  if (q.length < 2) return [];
  const marks: Mark[] = [];
  for (let p = 1; p <= doc.numPages; p++) {
    const page = await doc.getPage(p);
    const content = await page.getTextContent();
    for (const item of content.items) {
      if (!("str" in item) || !item.str) continue;
      const s = item.str.toLocaleLowerCase("pl");
      const [, , c, d, e, f] = item.transform as number[];
      const height = item.height || Math.hypot(c, d);
      let from = 0;
      for (;;) {
        const i = s.indexOf(q, from);
        if (i < 0) break;
        const x0 = e + (item.width * i) / s.length;
        const x1 = e + (item.width * (i + q.length)) / s.length;
        marks.push({ id: newMarkId(), page: p, kind: "redact", rect: [x0 - 1, f - height * 0.3, x1 + 1, f + height * 0.95] });
        from = i + q.length;
      }
    }
    page.cleanup();
  }
  return marks;
}

// --- Porównanie --------------------------------------------------------------------------------

export interface Change {
  kind: "added" | "removed" | "changed";
  /** Kilka słów przed zmianą (z nowej wersji, a przy usunięciu ze starej). */
  context: string;
  oldText: string;
  newText: string;
  oldPage: number | null;
  newPage: number | null;
}

interface Token {
  w: string;
  page: number;
}

function tokenize(pages: string[]): Token[] {
  return pages.flatMap((t, i) => t.split(/\s+/).filter(Boolean).map((w) => ({ w, page: i + 1 })));
}

/** Porównanie słowo po słowie całych Dokumentów (odporne na przesunięcia stron). */
export function compareTexts(oldPages: string[], newPages: string[]): Change[] {
  const a = tokenize(oldPages);
  const b = tokenize(newPages);
  const parts = diffArrays(
    a.map((t) => t.w),
    b.map((t) => t.w),
  );
  const changes: Change[] = [];
  let ia = 0;
  let ib = 0;
  let lastRemoved = false;
  for (const part of parts) {
    const n = part.count ?? part.value.length;
    if (part.removed) {
      changes.push({ kind: "removed", context: a.slice(Math.max(0, ia - 5), ia).map((x) => x.w).join(" "), oldText: part.value.join(" "), newText: "", oldPage: a[ia]?.page ?? null, newPage: b[ib]?.page ?? b[ib - 1]?.page ?? null });
      ia += n;
      lastRemoved = true;
      continue;
    } else if (part.added) {
      const prev = changes[changes.length - 1];
      const text = part.value.join(" ");
      if (prev && lastRemoved) {
        prev.kind = "changed";
        prev.newText = text;
        prev.newPage = b[ib]?.page ?? prev.newPage;
      } else {
        changes.push({ kind: "added", context: b.slice(Math.max(0, ib - 5), ib).map((x) => x.w).join(" "), oldText: "", newText: text, oldPage: a[ia]?.page ?? a[ia - 1]?.page ?? null, newPage: b[ib]?.page ?? null });
      }
      ib += n;
    } else {
      ia += n;
      ib += n;
    }
    lastRemoved = false;
  }
  return changes;
}
