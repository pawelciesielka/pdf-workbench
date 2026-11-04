// Kopiuje zasoby pdf.js (fonty standardowe, CMapy, wasm) do public/, żeby działały offline.
import { cpSync, mkdirSync } from "node:fs";
const src = "node_modules/pdfjs-dist";
const dst = "public/pdfjs";
mkdirSync(dst, { recursive: true });
for (const dir of ["cmaps", "standard_fonts", "wasm"]) {
  cpSync(`${src}/${dir}`, `${dst}/${dir}`, { recursive: true });
}
console.log("pdf.js assets copied to", dst);
