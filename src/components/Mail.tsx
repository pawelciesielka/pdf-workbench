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

type Outcome = { mail: MailItem; draft?: DraftResult; error?: string };

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
      setMails(await api.gmailList());
      setPicked({});
    } catch (e) {
      setError(errorText(e));
      await refreshStatus();
    } finally {
      setBusy(null);
    }
  }, [refreshStatus]);

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
    const results: Outcome[] = [];
    setBusy(t("mail.drafting"));
    for (const { mail, files } of jobs) {
      const signed = files.map((f) => byPath.get(f));
      const failed = signed.filter((s) => !s?.output);
      if (failed.length) {
        results.push({ mail, error: t("mail.notAllSigned", { n: failed.length }) });
        continue;
      }
      try {
        results.push({ mail, draft: await api.gmailReplyDraft(mail.id, signed.map((s) => s!.output!), reply) });
      } catch (e) {
        results.push({ mail, error: errorText(e) });
      }
    }
    setBusy(null);
    setOutcomes(results);
  };

  if (props.hidden) return null;

  const hasOwn = !!status?.clientId && !!status?.hasSecret;
  const canLogin = !!status && (status.builtin || hasOwn);
  const loggedIn = !!status?.connected && !!status?.email;
  const showSetup = !!status && !loggedIn && (forceSetup || !canLogin);
  const showLogin = !!status && !loggedIn && !showSetup;

  return (
    <div className="modal-backdrop" onMouseDown={busy ? undefined : props.onClose}>
      <div className="modal mail-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>📨 {t("mail.title")}</h2>
        {error && <div className="banner error">{error}</div>}

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

            {mails && !mails.length && !busy && <p className="hint">{t("mail.empty")}</p>}
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
          <div className="sig-list">
            {outcomes.map((o) => (
              <div key={o.mail.id} className={`sig-item ${o.error ? "bad" : "ok"}`}>
                <div className="sig-head">
                  <span className="sig-icon">{o.error ? "✖" : "✔"}</span>
                  <b>{o.mail.subject || t("mail.noSubject")}</b>
                </div>
                {o.error && <small className="sig-status">{o.error}</small>}
                {o.draft && (
                  <>
                    <small>{t("mail.draftTo", { to: [...o.draft.to, ...o.draft.cc].join(", ") })}</small>
                    {o.draft.labelWarning && <small>{t("mail.labelFailed", { error: o.draft.labelWarning })}</small>}
                    <small>
                      <button className="link-btn" onClick={() => api.openUrl(o.draft!.url)}>
                        {t("mail.openDraft")}
                      </button>
                    </small>
                  </>
                )}
              </div>
            ))}
          </div>
        )}

        {busy && <p className="hint">⏳ {busy}</p>}

        <div className="modal-footer">
          <button disabled={!!busy} onClick={props.onClose}>
            {t("common.close")}
          </button>
          {outcomes ? (
            <button
              className="primary"
              onClick={() => {
                setOutcomes(null);
                void loadMails();
              }}
            >
              {t("mail.back")}
            </button>
          ) : (
            status?.connected && (
              <button className="primary" disabled={!!busy || !fileCount} onClick={signAndReply}>
                ✍ {t("mail.signReply", { mails: selected.length, files: fileCount })}
              </button>
            )
          )}
        </div>
      </div>
    </div>
  );
}
