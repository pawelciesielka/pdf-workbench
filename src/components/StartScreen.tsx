import { useEffect, useState } from "react";
import { t } from "../i18n";
import { api, type RecentItem } from "../lib/api";

export function StartScreen({ onOpen, onPick }: { onOpen(path: string): void; onPick(): void }) {
  const [recent, setRecent] = useState<RecentItem[]>([]);
  useEffect(() => {
    api.listRecent().then(setRecent).catch(() => setRecent([]));
  }, []);

  const remove = async (path: string) => {
    await api.removeRecent(path);
    setRecent((r) => r.filter((x) => x.path !== path));
  };

  return (
    <div className="start">
      <div className="start-card">
        <div className="start-icon">📄</div>
        <h1>{t("start.title")}</h1>
        <p>{t("start.hint")}</p>
        <button className="primary big" onClick={onPick}>
          {t("start.open")}
        </button>
        {recent.length > 0 && (
          <div className="recent">
            <h2>{t("start.recent")}</h2>
            <ul>
              {recent.map((r) => (
                <li key={r.path} className={r.exists ? "" : "missing"}>
                  <button className="recent-open" disabled={!r.exists} onClick={() => onOpen(r.path)} title={r.path}>
                    <span className="recent-name">{r.name}</span>
                    <span className="recent-path">{r.exists ? r.path : t("start.missing")}</span>
                  </button>
                  <button className="icon-btn" title={t("start.remove")} onClick={() => remove(r.path)}>
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        <p className="shortcuts">{t("start.shortcuts")}</p>
      </div>
    </div>
  );
}
