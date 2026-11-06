import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import type { Mark, Rect } from "../lib/editing";
import { MARK_FONTS, newMarkId } from "../lib/editing";
import type { EditOp, PageObject } from "../lib/api";
import { loadViewerModule, pdfjsLib, type PDFDocumentProxy } from "../lib/pdf";
import { ObjectLayer, type LayerGeom, type TextStyle } from "./ObjectLayer";

type Mod = Awaited<ReturnType<typeof loadViewerModule>>;
type Viewer = InstanceType<Mod["PDFViewer"]>;
type Bus = InstanceType<Mod["EventBus"]>;
// Menedżer edytorów adnotacji pdf.js (typ wewnętrzny biblioteki).
type UiManager = { undo(): void; redo(): void; highlightSelection(method?: string): void };

export type EditorMode = "none" | "highlight" | "freetext" | "ink" | "stamp" | "redact" | "objects" | "addtext" | "signbox";

/** Tryby obsługiwane przez naszą nakładkę (pdf.js w trybie bez edytora). */
const OVERLAY_MODES = new Set<EditorMode>(["redact", "objects", "addtext", "signbox"]);

const EDITOR_TYPES: Record<Exclude<EditorMode, "redact" | "objects" | "addtext" | "signbox">, number> = {
  none: pdfjsLib.AnnotationEditorType.NONE,
  highlight: pdfjsLib.AnnotationEditorType.HIGHLIGHT,
  freetext: pdfjsLib.AnnotationEditorType.FREETEXT,
  ink: pdfjsLib.AnnotationEditorType.INK,
  stamp: pdfjsLib.AnnotationEditorType.STAMP,
};

export const HIGHLIGHT_COLORS: Record<string, string> = {
  yellow: "#FFFF98",
  green: "#53FFBC",
  blue: "#80EBFF",
  pink: "#FFCBE6",
};

export interface PdfViewerHandle {
  goToPage(n: number): void;
  currentPage(): number;
  zoomIn(): void;
  zoomOut(): void;
  fit(mode: "page-width" | "page-fit"): void;
  find(query: string, opts?: { again?: boolean; previous?: boolean }): void;
  clearFind(): void;
  showCitation(page: number, text: string): void;
  refresh(): void;
  undoAnnotation(): boolean;
  redoAnnotation(): boolean;
  setHighlightColor(color: string): void;
  /** Kolor i rozmiar Notatki tekstowej: dla zaznaczonych notatek i kolejnych nowych. */
  setFreeTextStyle(style: { color?: string; size?: number }): void;
  highlightText(page: number, text: string, color: string): Promise<void>;
  selectionMarks(kind: Mark["kind"]): Mark[];
  /** Rozszerza zaznaczenie do całych słów; zwraca tekst i przybliżony krój. */
  expandSelectionToWords(): { text: string; font: "serif" | "sans" } | null;
}

export interface SelectionInfo {
  page: number;
  text: string;
  x: number;
  y: number;
}

interface Props {
  doc: PDFDocumentProxy;
  initialPage?: number;
  editorMode: EditorMode;
  marks: Mark[];
  onAddMark(m: Mark): void;
  onRemoveMark(id: string): void;
  onPageChange(page: number): void;
  onScaleChange(scale: number): void;
  onFindCount(current: number, total: number): void;
  onSelection(sel: SelectionInfo | null): void;
  onEditorState(s: { canUndo: boolean; canRedo: boolean }): void;
  textStyle: TextStyle;
  objectsFor(page: number): Promise<PageObject[]>;
  onObjectOps(ops: EditOp[]): Promise<void>;
  /** Tryb „signbox”: narysowany prostokąt miejsca na widoczny podpis (punkty PDF). */
  onSignBox?(page: number, rect: Mark["rect"]): void;
}

const MIN_SCALE = 0.25;
const MAX_SCALE = 6;

function normalize(text: string) {
  return text.replace(/\s+/g, " ").trim();
}

