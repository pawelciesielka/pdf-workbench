import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { UpdateSettings, type Updates } from "./Updates";
import { MODEL_LABELS, t } from "../i18n";
import { api, errorText, type Settings } from "../lib/api";

export type Theme = "system" | "light" | "dark";

interface Props {
  settings: Settings;
  updates: Updates;
  theme: Theme;
  onTheme(t: Theme): void;
  onChanged(): void;
  onClose(): void;
}

export function SettingsDialog({ settings, updates, theme, onTheme, onChanged, onClose }: Props) {
  const [key, setKey] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion().then(setVersion, () => {});
  }, []);

  const saveKey = async () => {
    try {
      await api.setApiKey(key);
      setKey("");
      setMsg(t("settings.apiKeySaved"));
      onChanged();
    } catch (e) {
      setMsg(errorText(e));
    }
  };

  const removeKey = async () => {
    await api.clearApiKey();
    onChanged();
  };

  const status =
    settings.apiKeySource === "keyring"
      ? t("settings.apiKeySaved")
      : settings.apiKeySource === "env"
        ? t("settings.apiKeyEnv")
        : t("settings.apiKeyNone");

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal settings-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{t("settings.title")}</h2>

        <label>{t("settings.apiKey")}</label>
        <div className={`key-status ${settings.apiKeySource ? "ok" : ""}`}>{status}</div>
        <div className="row">
          <input type="password" value={key} placeholder="sk-ant-…" onChange={(e) => setKey(e.target.value)} onKeyDown={(e) => e.key === "Enter" && key && saveKey()} />
          <button className="primary" disabled={!key.trim()} onClick={saveKey}>
            {t("settings.save")}
          </button>
          {settings.apiKeySource === "keyring" && <button onClick={removeKey}>{t("settings.remove")}</button>}
        </div>
        <p className="hint">{t("settings.apiKeyHint")}</p>
        {msg && <p className="hint">{msg}</p>}

        <label>{t("settings.model")}</label>
        <select
          value={settings.model}
          onChange={async (e) => {
            await api.setModel(e.target.value);
            onChanged();
          }}
        >
          {settings.models.map((m) => (
            <option key={m} value={m}>
              {MODEL_LABELS[m] ?? m}
            </option>
          ))}
        </select>
        <p className="hint">{t("settings.modelHint")}</p>

        <label>{t("settings.theme")}</label>
        <div className="segmented">
          {(["system", "light", "dark"] as Theme[]).map((v) => (
            <button key={v} className={theme === v ? "on" : ""} onClick={() => onTheme(v)}>
              {t(`settings.theme.${v}`)}
            </button>
          ))}
        </div>

        <label>{t("settings.signing")}</label>
        <label className="check">
          <input
            type="checkbox"
            checked={settings.signOverwrite}
            onChange={async (e) => {
              await api.setSignOverwrite(e.target.checked);
              onChanged();
            }}
          />
          {t("settings.signOverwrite")}
        </label>
        <p className="hint">{t("settings.signOverwriteHint")}</p>

        <label>{t("settings.signedDir")}</label>
        <div className="row">
          <input readOnly value={settings.signedDir ?? t("settings.signedDirOff")} title={settings.signedDir ?? ""} />
          <button
            onClick={async () => {
              const dir = await openDialog({ directory: true, multiple: false, defaultPath: settings.signedDir ?? undefined });
              if (typeof dir !== "string") return;
              await api.setSignedDir(dir);
              onChanged();
            }}
          >
            {t("settings.signLogPick")}
          </button>
          {settings.signedDir && (
            <>
              <button onClick={() => api.revealInExplorer(settings.signedDir!).catch((e) => setMsg(errorText(e)))}>{t("settings.signedDirOpen")}</button>
              <button
                onClick={async () => {
                  await api.setSignedDir(null);
                  onChanged();
                }}
              >
                {t("settings.signLogDisable")}
              </button>
            </>
          )}
        </div>
        <p className="hint">{t("settings.signedDirHint")}</p>

        <label>{t("settings.signLog")}</label>
        <div className="row">
          <input readOnly value={settings.signLogDir ?? t("settings.signLogOff")} title={settings.signLogDir ?? ""} />
          <button
            onClick={async () => {
              const dir = await openDialog({ directory: true, multiple: false, defaultPath: settings.signLogDir ?? undefined });
              if (typeof dir !== "string") return;
              await api.setSignLogDir(dir);
              onChanged();
            }}
          >
            {t("settings.signLogPick")}
          </button>
          {settings.signLogDir && (
            <>
              <button
                onClick={() =>
                  api.revealInExplorer(`${settings.signLogDir!.replace(/[\\/]+$/, "")}\\dziennik-podpisow.md`).catch(() => api.revealInExplorer(settings.signLogDir!)).catch((e) => setMsg(errorText(e)))
                }
              >
                {t("settings.signLogOpen")}
              </button>
              <button
                onClick={async () => {
                  await api.setSignLogDir(null);
                  onChanged();
                }}
              >
                {t("settings.signLogDisable")}
              </button>
            </>
          )}
        </div>
        <p className="hint">{t("settings.signLogHint")}</p>

        <UpdateSettings updates={updates} version={version} />

        <div className="modal-footer">
          <button className="primary" onClick={onClose}>
            {t("settings.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
