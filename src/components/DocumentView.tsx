import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type CitationView, type DocumentInfo, type EditOp, type EditReport, type Selection, type Placement, type SignatureInfo, type ToolCall, type TrustInfo, type ToolOutcome } from "../lib/api";
import {
  applyMarks,
  compareTexts,
  deletePages,
  extractPages,
  findTextRects,
  insertPdf,
  MARK_FONTS,
  movePages,
  ocrPages,
  rotatePages,
  scannedPages,
  type Change,
  type Mark,
  type Rect,
} from "../lib/editing";
import { extractPageTexts, loadPdf, type PDFDocumentProxy } from "../lib/pdf";
import { ChatPanel, type ChatPanelHandle } from "./ChatPanel";
import { changesForChat, CompareView } from "./CompareView";
import { HIGHLIGHT_COLORS, PdfViewer, type EditorMode, type PdfViewerHandle, type SelectionInfo } from "./PdfViewer";
import { ColorMenu, type TextStyle } from "./ObjectLayer";
import { LAST_PAGE, pdfDate, SignaturesDialog, SignDialog, signersSummary, UnlockDialog, visibleLines, type SignChoice } from "./Signing";
import { Thumbnails } from "./Thumbnails";

export interface DocumentViewHandle {
  focusSearch(): void;
  toggleChat(): void;
  zoomIn(): void;
  zoomOut(): void;
  fitWidth(): void;
  undo(): void;
  redo(): void;
  save(asNew?: boolean): void;
  showCitation(page: number, text: string): void;
  runTool(call: ToolCall): Promise<ToolOutcome>;
  isDirty(): boolean;
  escape(): void;
}

interface Props {
  info: DocumentInfo;
  active: boolean;
  hasKey: boolean;
  onOpenSettings(): void;
  onSaved(info: DocumentInfo, message?: string): void;
  onOpenNew(info: DocumentInfo): void;
  onDirtyChange(dirty: boolean): void;
  onToast(message: string, kind?: "error" | "info"): void;
  onBatchSign(): void;
  /** Podpisywanie wielu plików: wskaż miejsce podpisu w tym Dokumencie. */
  placeRequest?: { index: number; total: number; onPlace(p: Placement | null, sameForRest: boolean): void; onCancel(): void };
}

type PromptState = { kind: "correct"; marks: Mark[]; value: string; original: string; page: number; font: "serif" | "sans" } | { kind: "findRedact"; value: string } | null;

const sameBytes = (a: Uint8Array | null, b: Uint8Array | null) => !!a && !!b && a.length === b.length && a.every((v, i) => v === b[i]);

const MODES: { mode: EditorMode; icon: string; label: string }[] = [
  { mode: "none", icon: "🖱", label: "edit.select" },
  { mode: "objects", icon: "⬚", label: "edit.objects" },
  { mode: "addtext", icon: "A+", label: "edit.addText" },
  { mode: "highlight", icon: "🖍", label: "edit.highlight" },
  { mode: "freetext", icon: "T", label: "edit.text" },
  { mode: "ink", icon: "✎", label: "edit.ink" },
  { mode: "stamp", icon: "🖼", label: "edit.image" },
  { mode: "redact", icon: "■", label: "edit.redact" },
];

function unionRect(rects: Rect[]): Rect {
  return [Math.min(...rects.map((r) => r[0])), Math.min(...rects.map((r) => r[1])), Math.max(...rects.map((r) => r[2])), Math.max(...rects.map((r) => r[3]))];
}

/** Błąd „nic nie zmieniono” — applyEdit nie zapisuje wtedy kroku cofania. */
class NothingChanged extends Error {}

const FONT_SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48];
const cleanFont = (f: string) => f.replace(/^[A-Z]{6}\+/, "");