/** Szuka tekstu w warstwie tekstu strony i zwraca zakres DOM (najpierw z odstępami, potem bez). */
function findRange(layer: HTMLElement, query: string): Range | null {
  const nodes: Text[] = [];
  const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n as Text);
  for (const strip of [false, true]) {
    let hay = "";
    const map: [Text, number][] = [];
    let prevSpace = true;
    for (const node of nodes) {
      const data = node.data;
      for (let i = 0; i < data.length; i++) {
        const ch = data[i];
        if (/\s/.test(ch)) {
          if (strip || prevSpace) continue;
          hay += " ";
          prevSpace = true;
        } else {
          hay += ch.toLocaleLowerCase("pl");
          prevSpace = false;
        }
        map.push([node, i]);
      }
      if (!strip && !prevSpace) {
        hay += " ";
        map.push([node, data.length]);
        prevSpace = true;
      }
    }
    const q = strip ? query.replace(/\s+/g, "").toLocaleLowerCase("pl") : normalize(query).toLocaleLowerCase("pl");
    const at = hay.indexOf(q);
    if (at >= 0 && q.length > 0) {
      const [sn, so] = map[at];
      const [en, eo] = map[at + q.length - 1];
      const range = document.createRange();
      range.setStart(sn, so);
      range.setEnd(en, Math.min(eo + 1, en.data.length));
      return range;
    }
  }
  return null;
}

