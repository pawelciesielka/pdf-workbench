import { useEffect, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type BatchItem, type Placement, type SignatureInfo, type SigningCert, type TrustInfo } from "../lib/api";

/** Data w formacie PDF: D:YYYYMMDDHHmmSS+HH'mm' (czas lokalny ze strefą). */
export function pdfDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const off = -d.getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  const a = Math.abs(off);
  return `D:${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}${sign}${p(Math.floor(a / 60))}'${p(a % 60)}'`;
}

/** Data do widocznego podpisu: 2026-11-06 14:32:05 +01:00. */
export function stampDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const off = -d.getTimezoneOffset();
  const a = Math.abs(off);
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())} ${off >= 0 ? "+" : "-"}${p(Math.floor(a / 60))}:${p(a % 60)}`;
}

/** „D:20261106143205+01'00'” → „06.11.2026 14:32”. */
export function formatPdfDate(s: string | null): string | null {
  const m = s?.match(/^D?:?(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?/);
  if (!m) return s;
  const [, y, mo = "01", d = "01", h, mi] = m;
  return `${d}.${mo}.${y}${h ? ` ${h}:${mi ?? "00"}` : ""}`;
}

const day = (ms: number) => new Date(ms).toLocaleDateString("pl-PL");
const dateTime = (ms: number) => new Date(ms).toLocaleString("pl-PL");

/** Krótki opis ważnych podpisów do paska nad dokumentem. */
export function signersSummary(sigs: SignatureInfo[], trust?: TrustInfo[] | null): string {
  const indexed = sigs.map((s, i) => ({ s, tr: trust?.[i] }));
  const valid = indexed.filter((x) => x.s.intact && x.s.kind === "signature");
  const list = valid.length ? valid : indexed.filter((x) => x.s.intact);
  if (!list.length) return "";
  const first = list[0];
  const who = [first.s.signer ?? t("sign.unknownSigner"), formatPdfDate(first.s.time)].filter(Boolean).join(", ");
  const badge = first.tr?.euQualified && first.tr.revocation !== "revoked" ? ` ✔ ${t("sign.trustEuShort")}` : "";
  return (list.length > 1 ? `${who} +${list.length - 1}` : who) + badge;
}

/** Wiersze widocznego podpisu. */
export function visibleLines(cert: SigningCert, reason: string, now: Date): string[] {
  const lines = [t("sign.visibleBy"), cert.name, t("sign.visibleDate", { date: stampDate(now) })];
  if (reason) lines.push(t("sign.visibleReason", { reason }));
  return lines;
}

export interface SignChoice {
  cert: SigningCert;
  visible: boolean;
  timestamp: boolean;
  ltv: boolean;
  reason: string;
  location: string;
}

const stored = (key: string, fallback: boolean) => {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v !== "0";
  } catch {
    return fallback;
  }
};

export function SignDialog(props: { files?: string[]; onCancel(): void; onSubmit(choice: SignChoice): void }) {
  const batch = props.files;
  const [certs, setCerts] = useState<SigningCert[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [certId, setCertId] = useState<string | null>(null);
  const [visible, setVisible] = useState(() => stored("sign.visible", true));
  const [timestamp, setTimestamp] = useState(() => stored("sign.timestamp", true));
  const [ltv, setLtv] = useState(() => stored("sign.ltv", true));
  const [reason, setReason] = useState("");
  const [location, setLocation] = useState(() => {
    try {
      return localStorage.getItem("sign.location") ?? "";
    } catch {
      return "";
    }
  });

  const load = () => {
    setCerts(null);
    setError(null);
    api
      .signingCerts()
      .then((list) => {
        setCerts(list);
        const now = Date.now();
        const saved = localStorage.getItem("sign.cert");
        const usable = list.filter((c) => c.validTo > now && c.validFrom <= now);
        setCertId((usable.find((c) => c.id === saved) ?? usable[0])?.id ?? null);
      })
      .catch((e) => setError(errorText(e)));
  };
  useEffect(load, []);

  const cert = certs?.find((c) => c.id === certId) ?? null;
  const submit = () => {
    if (!cert) return;
    try {
      localStorage.setItem("sign.cert", cert.id);
      localStorage.setItem("sign.visible", visible ? "1" : "0");
      localStorage.setItem("sign.timestamp", timestamp ? "1" : "0");
      localStorage.setItem("sign.ltv", ltv ? "1" : "0");
      localStorage.setItem("sign.location", location);
    } catch {
      /* brak dostępu do localStorage */
    }
    props.onSubmit({ cert, visible, timestamp, ltv: timestamp && ltv, reason: reason.trim(), location: location.trim() });
  };

  const now = Date.now();
  return (
    <div className="modal-backdrop" onMouseDown={props.onCancel}>
      <div className="modal sign-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>✍ {batch ? t("sign.batchTitle", { n: batch.length }) : t("sign.title")}</h2>
        <p className="hint">{t("sign.hint")}</p>
        {batch && (
          <div className="batch-files">
            {batch.map((f) => (
              <div key={f} title={f}>
                📄 {f.split(/[\\/]/).pop()}
              </div>
            ))}
          </div>
        )}
        <label>{t("sign.cert")}</label>
        {error && <div className="banner error">{error}</div>}
        {!certs && !error && <p className="hint">{t("sign.loadingCerts")}</p>}
        {certs && !certs.length && <p className="hint">{t("sign.noCerts")}</p>}
        {certs && certs.length > 0 && (
          <div className="cert-list">
            {certs.map((c) => {
              const expired = c.validTo <= now || c.validFrom > now;
              return (
                <label key={c.id} className={`cert-item ${certId === c.id ? "on" : ""} ${expired ? "expired" : ""}`}>
                  <input type="radio" name="cert" disabled={expired} checked={certId === c.id} onChange={() => setCertId(c.id)} />
                  <span className="cert-main">
                    <b>{c.name}</b>
                    {c.qualified && <span className="badge">{t("sign.qualified")}</span>}
                    <small>
                      {c.issuer} · {expired ? t("sign.expired", { date: day(c.validTo) }) : t("sign.validTo", { date: day(c.validTo) })}
                    </small>
                  </span>
                </label>
              );
            })}
          </div>
        )}
        <button className="link-btn refresh" onClick={load}>
          ↻ {t("sign.refresh")}
        </button>
        <label className="check">
          <input type="checkbox" checked={visible} onChange={(e) => setVisible(e.target.checked)} /> {batch ? t("sign.visibleBatch") : t("sign.visible")}
        </label>
        <label className="check">
          <input type="checkbox" checked={timestamp} onChange={(e) => setTimestamp(e.target.checked)} /> {t("sign.timestamp")}
        </label>
        <label className="check" title={t("sign.ltvHint")}>
          <input type="checkbox" checked={timestamp && ltv} disabled={!timestamp} onChange={(e) => setLtv(e.target.checked)} /> {t("sign.ltv")}
        </label>
        <label>{t("sign.reason")}</label>
        <input value={reason} placeholder={t("sign.reasonPlaceholder")} onChange={(e) => setReason(e.target.value)} />
        <label>{t("sign.location")}</label>
        <input value={location} onChange={(e) => setLocation(e.target.value)} />
        <div className="modal-footer">
          <button onClick={props.onCancel}>{t("common.cancel")}</button>
          <button className="primary" disabled={!cert} onClick={submit}>
            {batch ? t("sign.signN", { n: batch.length }) : visible ? t("sign.next") : t("sign.sign")}
          </button>
        </div>
      </div>
    </div>
  );
}

function TrustLines({ tr }: { tr: TrustInfo }) {
  return (
    <>
      {tr.euQualified ? (
        <small className="trust ok">✔ {t("sign.trustEu", { service: tr.euService ?? "" })}</small>
      ) : tr.euService ? (
        <small className="trust">{t("sign.trustEuNotQualified", { service: tr.euService })}</small>
      ) : null}
      <small className={`trust ${tr.chainTrusted ? "ok" : ""}`}>{tr.chainTrusted ? `✔ ${t("sign.trustChain")}` : t("sign.trustChainNo")}</small>
      <small className={`trust ${tr.revocation === "good" ? "ok" : tr.revocation === "revoked" ? "bad" : ""}`}>
        {tr.revocation === "good" ? `✔ ${t("sign.revGood")}` : tr.revocation === "revoked" ? `✖ ${t("sign.revRevoked")}` : t("sign.revUnknown")}
      </small>
      {tr.trustedTime && (
        <small className="trust ok">
          ✔ {t("sign.trustedTime", { date: dateTime(tr.trustedTime) })}
          {tr.timestampQualified ? ` (${t("sign.qualified")})` : ""}
        </small>
      )}
      {tr.ltv && <small className="trust ok">✔ {t("sign.ltvPresent")}</small>}
      {tr.notes.map((n) => (
        <small key={n} className="trust">
          {n}
        </small>
      ))}
    </>
  );
}

export function SignaturesDialog(props: { signatures: SignatureInfo[]; trust: TrustInfo[] | null; trustError: string | null; onClose(): void }) {
  return (
    <div className="modal-backdrop" onMouseDown={props.onClose}>
      <div className="modal sign-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{t("sign.listTitle")}</h2>
        <div className="sig-list">
          {props.signatures.map((s, i) => {
            const tr = props.trust?.[i];
            return (
              <div key={i} className={`sig-item ${s.intact ? "ok" : "bad"}`}>
                <div className="sig-head">
                  <span className="sig-icon">{s.intact ? "✔" : "✖"}</span>
                  <b>{s.kind === "timestamp" ? t("sign.kindTimestamp") : (s.signer ?? t("sign.unknownSigner"))}</b>
                  {s.qualified && <span className="badge">{t("sign.qualified")}</span>}
                </div>
                {s.kind === "timestamp" && s.signer && <small>{s.signer}</small>}
                {s.issuer && <small>{t("sign.issuer", { issuer: s.issuer })}</small>}
                {s.time && (
                  <small>
                    {t("sign.signedAt", { date: formatPdfDate(s.time) ?? s.time })}
                    {s.timestamped ? ` · ${t("sign.withTimestamp")}` : ""}
                  </small>
                )}
                {s.reason && <small>{t("sign.reasonLabel", { reason: s.reason })}</small>}
                <small className="sig-status">
                  {s.intact ? t("sign.statusOk") : `${t("sign.statusBroken")}: ${s.problem ?? ""}`}
                  {s.intact && !s.coversWhole ? ` ${t("sign.statusLater")}` : ""}
                </small>
                {s.intact && tr && <TrustLines tr={tr} />}
                {s.intact && !tr && !props.trustError && <small className="trust">{t("sign.trustChecking")}</small>}
              </div>
            );
          })}
        </div>
        {props.trustError && <p className="hint">{t("sign.trustFailed", { error: props.trustError })}</p>}
        <div className="modal-footer">
          <button className="primary" onClick={props.onClose}>
            {t("common.close")}
          </button>
        </div>
      </div>
    </div>
  );
}

export function UnlockDialog(props: { count: number; onCancel(): void; onConfirm(): void }) {
  return (
    <div className="modal-backdrop" onMouseDown={props.onCancel}>
      <div className="modal small" onMouseDown={(e) => e.stopPropagation()}>
        <h2>🔓 {t("sign.unlockTitle")}</h2>
        <p>{t("sign.unlockText", { n: props.count })}</p>
        <p className="hint">{t("sign.unlockSafe")}</p>
        <div className="modal-footer">
          <button className="primary" autoFocus onClick={props.onCancel}>
            {t("common.cancel")}
          </button>
          <button className="danger" onClick={props.onConfirm}>
            {t("sign.unlockConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Strona „ostatnia” w miejscu podpisu — dla plików o różnej liczbie stron. */
export const LAST_PAGE = 100000;

const levelLabel = (l: BatchItem["level"]) => (l === "B-LTA" ? "PAdES-LTA" : l ? `PAdES-${l}` : "");

/** Podpisywanie wielu plików: wybór opcji, postęp i wyniki. */
export function BatchSign(props: {
  files: string[];
  onClose(): void;
  onOpen(path: string): void;
  /** Wskazanie miejsc podpisu w głównym edytorze (kolejne pliki w kartach); null — anulowano. */
  placeInEditor(files: string[]): Promise<(Placement | null)[] | null>;
  /** Dalszy krok po podpisaniu (np. odpowiedzi w Gmailu); zastępuje „Zamknij”. */
  next?: { label: string; onNext(items: BatchItem[]): void };
  /** Zawsze kopie „… (podpisany).pdf”, niezależnie od ustawienia nadpisywania (np. poczta). */
  keepOriginals?: boolean;
}) {
  const [overwrite, setOverwrite] = useState(false);
  const [signedDir, setSignedDir] = useState<string | null>(null);
  const [items, setItems] = useState<(BatchItem | null)[] | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Trwa wskazywanie miejsc podpisu w edytorze — okno jest wtedy schowane. */
  const [placing, setPlacing] = useState(false);

  const start = async (c: SignChoice) => {
    if (!c.visible) return void run(c, []);
    setPlacing(true);
    const placements = await props.placeInEditor(props.files);
    setPlacing(false);
    if (placements) void run(c, placements);
    else props.onClose();
  };

  const run = async (c: SignChoice, placements: (Placement | null)[]) => {
    setItems(props.files.map(() => null));
    setRunning(true);
    const now = new Date();
    const settings = await api.getSettings().catch(() => null);
    const ow = !props.keepOriginals && !!settings?.signOverwrite;
    setOverwrite(ow);
    setSignedDir(settings?.signedDir ?? null);
    try {
      await api.signFiles(
        {
          paths: props.files,
          sign: { certId: c.cert.id, signingTime: pdfDate(now), reason: c.reason || undefined, location: c.location || undefined, timestamp: c.timestamp, ltv: c.ltv },
          visibleLines: c.visible ? visibleLines(c.cert, c.reason, now) : null,
          placements,
          overwrite: ow,
        },
        (index, item) => setItems((cur) => (cur ? cur.map((x, i) => (i === index ? item : x)) : cur)),
      );
    } catch (e) {
      setError(errorText(e));
    } finally {
      setRunning(false);
    }
  };

  if (placing) return null;
  if (!items) return <SignDialog files={props.files} onCancel={props.onClose} onSubmit={start} />;
  const done = items.filter(Boolean).length;
  const failed = items.filter((x) => x?.error).length;
  return (
    <div className="modal-backdrop">
      <div className="modal sign-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>✍ {t("sign.batchTitle", { n: props.files.length })}</h2>
        <p className="hint">
          {running
            ? t(signedDir ? "sign.batchRunningDir" : overwrite ? "sign.batchRunningOverwrite" : "sign.batchRunning", { done, total: props.files.length, dir: signedDir ?? "" })
            : t(signedDir ? "sign.batchDoneDir" : overwrite ? "sign.batchDoneOverwrite" : "sign.batchDone", { ok: done - failed, failed, dir: signedDir ?? "" })}
        </p>
        {error && <div className="banner error">{error}</div>}
        <div className="sig-list">
          {props.files.map((f, i) => {
            const it = items[i];
            const name = f.split(/[\\/]/).pop();
            return (
              <div key={f} className={`sig-item ${it?.error ? "bad" : it ? "ok" : ""}`}>
                <div className="sig-head">
                  <span className="sig-icon">{it ? (it.error ? "✖" : "✔") : running ? "…" : "–"}</span>
                  <b title={f}>{name}</b>
                  {it?.level && <span className="badge">{levelLabel(it.level)}</span>}
                </div>
                {it?.error && <small className="sig-status">{it.error}</small>}
                {it?.warning && <small>{it.warning}</small>}
                {it?.output && (
                  <small title={it.output}>
                    → {it.output.split(/[\\/]/).pop()}{" "}
                    <button className="link-btn" onClick={() => props.onOpen(it.output!)}>
                      {t("sign.open")}
                    </button>
                  </small>
                )}
              </div>
            );
          })}
        </div>
        <div className="modal-footer">
          {props.next ? (
            <>
              <button disabled={running} onClick={props.onClose}>
                {t("common.cancel")}
              </button>
              <button className="primary" disabled={running} onClick={() => props.next!.onNext(items.filter((x): x is BatchItem => !!x))}>
                {props.next.label}
              </button>
            </>
          ) : (
            <button className="primary" disabled={running} onClick={props.onClose}>
              {t("common.close")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
