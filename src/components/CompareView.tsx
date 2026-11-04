import { t } from "../i18n";
import type { Change } from "../lib/editing";

interface Props {
  otherName: string;
  changes: Change[];
  onGoTo(page: number): void;
  onSummarize(): void;
  onClose(): void;
}

const LABEL: Record<Change["kind"], string> = {
  added: "compare.added",
  removed: "compare.removed",
  changed: "compare.changed",
};

export function CompareView({ otherName, changes, onGoTo, onSummarize, onClose }: Props) {
  const counts = {
    added: changes.filter((c) => c.kind === "added").length,
    removed: changes.filter((c) => c.kind === "removed").length,
    changed: changes.filter((c) => c.kind === "changed").length,
  };
  return (
    <aside className="chat compare">
      <header className="chat-header">
        <strong className="compare-title">{t("compare.title", { name: otherName })}</strong>
        <button className="icon-btn" onClick={onClose} title={t("settings.close")}>
          ✕
        </button>
      </header>
      <div className="chat-notice">
        {changes.length === 0
          ? t("compare.none")
          : t("compare.summary", { added: counts.added, removed: counts.removed, changed: counts.changed })}
        <div className="compare-legend">{t("compare.legend")}</div>
      </div>
      <div className="chat-messages">
        {changes.map((c, i) => (
          <button key={i} className={`change change-${c.kind}`} onClick={() => c.newPage && onGoTo(c.newPage)}>
            <div className="change-head">
              <span>{t(LABEL[c.kind] as "compare.added")}</span>
              <span>
                {c.oldPage ? t("compare.oldPage", { page: c.oldPage }) : ""} {c.newPage ? `→ ${t("chat.page", { page: c.newPage })}` : ""}
              </span>
            </div>
            {c.context && <span className="change-context">…{c.context} </span>}
            {c.oldText && <del>{c.oldText.length > 400 ? c.oldText.slice(0, 400) + "…" : c.oldText}</del>}
            {c.newText && <ins>{c.newText.length > 400 ? c.newText.slice(0, 400) + "…" : c.newText}</ins>}
          </button>
        ))}
      </div>
      {changes.length > 0 && (
        <footer className="chat-input">
          <button className="primary" onClick={onSummarize}>
            ✨ {t("compare.summarize")}
          </button>
        </footer>
      )}
    </aside>
  );
}

/** Opis zmian dla czatu (z limitem długości). */
export function changesForChat(otherName: string, changes: Change[]): string {
  const lines = changes.map((c) => {
    const where = `${c.oldPage ? `stara s. ${c.oldPage}` : ""}${c.newPage ? ` → nowa s. ${c.newPage}` : ""}`.trim();
    if (c.kind === "added") return `+ [${where}] ${c.newText}`;
    if (c.kind === "removed") return `− [${where}] ${c.oldText}`;
    return `~ [${where}] (…${c.context}) „${c.oldText}” → „${c.newText}”`;
  });
  let text = lines.join("\n");
  if (text.length > 12000) text = text.slice(0, 12000) + "\n[…lista skrócona]";
  return `Porównałem bieżący Dokument z plikiem „${otherName}” (wcześniejsza wersja). Lista zmian w tekście (słowo po słowie):\n\n${text}\n\nPodsumuj najważniejsze zmiany merytoryczne: co dodano, usunięto i zmieniono, i co to oznacza w praktyce. Pomiń drobne zmiany formatowania.`;
}
