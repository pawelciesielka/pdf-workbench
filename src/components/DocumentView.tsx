import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type CitationView, type DocumentInfo, type Selection } from "../lib/api";
import { extractPageTexts, loadPdf, type PDFDocumentProxy } from "../lib/pdf";
import { ChatPanel, type ChatPanelHandle } from "./ChatPanel";
import { PdfViewer, type PdfViewerHandle, type SelectionInfo } from "./PdfViewer";
import { Thumbnails } from "./Thumbnails";

export interface DocumentViewHandle {
  focusSearch(): void;
  toggleChat(): void;
  zoomIn(): void;
  zoomOut(): void;
  fitWidth(): void;
}

interface Props {
  info: DocumentInfo;
  active: boolean;
  hasKey: boolean;
  onOpenSettings(): void;
}

export const DocumentView = forwardRef<DocumentViewHandle, Props>(function DocumentView({ info, active, hasKey, onOpenSettings }, ref) {
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [scale, setScale] = useState(1);
  const [showThumbs, setShowThumbs] = useState(true);
  const [showChat, setShowChat] = useState(true);
  const [chatWidth, setChatWidth] = useState(() => Number(localStorage.getItem("chatWidth")) || 420);
  const [query, setQuery] = useState("");
  const [findCount, setFindCount] = useState<{ current: number; total: number } | null>(null);
  const [floatingSel, setFloatingSel] = useState<SelectionInfo | null>(null);
  const [chatSelection, setChatSelection] = useState<Selection | null>(null);
  const [indexed, setIndexed] = useState(info.indexed);
  const viewerRef = useRef<PdfViewerHandle>(null);
  const chatRef = useRef<ChatPanelHandle>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // Wczytanie pliku, a w tle wyciągnięcie tekstu stron do lokalnego indeksu.
  useEffect(() => {
    let cancelled = false;
    let loaded: PDFDocumentProxy | null = null;
    (async () => {
      try {
        const data = await api.readDocument(info.path);
        const pdf = await loadPdf(data);
        loaded = pdf;
        if (cancelled) return void pdf.loadingTask.destroy();
        setDoc(pdf);
        if (!info.indexed) {
          const pages = await extractPageTexts(pdf);
          if (!cancelled) {
            await api.savePageTexts(info.hash, pages);
            setIndexed(true);
          }
        }
      } catch (e) {
        if (!cancelled) setLoadError(errorText(e));
      }
    })();
    return () => {
      cancelled = true;
      loaded?.loadingTask.destroy();
    };
  }, [info.path, info.hash, info.indexed]);

  useEffect(() => {
    if (active) requestAnimationFrame(() => viewerRef.current?.refresh());
  }, [active, showThumbs, showChat]);

  useEffect(() => setPageInput(String(page)), [page]);

  useImperativeHandle(ref, () => ({
    focusSearch() {
      searchRef.current?.focus();
      searchRef.current?.select();
    },
    toggleChat() {
      const opening = !showChat;
      setShowChat(opening);
      if (opening) setTimeout(() => chatRef.current?.focus(), 0);
    },
    zoomIn: () => viewerRef.current?.zoomIn(),
    zoomOut: () => viewerRef.current?.zoomOut(),
    fitWidth: () => viewerRef.current?.fit("page-width"),
  }));

  const runFind = (q: string, previous = false, again = false) => {
    if (!q.trim()) {
      viewerRef.current?.clearFind();
      setFindCount(null);
      return;
    }
    viewerRef.current?.find(q, { previous, again });
  };

  const askAboutSelection = () => {
    if (!floatingSel) return;
    setChatSelection({ page: floatingSel.page, text: floatingSel.text.slice(0, 4000) });
    setFloatingSel(null);
    setShowChat(true);
    window.getSelection()?.removeAllRanges();
  };

  const onCitation = (c: CitationView) => {
    if (c.page) viewerRef.current?.showCitation(c.page, c.citedText);
  };

  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = chatWidth;
    let w = startW;
    const move = (ev: MouseEvent) => {
      w = Math.min(800, Math.max(300, startW + (startX - ev.clientX)));
      setChatWidth(w);
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      localStorage.setItem("chatWidth", String(w));
      viewerRef.current?.refresh();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const total = doc?.numPages ?? info.pageCount ?? 0;

  return (
    <div className="document-view" style={{ display: active ? "flex" : "none" }}>
      <div className="toolbar">
        <button className={`icon-btn ${showThumbs ? "on" : ""}`} title={t("toolbar.thumbnails")} onClick={() => setShowThumbs((v) => !v)}>
          ▤
        </button>
        <div className="page-nav">
          <span>{t("toolbar.page")}</span>
          <input
            value={pageInput}
            onChange={(e) => setPageInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && viewerRef.current?.goToPage(Number(pageInput) || 1)}
            onBlur={() => setPageInput(String(page))}
          />
          <span>/ {total}</span>
        </div>
        <div className="sep" />
        <button className="icon-btn" title={t("toolbar.zoomOut")} onClick={() => viewerRef.current?.zoomOut()}>
          −
        </button>
        <span className="zoom-label">{Math.round(scale * 100)}%</span>
        <button className="icon-btn" title={t("toolbar.zoomIn")} onClick={() => viewerRef.current?.zoomIn()}>
          +
        </button>
        <button className="text-btn" title={t("toolbar.fitWidth")} onClick={() => viewerRef.current?.fit("page-width")}>
          ↔
        </button>
        <button className="text-btn" title={t("toolbar.fitPage")} onClick={() => viewerRef.current?.fit("page-fit")}>
          ⤢
        </button>
        <div className="spacer" />
        <div className="search">
          <input
            ref={searchRef}
            placeholder={t("toolbar.search")}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              runFind(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") runFind(query, e.shiftKey, true);
              if (e.key === "Escape") {
                setQuery("");
                runFind("");
                e.currentTarget.blur();
              }
            }}
          />
          {query && findCount && (
            <span className="find-count">
              {findCount.total ? t("toolbar.searchResults", { current: findCount.current, total: findCount.total }) : t("toolbar.noResults")}
            </span>
          )}
        </div>
        <button className={`icon-btn ${showChat ? "on" : ""}`} title={t("toolbar.chat")} onClick={() => setShowChat((v) => !v)}>
          💬
        </button>
      </div>

      <div className="workspace">
        {showThumbs && doc && <Thumbnails doc={doc} current={page} onSelect={(p) => viewerRef.current?.goToPage(p)} />}
        <div className="viewer-area">
          {loadError && <div className="banner error">{t("error.open")}: {loadError}</div>}
          {!doc && !loadError && <div className="loading">{t("loading")}</div>}
          {doc && (
            <PdfViewer
              ref={viewerRef}
              doc={doc}
              onPageChange={setPage}
              onScaleChange={setScale}
              onFindCount={(current, total) => setFindCount({ current, total })}
              onSelection={setFloatingSel}
            />
          )}
          {floatingSel && (
            <button className="ask-floating" style={{ left: floatingSel.x, top: floatingSel.y }} onMouseDown={(e) => e.preventDefault()} onClick={askAboutSelection}>
              ✨ {t("selection.ask")}
            </button>
          )}
        </div>
        {showChat && (
          <>
            <div className="resizer" onMouseDown={startResize} />
            <div style={{ width: chatWidth, display: "flex" }}>
              <ChatPanel
                ref={chatRef}
                doc={info}
                indexed={indexed}
                hasKey={hasKey}
                selection={chatSelection}
                onClearSelection={() => setChatSelection(null)}
                onOpenSettings={onOpenSettings}
                onCitation={onCitation}
              />
            </div>
          </>
        )}
      </div>
    </div>
  );
});
