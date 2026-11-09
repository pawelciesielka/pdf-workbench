// Wydanie: node scripts/release.mjs 0.2.0 ["Opis zmian"]
// Ustawia wersję, buduje podpisany instalator i składa w release/v<wersja>/ pliki do wgrania na GitHub:
// instalator i latest.json (z niego program sprawdza aktualizacje).
// Klucz podpisu: %USERPROFILE%\.tauri\pdf-workbench.key albo zmienna TAURI_SIGNING_PRIVATE_KEY.
import { execSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, copyFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const REPO = "pawelciesielka/pdf-workbench";
const [version, notes = ""] = process.argv.slice(2);
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("Użycie: node scripts/release.mjs <wersja, np. 0.2.0> [\"opis zmian\"]");
  process.exit(1);
}

const edit = (file, re, to) => {
  const s = readFileSync(file, "utf8");
  if (!re.test(s)) throw new Error(`Nie znaleziono wersji w ${file}`);
  writeFileSync(file, s.replace(re, to));
};
edit("package.json", /("version":\s*")[^"]+"/, `$1${version}"`);
edit("package-lock.json", /("name":\s*"pdf-workbench",\s*"version":\s*")[^"]+"/g, `$1${version}"`);
edit("src-tauri/tauri.conf.json", /("version":\s*")[^"]+"/, `$1${version}"`);
edit("src-tauri/Cargo.toml", /^(version\s*=\s*")[^"]+"/m, `$1${version}"`);

const env = { ...process.env };
if (!env.TAURI_SIGNING_PRIVATE_KEY) {
  const keyFile = join(homedir(), ".tauri", "pdf-workbench.key");
  if (!existsSync(keyFile)) throw new Error(`Brak klucza podpisu: ${keyFile}`);
  env.TAURI_SIGNING_PRIVATE_KEY = readFileSync(keyFile, "utf8");
}
env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= "";
env.PATH = `${join(homedir(), ".cargo", "bin")}${process.platform === "win32" ? ";" : ":"}${env.PATH}`;

execSync("npm run tauri build", { stdio: "inherit", env });

const nsis = "src-tauri/target/release/bundle/nsis";
const built = `PDF Workbench_${version}_x64-setup.exe`;
// GitHub zamienia spacje w nazwach plików wydania na kropki — od razu używamy takiej nazwy.
const asset = built.replaceAll(" ", ".");
const out = join("release", `v${version}`);
mkdirSync(out, { recursive: true });
copyFileSync(join(nsis, built), join(out, asset));
const signature = readFileSync(join(nsis, `${built}.sig`), "utf8").trim();
const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": { signature, url: `https://github.com/${REPO}/releases/download/v${version}/${asset}` },
  },
};
writeFileSync(join(out, "latest.json"), JSON.stringify(manifest, null, 2) + "\n");

console.log(`
Gotowe: ${out}
  ${asset}
  latest.json

Dalej:
  1. git commit -am "Wersja ${version}" && git push
  2. Nowe wydanie na https://github.com/${REPO}/releases/new z tagiem v${version}
  3. Dołącz oba pliki z ${out} i opublikuj (nie jako pre-release — program czyta „latest”).
`);
