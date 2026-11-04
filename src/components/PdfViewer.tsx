import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import { loadViewerModule, type PDFDocumentProxy } from "../lib/pdf";

type Viewer = InstanceType<Awaited<ReturnType<typeof loadViewerModule>>["PDFViewer"]>;
type Bus = InstanceType<Awaited<ReturnType<typeof loadViewerModule>>["EventBus"]>;

export interface PdfViewerHandle {
  goToPage(n: number): void;
  zoomIn(): void;
  zoomOut(): void;
  fit(mode: "page-width" | "page-fit"): void;
  find(query: string, opts?: { again?: boolean; previous?: boolean }): void;
  clearFind(): void;
  showCitation(page: number, text: string): void;
  refresh(): void;
}

export interface SelectionInfo {
  page: number;
  text: string;
  x: number;
  y: number;
}

interface Props {
  doc: PDFDocumentProxy;
  onPageChange(page: number): void;
  onScaleChange(scale: number): void;
  onFindCount(current: number, total: number): void;
  onSelection(sel: SelectionInfo | null): void;
}

const MIN_SCALE = 0.25;
const MAX_SCALE = 6;

function normalize(text: string) {
  return text.replace(/\s+/g, " ").trim();
}

export const PdfViewer = forwardRef<PdfViewerHandle, Props>(function PdfViewer(props, ref) {
  const containerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const viewerRef = useRef<Viewer | null>(null);
  const busRef = useRef<Bus | null>(null);
  const cb = useRef(props);
  cb.current = props;

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
      });
      linkService.setViewer(viewer);
      eventBus.on("pagesinit", () => {
        viewer.currentScaleValue = "page-width";
      });
      eventBus.on("pagechanging", (e: { pageNumber: number }) => cb.current.onPageChange(e.pageNumber));
      eventBus.on("scalechanging", (e: { scale: number }) => cb.current.onScaleChange(e.scale));
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
      viewerRef.current?.setDocument(null as unknown as PDFDocumentProxy);
      viewerRef.current = null;
    };
  }, [props.doc]);

  // Zaznaczenie tekstu → pływający przycisk „Zapytaj o to”.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onUp = () => {
      setTimeout(() => {
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
    return {
      goToPage(n) {
        const v = viewerRef.current;
        if (v) v.currentPageNumber = Math.min(Math.max(1, n), v.pagesCount);
      },
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
        // Podświetlamy początek cytowanego fragmentu wyszukiwaniem frazy.
        const snippet = normalize(text).split(" ").slice(0, 10).join(" ");
        if (snippet.length >= 3) setTimeout(() => dispatchFind(snippet, false, false), 50);
      },
      refresh() {
        const v = viewerRef.current;
        if (!v) return;
        const value = v.currentScaleValue;
        v.currentScaleValue = value;
        v.update();
      },
    };
  }, []);

  return (
    <div className="pdf-container" ref={containerRef}>
      <div className="pdfViewer" ref={innerRef} />
    </div>
  );
});
