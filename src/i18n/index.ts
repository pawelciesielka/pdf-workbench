// Prosty moduł tłumaczeń. Nowy język = nowy plik JSON z tymi samymi kluczami.
import pl from "./pl.json";

type Key = keyof typeof pl;
const dict: Record<string, string> = pl;

export function t(key: Key, vars?: Record<string, string | number>): string {
  let s = dict[key] ?? key;
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, String(v));
  return s;
}

export const MODEL_LABELS: Record<string, string> = {
  "claude-opus-5-5": "Claude Opus 5.5",
  "claude-sonnet-5-5": "Claude Sonnet 5.5",
  "claude-haiku-5-5": "Claude Haiku 5.5",
};
