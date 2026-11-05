import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { MODEL_LABELS, t } from "../i18n";
import {
  api,
  errorText,
  type ChatEvent,
  type CitationView,
  type Conversation,
  type DocumentInfo,
  type MessageView,
  type Segment,
  type Selection,
  type ToolCall,
  type ToolOutcome,
  type ToolView,
  type WorkspaceInfo,
} from "../lib/api";
import { MessageBody, plainText } from "./MessageBody";

export interface ChatPanelHandle {
  focus(): void;
  sendText(text: string, actionLabel?: string): void;
}

export type ChatScope = { kind: "document"; doc: DocumentInfo } | { kind: "workspace"; workspace: WorkspaceInfo };

interface Props {
  scope: ChatScope;
  indexed: boolean;
  hasKey: boolean;
  selection: Selection | null;
  onClearSelection(): void;
  onOpenSettings(): void;
  onCitation(c: CitationView): void;
  onTool(call: ToolCall): Promise<ToolOutcome>;
}

interface LiveTool {
  call: ToolCall;
  status: "running" | "proposal" | "done";
  ok?: boolean;
  result?: string;
}

interface Streaming {
  turns: Segment[][];
  blocks: Record<number, Segment>;
  model: string | null;
  fallback: string | null;
  tools: LiveTool[];
}

/** Zmiany wymagające „Zastosuj” użytkownika (Propozycje zmiany). */
const CONFIRM_TOOLS = new Set(["delete_pages", "move_pages", "redact_text", "replace_text"]);

const QUICK_ACTIONS = [
  ["actions.summary", "actions.summary.prompt"],
  ["actions.dates", "actions.dates.prompt"],
  ["actions.parties", "actions.parties.prompt"],
  ["actions.obligations", "actions.obligations.prompt"],
] as const;