export const PdfViewer = forwardRef<PdfViewerHandle, Props>(function PdfViewer(props, ref) {
  const containerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const viewerRef = useRef<Viewer | null>(null);
  const busRef = useRef<Bus | null>(null);
  const uiRef = useRef<UiManager | null>(null);
  const editorState = useRef({ canUndo: false, canRedo: false });
  const [layout, setLayout] = useState(0); // zmiana wymusza przeliczenie nakładki oznaczeń
  const [draft, setDraft] = useState<{ page: number; x0: number; y0: number; x1: number; y1: number } | null>(null);
  const cb = useRef(props);
  cb.current = props;
  /** Pozycja widoku sprzed przeładowania Dokumentu (po edycji wracamy w to samo miejsce). */
  const savedView = useRef<{ scale: number; top: number; left: number; pages: number } | null>(null);
  const [currentPage, setCurrentPage] = useState(1);

  useEffect(() => {
    let disposed = false;
    (async () => {
      const mod = await loadViewerModule();
      if (disposed || !containerRef.current || !innerRef.current) return;
      const eventBus = new mod.EventBus();
      const linkService = new mod.PDFLinkService({ eventBus });
      const findController = new mod.PDFFindController({ eventBus, linkService });
      const viewer = new mod.PDFViewer({
        container: containerRef.current,
        viewer: innerRef.current,
        eventBus,
        linkService,
        findController,
        annotationEditorMode: pdfjsLib.AnnotationEditorType.NONE,
        annotationEditorHighlightColors: Object.entries(HIGHLIGHT_COLORS)
          .map(([k, v]) => `${k}=${v}`)
          .join(","),
      });
      linkService.setViewer(viewer);
      eventBus.on("pagesinit", () => {
        const saved = savedView.current;
        savedView.current = null;
        if (saved && saved.pages === viewer.pagesCount) {
          viewer.currentScale = saved.scale;
          requestAnimationFrame(() => {
            if (containerRef.current) {
              containerRef.current.scrollTop = saved.top;
              containerRef.current.scrollLeft = saved.left;
            }
          });
        } else {
          viewer.currentScaleValue = "page-width";
          if (cb.current.initialPage && cb.current.initialPage > 1) viewer.currentPageNumber = Math.min(cb.current.initialPage, viewer.pagesCount);
        }
        setCurrentPage(viewer.currentPageNumber);
        applyMode(cb.current.editorMode);
      });
      // pdf.js przesuwa tylko zaznaczone Adnotacje: pierwsze przeciągnięcie jedynie zaznacza.
      // Zaznaczamy więc Adnotację już przy wciśnięciu przycisku — da się ją złapać i przesunąć od razu.
      containerRef.current.addEventListener(
        "pointerdown",
        (e) => {
          if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) return;
          const target = e.target as HTMLElement;
          if (target.closest(".editToolbar, [contenteditable='true']")) return;
          const div = target.closest<HTMLElement>(".annotationEditorLayer [id^='pdfjs_internal_editor_']");
          const ui = uiRef.current as unknown as { getEditor(id: string): { isSelected: boolean } | undefined; setSelected(ed: unknown): void } | null;
          const ed = div && ui?.getEditor(div.id);
          if (ed && !ed.isSelected) ui!.setSelected(ed);
        },
        { capture: true },
      );
      // Notatka tekstowa (FreeText) to sam tekst na stronie: pdf.js dokleja do niej żółte okienko
      // z tą samą treścią (jak do komentarza), które tylko zasłania stronę — usuwamy je.
      eventBus.on("annotationlayerrendered", (e: { source: { div: HTMLElement } }) => {
        for (const el of e.source.div.querySelectorAll<HTMLElement>(".annotationLayer .freeTextAnnotation")) {
          el.classList.remove("popupTriggerArea");
          el.removeAttribute("aria-haspopup");
          const id = el.dataset.annotationId;
          if (id) e.source.div.querySelector(`.annotationLayer [data-annotation-id="popup_${CSS.escape(id)}"]`)?.remove();
        }
      });
      eventBus.on("annotationeditoruimanager",(e: { uiManager: UiManager }) => {
        uiRef.current = e.uiManager;
        if (import.meta.env.DEV) (window as unknown as { __pdfui: unknown }).__pdfui = e.uiManager;
      });
      // pdf.js prosi aplikację o zmianę trybu (np. przy podświetlaniu zaznaczenia) — obsługujemy to sami.
      eventBus.on("showannotationeditorui", (e: { mode: number }) => {
        viewer.annotationEditorMode = { mode: e.mode };
      });
      eventBus.on("editingstateschanged", (e: { details: { hasSomethingToUndo?: boolean; hasSomethingToRedo?: boolean } }) => {
        editorState.current = { canUndo: !!e.details.hasSomethingToUndo, canRedo: !!e.details.hasSomethingToRedo };
        cb.current.onEditorState(editorState.current);
      });
      eventBus.on("pagechanging", (e: { pageNumber: number }) => {
        setCurrentPage(e.pageNumber);
        cb.current.onPageChange(e.pageNumber);
      });
      eventBus.on("scalechanging", (e: { scale: number }) => {
        cb.current.onScaleChange(e.scale);
        setTimeout(() => setLayout((n) => n + 1), 0);
      });
      eventBus.on("pagesloaded", () => setLayout((n) => n + 1));
      const onCount = (e: { matchesCount?: { current: number; total: number } }) => {
        if (e.matchesCount) cb.current.onFindCount(e.matchesCount.current, e.matchesCount.total);
      };
      eventBus.on("updatefindmatchescount", onCount);
      eventBus.on("updatefindcontrolstate", onCount);
      viewer.setDocument(props.doc);
      linkService.setDocument(props.doc, null);
      viewerRef.current = viewer;
      busRef.current = eventBus;
    })();
    return () => {
      disposed = true;
      const v = viewerRef.current;
      const c = containerRef.current;
      if (v && c && v.pagesCount) savedView.current = { scale: v.currentScale, top: c.scrollTop, left: c.scrollLeft, pages: v.pagesCount };
      viewerRef.current?.setDocument(null as unknown as PDFDocumentProxy);
      viewerRef.current = null;
      uiRef.current = null;
    };
  }, [props.doc]);

  const applyMode = (mode: EditorMode) => {
    const v = viewerRef.current;
    if (!v) return;
    const type = OVERLAY_MODES.has(mode) ? EDITOR_TYPES.none : EDITOR_TYPES[mode as keyof typeof EDITOR_TYPES];
    try {
      if ((v.annotationEditorMode as unknown as number) !== type) v.annotationEditorMode = { mode: type };
    } catch {
      /* dokument jeszcze się ładuje — tryb zostanie ustawiony po pagesinit */
    }
  };
  useEffect(() => applyMode(props.editorMode), [props.editorMode]);

  // Zaznaczenie tekstu → pływające przyciski (pytanie, Korekta, Redakcja).
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onUp = () => {
      setTimeout(() => {
        if (cb.current.editorMode !== "none") return;
        const sel = window.getSelection();
        const text = sel ? normalize(sel.toString()) : "";
        if (!sel || text.length < 2 || sel.rangeCount === 0 || !el.contains(sel.anchorNode)) {
          cb.current.onSelection(null);
          return;
        }
        const node = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode?.parentElement;
        const pageEl = node?.closest<HTMLElement>(".page");
        const page = Number(pageEl?.dataset.pageNumber ?? viewerRef.current?.currentPageNumber ?? 1);
        const rect = sel.getRangeAt(0).getBoundingClientRect();
        cb.current.onSelection({ page, text, x: rect.left + rect.width / 2, y: rect.top });
      }, 0);
    };
    const onScroll = () => cb.current.onSelection(null);
    el.addEventListener("mouseup", onUp);
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("mouseup", onUp);
      el.removeEventListener("scroll", onScroll);
    };
  }, []);

  // Ctrl + kółko myszy = powiększenie.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey || !viewerRef.current) return;
      e.preventDefault();
      const v = viewerRef.current;
      v.currentScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.currentScale * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  /** Strona pod punktem ekranu i jej prostokąt. */
  const pageAt = (x: number, y: number): { page: number; rect: DOMRect } | null => {
    const pages = innerRef.current?.querySelectorAll<HTMLElement>(".page") ?? [];
    for (const p of pages) {
      const r = p.getBoundingClientRect();
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return { page: Number(p.dataset.pageNumber), rect: r };
    }
    return null;
  };

  const clientToPdf = useCallback((page: number, pageRect: DOMRect, ax: number, ay: number, bx: number, by: number): Rect | null => {
    const view = viewerRef.current?.getPageView(page - 1);
    if (!view) return null;
    const [px0, py0] = view.viewport.convertToPdfPoint(ax - pageRect.left, ay - pageRect.top);
    const [px1, py1] = view.viewport.convertToPdfPoint(bx - pageRect.left, by - pageRect.top);
    return [Math.min(px0, px1), Math.min(py0, py1), Math.max(px0, px1), Math.max(py0, py1)];
  }, []);

  // Rysowanie prostokątów Redakcji.
  const onLayerDown = (e: React.MouseEvent) => {
    if (props.editorMode !== "redact" && props.editorMode !== "signbox") return;
    const hit = pageAt(e.clientX, e.clientY);
    if (!hit) return;
    e.preventDefault();
    setDraft({ page: hit.page, x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY });
  };
  const onLayerMove = (e: React.MouseEvent) => {
    if (draft) setDraft({ ...draft, x1: e.clientX, y1: e.clientY });
  };
  const onLayerUp = () => {
    if (!draft) return;
    const pageEl = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${draft.page}"]`);
    if (pageEl && Math.abs(draft.x1 - draft.x0) > 4 && Math.abs(draft.y1 - draft.y0) > 4) {
      const r = pageEl.getBoundingClientRect();
      const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));
      const rect = clientToPdf(
        draft.page,
        r,
        clamp(draft.x0, r.left, r.right),
        clamp(draft.y0, r.top, r.bottom),
        clamp(draft.x1, r.left, r.right),
        clamp(draft.y1, r.top, r.bottom),
      );
      if (rect && cb.current.editorMode === "signbox") cb.current.onSignBox?.(draft.page, rect);
      else if (rect) cb.current.onAddMark({ id: newMarkId(), page: draft.page, rect, kind: "redact" });
    }
    setDraft(null);
  };

  // Położenie oznaczeń w pikselach nakładki.
  const markBoxes = (() => {
    void layout;
    const layer = layerRef.current;
    const v = viewerRef.current;
    if (!layer || !v) return [];
    const lr = layer.getBoundingClientRect();
    return props.marks.flatMap((m) => {
      const pageEl = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${m.page}"]`);
      const view = v.getPageView(m.page - 1);
      if (!pageEl || !view) return [];
      const pr = pageEl.getBoundingClientRect();
      const [x1, y1] = view.viewport.convertToViewportPoint(m.rect[0], m.rect[1]);
      const [x2, y2] = view.viewport.convertToViewportPoint(m.rect[2], m.rect[3]);
      return [{ m, left: pr.left - lr.left + Math.min(x1, x2), top: pr.top - lr.top + Math.min(y1, y2), width: Math.abs(x2 - x1), height: Math.abs(y2 - y1) }];
    });
  })();

  useEffect(() => {
    const onResize = () => setLayout((n) => n + 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useImperativeHandle(ref, () => {
    const dispatchFind = (query: string, again: boolean, previous: boolean) =>
      busRef.current?.dispatch("find", {
        source: null,
        type: again ? "again" : "",
        query,
        caseSensitive: false,
        entireWord: false,
        highlightAll: true,
        findPrevious: previous,
        matchDiacritics: false,
      });
    const waitForTextLayer = (page: number) =>
      new Promise<HTMLElement>((resolve, reject) => {
        const check = () => {
          const layer = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${page}"] .textLayer`);
          if (layer && layer.querySelector("span")) {
            resolve(layer);
            return true;
          }
          return false;
        };
        if (check()) return;
        const bus = busRef.current;
        const onRendered = (e: { pageNumber: number }) => {
          if (e.pageNumber === page && check()) {
            bus?.off("textlayerrendered", onRendered);
            clearTimeout(timer);
          }
        };
        bus?.on("textlayerrendered", onRendered);
        const timer = setTimeout(() => {
          bus?.off("textlayerrendered", onRendered);
          if (!check()) reject(new Error(`Strona ${page} nie ma warstwy tekstu (może to skan — użyj OCR).`));
        }, 5000);
      });
    return {
      goToPage(n) {
        const v = viewerRef.current;
        if (v) v.currentPageNumber = Math.min(Math.max(1, n), v.pagesCount);
      },
      currentPage: () => viewerRef.current?.currentPageNumber ?? 1,
      zoomIn() {
        const v = viewerRef.current;
        if (v) v.currentScale = Math.min(MAX_SCALE, v.currentScale * 1.2);
      },
      zoomOut() {
        const v = viewerRef.current;
        if (v) v.currentScale = Math.max(MIN_SCALE, v.currentScale / 1.2);
      },
      fit(mode) {
        if (viewerRef.current) viewerRef.current.currentScaleValue = mode;
      },
      find(query, opts) {
        dispatchFind(query, !!opts?.again, !!opts?.previous);
      },
      clearFind() {
        busRef.current?.dispatch("findbarclose", { source: null });
      },
      showCitation(page, text) {
        const v = viewerRef.current;
        if (!v) return;
        v.currentPageNumber = page;
        const snippet = normalize(text).split(" ").slice(0, 10).join(" ");
        if (snippet.length >= 3) setTimeout(() => dispatchFind(snippet, false, false), 50);
      },
      refresh() {
        const v = viewerRef.current;
        if (!v) return;
        const value = v.currentScaleValue;
        v.currentScaleValue = value;
        v.update();
        setLayout((n) => n + 1);
      },
      undoAnnotation() {
        if (!uiRef.current || !editorState.current.canUndo) return false;
        uiRef.current.undo();
        return true;
      },
      redoAnnotation() {
        if (!uiRef.current || !editorState.current.canRedo) return false;
        uiRef.current.redo();
        return true;
      },
      setHighlightColor(color) {
        busRef.current?.dispatch("switchannotationeditorparams", {
          source: null,
          type: pdfjsLib.AnnotationEditorParamsType.HIGHLIGHT_COLOR,
          value: HIGHLIGHT_COLORS[color] ?? color,
        });
      },
      setFreeTextStyle({ color, size }) {
        const bus = busRef.current;
        if (color) bus?.dispatch("switchannotationeditorparams", { source: null, type: pdfjsLib.AnnotationEditorParamsType.FREETEXT_COLOR, value: color });
        if (size) bus?.dispatch("switchannotationeditorparams", { source: null, type: pdfjsLib.AnnotationEditorParamsType.FREETEXT_SIZE, value: size });
      },
      async highlightText(page, text, color) {
        const v = viewerRef.current;
        const ui = uiRef.current;
        if (!v || !ui) throw new Error("Podgląd nie jest gotowy.");
        if (page < 1 || page > v.pagesCount) throw new Error(`Dokument ma ${v.pagesCount} stron.`);
        v.currentPageNumber = page;
        const layer = await waitForTextLayer(page);
        const range = findRange(layer, text);
        if (!range) throw new Error(`Nie znaleziono tekstu „${text.slice(0, 60)}” na stronie ${page}.`);
        this.setHighlightColor(color);
        const sel = window.getSelection()!;
        sel.removeAllRanges();
        sel.addRange(range);
        ui.highlightSelection("main_toolbar");
        // pdf.js przełącza się asynchronicznie w tryb podświetlania i dopiero wtedy tworzy edytor.
        await new Promise((r) => setTimeout(r, 450));
        sel.removeAllRanges();
        applyMode(cb.current.editorMode);
      },
      expandSelectionToWords() {
        const sel = window.getSelection();
        if (!sel || sel.rangeCount === 0) return null;
        const r = sel.getRangeAt(0);
        const isWord = (ch: string | undefined) => !!ch && !/\s/.test(ch);
        const range = r.cloneRange();
        if (range.startContainer.nodeType === Node.TEXT_NODE) {
          const t = range.startContainer as Text;
          let o = range.startOffset;
          while (o > 0 && isWord(t.data[o - 1])) o--;
          range.setStart(t, o);
        }
        if (range.endContainer.nodeType === Node.TEXT_NODE) {
          const t = range.endContainer as Text;
          let o = range.endOffset;
          while (o < t.data.length && isWord(t.data[o])) o++;
          range.setEnd(t, o);
        }
        sel.removeAllRanges();
        sel.addRange(range);
        const el = (range.startContainer.nodeType === Node.TEXT_NODE ? range.startContainer.parentElement : (range.startContainer as Element)) as HTMLElement | null;
        const fam = el ? getComputedStyle(el).fontFamily.toLowerCase() : "";
        const font = /sans|arial|helvetica|calibri|verdana|segoe/.test(fam) ? "sans" : /serif|times|cambria|georgia|garamond/.test(fam) ? "serif" : "sans";
        return { text: normalize(sel.toString()), font };
      },
      selectionMarks(kind) {
        const sel = window.getSelection();
        if (!sel || sel.rangeCount === 0) return [];
        const out: Mark[] = [];
        for (const r of sel.getRangeAt(0).getClientRects()) {
          if (r.width < 1 || r.height < 1) continue;
          const hit = pageAt(r.left + r.width / 2, r.top + r.height / 2);
          if (!hit) continue;
          const rect = clientToPdf(hit.page, hit.rect, r.left, r.top, r.right, r.bottom);
          if (rect) out.push({ id: newMarkId(), page: hit.page, rect, kind });
        }
        return out;
      },
    };
  }, [clientToPdf]);

  // Geometria dla warstwy obiektów (układ nakładki ↔ przestrzeń PDF).
  const geom: LayerGeom = {
    pageAt(lx, ly) {
      const lr = layerRef.current?.getBoundingClientRect();
      return lr ? (pageAt(lx + lr.left, ly + lr.top)?.page ?? null) : null;
    },
    toPdf(page, lx, ly) {
      const lr = layerRef.current?.getBoundingClientRect();
      const pageEl = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${page}"]`);
      const view = viewerRef.current?.getPageView(page - 1);
      if (!lr || !pageEl || !view) return null;
      const pr = pageEl.getBoundingClientRect();
      const [x, y] = view.viewport.convertToPdfPoint(lx + lr.left - pr.left, ly + lr.top - pr.top);
      return [x, y];
    },
    toLayer(page, x, y) {
      const lr = layerRef.current?.getBoundingClientRect();
      const pageEl = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${page}"]`);
      const view = viewerRef.current?.getPageView(page - 1);
      if (!lr || !pageEl || !view) return null;
      const pr = pageEl.getBoundingClientRect();
      const [vx, vy] = view.viewport.convertToViewportPoint(x, y);
      return [pr.left - lr.left + vx, pr.top - lr.top + vy];
    },
    scale: () => viewerRef.current?.currentScale ?? 1,
    pageCanvas: (page) => innerRef.current?.querySelector<HTMLCanvasElement>(`.page[data-page-number="${page}"] canvas`) ?? null,
    pageBox(page) {
      const lr = layerRef.current?.getBoundingClientRect();
      const pageEl = innerRef.current?.querySelector<HTMLElement>(`.page[data-page-number="${page}"]`);
      if (!lr || !pageEl) return null;
      const pr = pageEl.getBoundingClientRect();
      return { left: pr.left - lr.left, top: pr.top - lr.top, width: pr.width, height: pr.height };
    },
    currentPage: () => currentPage,
  };
  const objectMode = props.editorMode === "objects" || props.editorMode === "addtext";

  const draftBox = (() => {
    if (!draft || !layerRef.current) return null;
    const lr = layerRef.current.getBoundingClientRect();
    return {
      left: Math.min(draft.x0, draft.x1) - lr.left,
      top: Math.min(draft.y0, draft.y1) - lr.top,
      width: Math.abs(draft.x1 - draft.x0),
      height: Math.abs(draft.y1 - draft.y0),
    };
  })();

  return (
    <div className={`pdf-container mode-${props.editorMode}`} ref={containerRef}>
      <div className="pdfViewer" ref={innerRef} />
      <div
        ref={layerRef}
        className="mark-layer"
        style={{ height: innerRef.current?.scrollHeight ?? 0, pointerEvents: OVERLAY_MODES.has(props.editorMode) ? "auto" : "none" }}
        onMouseDown={onLayerDown}
        onMouseMove={onLayerMove}
        onMouseUp={onLayerUp}
        onMouseLeave={onLayerUp}
      >
        {markBoxes.map(({ m, ...box }) => (
          <div key={m.id} className={`mark mark-${m.kind}`} style={box} title={m.kind === "correct" ? `Korekta: ${m.text}` : "Redakcja"}>
            {m.kind === "correct" && (
              <span className="mark-text" style={{ fontSize: Math.max(6, box.height * 0.82), fontFamily: MARK_FONTS[m.font ?? "sans"] }}>
                {m.text}
              </span>
            )}
            <button className="mark-remove" onMouseDown={(e) => e.stopPropagation()} onClick={() => props.onRemoveMark(m.id)}>
              ✕
            </button>
          </div>
        ))}
        {draftBox && <div className="mark mark-draft" style={draftBox} />}
        {objectMode && (
          <ObjectLayer
            mode={props.editorMode as "objects" | "addtext"}
            geom={geom}
            docKey={props.doc}
            layout={layout}
            textStyle={props.textStyle}
            objectsFor={props.objectsFor}
            onOps={props.onObjectOps}
          />
        )}
      </div>
    </div>
  );
});
