import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { DocumentView, type DocumentViewHandle } from "./components/DocumentView";
import { SettingsDialog, type Theme } from "./components/SettingsDialog";
import { BatchSign } from "./components/Signing";
import type { Placement } from "./lib/api";
import { StartScreen } from "./components/StartScreen";
import { WorkspaceDialog, WorkspaceView } from "./components/Workspaces";
import { t } from "./i18n";
import { api, errorText, type CitationView, type DocumentInfo, type Settings, type ToolCall, type ToolOutcome, type WorkspaceInfo } from "./lib/api";

type Tab = { id: string; kind: "document"; info: DocumentInfo } | { id: string; kind: "workspace"; workspace: WorkspaceInfo };

let tabSeq = 0;
const newTabId = () => `tab${++tabSeq}`;

function loadTheme(): Theme {
  try {
    return (localStorage.getItem("theme") as Theme) || "system";
  } catch {
    return "system";
  }
}

export default function App() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [dirty, setDirty] = useState<Record<string, boolean>>({});
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [wsDialog, setWsDialog] = useState<{ editing: WorkspaceInfo | null } | null>(null);
  const [theme, setTheme] = useState<Theme>(loadTheme);
  const [dragging, setDragging] = useState(false);
  /** Pliki wybrane do podpisania naraz. */
  const [batchFiles, setBatchFiles] = useState<string[] | null>(null);
  /** Wskazywanie miejsc podpisu w kolejnych plikach (otwieranych w kartach edytora). */
  const [placeSession, setPlaceSession] = useState<{
    files: string[];
    index: number;
    placements: (Placement | null)[];
    tabId: string;
    resolve(p: (Placement | null)[] | null): void;
  } | null>(null);
  const [toast, setToast] = useState<{ text: string; kind: "error" | "info" } | null>(null);
  const views = useRef(new Map<string, DocumentViewHandle>());
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  if (import.meta.env.DEV) {
    // Uchwyt do testów w trybie deweloperskim (WebView2 + CDP).
    (window as unknown as { __pdfwb: unknown }).__pdfwb = { views: views.current, tabs: tabsRef, open: (p: string) => openPathsRef.current?.([p]), batch: (files: string[]) => setBatchFiles(files) };
  }
  const openPathsRef = useRef<((paths: string[]) => Promise<string | undefined>) | null>(null);

  const refreshSettings = useCallback(() => {
    api.getSettings().then(setSettings).catch(() => undefined);
  }, []);
  useEffect(refreshSettings, [refreshSettings]);

  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", theme);
    // Pasek tytułu okna (systemowy) w tym samym motywie; null = zgodnie z systemem.
    getCurrentWindow()
      .setTheme(theme === "system" ? null : theme)
      .catch(() => {});
    try {
      localStorage.setItem("theme", theme);
    } catch {
      /* brak dostępu do localStorage */
    }
  }, [theme]);

  const toastTimer = useRef<number | undefined>(undefined);
  const showToast = useCallback((text: string, kind: "error" | "info" = "error") => {
    setToast({ text, kind });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), kind === "error" ? 6000 : 3500);
  }, []);

  const addDocTab = useCallback((info: DocumentInfo) => {
    const existing = tabsRef.current.find((x) => x.kind === "document" && x.info.hash === info.hash);
    if (existing) {
      setActiveId(existing.id);
      return existing.id;
    }
    const id = newTabId();
    const tab: Tab = { id, kind: "document", info };
    tabsRef.current = [...tabsRef.current, tab];
    setTabs((ts) => (ts.some((x) => x.kind === "document" && x.info.hash === info.hash) ? ts : [...ts, tab]));
    setActiveId(id);
    return id;
  }, []);

  const openPaths = useCallback(
    async (paths: string[]) => {
      let last: string | undefined;
      for (const path of paths) {
        try {
          last = addDocTab(await api.openDocument(path));
        } catch (e) {
          showToast(`${t("error.open")}: ${errorText(e)}`);
        }
      }
      return last;
    },
    [addDocTab, showToast],
  );

  openPathsRef.current = openPaths;

  const pickFiles = useCallback(async () => {
    const picked = await openDialog({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!picked) return;
    openPaths(Array.isArray(picked) ? picked : [picked]);
  }, [openPaths]);

  const closeTab = useCallback((id: string) => {
    if (dirtyRef.current[id] && !window.confirm(t("tabs.closeDirty"))) return;
    const ts = tabsRef.current;
    const idx = ts.findIndex((x) => x.id === id);
    const next = ts.filter((x) => x.id !== id);
    setTabs(next);
    setDirty((d) => {
      const { [id]: _, ...rest } = d;
      return rest;
    });
    setActiveId((cur) => (cur === id ? (next[Math.min(idx, next.length - 1)]?.id ?? null) : cur));
  }, []);

  const openWorkspace = useCallback(
    async (ws: WorkspaceInfo) => {
      setWsDialog(null);
      // Dokumenty Przestrzeni otwieramy w tle jako karty, potem pokazujemy kartę Przestrzeni.
      for (const d of ws.documents) {
        if (d.path && !tabsRef.current.some((x) => x.kind === "document" && x.info.hash === d.hash)) {
          try {
            const info = await api.openDocument(d.path);
            const tab: Tab = { id: newTabId(), kind: "document", info };
            tabsRef.current = [...tabsRef.current, tab];
            setTabs((ts) => [...ts, tab]);
          } catch {
            /* brakujący plik — zostaje na liście jako niedostępny */
          }
        }
      }
      const existing = tabsRef.current.find((x) => x.kind === "workspace" && x.workspace.id === ws.id);
      if (existing) {
        setTabs((ts) => ts.map((x) => (x.id === existing.id ? { ...x, workspace: ws } : x)));
        setActiveId(existing.id);
      } else {
        const id = newTabId();
        const tab: Tab = { id, kind: "workspace", workspace: ws };
        tabsRef.current = [...tabsRef.current, tab];
        setTabs((ts) => [...ts, tab]);
        setActiveId(id);
      }
    },
    [],
  );

  const docTabByHashOrName = (hash: string | null, name?: string) =>
    tabsRef.current.find((x) => x.kind === "document" && (x.info.hash === hash || (!!name && x.info.name.toLowerCase() === name.toLowerCase()))) as
      | Extract<Tab, { kind: "document" }>
      | undefined;

  const wsCitation = (c: CitationView) => {
    const tab = docTabByHashOrName(c.documentHash, c.documentName ?? undefined);
    if (!tab) return showToast(t("ws.docNotOpen"));
    setActiveId(tab.id);
    if (c.page) setTimeout(() => views.current.get(tab.id)?.showCitation(c.page!, c.citedText), 60);
  };

  const wsTool = async (call: ToolCall): Promise<ToolOutcome> => {
    const name = String(call.input.document ?? "");
    const tab = docTabByHashOrName(null, name);
    if (!tab) return { ok: false, text: `Dokument „${name}” nie jest otwarty w aplikacji.` };
    setActiveId(tab.id);
    await new Promise((r) => setTimeout(r, 80));
    const view = views.current.get(tab.id);
    if (!view) return { ok: false, text: "Widok Dokumentu nie jest gotowy." };
    return view.runTool(call);
  };

  // Pliki z wiersza poleceń, z drugiej instancji (skojarzenie .pdf) i przeciągnięte na okno.
  const initialLoaded = useRef(false);
  useEffect(() => {
    if (!initialLoaded.current) {
      initialLoaded.current = true;
      api.getInitialFiles().then((f) => {
        if (f.length) openPaths(f);
      });
    }
    const unlistenOpen = listen<string[]>("open-files", (e) => openPaths(e.payload));
    const unlistenDrop = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === "enter" || p.type === "over") setDragging(true);
      else if (p.type === "leave") setDragging(false);
      else if (p.type === "drop") {
        setDragging(false);
        const pdfs = p.paths.filter((x) => x.toLowerCase().endsWith(".pdf"));
        if (pdfs.length) openPaths(pdfs);
      }
    });
    const unlistenClose = getCurrentWindow().onCloseRequested((e) => {
      if (Object.values(dirtyRef.current).some(Boolean) && !window.confirm(t("app.closeDirty"))) e.preventDefault();
    });
    return () => {
      unlistenOpen.then((u) => u());
      unlistenDrop.then((u) => u());
      unlistenClose.then((u) => u());
    };
  }, [openPaths]);

  // Skróty klawiszowe (faza przechwytywania, żeby Ctrl+Z nie trafiał podwójnie do pdf.js).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const view = activeId ? views.current.get(activeId) : undefined;
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);
      if (e.key === "Escape" && view && !typing) {
        view.escape();
        return;
      }
      if (!e.ctrlKey) return;
      const k = e.key.toLowerCase();
      const stop = () => {
        e.preventDefault();
        e.stopPropagation();
      };
      if (k === "o") {
        stop();
        pickFiles();
      } else if (k === "s" && view) {
        stop();
        view.save(e.shiftKey);
      } else if (k === "z" && view && !typing) {
        stop();
        if (e.shiftKey) view.redo();
        else view.undo();
      } else if (k === "y" && view && !typing) {
        stop();
        view.redo();
      } else if (k === "w" && activeId) {
        stop();
        closeTab(activeId);
      } else if (k === "tab" && tabs.length > 1) {
        stop();
        const i = tabs.findIndex((x) => x.id === activeId);
        const n = (i + (e.shiftKey ? -1 : 1) + tabs.length) % tabs.length;
        setActiveId(tabs[n].id);
      } else if (k === "f" && view) {
        stop();
        view.focusSearch();
      } else if (k === "j" && view) {
        stop();
        view.toggleChat();
      } else if ((k === "=" || k === "+") && view) {
        stop();
        view.zoomIn();
      } else if (k === "-" && view) {
        stop();
        view.zoomOut();
      } else if (k === "0" && view) {
        stop();
        view.fitWidth();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [activeId, tabs, pickFiles, closeTab]);

  /** Otwiera plik `index` w karcie; pliki, których nie da się otworzyć, dostają domyślne miejsce. */
  const placeNext = async (files: string[], index: number, placements: (Placement | null)[], resolve: (p: (Placement | null)[] | null) => void) => {
    let i = index;
    const acc = [...placements];
    while (i < files.length) {
      const tabId = await openPaths([files[i]]);
      if (tabId) {
        setPlaceSession({ files, index: i, placements: acc, tabId, resolve });
        return;
      }
      acc.push(null);
      i++;
    }
    setPlaceSession(null);
    resolve(acc);
  };

  const placeInEditor = (files: string[]) =>
    new Promise<(Placement | null)[] | null>((resolve) => void placeNext(files, 0, [], resolve));

  const onPlaced = (p: Placement | null, sameForRest: boolean) => {
    const s = placeSession;
    if (!s) return;
    const placements = [...s.placements, p];
    if (sameForRest) while (placements.length < s.files.length) placements.push(p);
    if (placements.length >= s.files.length) {
      setPlaceSession(null);
      s.resolve(placements);
    } else void placeNext(s.files, placements.length, placements, s.resolve);
  };

  const cancelPlacing = () => {
    placeSession?.resolve(null);
    setPlaceSession(null);
  };

  const startBatchSign = async () => {
    const picked = await openDialog({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    const files = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (files.length) setBatchFiles(files);
  };

  const cycleTheme = () => setTheme((th) => (th === "system" ? "dark" : th === "dark" ? "light" : "system"));
  const docTabs = tabs.filter((x): x is Extract<Tab, { kind: "document" }> => x.kind === "document");
  const hasKey = !!settings?.apiKeySource;

  return (
    <div className="app">
      <div className="tabbar">
        <div className="tabs">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={`tab ${tab.id === activeId ? "active" : ""} ${tab.kind === "workspace" ? "ws-tab" : ""}`}
              onClick={() => setActiveId(tab.id)}
              onAuxClick={(e) => e.button === 1 && closeTab(tab.id)}
              title={tab.kind === "document" ? tab.info.path : tab.workspace.name}
            >
              <span className="tab-name">
                {tab.kind === "workspace" ? `🗂 ${tab.workspace.name}` : tab.info.name}
                {dirty[tab.id] && <span className="dirty-dot" title={t("tabs.unsaved")}> ●</span>}
              </span>
              <button
                className="tab-close"
                title={t("tabs.close")}
                onClick={(e) => {
                  e.stopPropagation();
                  closeTab(tab.id);
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button className="icon-btn" title={t("tabs.new")} onClick={pickFiles}>
            ＋
          </button>
        </div>
        <button className="icon-btn" title={t("sign.batchMenu")} onClick={startBatchSign}>
          ✍
        </button>
        <button className="icon-btn" title={t("ws.title")} onClick={() => setWsDialog({ editing: null })}>
          🗂
        </button>
        <button className="icon-btn" title={t("toolbar.theme")} onClick={cycleTheme}>
          {theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐"}
        </button>
        <button className="icon-btn" title={t("toolbar.settings")} onClick={() => setShowSettings(true)}>
          ⚙
        </button>
      </div>

      <main className="content">
        {tabs.length === 0 && <StartScreen onOpen={(p) => openPaths([p])} onPick={pickFiles} onWorkspaces={() => setWsDialog({ editing: null })} />}
        {tabs.map((tab) =>
          tab.kind === "document" ? (
            <DocumentView
              key={tab.id}
              ref={(h) => {
                if (h) views.current.set(tab.id, h);
                else views.current.delete(tab.id);
              }}
              info={tab.info}
              active={tab.id === activeId}
              hasKey={hasKey}
              onOpenSettings={() => setShowSettings(true)}
              onSaved={(info, message) => {
                setTabs((ts) => ts.map((x) => (x.id === tab.id ? { ...x, info } : x)));
                if (message) showToast(message, "info");
              }}
              onOpenNew={addDocTab}
              onDirtyChange={(d) => setDirty((cur) => (cur[tab.id] === d ? cur : { ...cur, [tab.id]: d }))}
              onToast={showToast}
              onBatchSign={startBatchSign}
              placeRequest={
                placeSession?.tabId === tab.id
                  ? { index: placeSession.index, total: placeSession.files.length, onPlace: onPlaced, onCancel: cancelPlacing }
                  : undefined
              }
            />
          ) : (
            <WorkspaceView
              key={tab.id}
              workspace={tab.workspace}
              active={tab.id === activeId}
              hasKey={hasKey}
              openHashes={docTabs.map((d) => d.info.hash)}
              onOpenSettings={() => setShowSettings(true)}
              onCitation={wsCitation}
              onTool={wsTool}
              onOpenDoc={(d) => {
                const tabDoc = docTabByHashOrName(d.hash);
                if (tabDoc) setActiveId(tabDoc.id);
                else if (d.path) openPaths([d.path]);
              }}
              onEdit={() => setWsDialog({ editing: tab.workspace })}
            />
          ),
        )}
      </main>

      {dragging && <div className="drop-overlay">{t("drop.here")}</div>}
      {toast && <div className={`toast ${toast.kind}`}>{toast.text}</div>}
      {showSettings && settings && (
        <SettingsDialog settings={settings} theme={theme} onTheme={setTheme} onChanged={refreshSettings} onClose={() => setShowSettings(false)} />
      )}
      {batchFiles && <BatchSign files={batchFiles} onClose={() => setBatchFiles(null)} onOpen={(p) => openPaths([p])} placeInEditor={placeInEditor} />}
      {wsDialog && <WorkspaceDialog openDocs={docTabs.map((d) => d.info)} editing={wsDialog.editing} onOpen={openWorkspace} onClose={() => setWsDialog(null)} />}
    </div>
  );
}
