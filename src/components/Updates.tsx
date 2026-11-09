// Aktualizacje z wydań na GitHubie (wtyczka updater Tauri; latest.json i instalator podpisany kluczem wydawcy).
import { useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { t } from "../i18n";
import { errorText } from "../lib/api";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "none" }
  | { kind: "available"; update: Update }
  | { kind: "installing"; percent: number | null }
  | { kind: "error"; text: string };

const SKIP_KEY = "updates.skipVersion";

/** Stan aktualizacji wspólny dla paska powiadomienia i Ustawień. */
export function useUpdates(hasUnsaved: () => boolean) {
  const [state, setState] = useState<UpdateState>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);
  const busy = useRef(false);

  const runCheck = async (manual: boolean) => {
    if (busy.current || import.meta.env.DEV) {
      if (import.meta.env.DEV && manual) setState({ kind: "error", text: t("updates.devMode") });
      return;
    }
    busy.current = true;
    setState({ kind: "checking" });
    try {
      const update = await check();
      if (!update) setState({ kind: "none" });
      else {
        setState({ kind: "available", update });
        let skipped: string | null = null;
        try {
          skipped = localStorage.getItem(SKIP_KEY);
        } catch {
          /* brak pamięci przeglądarki — pokazujemy zawsze */
        }
        setDismissed(!manual && skipped === update.version);
      }
    } catch (e) {
      // Automatyczne sprawdzenie bez internetu nie powinno niepokoić — błąd tylko przy ręcznym.
      setState(manual ? { kind: "error", text: errorText(e) } : { kind: "idle" });
    } finally {
      busy.current = false;
    }
  };

  useEffect(() => {
    const id = window.setTimeout(() => runCheck(false), 4000);
    return () => window.clearTimeout(id);
  }, []);

  const install = async () => {
    if (state.kind !== "available") return;
    if (hasUnsaved() && !window.confirm(t("updates.unsaved"))) return;
    const update = state.update;
    let total = 0;
    let done = 0;
    setState({ kind: "installing", percent: null });
    try {
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data.contentLength ?? 0;
        else if (ev.event === "Progress") {
          done += ev.data.chunkLength;
          if (total) setState({ kind: "installing", percent: Math.min(100, Math.round((done / total) * 100)) });
        }
      });
      // Na Windows instalator sam zamyka program; na wszelki wypadek uruchamiamy go ponownie.
      await relaunch();
    } catch (e) {
      setState({ kind: "error", text: errorText(e) });
    }
  };

  const later = () => {
    setDismissed(true);
    if (state.kind === "available") {
      try {
        localStorage.setItem(SKIP_KEY, state.update.version);
      } catch {
        /* bez znaczenia */
      }
    }
  };

  return { state, dismissed, check: () => runCheck(true), install, later };
}

export type Updates = ReturnType<typeof useUpdates>;

/** Pasek u dołu okna: „Dostępna nowa wersja…”. */
export function UpdateBanner({ updates }: { updates: Updates }) {
  const { state, dismissed } = updates;
  if (state.kind === "installing")
    return (
      <div className="update-banner">
        <span>{state.percent == null ? t("updates.downloading") : t("updates.downloadingPct", { n: state.percent })}</span>
      </div>
    );
  if (state.kind !== "available" || dismissed) return null;
  return (
    <div className="update-banner">
      <span>{t("updates.available", { v: state.update.version })}</span>
      <button className="primary" onClick={updates.install}>
        {t("updates.install")}
      </button>
      <button onClick={updates.later}>{t("updates.later")}</button>
    </div>
  );
}

/** Sekcja w Ustawieniach: wersja programu i ręczne sprawdzenie. */
export function UpdateSettings({ updates, version }: { updates: Updates; version: string }) {
  const { state } = updates;
  const status =
    state.kind === "checking"
      ? t("updates.checking")
      : state.kind === "none"
        ? t("updates.upToDate")
        : state.kind === "available"
          ? t("updates.available", { v: state.update.version })
          : state.kind === "installing"
            ? t("updates.downloading")
            : state.kind === "error"
              ? t("updates.error", { e: state.text })
              : "";
  return (
    <>
      <label>{t("updates.title")}</label>
      <div className="row">
        <span>{t("updates.version", { v: version })}</span>
        {state.kind === "available" ? (
          <button className="primary" onClick={updates.install}>
            {t("updates.install")}
          </button>
        ) : (
          <button disabled={state.kind === "checking" || state.kind === "installing"} onClick={updates.check}>
            {t("updates.check")}
          </button>
        )}
      </div>
      {status && <p className="hint">{status}</p>}
    </>
  );
}
