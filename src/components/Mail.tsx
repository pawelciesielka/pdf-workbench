import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type BatchItem, type DraftResult, type GmailStatus, type MailItem } from "../lib/api";

const DEFAULT_REPLY = "Dzień dobry,\n\nw załączeniu przesyłam podpisane dokumenty.\n\nPozdrawiam";

const readReply = () => {
  try {
    return localStorage.getItem("mail.replyText") ?? DEFAULT_REPLY;
  } catch {
    return DEFAULT_REPLY;
  }
};

/** „Jan Kowalski <jan@x.pl>” → „Jan Kowalski”. */
const senderName = (from: string) => from.replace(/<[^>]*>/, "").replace(/"/g, "").trim() || from;
const when = (ms: number) => new Date(ms).toLocaleString("pl-PL", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
const kb = (n: number) => (n > 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} kB`);

type Outcome = {
  mail: MailItem;
  /** Szkic jeszcze powstaje. */
  pending?: boolean;
  draft?: DraftResult;
  error?: string;
  sending?: boolean;
  sent?: boolean;
  sendError?: string;
};

/**
 * Poczta do podpisu: maile z PDF-ami z Gmaila → pobranie załączników → podpis wielu plików naraz
 * → szkic „Odpowiedz wszystkim” z podpisanymi plikami w tym samym wątku.
 */
export function MailDialog(props: {
  hidden: boolean;
  onClose(): void;
  /** Podpisanie plików (okno podpisu wielu plików); null — anulowano. */
  sign(files: string[]): Promise<BatchItem[] | null>;
}) {
  const [status, setStatus] = useState<GmailStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [mails, setMails] = useState<MailItem[] | null>(null);
  /** id maila → zaznaczone części (załączniki). Brak klucza = mail niezaznaczony. */
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [reply, setReply] = useState(readReply);
  const [showOptions, setShowOptions] = useState(false);
  const [query, setQuery] = useState("");
  const [doneLabel, setDoneLabel] = useState("");
  const [manual, setManual] = useState({ id: "", secret: "" });
  const [outcomes, setOutcomes] = useState<Outcome[] | null>(null);
  /** Wczytanie innego klienta OAuth mimo zapisanego. */
  const [forceSetup, setForceSetup] = useState(false);
  /** Tylko maile od nadawców z domeny konta (zapamiętane do wyłączenia). */
  const [orgOnly, setOrgOnly] = useState(() => {
    try {
      return localStorage.getItem("mail.orgOnly") === "1";
    } catch {
      return false;
    }
  });
  const toggleOrgOnly = (on: boolean) => {
    setOrgOnly(on);
    try {
      localStorage.setItem("mail.orgOnly", on ? "1" : "0");
    } catch {
      /* brak localStorage */
    }
  };

  const refreshStatus = useCallback(async () => {
    const s = await api.gmailStatus();
    setStatus(s);
    setForceSetup(false);
    setQuery(s.query);
    setDoneLabel(s.doneLabel);
    return s;
  }, []);

  const loadMails = useCallback(async () => {
    setBusy(t("mail.loading"));
    setError(null);
    try {
      setMails(await api.gmailList(orgOnly));
      setPicked({});
    } catch (e) {
      setError(errorText(e));
      await refreshStatus();
    } finally {
      setBusy(null);
    }
  }, [refreshStatus, orgOnly]);

  // Także po przełączeniu filtra organizacji (zmienia się loadMails).
  useEffect(() => {
    refreshStatus()
      .then((s) => {
        if (s.connected && s.email) void loadMails();
      })
      .catch((e) => setError(errorText(e)));
  }, [refreshStatus, loadMails]);

  const run = async (label: string, f: () => Promise<unknown>) => {
    setBusy(label);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const loadClientFile = async () => {
    const path = await openDialog({ multiple: false, filters: [{ name: "JSON", extensions: ["json"] }] });
    if (typeof path === "string") await run(t("mail.saving"), async () => (await api.gmailLoadClientFile(path), refreshStatus()));
  };

  const connect = () =>
    run(t("mail.waitingBrowser"), async () => {
      await api.gmailConnect();
      await refreshStatus();
      await loadMails();
    });

  const disconnect = () =>
    run(t("mail.saving"), async () => {
      await api.gmailDisconnect();
      setMails(null);
      await refreshStatus();
    });

  const saveOptions = () =>
    run(t("mail.saving"), async () => {
      await api.gmailSetOptions(query, doneLabel);
      try {
        localStorage.setItem("mail.replyText", reply);
      } catch {
        /* brak localStorage */
      }
      setShowOptions(false);
      await refreshStatus();
      await loadMails();
    });

  const toggleMail = (m: MailItem) =>
    setPicked((p) => {
      const { [m.id]: cur, ...rest } = p;
      return cur ? rest : { ...p, [m.id]: m.attachments.map((a) => a.partId) };
    });

  const toggleAttachment = (m: MailItem, partId: string) =>
    setPicked((p) => {
      const cur = p[m.id] ?? [];
      const next = cur.includes(partId) ? cur.filter((x) => x !== partId) : [...cur, partId];
      const { [m.id]: _, ...rest } = p;
      return next.length ? { ...rest, [m.id]: next } : rest;
    });

  const selected = (mails ?? []).filter((m) => picked[m.id]?.length);
  const fileCount = selected.reduce((n, m) => n + picked[m.id].length, 0);

  const signAndReply = async () => {
    try {
      localStorage.setItem("mail.replyText", reply);
    } catch {
      /* brak localStorage */
    }
    setError(null);
    const jobs: { mail: MailItem; files: string[] }[] = [];
    setBusy(t("mail.downloading"));
    try {
      for (const mail of selected) jobs.push({ mail, files: await api.gmailDownload(mail.id, picked[mail.id]) });
    } catch (e) {
      setError(errorText(e));
      setBusy(null);
      return;
    }
    setBusy(null);
    const all = jobs.flatMap((j) => j.files);
    if (!all.length) return setError(t("mail.noFiles"));
    const items = await props.sign(all);
    if (!items) return;
    const byPath = new Map(items.map((i) => [i.path, i]));
    // Od razu widok wyników (bez powrotu do listy maili), szkice dochodzą po kolei.
    setOutcomes(jobs.map(({ mail }) => ({ mail, pending: true })));
    setBusy(t("mail.drafting"));
    for (const [i, { mail, files }] of jobs.entries()) {
      const signed = files.map((f) => byPath.get(f));
      const failed = signed.filter((s) => !s?.output);
      let result: Outcome;
      if (failed.length) result = { mail, error: t("mail.notAllSigned", { n: failed.length }) };
      else {
        try {
          result = { mail, draft: await api.gmailReplyDraft(mail.id, signed.map((s) => s!.output!), reply) };
        } catch (e) {
          result = { mail, error: errorText(e) };
        }
      }
      updateOutcome(i, () => result);
    }
    setBusy(null);
  };

  const updateOutcome = (i: number, f: (o: Outcome) => Outcome) => setOutcomes((cur) => (cur ? cur.map((o, k) => (k === i ? f(o) : o)) : cur));

  const send = async (i: number, draftId: string) => {
    updateOutcome(i, (o) => ({ ...o, sending: true, sendError: undefined }));
    try {
      await api.gmailSendDraft(draftId);
      updateOutcome(i, (o) => ({ ...o, sending: false, sent: true }));
    } catch (e) {
      updateOutcome(i, (o) => ({ ...o, sending: false, sendError: errorText(e) }));
    }
  };

  const unsent = (outcomes ?? []).map((o, i) => ({ o, i })).filter(({ o }) => o.draft && !o.sent && !o.sending);
  const sendAll = async () => {
    for (const { o, i } of unsent) await send(i, o.draft!.draftId);
  };

  if (props.hidden) return null;

  const domain = status?.email?.split("@")[1]?.toLowerCase() ?? null;
  const hasOwn = !!status?.clientId && !!status?.hasSecret;
  const canLogin = !!status && (status.builtin || hasOwn);
  const loggedIn = !!status?.connected && !!status?.email;
  const showSetup = !!status && !loggedIn && (forceSetup || !canLogin);
  const showLogin = !!status && !loggedIn && !showSetup;

  return (
    <div className="modal-backdrop" onMouseDown={busy ? undefined : props.onClose}>
      <div className="modal mail-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="mail-titlebar">
          <h2>📨 {t("mail.title")}</h2>
          <button disabled={!!busy} onClick={props.onClose}>
            {t("common.close")}
          </button>
        </div>
        {error &&<div className="banner error">{error}</div>}

        {!status && <p className="hint">{t("mail.loading")}</p>}

        {showLogin && (
          <div className="mail-setup mail-login">
            <p>{t("mail.loginIntro")}</p>
            <button className="primary big google-btn" disabled={!!busy} onClick={connect}>
              <b>G</b> {t("mail.connect")}
            </button>
            <p className="hint">{t("mail.loginHint")}</p>
            {hasOwn && status!.builtin ? (
              <p className="hint">
                {t("mail.ownClientUsed")}{" "}
                <button className="link-btn" disabled={!!busy} onClick={() => run(t("mail.saving"), async () => (await api.gmailClearClient(), refreshStatus()))}>
                  {t("mail.useBuiltin")}
                </button>
              </p>
            ) : (
              <button className="link-btn advanced" onClick={() => setForceSetup(true)}>
                {t("mail.advanced")}
              </button>
            )}
          </div>
        )}

        {showSetup && (
          <div className="mail-setup">
            {!status!.builtin && <p className="hint">{t("mail.noBuiltin")}</p>}
            <p>{t("mail.setupIntro")}</p>
            <ol>
              <li>
                {t("mail.setup1")}{" "}
                <button className="link-btn" onClick={() => api.openUrl("https://console.cloud.google.com/apis/library/gmail.googleapis.com")}>
                  {t("mail.setup1Link")}
                </button>
              </li>
              <li>{t("mail.setup2")}</li>
              <li>
                {t("mail.setup3")}{" "}
                <button className="link-btn" onClick={() => api.openUrl("https://console.cloud.google.com/apis/credentials")}>
                  {t("mail.setup3Link")}
                </button>
              </li>
              <li>{t("mail.setup4")}</li>
            </ol>
            <button className="primary" disabled={!!busy} onClick={loadClientFile}>
              {t("mail.loadJson")}
            </button>
            <details>
              <summary>{t("mail.manual")}</summary>
              <label>Client ID</label>
              <input value={manual.id} placeholder="….apps.googleusercontent.com" onChange={(e) => setManual({ ...manual, id: e.target.value })} />
              <label>Client secret</label>
              <input type="password" value={manual.secret} onChange={(e) => setManual({ ...manual, secret: e.target.value })} />
              <button
                disabled={!manual.id.trim() || !manual.secret.trim() || !!busy}
                onClick={() => run(t("mail.saving"), async () => (await api.gmailSetClient(manual.id, manual.secret), refreshStatus()))}
              >
                {t("settings.save")}
              </button>
            </details>
            {canLogin && (
              <button className="link-btn advanced" onClick={() => setForceSetup(false)}>
                ← {t("mail.backToLogin")}
              </button>
            )}
          </div>
        )}

        {status?.connected && status.email && !outcomes && (
          <>
            <div className="mail-account">
              <span>
                {t("mail.account")} <b>{status.email}</b>
              </span>
              <span className="mail-actions">
                <button className="link-btn" disabled={!!busy} onClick={loadMails}>
                  ↻ {t("mail.refresh")}
                </button>
                <button className="link-btn" onClick={() => setShowOptions((v) => !v)}>
                  ⚙ {t("mail.options")}
                </button>
                <button className="link-btn" disabled={!!busy} onClick={disconnect}>
                  {t("mail.disconnect")}
                </button>
              </span>
            </div>

            {showOptions && (
              <div className="mail-options">
                <label>{t("mail.query")}</label>
                <input value={query} onChange={(e) => setQuery(e.target.value)} />
                <p className="hint">{t("mail.queryHint")}</p>
                <label>{t("mail.doneLabel")}</label>
                <input value={doneLabel} onChange={(e) => setDoneLabel(e.target.value)} />
                <p className="hint">{t("mail.doneLabelHint")}</p>
                <label>{t("mail.replyText")}</label>
                <textarea rows={5} value={reply} onChange={(e) => setReply(e.target.value)} />
                <p className="hint">{t("mail.folderHint", { folder: status.folder })}</p>
                <div className="row">
                  <button className="primary" disabled={!!busy} onClick={saveOptions}>
                    {t("settings.save")}
                  </button>
                </div>
              </div>
            )}

            <div className="mail-toolbar">
              <label className="check" title={t("mail.orgOnlyHint")}>
                <input type="checkbox" checked={orgOnly} disabled={!!busy} onChange={(e) => toggleOrgOnly(e.target.checked)} />{" "}
                {t("mail.orgOnly", { domain: domain ?? "" })}
              </label>
              <button className="primary" disabled={!!busy || !fileCount} onClick={signAndReply}>
                ✍ {t("mail.signReply", { mails: selected.length, files: fileCount })}
              </button>
            </div>

            {mails && !mails.length && !busy && <p className="hint">{orgOnly ? t("mail.emptyOrg", { domain: domain ?? "" }) : t("mail.empty")}</p>}
            {mails && mails.length > 0 && (
              <div className="mail-list">
                {mails.map((m) => {
                  const sel = picked[m.id] ?? [];
                  return (
                    <div key={m.id} className={`mail-item ${sel.length ? "on" : ""} ${m.unread ? "unread" : ""}`}>
                      <label className="mail-head">
                        <input type="checkbox" checked={sel.length > 0} onChange={() => toggleMail(m)} />
                        <span className="mail-from" title={m.from}>
                          {senderName(m.from)}
                        </span>
                        <span className="mail-date">{when(m.date)}</span>
                      </label>
                      <div className="mail-subject">{m.subject || t("mail.noSubject")}</div>
                      <div className="mail-snippet">{m.snippet}</div>
                      <div className="mail-files">
                        {m.attachments.map((a) => (
                          <label key={a.partId} className="mail-file">
                            <input type="checkbox" checked={sel.includes(a.partId)} onChange={() => toggleAttachment(m, a.partId)} />📄 {a.filename}
                            <small>{kb(a.size)}</small>
                          </label>
                        ))}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
            <p className="hint">{t("mail.draftHint")}</p>
          </>
        )}

        {outcomes && (
          <div className="mail-toolbar">
            <button
              disabled={!!busy || outcomes.some((o) => o.sending)}
              onClick={() => {
                setOutcomes(null);
                void loadMails();
              }}
            >
              ← {t("mail.back")}
            </button>
            {unsent.length > 1 && (
              <button className="primary" disabled={!!busy} onClick={sendAll}>
                ➤ {t("mail.sendAll", { n: unsent.length })}
              </button>
            )}
          </div>
        )}
        {outcomes && (
          <div className="sig-list">
            {outcomes.map((o, i) => (
              <div key={o.mail.id} className={`sig-item ${o.error || o.sendError ? "bad" : o.pending ? "" : "ok"}`}>
                <div className="sig-head">
                  <span className="sig-icon">{o.pending || o.sending ? "…" : o.error ? "✖" : o.sent ? "✔" : "✎"}</span>
                  <b>{o.mail.subject || t("mail.noSubject")}</b>
                </div>
                {o.pending && <small>{t("mail.drafting")}</small>}
                {o.error && <small className="sig-status">{o.error}</small>}
                {o.draft && (
                  <>
                    <small className="mail-rcpt">
                      <b>{t("mail.to")}</b> {o.draft.to.join(", ")}
                    </small>
                    {o.draft.cc.length > 0 && (
                      <small className="mail-rcpt">
                        <b>{t("mail.cc")}</b> {o.draft.cc.join(", ")}
                      </small>
                    )}
                    {o.draft.labelWarning && <small>{t("mail.labelFailed", { error: o.draft.labelWarning })}</small>}
                    {o.sendError && <small className="sig-status">{t("mail.sendFailed", { error: o.sendError })}</small>}
                    <div className="mail-send-row">
                      {o.sent ? (
                        <small className="trust ok">✔ {t("mail.sent")}</small>
                      ) : (
                        <>
                          <button className="primary" disabled={o.sending} onClick={() => send(i, o.draft!.draftId)}>
                            {o.sending ? t("mail.sending") : `➤ ${t("mail.send")}`}
                          </button>
                          <button className="link-btn" disabled={o.sending} onClick={() => api.openUrl(o.draft!.url)}>
                            {t("mail.openDraft")}
                          </button>
                        </>
                      )}
                    </div>
                  </>
                )}
              </div>
            ))}
          </div>
        )}

        {busy && <p className="hint">⏳ {busy}</p>}
      </div>
    </div>
  );
}
