import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { DocumentView, type DocumentViewHandle } from "./components/DocumentView";
import { SettingsDialog, type Theme } from "./components/SettingsDialog";
import { StartScreen } from "./components/StartScreen";
import { t } from "./i18n";
import { api, errorText, type DocumentInfo, type Settings } from "./lib/api";

function loadTheme(): Theme {
  try {
    return (localStorage.getItem("theme") as Theme) || "system";
  } catch {
    return "system";
  }
}

export default function App() {
  const [tabs, setTabs] = useState<DocumentInfo[]>([]);
  const [activeHash, setActiveHash] = useState<string | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [theme, setTheme] = useState<Theme>(loadTheme);
  const [dragging, setDragging] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const views = useRef(new Map<string, DocumentViewHandle>());
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;

  const refreshSettings = useCallback(() => {
    api.getSettings().then(setSettings).catch(() => undefined);
  }, []);
  useEffect(refreshSettings, [refreshSettings]);

  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem("theme", theme);
    } catch {
      /* brak dostępu do localStorage */
    }
  }, [theme]);

  const showError = (msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 5000);
  };

  const openPaths = useCallback(async (paths: string[]) => {
    for (const path of paths) {
      try {
        const info = await api.openDocument(path);
        setTabs((ts) => (ts.some((x) => x.hash === info.hash) ? ts : [...ts, info]));
        setActiveHash(info.hash);
      } catch (e) {
        showError(`${t("error.open")}: ${errorText(e)}`);
      }
    }
  }, []);

  const pickFiles = useCallback(async () => {
    const picked = await openDialog({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!picked) return;
    openPaths(Array.isArray(picked) ? picked : [picked]);
  }, [openPaths]);

  const closeTab = useCallback((hash: string) => {
    const ts = tabsRef.current;
    const idx = ts.findIndex((x) => x.hash === hash);
    const next = ts.filter((x) => x.hash !== hash);
    setTabs(next);
    setActiveHash((cur) => (cur === hash ? (next[Math.min(idx, next.length - 1)]?.hash ?? null) : cur));
  }, []);

  // Pliki z wiersza poleceń, z drugiej instancji (skojarzenie .pdf) i przeciągnięte na okno.
  useEffect(() => {
    api.getInitialFiles().then((f) => {
      if (f.length) openPaths(f);
    });
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
    return () => {
      unlistenOpen.then((u) => u());
      unlistenDrop.then((u) => u());
    };
  }, [openPaths]);

  // Skróty klawiszowe.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey) return;
      const view = activeHash ? views.current.get(activeHash) : undefined;
      const k = e.key.toLowerCase();
      if (k === "o") {
        e.preventDefault();
        pickFiles();
      } else if (k === "w" && activeHash) {
        e.preventDefault();
        closeTab(activeHash);
      } else if (k === "tab" && tabs.length > 1) {
        e.preventDefault();
        const i = tabs.findIndex((x) => x.hash === activeHash);
        const n = (i + (e.shiftKey ? -1 : 1) + tabs.length) % tabs.length;
        setActiveHash(tabs[n].hash);
      } else if (k === "f" && view) {
        e.preventDefault();
        view.focusSearch();
      } else if (k === "j" && view) {
        e.preventDefault();
        view.toggleChat();
      } else if ((k === "=" || k === "+") && view) {
        e.preventDefault();
        view.zoomIn();
      } else if (k === "-" && view) {
        e.preventDefault();
        view.zoomOut();
      } else if (k === "0" && view) {
        e.preventDefault();
        view.fitWidth();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeHash, tabs, pickFiles, closeTab]);

  const cycleTheme = () => setTheme((th) => (th === "system" ? "dark" : th === "dark" ? "light" : "system"));

  return (
    <div className="app">
      <div className="tabbar">
        <div className="tabs">
          {tabs.map((d) => (
            <div
              key={d.hash}
              className={`tab ${d.hash === activeHash ? "active" : ""}`}
              onClick={() => setActiveHash(d.hash)}
              onAuxClick={(e) => e.button === 1 && closeTab(d.hash)}
              title={d.path}
            >
              <span className="tab-name">{d.name}</span>
              <button
                className="tab-close"
                title={t("tabs.close")}
                onClick={(e) => {
                  e.stopPropagation();
                  closeTab(d.hash);
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
        <button className="icon-btn" title={t("toolbar.theme")} onClick={cycleTheme}>
          {theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐"}
        </button>
        <button className="icon-btn" title={t("toolbar.settings")} onClick={() => setShowSettings(true)}>
          ⚙
        </button>
      </div>

      <main className="content">
        {tabs.length === 0 && <StartScreen onOpen={(p) => openPaths([p])} onPick={pickFiles} />}
        {tabs.map((d) => (
          <DocumentView
            key={d.hash}
            ref={(h) => {
              if (h) views.current.set(d.hash, h);
              else views.current.delete(d.hash);
            }}
            info={d}
            active={d.hash === activeHash}
            hasKey={!!settings?.apiKeySource}
            onOpenSettings={() => setShowSettings(true)}
          />
        ))}
      </main>

      {dragging && <div className="drop-overlay">{t("drop.here")}</div>}
      {toast && <div className="toast">{toast}</div>}
      {showSettings && settings && (
        <SettingsDialog settings={settings} theme={theme} onTheme={setTheme} onChanged={refreshSettings} onClose={() => setShowSettings(false)} />
      )}
    </div>
  );
}
