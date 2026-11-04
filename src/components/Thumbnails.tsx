import { memo, useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import type { PDFDocumentProxy } from "../lib/pdf";

const THUMB_WIDTH = 112;

interface ThumbProps {
  doc: PDFDocumentProxy;
  page: number;
  active: boolean;
  selected: boolean;
  dropHint: "before" | "after" | null;
  onClick(e: React.MouseEvent): void;
  onDragStart(e: React.DragEvent): void;
  onDragOver(e: React.DragEvent, half: "before" | "after"): void;
  onDrop(e: React.DragEvent, half: "before" | "after"): void;
}

const Thumb = memo(function Thumb(p: ThumbProps) {
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
      const pg = await p.doc.getPage(p.page);
      if (cancelled || !canvasRef.current) return;
      const base = pg.getViewport({ scale: 1 });
      const scale = (THUMB_WIDTH / base.width) * (window.devicePixelRatio || 1);
      const viewport = pg.getViewport({ scale });
      const canvas = canvasRef.current;
      canvas.width = Math.floor(viewport.width);
      canvas.height = Math.floor(viewport.height);
      task = pg.render({ canvas, viewport });
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
  }, [visible, rendered, p.doc, p.page]);

  useEffect(() => {
    if (p.active) ref.current?.scrollIntoView({ block: "nearest" });
  }, [p.active]);

  const half = (e: React.DragEvent): "before" | "after" => {
    const r = e.currentTarget.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2 ? "before" : "after";
  };

  return (
    <button
      ref={ref}
      className={`thumb ${p.active ? "active" : ""} ${p.selected ? "selected" : ""} ${p.dropHint ? `drop-${p.dropHint}` : ""}`}
      onClick={p.onClick}
      draggable
      onDragStart={p.onDragStart}
      onDragOver={(e) => p.onDragOver(e, half(e))}
      onDrop={(e) => p.onDrop(e, half(e))}
      title={`${t("toolbar.page")} ${p.page}`}
    >
      <canvas ref={canvasRef} style={{ width: THUMB_WIDTH }} />
      <span>{p.page}</span>
    </button>
  );
});

interface Props {
  doc: PDFDocumentProxy;
  current: number;
  selected: number[];
  busy: boolean;
  onSelect(pages: number[], goTo?: number): void;
  onMove(pages: number[], afterPage: number): void;
  onRotate(pages: number[]): void;
  onDelete(pages: number[]): void;
  onExtract(pages: number[]): void;
  onInsert(afterPage: number): void;
}

export function Thumbnails(props: Props) {
  const { doc, selected } = props;
  const pages = Array.from({ length: doc.numPages }, (_, i) => i + 1);
  const anchor = useRef<number | null>(null);
  const [drop, setDrop] = useState<{ page: number; half: "before" | "after" } | null>(null);
  const dragged = useRef<number[]>([]);

  const click = (page: number, e: React.MouseEvent) => {
    if (e.shiftKey && anchor.current) {
      const [a, b] = [Math.min(anchor.current, page), Math.max(anchor.current, page)];
      props.onSelect(Array.from({ length: b - a + 1 }, (_, i) => a + i));
    } else if (e.ctrlKey) {
      props.onSelect(selected.includes(page) ? selected.filter((p) => p !== page) : [...selected, page]);
      anchor.current = page;
    } else {
      props.onSelect([page], page);
      anchor.current = page;
    }
  };

  const target = selected.length ? selected : [props.current];

  return (
    <div className="thumbnails-wrap">
      <div className="thumb-actions">
        <span className="thumb-count">{selected.length > 1 ? t("pages.selected", { n: selected.length }) : t("pages.page", { n: target[0] })}</span>
        <button className="icon-btn" disabled={props.busy} title={t("pages.rotate")} onClick={() => props.onRotate(target)}>
          ↻
        </button>
        <button className="icon-btn" disabled={props.busy} title={t("pages.delete")} onClick={() => props.onDelete(target)}>
          🗑
        </button>
        <button className="icon-btn" disabled={props.busy} title={t("pages.extract")} onClick={() => props.onExtract(target)}>
          ⇪
        </button>
        <button className="icon-btn" disabled={props.busy} title={t("pages.insert")} onClick={() => props.onInsert(Math.max(...target))}>
          ⊕
        </button>
      </div>
      <div className="thumbnails" onDragLeave={() => setDrop(null)}>
        {pages.map((p) => (
          <Thumb
            key={p}
            doc={doc}
            page={p}
            active={p === props.current}
            selected={selected.includes(p)}
            dropHint={drop?.page === p ? drop.half : null}
            onClick={(e) => click(p, e)}
            onDragStart={(e) => {
              dragged.current = selected.includes(p) ? selected : [p];
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", String(p));
            }}
            onDragOver={(e, half) => {
              e.preventDefault();
              setDrop({ page: p, half });
            }}
            onDrop={(e, half) => {
              e.preventDefault();
              setDrop(null);
              const after = half === "before" ? p - 1 : p;
              if (dragged.current.length && !props.busy) props.onMove(dragged.current, after);
            }}
          />
        ))}
      </div>
    </div>
  );
}
