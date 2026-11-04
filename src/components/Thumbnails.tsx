import { memo, useEffect, useRef, useState } from "react";
import type { PDFDocumentProxy } from "../lib/pdf";

const THUMB_WIDTH = 112;

const Thumb = memo(function Thumb({ doc, page, active, onClick }: { doc: PDFDocumentProxy; page: number; active: boolean; onClick(): void }) {
  const ref = useRef<HTMLButtonElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(false);
  const [rendered, setRendered] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(([e]) => e.isIntersecting && setVisible(true), { rootMargin: "400px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || rendered) return;
    let cancelled = false;
    let task: { cancel(): void; promise: Promise<void> } | null = null;
    (async () => {
      const p = await doc.getPage(page);
      if (cancelled || !canvasRef.current) return;
      const base = p.getViewport({ scale: 1 });
      const scale = (THUMB_WIDTH / base.width) * (window.devicePixelRatio || 1);
      const viewport = p.getViewport({ scale });
      const canvas = canvasRef.current;
      canvas.width = Math.floor(viewport.width);
      canvas.height = Math.floor(viewport.height);
      task = p.render({ canvas, viewport });
      try {
        await task.promise;
        if (!cancelled) setRendered(true);
      } catch {
        /* przerwane renderowanie */
      }
    })();
    return () => {
      cancelled = true;
      task?.cancel();
    };
  }, [visible, rendered, doc, page]);

  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <button ref={ref} className={`thumb ${active ? "active" : ""}`} onClick={onClick} title={`Strona ${page}`}>
      <canvas ref={canvasRef} style={{ width: THUMB_WIDTH }} />
      <span>{page}</span>
    </button>
  );
});

export function Thumbnails({ doc, current, onSelect }: { doc: PDFDocumentProxy; current: number; onSelect(page: number): void }) {
  const pages = Array.from({ length: doc.numPages }, (_, i) => i + 1);
  return (
    <div className="thumbnails">
      {pages.map((p) => (
        <Thumb key={p} doc={doc} page={p} active={p === current} onClick={() => onSelect(p)} />
      ))}
    </div>
  );
}
