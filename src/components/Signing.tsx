import { useEffect, useState } from "react";
import { t } from "../i18n";
import { api, errorText, type SignatureInfo, type SigningCert } from "../lib/api";

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

/** Krótki opis ważnych podpisów do paska nad dokumentem. */
export function signersSummary(sigs: SignatureInfo[]): string {
  const valid = sigs.filter((s) => s.intact && s.kind === "signature");
  const list = valid.length ? valid : sigs.filter((s) => s.intact);
  if (!list.length) return "";
  const first = list[0];
  const who = [first.signer ?? t("sign.unknownSigner"), formatPdfDate(first.time)].filter(Boolean).join(", ");
  return list.length > 1 ? `${who} +${list.length - 1}` : who;
}

export interface SignChoice {
  cert: SigningCert;
  visible: boolean;
  timestamp: boolean;
  reason: string;
  location: string;
}

export function SignDialog(props: { onCancel(): void; onSubmit(choice: SignChoice): void }) {
  const [certs, setCerts] = useState<SigningCert[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [certId, setCertId] = useState<string | null>(null);
  const [visible, setVisible] = useState(() => localStorage.getItem("sign.visible") !== "0");
  const [timestamp, setTimestamp] = useState(() => localStorage.getItem("sign.timestamp") !== "0");
  const [reason, setReason] = useState("");
  const [location, setLocation] = useState(() => localStorage.getItem("sign.location") ?? "");

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
      localStorage.setItem("sign.location", location);
    } catch {
      /* brak dostępu do localStorage */
    }
    props.onSubmit({ cert, visible, timestamp, reason: reason.trim(), location: location.trim() });
  };

  const now = Date.now();
  return (
    <div className="modal-backdrop" onMouseDown={props.onCancel}>
      <div className="modal sign-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>✍ {t("sign.title")}</h2>
        <p className="hint">{t("sign.hint")}</p>
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
          <input type="checkbox" checked={visible} onChange={(e) => setVisible(e.target.checked)} /> {t("sign.visible")}
        </label>
        <label className="check">
          <input type="checkbox" checked={timestamp} onChange={(e) => setTimestamp(e.target.checked)} /> {t("sign.timestamp")}
        </label>
        <label>{t("sign.reason")}</label>
        <input value={reason} placeholder={t("sign.reasonPlaceholder")} onChange={(e) => setReason(e.target.value)} />
        <label>{t("sign.location")}</label>
        <input value={location} onChange={(e) => setLocation(e.target.value)} />
        <div className="modal-footer">
          <button onClick={props.onCancel}>{t("common.cancel")}</button>
          <button className="primary" disabled={!cert} onClick={submit}>
            {visible ? t("sign.next") : t("sign.sign")}
          </button>
        </div>
      </div>
    </div>
  );
}

export function SignaturesDialog(props: { signatures: SignatureInfo[]; onClose(): void }) {
  return (
    <div className="modal-backdrop" onMouseDown={props.onClose}>
      <div className="modal sign-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{t("sign.listTitle")}</h2>
        <div className="sig-list">
          {props.signatures.map((s, i) => (
            <div key={i} className={`sig-item ${s.intact ? "ok" : "bad"}`}>
              <div className="sig-head">
                <span className="sig-icon">{s.intact ? "✔" : "✖"}</span>
                <b>{s.kind === "timestamp" ? t("sign.kindTimestamp") : (s.signer ?? t("sign.unknownSigner"))}</b>
                {s.qualified && <span className="badge">{t("sign.qualified")}</span>}
              </div>
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
            </div>
          ))}
        </div>
        <p className="hint">{t("sign.trustNote")}</p>
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
