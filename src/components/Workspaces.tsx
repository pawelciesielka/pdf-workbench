import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type CitationView, type DocumentInfo, type ToolCall, type ToolOutcome, type WorkspaceInfo } from "../lib/api";
import { ChatPanel, type ChatPanelHandle } from "./ChatPanel";

interface DialogProps {
  openDocs: DocumentInfo[];
  editing?: WorkspaceInfo | null;
  onOpen(ws: WorkspaceInfo): void;
  onClose(): void;
}

/** Lista Przestrzeni roboczych oraz tworzenie/edycja z otwartych kart. */
export function WorkspaceDialog({ openDocs, editing, onOpen, onClose }: DialogProps) {
  const [list, setList] = useState<WorkspaceInfo[]>([]);
  const [form, setForm] = useState<{ id: number | null; name: string; hashes: string[] } | null>(
    editing ? { id: editing.id, name: editing.name, hashes: editing.documents.map((d) => d.hash) } : null,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.listWorkspaces().then(setList).catch((e) => setError(errorText(e)));
  }, []);

  const candidates = (() => {
    const map = new Map<string, string>();
    for (const d of openDocs) map.set(d.hash, d.name);
    for (const d of editing?.documents ?? []) if (!map.has(d.hash)) map.set(d.hash, d.name);
    return [...map.entries()];
  })();

  const save = async () => {
    if (!form) return;
    try {
      const ws = await api.saveWorkspace(form.id, form.name, form.hashes);
      onOpen(ws);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const remove = async (ws: WorkspaceInfo) => {
    if (!window.confirm(t("ws.deleteConfirm", { name: ws.name }))) return;
    await api.deleteWorkspace(ws.id);
    setList((l) => l.filter((x) => x.id !== ws.id));
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{t("ws.title")}</h2>
        {!form && (
          <>
            {list.length === 0 && <p className="hint">{t("ws.none")}</p>}
            <ul className="ws-list">
              {list.map((ws) => (
                <li key={ws.id}>
                  <button className="recent-open" onClick={() => onOpen(ws)}>
                    <span className="recent-name">{ws.name}</span>
                    <span className="recent-path">{ws.documents.map((d) => d.name).join(" · ")}</span>
                  </button>
                  <button className="icon-btn" title={t("ws.delete")} onClick={() => remove(ws)}>
                    🗑
                  </button>
                </li>
              ))}
            </ul>
            <div className="modal-footer">
              <button onClick={onClose}>{t("settings.close")}</button>
              <button className="primary" disabled={openDocs.length === 0} onClick={() => setForm({ id: null, name: "", hashes: openDocs.map((d) => d.hash) })}>
                {t("ws.new")}
              </button>
            </div>
            {openDocs.length === 0 && <p className="hint">{t("ws.needDocs")}</p>}
          </>
        )}
        {form && (
          <>
            <label>{t("ws.name")}</label>
            <input autoFocus value={form.name} placeholder={t("ws.namePlaceholder")} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            <label>{t("ws.docs")}</label>
            <div className="ws-docs">
              {candidates.map(([hash, name]) => (
                <label key={hash} className="check">
                  <input
                    type="checkbox"
                    checked={form.hashes.includes(hash)}
                    onChange={(e) => setForm({ ...form, hashes: e.target.checked ? [...form.hashes, hash] : form.hashes.filter((h) => h !== hash) })}
                  />
                  {name}
                </label>
              ))}
            </div>
            <p className="hint">{t("ws.docsHint")}</p>
            <div className="modal-footer">
              <button onClick={() => (editing ? onClose() : setForm(null))}>{t("common.cancel")}</button>
              <button className="primary" disabled={!form.name.trim() || form.hashes.length === 0} onClick={save}>
                {t("ws.save")}
              </button>
            </div>
          </>
        )}
        {error && <p className="banner error">{error}</p>}
      </div>
    </div>
  );
}

export interface WorkspaceViewHandle {
  focus(): void;
}

interface ViewProps {
  workspace: WorkspaceInfo;
  active: boolean;
  hasKey: boolean;
  openHashes: string[];
  onOpenSettings(): void;
  onCitation(c: CitationView): void;
  onTool(call: ToolCall): Promise<ToolOutcome>;
  onOpenDoc(doc: { hash: string; path: string | null }): void;
  onEdit(): void;
}

/** Karta Przestrzeni roboczej: lista Dokumentów i wspólne Rozmowy. */
export const WorkspaceView = forwardRef<WorkspaceViewHandle, ViewProps>(function WorkspaceView(p, ref) {
  const chatRef = useRef<ChatPanelHandle>(null);
  useImperativeHandle(ref, () => ({ focus: () => chatRef.current?.focus() }));
  return (
    <div className="workspace-view" style={{ display: p.active ? "flex" : "none" }}>
      <nav className="ws-sidebar">
        <h2>🗂 {p.workspace.name}</h2>
        <ul>
          {p.workspace.documents.map((d) => (
            <li key={d.hash}>
              <button className={`ws-doc ${p.openHashes.includes(d.hash) ? "open" : ""}`} disabled={!d.path} title={d.path ?? t("start.missing")} onClick={() => p.onOpenDoc(d)}>
                📄 {d.name}
              </button>
            </li>
          ))}
        </ul>
        <button className="link-btn" onClick={p.onEdit}>
          {t("ws.edit")}
        </button>
        <p className="hint">{t("ws.frozenHint")}</p>
      </nav>
      <div className="ws-chat">
        <ChatPanel
          key={p.workspace.id + ":" + p.workspace.updatedAt}
          ref={chatRef}
          scope={{ kind: "workspace", workspace: p.workspace }}
          indexed
          hasKey={p.hasKey}
          selection={null}
          onClearSelection={() => undefined}
          onOpenSettings={p.onOpenSettings}
          onCitation={p.onCitation}
          onTool={p.onTool}
        />
      </div>
    </div>
  );
});
