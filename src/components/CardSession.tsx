import { relaunch } from "@tauri-apps/plugin-process";
import { useEffect, useState } from "react";
import { t } from "../i18n";
import { api, errorText } from "../lib/api";

/** Klucz localStorage: dokumenty do ponownego otwarcia po restarcie „Rozłącz”. */
export const RESTORE_KEY = "restoreAfterDisconnect";

/** Dokumenty zapamiętane przed restartem (jednorazowo — odczyt czyści wpis). */
export function takeRestorePaths(): string[] {
  try {
    const v = localStorage.getItem(RESTORE_KEY);
    localStorage.removeItem(RESTORE_KEY);
    const paths: unknown = v ? JSON.parse(v) : [];
    return Array.isArray(paths) ? paths.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}

/** Znacznik w pasku kart: po podpisie kluczem SimplySign biblioteka SimplySign CSP pamięta PIN
 * w procesie pdf-workbench aż do jego zamknięcia. „Rozłącz” uruchamia program ponownie (PIN
 * znika razem z procesem) i otwiera z powrotem te same dokumenty. */
export function CardSession(props: { openPaths(): string[]; hasUnsaved(): boolean; onError(text: string): void }) {
  const [since, setSince] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    let alive = true;
    const check = () =>
      api
        .simplySignStatus()
        .then((s) => alive && (setSince(s.pinSince), setNow(Date.now())))
        .catch(() => alive && setSince(null));
    check();
    const id = window.setInterval(check, 5000);
    window.addEventListener("focus", check);
    // Podpis mógł właśnie się zakończyć — sprawdź chwilę po każdym kliknięciu.
    const onClick = () => window.setTimeout(check, 1500);
    window.addEventListener("pointerup", onClick);
    return () => {
      alive = false;
      window.clearInterval(id);
      window.removeEventListener("focus", check);
      window.removeEventListener("pointerup", onClick);
    };
  }, []);

  const disconnect = async () => {
    if (props.hasUnsaved() && !window.confirm(t("card.disconnectDirty"))) return;
    try {
      localStorage.setItem(RESTORE_KEY, JSON.stringify(props.openPaths()));
    } catch {
      /* bez localStorage dokumenty po prostu nie otworzą się ponownie */
    }
    try {
      await relaunch();
    } catch (e) {
      props.onError(errorText(e));
    }
  };

  if (since == null) return null;
  const min = Math.floor((now - since) / 60000);
  return (
    <span className="card-session" title={t("card.hint")}>
      🔓 {t("card.active", { time: min < 1 ? t("card.justNow") : t("card.minutes", { n: min }) })}
      <button title={t("card.disconnectHint")} onClick={disconnect}>
        {t("card.disconnect")}
      </button>
      <button
        className="card-dismiss"
        title={t("card.dismiss")}
        onClick={() => {
          api.simplySignForget().catch(() => {});
          setSince(null);
        }}
      >
        ✕
      </button>
    </span>
  );
}