export const DocumentView = forwardRef<DocumentViewHandle, Props>(function DocumentView(props, ref) {
  const { info, active, hasKey, onOpenSettings } = props;
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [scale, setScale] = useState(1);
  const [showThumbs, setShowThumbs] = useState(true);
  const [showChat, setShowChat] = useState(() => {
    try {
      return localStorage.getItem("showChat") !== "0";
    } catch {
      return true;
    }
  });
  const [chatWidth, setChatWidth] = useState(() => Number(localStorage.getItem("chatWidth")) || 420);
  const [query, setQuery] = useState("");
  const [findCount, setFindCount] = useState<{ current: number; total: number } | null>(null);
  const [floatingSel, setFloatingSel] = useState<SelectionInfo | null>(null);
  const [chatSelection, setChatSelection] = useState<Selection | null>(null);
  const [indexed, setIndexed] = useState(info.indexed);
  const [editorMode, setEditorMode] = useState<EditorMode>("none");
  const [editorState, setEditorState] = useState({ canUndo: false, canRedo: false });
  const [highlightColor, setHighlightColor] = useState("yellow");
  const [marks, setMarks] = useState<Mark[]>([]);
  const [edited, setEdited] = useState(false);
  const [annotDirty, setAnnotDirty] = useState(false);
  const [hasRedaction, setHasRedaction] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [pageTexts, setPageTexts] = useState<string[]>([]);
  const [selectedPages, setSelectedPages] = useState<number[]>([]);
  const [compare, setCompare] = useState<{ name: string; changes: Change[] } | null>(null);
  const [prompt, setPrompt] = useState<PromptState>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [, setStackVersion] = useState(0);
  const [docVersion, setDocVersion] = useState(0);
  const [textStyle, setTextStyle] = useState<TextStyle>({ fontSize: 12, color: "#000000", bold: false });
  /** Kolor i rozmiar Notatek tekstowych (pdf.js FreeText). */
  const [noteStyle, setNoteStyle] = useState({ color: "#000000", size: 10 });
  const changeNoteStyle = (patch: { color?: string; size?: number }) => {
    setNoteStyle((s) => ({ ...s, ...patch }));
    viewerRef.current?.setFreeTextStyle(patch);
  };
  const [signatures, setSignatures] = useState<SignatureInfo[]>([]);
  /** Zaufanie do podpisów (łańcuch, unieważnienie, lista UE) — sprawdzane w tle po wczytaniu. */
  const [trust, setTrust] = useState<TrustInfo[] | null>(null);
  const [trustError, setTrustError] = useState<string | null>(null);
  /** „Twarde” odblokowanie edycji podpisanego dokumentu (do zamknięcia/przeładowania pliku). */
  const [unlocked, setUnlocked] = useState(false);
  const [dialog, setDialog] = useState<"sign" | "signatures" | "unlock" | null>(null);
  /** Wybór z okna podpisu czekający na wskazanie miejsca widocznego podpisu. */
  const pendingSign = useRef<SignChoice | null>(null);
  const [sameForRest, setSameForRest] = useState(false);
  const placeRequest = props.placeRequest;
  /** Bajty aktualnie wyświetlanego stanu (bez niezapisanych Adnotacji) — do listy obiektów. */
  const currentBytes = useRef<Uint8Array | null>(null);
  const viewerRef = useRef<PdfViewerHandle>(null);
  const chatRef = useRef<ChatPanelHandle>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const docRef = useRef<PDFDocumentProxy | null>(null);
  const diskBytes = useRef<Uint8Array | null>(null);
  const undoStack = useRef<Uint8Array[]>([]);
  const redoStack = useRef<Uint8Array[]>([]);
  const firstPage = useRef(1);
  const propsRef = useRef(props);
  propsRef.current = props;

  const dirty = edited || annotDirty;
  const intactSigs = signatures.filter((x) => x.intact);
  const locked = intactSigs.length > 0 && !unlocked;
  const lockedRef = useRef(locked);
  lockedRef.current = locked;
  useEffect(() => props.onDirtyChange(dirty), [dirty]); // eslint-disable-line react-hooks/exhaustive-deps

  /** Ładuje bajty jako bieżący stan Dokumentu (kopia bajtów — pdf.js przenosi bufor do workera). */
  const loadBytes = useCallback(async (bytes: Uint8Array, keepPage?: number) => {
    const pdf = await loadPdf(bytes.slice().buffer as ArrayBuffer);
    currentBytes.current = bytes;
    const old = docRef.current;
    docRef.current = pdf;
    firstPage.current = keepPage ?? 1;
    setDoc(pdf);
    setDocVersion((n) => n + 1);
    setSelectedPages([]);
    (pdf.annotationStorage as unknown as { onSetModified: (() => void) | null }).onSetModified = () => setAnnotDirty(true);
    old?.loadingTask.destroy();
    const texts = await extractPageTexts(pdf);
    if (docRef.current === pdf) setPageTexts(texts);
    return { pdf, texts };
  }, []);

  // Wczytanie pliku z dysku (także po zapisie: nowy skrót = nowy Dokument).
  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    undoStack.current = [];
    redoStack.current = [];
    setEdited(false);
    setAnnotDirty(false);
    setHasRedaction(false);
    setMarks([]);
    setCompare(null);
    setSignatures([]);
    setTrust(null);
    setTrustError(null);
    setUnlocked(false);
    (async () => {
      try {
        const data = new Uint8Array(await api.readDocument(info.path));
        if (cancelled) return;
        diskBytes.current = data;
        api
          .signatures(data)
          .then((sigs) => {
            if (cancelled) return;
            setSignatures(sigs);
            if (sigs.some((x) => x.intact))
              api
                .signatureTrust(data)
                .then((tr) => !cancelled && setTrust(tr))
                .catch((e) => !cancelled && setTrustError(errorText(e)));
          })
          .catch(() => {});
        const { texts } = await loadBytes(data, firstPage.current);
        if (!info.indexed && !cancelled) {
          await api.savePageTexts(info.hash, texts);
          setIndexed(true);
        }
      } catch (e) {
        if (!cancelled) setLoadError(errorText(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [info.path, info.hash, info.indexed, loadBytes]);

  useEffect(
    () => () => {
      void docRef.current?.loadingTask.destroy();
    },
    [],
  );

  useEffect(() => {
    if (active) requestAnimationFrame(() => viewerRef.current?.refresh());
  }, [active, showThumbs, showChat, compare]);

  useEffect(() => setPageInput(String(page)), [page]);

  // Widoczność czatu zapamiętywana między kartami i uruchomieniami.
  useEffect(() => {
    try {
      localStorage.setItem("showChat", showChat ? "1" : "0");
    } catch {
      /* brak dostępu do localStorage */
    }
  }, [showChat]);

  // Podpisywanie wielu plików: od razu tryb wskazywania miejsca podpisu.
  useEffect(() => {
    if (placeRequest && active && doc) setEditorMode("signbox");
  }, [placeRequest?.index, active, doc]); // eslint-disable-line react-hooks/exhaustive-deps

  // Podpisany dokument: z narzędzi edycji zostaje tylko zaznaczanie.
  useEffect(() => {
    if (locked) setEditorMode((m) => (m === "none" || m === "signbox" ? m : "none"));
  }, [locked]);

  /** Edycja strukturalna: zapamiętuje stan do cofnięcia, wykonuje operację i ładuje wynik. */
  const applyEdit = useCallback(
    async (label: string, fn: (doc: PDFDocumentProxy, bytes: Uint8Array) => Promise<Uint8Array>): Promise<void> => {
      const d = docRef.current;
      if (!d) throw new Error(t("loading"));
      if (lockedRef.current) throw new Error(t("sign.lockedError"));
      // Krótkie operacje bez migania nakładki „Proszę czekać”.
      setBusy("");
      const timer = window.setTimeout(() => setBusy(label), 600);
      try {
        const before = await d.saveDocument();
        const after = await fn(d, before);
        undoStack.current.push(before);
        redoStack.current = [];
        await loadBytes(after, viewerRef.current?.currentPage());
        setEdited(true);
        setAnnotDirty(false);
        setStackVersion((n) => n + 1);
      } finally {
        window.clearTimeout(timer);
        setBusy(null);
      }
    },
    [loadBytes],
  );

  const fallbackNote = (r: EditReport) => (r.fontFallbacks.length ? t("obj.fontFallback", { fonts: r.fontFallbacks.map(cleanFont).join(", ") }) : "");

  const objectOps = (ops: EditOp[]) =>
    applyEdit(t("obj.applying"), async (_d, b) => {
      const r = await api.editObjects(b, ops);
      const note = fallbackNote(r.report);
      if (note) propsRef.current.onToast(note, "info");
      return r.data;
    }).catch((e) => propsRef.current.onToast(errorText(e)));

  const objectsFor = (p: number) => (currentBytes.current ? api.pageObjects(currentBytes.current, p) : Promise.resolve([]));

  /** Podmiana tekstu w treści strony; rzuca NothingChanged, gdy nie znaleziono w pojedynczym obiekcie. */
  const replaceInContent = async (m: { pages?: number[]; find: string; replace: string; within?: Rect }) => {
    let report: EditReport = { count: 0, fontFallbacks: [] };
    await applyEdit(t("obj.applying"), async (_d, b) => {
      const r = await api.replaceText(b, m);
      if (!r.report.count) throw new NothingChanged();
      report = r.report;
      return r.data;
    });
    return report;
  };

  const run = (label: string, fn: (doc: PDFDocumentProxy, bytes: Uint8Array) => Promise<Uint8Array>, after?: () => void) =>
    applyEdit(label, fn)
      .then(after)
      .catch((e) => propsRef.current.onToast(errorText(e)));

  const undo = async () => {
    if (viewerRef.current?.undoAnnotation()) return;
    const prev = undoStack.current.pop();
    const d = docRef.current;
    if (!prev || !d) return;
    redoStack.current.push(await d.saveDocument());
    await loadBytes(prev, viewerRef.current?.currentPage());
    setEdited(!sameBytes(prev, diskBytes.current));
    setAnnotDirty(false);
    setStackVersion((n) => n + 1);
  };

  const redo = async () => {
    if (viewerRef.current?.redoAnnotation()) return;
    const next = redoStack.current.pop();
    const d = docRef.current;
    if (!next || !d) return;
    undoStack.current.push(await d.saveDocument());
    await loadBytes(next, viewerRef.current?.currentPage());
    setEdited(true);
    setStackVersion((n) => n + 1);
  };

  const save = async (asNew = false) => {
    const d = docRef.current;
    if (!d || busy) return;
    if (marks.length && !window.confirm(t("edit.unappliedMarks", { n: marks.length }))) return;
    if (dirty && intactSigs.length && !window.confirm(t("sign.saveConfirm"))) return;
    let path = info.path;
    if (asNew) {
      const picked = await saveDialog({ defaultPath: info.path.replace(/\.pdf$/i, " (kopia).pdf"), filters: [{ name: "PDF", extensions: ["pdf"] }] });
      if (!picked) return;
      path = picked;
    }
    setBusy(t("edit.saving"));
    try {
      // Bez zmian zapisujemy dokładnie bajty z dysku (pdf.js mógłby dopisać aktualizację i naruszyć podpis).
      const bytes = dirty || !diskBytes.current ? await d.saveDocument() : diskBytes.current;
      const res = await api.saveDocument(bytes, { path, parentHash: info.hash, backup: !asNew && !hasRedaction });
      const msg = res.backupPath ? t("edit.backupCreated", { name: res.backupPath.split("\\").pop() ?? "" }) : t("edit.saved");
      props.onSaved(res.document, msg);
    } catch (e) {
      props.onToast(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const extract = async (pages: number[]) => {
    const d = docRef.current;
    if (!d) return;
    const picked = await saveDialog({
      defaultPath: info.path.replace(/\.pdf$/i, ` (strony ${pages[0]}${pages.length > 1 ? "-" + pages[pages.length - 1] : ""}).pdf`),
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!picked) return;
    setBusy(t("pages.extracting"));
    try {
      const bytes = await extractPages(d, await d.saveDocument(), pages);
      const res = await api.saveDocument(bytes, { path: picked, parentHash: null, backup: false });
      props.onOpenNew(res.document);
    } catch (e) {
      props.onToast(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const insertFile = async (afterPage: number) => {
    const picked = await openDialog({ multiple: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!picked || Array.isArray(picked)) return;
    const other = new Uint8Array(await api.readDocument(picked));
    run(t("pages.inserting"), (d, b) => insertPdf(d, b, other, afterPage));
  };

  const progress = (label: string) => (done: number, total: number) => setBusy(`${label} ${Math.min(done + 1, total)}/${total}…`);

  const runOcr = (pages: number[]) =>
    run(t("ocr.running"), async (_d, b) => {
      const r = await ocrPages(b, pages, progress(t("ocr.running")));
      propsRef.current.onToast(t("ocr.done", { lines: r.lines, pages: pages.length }), "info");
      return r.bytes;
    });

  const applyPendingMarks = () => {
    if (!marks.length) return;
    const toApply = marks;
    const redacts = toApply.some((m) => m.kind === "redact");
    run(
      t("edit.applying"),
      (_d, b) => applyMarks(b, toApply, progress(t("edit.applying"))),
      () => {
        setMarks([]);
        if (redacts) setHasRedaction(true);
        setEditorMode("none");
        propsRef.current.onToast(t("edit.marksApplied"), "info");
      },
    );
  };

  const startCompare = async () => {
    setMenuOpen(false);
    const picked = await openDialog({ multiple: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!picked || Array.isArray(picked)) return;
    setBusy(t("compare.running"));
    try {
      const other = await loadPdf(await api.readDocument(picked));
      const otherTexts = await extractPageTexts(other);
      other.loadingTask.destroy();
      setCompare({ name: picked.split("\\").pop() ?? picked, changes: compareTexts(otherTexts, pageTexts) });
    } catch (e) {
      props.onToast(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  /** Podpis: wybór pliku docelowego, podpis certyfikatem z Windows i zapis. */
  const signDocument = async (choice: SignChoice, place?: { page: number; rect: Rect }) => {
    const d = docRef.current;
    if (!d) return;
    const overwrite = await api.getSettings().then((s) => s.signOverwrite, () => false);
    const target = overwrite
      ? info.path
      : await saveDialog({
          title: t("sign.saveTitle"),
          defaultPath: info.path.replace(/\.pdf$/i, " (podpisany).pdf"),
          filters: [{ name: "PDF", extensions: ["pdf"] }],
        });
    if (!target) return;
    setBusy(t("sign.signing"));
    try {
      const source = dirty || !diskBytes.current ? await d.saveDocument() : diskBytes.current;
      const now = new Date();
      const lines = visibleLines(choice.cert, choice.reason, now);
      const signingTime = pdfDate(now);
      const { data: signed, report } = await api.signPdf(source, {
        certId: choice.cert.id,
        signingTime,
        reason: choice.reason || undefined,
        location: choice.location || undefined,
        timestamp: choice.timestamp,
        ltv: choice.ltv,
        visible: place ? { page: place.page, rect: place.rect, lines } : undefined,
      });
      const level = t("sign.level", { level: report.level === "B-LTA" ? "PAdES-LTA" : `PAdES-${report.level}` });
      const res = await api.saveDocument(signed, { path: target, parentHash: info.hash, backup: false });
      if (res.document.path.toLowerCase() === info.path.toLowerCase()) props.onSaved(res.document, `${t("sign.done")} ${level}`);
      else {
        props.onOpenNew(res.document);
        props.onToast(`${t("sign.doneNew", { name: res.document.name })} ${level}`, "info");
      }
      if (report.warning) props.onToast(report.warning, "error");
      await api
        .logSignature({
          source: info.path,
          output: res.document.path,
          certId: choice.cert.id,
          signingTime,
          level: report.level,
          reason: choice.reason || undefined,
          location: choice.location || undefined,
          warning: report.warning ?? null,
        })
        .catch((e) => props.onToast(errorText(e), "error"));
    } catch (e) {
      props.onToast(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const submitSign = (choice: SignChoice) => {
    setDialog(null);
    if (choice.visible) {
      pendingSign.current = choice;
      setEditorMode("signbox");
    } else void signDocument(choice);
  };

  const placeSignature = (p: number, rect: Rect) => {
    if (placeRequest) {
      setEditorMode("none");
      const last = p === docRef.current?.numPages;
      placeRequest.onPlace({ page: last ? LAST_PAGE : p, rect }, sameForRest);
      return;
    }
    const choice = pendingSign.current;
    pendingSign.current = null;
    setEditorMode("none");
    if (choice) void signDocument(choice, { page: p, rect });
  };

  const runTool = async (call: ToolCall): Promise<ToolOutcome> => {
    const i = call.input as Record<string, unknown>;
    if (lockedRef.current && call.name !== "go_to_page")
      return {
        ok: false,
        text: "Dokument jest podpisany elektronicznie, więc edycja jest zablokowana (każda zmiana unieważniłaby podpis). Użytkownik może ją odblokować przyciskiem „Odblokuj edycję” na pasku nad dokumentem.",
      };
    const pages = (i.pages as number[]) ?? [];
    const unsaved = t("tool.unsaved");
    switch (call.name) {
      case "highlight_text": {
        await viewerRef.current!.highlightText(Number(i.page), String(i.text), String(i.color));
        return { ok: true, text: `Podświetlono na stronie ${i.page}. ${unsaved}` };
      }
      case "go_to_page":
        viewerRef.current?.goToPage(Number(i.page));
        return { ok: true, text: `Pokazano stronę ${i.page}.` };
      case "rotate_pages":
        await applyEdit(t("pages.rotating"), (d, b) => rotatePages(d, b, pages, Number(i.degrees)));
        return { ok: true, text: `Obrócono strony ${pages.join(", ")}. ${unsaved}` };
      case "delete_pages":
        await applyEdit(t("pages.deleting"), (d, b) => deletePages(d, b, pages));
        return { ok: true, text: `Usunięto strony ${pages.join(", ")}; Dokument ma teraz ${docRef.current?.numPages} stron. ${unsaved}` };
      case "move_pages":
        await applyEdit(t("pages.moving"), (d, b) => movePages(d, b, pages, Number(i.after_page)));
        return { ok: true, text: `Przeniesiono strony ${pages.join(", ")}. ${unsaved}` };
      case "redact_text": {
        const d = docRef.current!;
        const found = await findTextRects(d, String(i.text));
        if (!found.length) return { ok: false, text: `Nie znaleziono tekstu „${i.text}” w warstwie tekstu Dokumentu.` };
        await applyEdit(t("edit.applying"), (_d, b) => applyMarks(b, found, progress(t("edit.applying"))));
        setHasRedaction(true);
        const onPages = [...new Set(found.map((m) => m.page))].join(", ");
        return { ok: true, text: `Zaczerniono ${found.length} wystąpień na stronach ${onPages}. ${unsaved}` };
      }
      case "replace_text": {
        try {
          const r = await replaceInContent({ pages: pages.length ? pages : undefined, find: String(i.text), replace: String(i.replacement) });
          const fb = r.fontFallbacks.length
            ? ` Czcionka ${r.fontFallbacks.map(cleanFont).join(", ")} nie miała potrzebnych znaków — w zmienionych miejscach użyto podobnej czcionki systemowej.`
            : "";
          return { ok: true, text: `Zamieniono tekst (liczba miejsc: ${r.count}).${fb} ${unsaved}` };
        } catch (e) {
          if (e instanceof NothingChanged)
            return {
              ok: false,
              text: `Nie znaleziono „${i.text}” w pojedynczym fragmencie tekstu strony (tekst może być podzielony na kilka fragmentów albo strona jest skanem). Spróbuj krótszego, dokładnego fragmentu.`,
            };
          throw e;
        }
      }
      default:
        return { ok: false, text: `Nieznane narzędzie ${call.name}.` };
    }
  };

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
    undo,
    redo,
    save,
    showCitation: (p, text) => viewerRef.current?.showCitation(p, text),
    runTool,
    isDirty: () => dirty,
    escape() {
      if (placeRequest) placeRequest.onCancel();
      pendingSign.current = null;
      setEditorMode("none");
      setMenuOpen(false);
      setPrompt(null);
    },
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
    setCompare(null);
    setShowChat(true);
    window.getSelection()?.removeAllRanges();
  };

  const redactSelection = () => {
    const m = viewerRef.current?.selectionMarks("redact") ?? [];
    setMarks((cur) => [...cur, ...m]);
    setFloatingSel(null);
    window.getSelection()?.removeAllRanges();
  };

  const correctSelection = () => {
    if (!floatingSel) return;
    // Całe słowa: na skanach częściowe zakrycie zostawiałoby resztki liter.
    const expanded = viewerRef.current?.expandSelectionToWords();
    const m = viewerRef.current?.selectionMarks("correct") ?? [];
    if (!m.length) return;
    const original = expanded?.text || floatingSel.text;
    const pageMarks = m.filter((x) => x.page === m[0].page);
    setPrompt({
      kind: "correct",
      marks: [{ ...pageMarks[0], rect: unionRect(pageMarks.map((x) => x.rect)) }],
      value: original,
      original,
      page: pageMarks[0].page,
      font: expanded?.font ?? "sans",
    });
    setFloatingSel(null);
    window.getSelection()?.removeAllRanges();
  };

  const submitPrompt = async () => {
    if (!prompt) return;
    if (prompt.kind === "correct") {
      const p = prompt;
      setPrompt(null);
      if (p.value === p.original) return;
      // Najpierw prawdziwa edycja tekstu (strona zostaje wektorowa); gdy fragment obejmuje kilka
      // obiektów tekstowych albo strona to skan — Korekta przez obraz.
      try {
        const r = await replaceInContent({ pages: [p.page], find: p.original, replace: p.value, within: p.marks[0].rect });
        propsRef.current.onToast([t("obj.corrected"), fallbackNote(r)].filter(Boolean).join(" "), "info");
      } catch (e) {
        if (!(e instanceof NothingChanged)) {
          propsRef.current.onToast(errorText(e));
          return;
        }
        setMarks((cur) => [...cur, ...p.marks.map((m) => ({ ...m, text: p.value, original: p.original, font: p.font }))]);
        propsRef.current.onToast(t("obj.correctAsImage"), "info");
      }
      return;
    } else {
      const d = docRef.current;
      if (!d) return;
      const found = await findTextRects(d, prompt.value);
      setMarks((cur) => [...cur, ...found]);
      props.onToast(found.length ? t("edit.found", { n: found.length }) : t("edit.notFound"), found.length ? "info" : "error");
    }
    setPrompt(null);
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
  const scanned = scannedPages(pageTexts);
  const canUndo = editorState.canUndo || undoStack.current.length > 0;
  const canRedo = editorState.canRedo || redoStack.current.length > 0;

  return (
    <div className="document-view" style={{ display: active ? "flex" : "none" }}>
      <div className="toolbar">
        <button className={`icon-btn ${showThumbs ? "on" : ""}`} title={t("toolbar.thumbnails")} onClick={() => setShowThumbs((v) => !v)}>
          ▤
        </button>
        <div className="page-nav">
          <input
            value={pageInput}
            onChange={(e) => setPageInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && viewerRef.current?.goToPage(Number(pageInput) || 1)}
            onBlur={() => setPageInput(String(page))}
            title={t("toolbar.page")}
          />
          <span>/ {total}</span>
        </div>
        <button className="icon-btn" title={t("toolbar.zoomOut")} onClick={() => viewerRef.current?.zoomOut()}>
          −
        </button>
        <span className="zoom-label">{Math.round(scale * 100)}%</span>
        <button className="icon-btn" title={t("toolbar.zoomIn")} onClick={() => viewerRef.current?.zoomIn()}>
          +
        </button>
        <button className="icon-btn" title={t("toolbar.fitWidth")} onClick={() => viewerRef.current?.fit("page-width")}>
          ↔
        </button>
        <div className="sep" />
        <div className="mode-group">
          {MODES.map((m) => (
            <button
              key={m.mode}
              className={`icon-btn ${editorMode === m.mode ? "on" : ""}`}
              title={locked && m.mode !== "none" ? t("sign.lockedHint") : t(m.label as "edit.select")}
              onClick={() => setEditorMode(m.mode)}
              disabled={!doc || !!busy || (locked && m.mode !== "none")}
            >
              {m.icon}
            </button>
          ))}
        </div>
        {editorMode === "highlight" && (
          <div className="swatches">
            {Object.entries(HIGHLIGHT_COLORS).map(([name, hex]) => (
              <button
                key={name}
                className={`swatch ${highlightColor === name ? "on" : ""}`}
                style={{ background: hex }}
                title={name}
                onClick={() => {
                  setHighlightColor(name);
                  viewerRef.current?.setHighlightColor(name);
                }}
              />
            ))}
          </div>
        )}
        {editorMode === "freetext" && (
          <div className="text-style">
            <select value={noteStyle.size} title={t("obj.fontSize")} onChange={(e) => changeNoteStyle({ size: Number(e.target.value) })}>
              {FONT_SIZES.map((n) => (
                <option key={n} value={n}>
                  {n} pt
                </option>
              ))}
            </select>
            <ColorMenu value={noteStyle.color} onPick={(color) => changeNoteStyle({ color })} />
            <span className="mode-hint">{t("note.hint")}</span>
          </div>
        )}
        {editorMode === "stamp" && (
          <div className="text-style">
            <span className="mode-hint">{t("image.hint")}</span>
          </div>
        )}
        {(editorMode === "addtext" || editorMode === "objects") && (
          <div className="text-style">
            {editorMode === "addtext" && (
              <>
                <select value={textStyle.fontSize} title={t("obj.fontSize")} onChange={(e) => setTextStyle({ ...textStyle, fontSize: Number(e.target.value) })}>
                  {FONT_SIZES.map((n) => (
                    <option key={n} value={n}>
                      {n} pt
                    </option>
                  ))}
                </select>
                <input type="color" value={textStyle.color} title={t("obj.color")} onChange={(e) => setTextStyle({ ...textStyle, color: e.target.value })} />
                <button className={`icon-btn ${textStyle.bold ? "on" : ""}`} title={t("obj.bold")} onClick={() => setTextStyle({ ...textStyle, bold: !textStyle.bold })}>
                  <b>B</b>
                </button>
              </>
            )}
            <span className="mode-hint">{t(editorMode === "addtext" ? "obj.addHint" : "obj.hint")}</span>
          </div>
        )}
        <button className="icon-btn" title={t("edit.undo")} disabled={!canUndo || !!busy} onClick={undo}>
          ↶
        </button>
        <button className="icon-btn" title={t("edit.redo")} disabled={!canRedo || !!busy} onClick={redo}>
          ↷
        </button>
        <button className={`save-btn ${dirty ? "dirty" : ""}`} title={t("edit.saveHint")} disabled={!!busy || !doc} onClick={() => save(false)}>
          💾 {t("edit.save")}
        </button>
        <button className="icon-btn" title={t("sign.button")} disabled={!!busy || !doc} onClick={() => setDialog("sign")}>
          ✍
        </button>
        <div className="menu-wrap">
          <button className="icon-btn" title={t("edit.more")} onClick={() => setMenuOpen((v) => !v)}>
            ⋯
          </button>
          {menuOpen && (
            <div className="menu" onMouseLeave={() => setMenuOpen(false)}>
              <button onClick={() => (setMenuOpen(false), save(true))}>{t("edit.saveAs")}</button>
              <button onClick={() => (setMenuOpen(false), api.revealInExplorer(info.path).catch((e) => props.onToast(errorText(e))))}>
                {t("edit.reveal")}
              </button>
              <hr />
              <button onClick={() => (setMenuOpen(false), setDialog("sign"))}>✍ {t("sign.menu")}</button>
              <button onClick={() => (setMenuOpen(false), props.onBatchSign())}>✍ {t("sign.batchMenu")}</button>
              <button disabled={!signatures.length} onClick={() => (setMenuOpen(false), setDialog("signatures"))}>
                {t("sign.menuList")}
              </button>
              <hr />
              <button disabled={!scanned.length || locked} onClick={() => (setMenuOpen(false), runOcr(scanned))}>
                {t("ocr.scanned", { n: scanned.length })}
              </button>
              <button disabled={locked} onClick={() => (setMenuOpen(false), runOcr([page]))}>{t("ocr.current", { page })}</button>
              <hr />
              <button disabled={locked} onClick={() => (setMenuOpen(false), setPrompt({ kind: "findRedact", value: "" }))}>{t("edit.findRedact")}</button>
              <button disabled={locked} onClick={() => (setMenuOpen(false), insertFile(page))}>{t("pages.insertAfterCurrent", { page })}</button>
              <button onClick={startCompare}>{t("compare.start")}</button>
            </div>
          )}
        </div>
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
        <button className={`icon-btn ${showChat && !compare ? "on" : ""}`} title={t("toolbar.chat")} onClick={() => (compare ? setCompare(null) : setShowChat((v) => !v))}>
          💬
        </button>
      </div>

      {signatures.length > 0 && (
        <div className={`info-bar sign-bar ${locked ? "locked" : intactSigs.length ? "unlocked" : "broken"}`}>
          <span>
            {locked
              ? `🔒 ${t("sign.lockedBanner", { who: signersSummary(signatures, trust) })}`
              : intactSigs.length
                ? `⚠ ${t("sign.unlockedBanner", { who: signersSummary(signatures, trust) })}`
                : `⚠ ${t("sign.brokenBanner")}`}
          </span>
          <button className="link-btn" onClick={() => setDialog("signatures")}>
            {t("sign.details")}
          </button>
          {locked && <button onClick={() => setDialog("unlock")}>🔓 {t("sign.unlock")}</button>}
          {!locked && intactSigs.length > 0 && !dirty && <button onClick={() => setUnlocked(false)}>🔒 {t("sign.relock")}</button>}
        </div>
      )}
      {placeRequest && (
        <div className="info-bar redact-bar batch-place-bar">
          <span>✍ {t("sign.batchPlaceBar", { i: placeRequest.index + 1, n: placeRequest.total })}</span>
          {placeRequest.total - placeRequest.index > 1 && (
            <label className="check">
              <input type="checkbox" checked={sameForRest} onChange={(e) => setSameForRest(e.target.checked)} />{" "}
              {t("sign.sameForRest", { n: placeRequest.total - placeRequest.index - 1 })}
            </label>
          )}
          <button onClick={() => placeRequest.onPlace(null, sameForRest)}>{t("sign.defaultPlace")}</button>
          <button onClick={placeRequest.onCancel}>{t("common.cancel")}</button>
        </div>
      )}
      {editorMode === "signbox" && !placeRequest && (
        <div className="info-bar redact-bar">
          ✍ {t("sign.placeHint")}
          <button
            onClick={() => {
              pendingSign.current = null;
              setEditorMode("none");
            }}
          >
            {t("common.cancel")}
          </button>
        </div>
      )}
      {scanned.length > 0 && editorMode === "none" && !locked && (
        <div className="info-bar">
          {t("ocr.banner", { n: scanned.length, total })}
          <button className="link-btn" disabled={!!busy} onClick={() => runOcr(scanned)}>
            {t("ocr.run")}
          </button>
        </div>
      )}
      {(marks.length > 0 || editorMode === "redact") && (
        <div className="info-bar redact-bar">
          {marks.length ? t("edit.marksPending", { n: marks.length }) : t("edit.redactHint")}
          {marks.length > 0 && (
            <>
              <button className="primary" disabled={!!busy} onClick={applyPendingMarks}>
                {t("edit.applyMarks")}
              </button>
              <button onClick={() => setMarks([])}>{t("edit.clearMarks")}</button>
            </>
          )}
          <button className="link-btn" onClick={() => setPrompt({ kind: "findRedact", value: "" })}>
            {t("edit.findRedact")}
          </button>
        </div>
      )}

      <div className="workspace">
        {showThumbs && doc && (
          <Thumbnails
            key={docVersion}
            doc={doc}
            current={page}
            selected={selectedPages}
            busy={!!busy}
            locked={locked}
            onSelect={(pages, goTo) => {
              setSelectedPages(pages);
              if (goTo) viewerRef.current?.goToPage(goTo);
            }}
            onMove={(pages, after) => run(t("pages.moving"), (d, b) => movePages(d, b, pages, after))}
            onRotate={(pages) => run(t("pages.rotating"), (d, b) => rotatePages(d, b, pages, 90))}
            onDelete={(pages) => {
              if (window.confirm(t("pages.deleteConfirm", { pages: pages.join(", ") }))) run(t("pages.deleting"), (d, b) => deletePages(d, b, pages));
            }}
            onExtract={extract}
            onInsert={insertFile}
          />
        )}
        <div className="viewer-area">
          {loadError && <div className="banner error">{t("error.open")}: {loadError}</div>}
          {!doc && !loadError && <div className="loading">{t("loading")}</div>}
          {doc && (
            <PdfViewer
              ref={viewerRef}
              doc={doc}
              initialPage={firstPage.current}
              editorMode={editorMode}
              marks={marks}
              onAddMark={(m) => setMarks((cur) => [...cur, m])}
              onRemoveMark={(id) => setMarks((cur) => cur.filter((m) => m.id !== id))}
              onPageChange={setPage}
              onScaleChange={setScale}
              onFindCount={(current, total) => setFindCount({ current, total })}
              onSelection={setFloatingSel}
              onEditorState={setEditorState}
              textStyle={textStyle}
              objectsFor={objectsFor}
              onObjectOps={objectOps}
              onSignBox={placeSignature}
            />
          )}
          {floatingSel && (
            <div className="selection-menu" style={{ left: floatingSel.x, top: floatingSel.y }} onMouseDown={(e) => e.preventDefault()}>
              <button className="primary" onClick={askAboutSelection}>
                ✨ {t("selection.ask")}
              </button>
              {!locked && <button onClick={correctSelection}>✏ {t("selection.correct")}</button>}
              {!locked && <button onClick={redactSelection}>■ {t("selection.redact")}</button>}
            </div>
          )}
          {busy && (
            <div className="busy-overlay">
              <div className="busy-card">
                <div className="spinner" /> {busy}
              </div>
            </div>
          )}
          {prompt && (
            <div className="modal-backdrop" onMouseDown={() => setPrompt(null)}>
              <div className="modal small" onMouseDown={(e) => e.stopPropagation()}>
                <h2>{prompt.kind === "correct" ? t("edit.correctTitle") : t("edit.findRedact")}</h2>
                <p className="hint">{prompt.kind === "correct" ? t("edit.correctHint") : t("edit.findRedactHint")}</p>
                {prompt.kind === "correct" && (
                  <div className="font-choice">
                    <span>{t("edit.correctFont")}</span>
                    {(["serif", "sans"] as const).map((f) => (
                      <button key={f} className={prompt.font === f ? "on" : ""} style={{ fontFamily: MARK_FONTS[f] }} onClick={() => setPrompt({ ...prompt, font: f })}>
                        {t(f === "serif" ? "edit.fontSerif" : "edit.fontSans")}
                      </button>
                    ))}
                  </div>
                )}
                <input
                  autoFocus
                  value={prompt.value}
                  style={prompt.kind === "correct" ? { fontFamily: MARK_FONTS[prompt.font] } : undefined}
                  onChange={(e) => setPrompt({ ...prompt, value: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && prompt.value.trim()) submitPrompt();
                    if (e.key === "Escape") setPrompt(null);
                  }}
                />
                <div className="modal-footer">
                  <button onClick={() => setPrompt(null)}>{t("common.cancel")}</button>
                  <button className="primary" disabled={!prompt.value.trim()} onClick={submitPrompt}>
                    {prompt.kind === "correct" ? t("edit.addCorrection") : t("edit.findAndMark")}
                  </button>
                </div>
              </div>
            </div>
          )}
          {dialog === "sign" && <SignDialog onCancel={() => setDialog(null)} onSubmit={submitSign} />}
          {dialog === "signatures" && <SignaturesDialog signatures={signatures} trust={trust} trustError={trustError} onClose={() => setDialog(null)} />}
          {dialog === "unlock" && (
            <UnlockDialog
              count={intactSigs.length}
              onCancel={() => setDialog(null)}
              onConfirm={() => {
                setUnlocked(true);
                setDialog(null);
              }}
            />
          )}
        </div>
        {showChat && (
          <>
            <div className="resizer" onMouseDown={startResize} />
            <div style={{ width: chatWidth, display: "flex", minWidth: 0 }}>
              {compare && (
                <CompareView
                  otherName={compare.name}
                  changes={compare.changes}
                  onGoTo={(p) => viewerRef.current?.goToPage(p)}
                  onClose={() => setCompare(null)}
                  onSummarize={() => {
                    const text = changesForChat(compare.name, compare.changes);
                    setCompare(null);
                    chatRef.current?.sendText(text, t("compare.summarizeAction", { name: compare.name }));
                  }}
                />
              )}
              <div style={{ display: compare ? "none" : "flex", flex: 1, minWidth: 0 }}>
                <ChatPanel
                  key={info.hash}
                  ref={chatRef}
                  scope={{ kind: "document", doc: info }}
                  indexed={indexed}
                  hasKey={hasKey}
                  selection={chatSelection}
                  onClearSelection={() => setChatSelection(null)}
                  onOpenSettings={onOpenSettings}
                  onCitation={onCitation}
                  onTool={runTool}
                />
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
});
