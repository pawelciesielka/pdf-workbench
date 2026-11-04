import { useState } from "react";
import { MODEL_LABELS, t } from "../i18n";
import { api, errorText, type Settings } from "../lib/api";

export type Theme = "system" | "light" | "dark";

interface Props {
  settings: Settings;
  theme: Theme;
  onTheme(t: Theme): void;
  onChanged(): void;
  onClose(): void;
}

export function SettingsDialog({ settings, theme, onTheme, onChanged, onClose }: Props) {
  const [key, setKey] = useState("");
  const [msg, setMsg] = useState<string | null>(null);

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
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
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

        <div className="modal-footer">
          <button className="primary" onClick={onClose}>
            {t("settings.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