function formatTokens(n: number) {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)} tys.` : String(n);
}

const list = (v: unknown) => (Array.isArray(v) ? v.join(", ") : String(v ?? ""));

export function describeTool(name: string, input: Record<string, unknown>): string {
  const i = input as Record<string, string | number | number[]>;
  switch (name) {
    case "highlight_text":
      return t("tool.highlight", { page: String(i.page), text: String(i.text).slice(0, 80) });
    case "go_to_page":
      return t("tool.goto", { page: String(i.page) });
    case "rotate_pages":
      return t("tool.rotate", { pages: list(i.pages), deg: String(i.degrees) });
    case "delete_pages":
      return t("tool.delete", { pages: list(i.pages) });
    case "move_pages":
      return t("tool.move", { pages: list(i.pages), after: String(i.after_page) });
    case "redact_text":
      return t("tool.redact", { text: String(i.text).slice(0, 80) });
    case "replace_text":
      return t("tool.replace", { text: String(i.text).slice(0, 80), replacement: String(i.replacement).slice(0, 80) });
    default:
      return name;
  }
}

function segmentsOf(blocks: Record<number, Segment>) {
  return Object.entries(blocks)
    .sort(([a], [b]) => Number(a) - Number(b))
    .map(([, s]) => s);
}

export const ChatPanel = forwardRef<ChatPanelHandle, Props>(function ChatPanel(props, ref) {
  const { scope } = props;
  const isWorkspace = scope.kind === "workspace";
  const initial = scope.kind === "document" ? scope.doc.conversations : scope.workspace.conversations;
  const previous = scope.kind === "document" ? scope.doc.previousConversations : [];
  const [conversations, setConversations] = useState<Conversation[]>(initial);
  const [activeId, setActiveId] = useState<number | null>(initial[0]?.id ?? null);
  const [messages, setMessages] = useState<MessageView[]>([]);
  const [pendingUser, setPendingUser] = useState<MessageView | null>(null);
  const [streaming, setStreaming] = useState<Streaming | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<number | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const streamingConv = useRef<number | null>(null);
  const resolvers = useRef(new Map<string, (o: ToolOutcome) => void>());
  const propsRef = useRef(props);
  propsRef.current = props;

  const active = useMemo(
    () => conversations.find((c) => c.id === activeId) ?? previous.find((c) => c.id === activeId) ?? null,
    [conversations, previous, activeId],
  );

  useEffect(() => {
    setMessages([]);
    setError(null);
    if (activeId == null) return;
    api.getMessages(activeId).then(setMessages).catch((e) => setError(errorText(e)));
  }, [activeId]);

  useEffect(() => {
    if (props.selection) inputRef.current?.focus();
  }, [props.selection]);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages, streaming, pendingUser]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (el) stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  };

  const busy = streaming !== null;
  const readOnly = active?.readOnly ?? false;

  const runTool = useCallback(async (call: ToolCall) => {
    const confirm = CONFIRM_TOOLS.has(call.name);
    setStreaming((s) => s && { ...s, tools: [...s.tools, { call, status: confirm ? "proposal" : "running" }] });
    let outcome: ToolOutcome;
    if (confirm) {
      outcome = await new Promise<ToolOutcome>((resolve) => resolvers.current.set(call.id, resolve));
    } else {
      outcome = await propsRef.current.onTool(call).catch((e) => ({ ok: false, text: errorText(e) }));
    }
    setStreaming((s) => s && { ...s, tools: s.tools.map((x) => (x.call.id === call.id ? { ...x, status: "done", ok: outcome.ok, result: outcome.text } : x)) });
    await api.toolResult(call.id, outcome);
  }, []);

  const decide = async (call: ToolCall, accept: boolean) => {
    const resolve = resolvers.current.get(call.id);
    if (!resolve) return;
    resolvers.current.delete(call.id);
    if (!accept) return resolve({ ok: false, text: "Użytkownik odrzucił tę Propozycję zmiany." });
    setStreaming((s) => s && { ...s, tools: s.tools.map((x) => (x.call.id === call.id ? { ...x, status: "running" } : x)) });
    resolve(await propsRef.current.onTool(call).catch((e) => ({ ok: false, text: errorText(e) })));
  };

  const send = useCallback(
    async (text: string, actionLabel: string | null = null) => {
      if (!text.trim() || busy || readOnly) return;
      setError(null);
      let conv = active;
      try {
        if (!conv) {
          conv = await api.createConversation(scope.kind === "document" ? { hash: scope.doc.hash } : { workspaceId: scope.workspace.id });
          const created = conv;
          setConversations((cs) => [created, ...cs]);
          setActiveId(created.id);
        }
      } catch (e) {
        setError(errorText(e));
        return;
      }
      const convId = conv.id;
      const selection = props.selection;
      setPendingUser({ id: -1, role: "user", createdAt: Date.now(), display: { text, selection, action: actionLabel } });
      setInput("");
      props.onClearSelection();
      stickToBottom.current = true;
      setStreaming({ turns: [], blocks: {}, model: null, fallback: null, tools: [] });
      streamingConv.current = convId;

      const onEvent = (e: ChatEvent) => {
        switch (e.type) {
          case "Started":
            setStreaming((s) => {
              if (!s) return s;
              const done = segmentsOf(s.blocks);
              return { ...s, model: e.model, turns: done.length ? [...s.turns, done] : s.turns, blocks: {} };
            });
            break;
          case "BlockStart":
            setStreaming((s) => s && { ...s, blocks: { ...s.blocks, [e.index]: { text: "", citations: [] } } });
            break;
          case "TextDelta":
            setStreaming((s) => {
              if (!s) return s;
              const b = s.blocks[e.index] ?? { text: "", citations: [] };
              return { ...s, blocks: { ...s.blocks, [e.index]: { ...b, text: b.text + e.text } } };
            });
            break;
          case "Citation":
            setStreaming((s) => {
              if (!s) return s;
              const b = s.blocks[e.index] ?? { text: "", citations: [] };
              return { ...s, blocks: { ...s.blocks, [e.index]: { ...b, citations: [...b.citations, e.citation] } } };
            });
            break;
          case "Fallback":
            setStreaming((s) => s && { ...s, fallback: e.toModel });
            break;
          case "ToolCall":
            runTool({ id: e.id, name: e.name, input: e.input });
            break;
          case "Done":
          case "Error":
            if (e.type === "Error") {
              setError(e.message);
              setInput((cur) => cur || (actionLabel ? "" : text));
            }
            if (e.type === "Done" && e.conversation) {
              const updated = e.conversation;
              setConversations((cs) => [updated, ...cs.filter((c) => c.id !== updated.id)]);
            }
            for (const r of resolvers.current.values()) r({ ok: false, text: "Rozmowa zakończona." });
            resolvers.current.clear();
            api
              .getMessages(convId)
              .then(setMessages)
              .finally(() => {
                setPendingUser(null);
                setStreaming(null);
              });
            break;
        }
      };

      api.sendMessage(convId, text, selection, actionLabel, onEvent).catch((e) => {
        setError(errorText(e));
        setPendingUser(null);
        setStreaming(null);
      });
    },
    [active, busy, readOnly, scope, props, runTool],
  );

  useImperativeHandle(ref, () => ({ focus: () => inputRef.current?.focus(), sendText: (text, label) => send(text, label ?? null) }), [send]);

  const newConversation = () => {
    setActiveId(null);
    setMessages([]);
    inputRef.current?.focus();
  };

  const continueHere = async () => {
    if (!active || scope.kind !== "document") return;
    try {
      const c = await api.continueConversation(active.id, scope.doc.hash);
      setConversations((cs) => [c, ...cs]);
      setActiveId(c.id);
      inputRef.current?.focus();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const deleteConversation = async () => {
    if (!active || active.readOnly) return;
    if (!window.confirm(t("chat.deleteConfirm", { title: active.title }))) return;
    await api.deleteConversation(active.id);
    setConversations((cs) => cs.filter((c) => c.id !== active.id));
    setActiveId(null);
  };

  const copy = (m: MessageView) => {
    if (m.role !== "assistant") return;
    navigator.clipboard.writeText(plainText(m.display.segments));
    setCopied(m.id);
    setTimeout(() => setCopied(null), 1500);
  };

  const prevGroups = useMemo(() => {
    const groups = new Map<string, Conversation[]>();
    for (const c of previous) {
      const label = c.replacedByEditAt
        ? t("chat.previousEdit", { date: new Date(c.replacedByEditAt).toLocaleString("pl-PL", { dateStyle: "short", timeStyle: "short" }) })
        : t("chat.previous");
      groups.set(label, [...(groups.get(label) ?? []), c]);
    }
    return [...groups.entries()];
  }, [previous]);

  const totalTokens = active ? active.inputTokens + active.outputTokens + active.cacheReadTokens + active.cacheWriteTokens : 0;
  const docs = scope.kind === "document" ? [scope.doc] : [];
  const tooLarge = docs.some((d) => d.size > 22 * 1024 * 1024 || (d.pageCount ?? 0) > 550);
  const shown = pendingUser ? [...messages, pendingUser] : messages;

  const renderTools = (tools: ToolView[] | LiveTool[]) =>
    tools.length > 0 && (
      <div className="tool-list">
        {tools.map((tv, i) => {
          const live = "call" in tv ? tv : null;
          const name = live ? live.call.name : (tv as ToolView).name;
          const input = live ? live.call.input : (tv as ToolView).input;
          const status = live ? live.status : "done";
          const ok = live ? live.ok : (tv as ToolView).ok;
          const result = live ? live.result : (tv as ToolView).result;
          return (
            <div key={i} className={`tool ${status} ${status === "done" ? (ok ? "ok" : "fail") : ""}`}>
              <div className="tool-title">
                <span>{status === "running" ? "⏳" : status === "proposal" ? "❓" : ok ? "✓" : "✗"}</span>
                {status === "proposal" && <strong>{t("chat.proposal")}:</strong>} {describeTool(name, input)}
              </div>
              {status === "proposal" && live && (
                <div className="tool-actions">
                  <button className="primary" onClick={() => decide(live.call, true)}>
                    {t("chat.apply")}
                  </button>
                  <button onClick={() => decide(live.call, false)}>{t("chat.reject")}</button>
                </div>
              )}
              {status === "done" && !ok && result && <div className="tool-result">{result}</div>}
            </div>
          );
        })}
      </div>
    );

  return (
    <aside className="chat">
      <header className="chat-header">
        <select className="conv-select" value={activeId ?? ""} onChange={(e) => setActiveId(e.target.value ? Number(e.target.value) : null)} disabled={busy}>
          <option value="">{t("chat.new")}</option>
          {conversations.map((c) => (
            <option key={c.id} value={c.id}>
              {c.title}
            </option>
          ))}
          {prevGroups.map(([label, cs]) => (
            <optgroup key={label} label={label}>
              {cs.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.title}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
        <button className="icon-btn" onClick={newConversation} title={t("chat.new")} disabled={busy}>
          ＋
        </button>
        {active && !active.readOnly && (
          <button className="icon-btn" onClick={deleteConversation} title={t("chat.delete")} disabled={busy}>
            🗑
          </button>
        )}
      </header>

      <div className="chat-notice">
        <span className="dot" /> {t("chat.sentToAi")}
        {isWorkspace && <div>{t("ws.chatHint", { n: scope.workspace.documents.length })}</div>}
        {tooLarge && <div>{t("chat.modePages")}</div>}
        {tooLarge && !props.indexed && <div>{t("chat.indexing")}</div>}
      </div>

      <div className="chat-messages" ref={scrollRef} onScroll={onScroll}>
        {readOnly && (
          <div className="banner">
            {t("chat.readOnly")}{" "}
            <button className="link-btn" onClick={continueHere}>
              {t("chat.continue")}
            </button>
          </div>
        )}
        {!props.hasKey && (
          <div className="banner">
            {t("chat.noKey")}{" "}
            <button className="link-btn" onClick={props.onOpenSettings}>
              {t("chat.openSettings")}
            </button>
          </div>
        )}
        {shown.length === 0 && !streaming && props.hasKey && <div className="chat-empty">{isWorkspace ? t("ws.empty") : t("chat.empty")}</div>}

        {shown.map((m) =>
          m.role === "user" ? (
            <div key={m.id} className="msg user">
              {m.display.selection && (
                <blockquote className="sel-quote">
                  <small>{t("chat.selection", { page: m.display.selection.page })}</small>
                  {m.display.selection.text}
                </blockquote>
              )}
              <div>{m.display.action ? <strong>⚡ {m.display.action}</strong> : m.display.text}</div>
            </div>
          ) : (
            <div key={m.id} className="msg assistant">
              <MessageBody segments={m.display.segments} onCitation={props.onCitation} showDocNames={isWorkspace} />
              {renderTools(m.display.tools ?? [])}
              {m.display.stopReason === "refusal" && <div className="msg-note">{t("chat.refusal")}</div>}
              {m.display.stopReason === "cancelled" && <div className="msg-note">{t("chat.cancelled")}</div>}
              {m.display.stopReason === "max_tokens" && <div className="msg-note">{t("chat.truncated")}</div>}
              <div className="msg-tools">
                <span>{MODEL_LABELS[m.display.model] ?? m.display.model}</span>
                <button className="link-btn" onClick={() => copy(m)}>
                  {copied === m.id ? t("chat.copied") : t("chat.copy")}
                </button>
              </div>
            </div>
          ),
        )}

        {streaming && (
          <div className="msg assistant">
            {streaming.fallback && <div className="msg-note">{t("chat.fallback", { model: streaming.fallback })}</div>}
            {streaming.turns.map((segs, i) => (
              <MessageBody key={i} segments={segs} onCitation={props.onCitation} showDocNames={isWorkspace} />
            ))}
            {renderTools(streaming.tools)}
            {segmentsOf(streaming.blocks).length === 0 ? (
              streaming.tools.every((x) => x.status === "done") && (
                <div className="typing">
                  <span />
                  <span />
                  <span />
                </div>
              )
            ) : (
              <MessageBody segments={segmentsOf(streaming.blocks)} onCitation={props.onCitation} showDocNames={isWorkspace} />
            )}
          </div>
        )}
        {error && <div className="banner error">{error}</div>}
      </div>

      {!readOnly && (
        <footer className="chat-input">
          <div className="quick-actions">
            {QUICK_ACTIONS.map(([label, prompt]) => (
              <button key={label} disabled={busy || !props.hasKey} onClick={() => send(t(prompt), t(label))}>
                {t(label)}
              </button>
            ))}
          </div>
          {props.selection && (
            <div className="sel-chip">
              <span>
                <small>{t("chat.selection", { page: props.selection.page })}</small>
                {props.selection.text.length > 160 ? props.selection.text.slice(0, 160) + "…" : props.selection.text}
              </span>
              <button className="icon-btn" title={t("chat.removeSelection")} onClick={props.onClearSelection}>
                ✕
              </button>
            </div>
          )}
          <div className="input-row">
            <textarea
              ref={inputRef}
              value={input}
              rows={2}
              placeholder={isWorkspace ? t("ws.placeholder") : t("chat.placeholder")}
              disabled={!props.hasKey}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send(input);
                }
              }}
            />
            {busy ? (
              <button className="primary" onClick={() => streamingConv.current != null && api.stopGeneration(streamingConv.current)}>
                {t("chat.stop")}
              </button>
            ) : (
              <button className="primary" disabled={!input.trim() || !props.hasKey} onClick={() => send(input)}>
                {t("chat.send")}
              </button>
            )}
          </div>
          {active && totalTokens > 0 && (
            <div className="usage">
              {MODEL_LABELS[active.model] ?? active.model} · {t("chat.usage", { tokens: formatTokens(totalTokens), cost: active.costUsd.toFixed(3) })}
            </div>
          )}
        </footer>
      )}
    </aside>
  );
});
