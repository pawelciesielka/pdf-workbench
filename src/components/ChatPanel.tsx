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
} from "../lib/api";
import { MessageBody, plainText } from "./MessageBody";

export interface ChatPanelHandle {
  focus(): void;
}

interface Props {
  doc: DocumentInfo;
  indexed: boolean;
  hasKey: boolean;
  selection: Selection | null;
  onClearSelection(): void;
  onOpenSettings(): void;
  onCitation(c: CitationView): void;
}

interface Streaming {
  blocks: Record<number, Segment>;
  model: string | null;
  fallback: string | null;
}

const QUICK_ACTIONS = [
  ["actions.summary", "actions.summary.prompt"],
  ["actions.dates", "actions.dates.prompt"],
  ["actions.parties", "actions.parties.prompt"],
  ["actions.obligations", "actions.obligations.prompt"],
] as const;

function formatTokens(n: number) {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)} tys.` : String(n);
}

export const ChatPanel = forwardRef<ChatPanelHandle, Props>(function ChatPanel(props, ref) {
  const { doc } = props;
  const [conversations, setConversations] = useState<Conversation[]>(doc.conversations);
  const previous = doc.previousConversations;
  const [activeId, setActiveId] = useState<number | null>(doc.conversations[0]?.id ?? null);
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

  useImperativeHandle(ref, () => ({ focus: () => inputRef.current?.focus() }), []);

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

  // Przewijanie do dołu, o ile użytkownik sam nie przewinął wyżej.
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

  const send = useCallback(
    async (text: string, action: string | null = null) => {
      if (!text.trim() || busy || readOnly) return;
      setError(null);
      let conv = active;
      try {
        if (!conv) {
          conv = await api.createConversation(doc.hash);
          setConversations((cs) => [conv!, ...cs]);
          setActiveId(conv.id);
        }
      } catch (e) {
        setError(errorText(e));
        return;
      }
      const convId = conv.id;
      const selection = props.selection;
      const actionLabel = action ? t(action as "actions.summary") : null;
      setPendingUser({ id: -1, role: "user", createdAt: Date.now(), display: { text, selection, action: actionLabel } });
      setInput("");
      props.onClearSelection();
      stickToBottom.current = true;
      setStreaming({ blocks: {}, model: null, fallback: null });

      const onEvent = (e: ChatEvent) => {
        switch (e.type) {
          case "Started":
            setStreaming((s) => s && { ...s, model: e.model });
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
              const c = { page: e.page, citedText: e.citedText };
              return { ...s, blocks: { ...s.blocks, [e.index]: { ...b, citations: [...b.citations, c] } } };
            });
            break;
          case "Fallback":
            setStreaming((s) => s && { ...s, fallback: e.toModel });
            break;
          case "Done":
          case "Error":
            if (e.type === "Error") {
              setError(e.message);
              setInput((cur) => cur || text);
            }
            if (e.type === "Done" && e.conversation) {
              const updated = e.conversation;
              setConversations((cs) => [updated, ...cs.filter((c) => c.id !== updated.id)]);
            }
            api
              .getMessages(convId)
              .then((m) => setMessages(m))
              .finally(() => {
                setPendingUser(null);
                setStreaming(null);
              });
            break;
        }
      };

      streamingConv.current = convId;
      api.sendMessage(convId, text, selection, actionLabel, onEvent).catch((e) => {
        setError(errorText(e));
        setPendingUser(null);
        setStreaming(null);
      });
    },
    [active, busy, readOnly, doc.hash, props],
  );

  const newConversation = () => {
    setActiveId(null);
    setMessages([]);
    inputRef.current?.focus();
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

  const streamingSegments = streaming
    ? Object.entries(streaming.blocks)
        .sort(([a], [b]) => Number(a) - Number(b))
        .map(([, s]) => s)
    : [];

  const totalTokens = active ? active.inputTokens + active.outputTokens + active.cacheReadTokens + active.cacheWriteTokens : 0;
  const tooLarge = doc.size > 22 * 1024 * 1024 || (doc.pageCount ?? 0) > 550;
  const shown = pendingUser ? [...messages, pendingUser] : messages;

  return (
    <aside className="chat">
      <header className="chat-header">
        <select
          className="conv-select"
          value={activeId ?? ""}
          onChange={(e) => setActiveId(e.target.value ? Number(e.target.value) : null)}
          disabled={busy}
        >
          <option value="">{t("chat.new")}</option>
          {conversations.map((c) => (
            <option key={c.id} value={c.id}>
              {c.title}
            </option>
          ))}
          {previous.length > 0 && (
            <optgroup label={t("chat.previous")}>
              {previous.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.title}
                </option>
              ))}
            </optgroup>
          )}
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
        {tooLarge && <div>{t("chat.modePages")}</div>}
        {tooLarge && !props.indexed && <div>{t("chat.indexing")}</div>}
      </div>

      <div className="chat-messages" ref={scrollRef} onScroll={onScroll}>
        {readOnly && <div className="banner">{t("chat.readOnly")}</div>}
        {!props.hasKey && (
          <div className="banner">
            {t("chat.noKey")}{" "}
            <button className="link-btn" onClick={props.onOpenSettings}>
              {t("chat.openSettings")}
            </button>
          </div>
        )}
        {shown.length === 0 && !streaming && props.hasKey && <div className="chat-empty">{t("chat.empty")}</div>}

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
              <MessageBody segments={m.display.segments} onCitation={props.onCitation} />
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
            {streamingSegments.length === 0 ? (
              <div className="typing">
                <span />
                <span />
                <span />
              </div>
            ) : (
              <MessageBody segments={streamingSegments} onCitation={props.onCitation} />
            )}
          </div>
        )}
        {error && <div className="banner error">{error}</div>}
      </div>

      {!readOnly && (
        <footer className="chat-input">
          <div className="quick-actions">
            {QUICK_ACTIONS.map(([label]) => (
              <button key={label} disabled={busy || !props.hasKey} onClick={() => send(t(`${label}.prompt` as "actions.summary.prompt"), label)}>
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
              placeholder={t("chat.placeholder")}
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
