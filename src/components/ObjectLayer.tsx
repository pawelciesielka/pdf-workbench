import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { t } from "../i18n";
import type { EditOp, PageObject } from "../lib/api";

type Pt = [number, number];
type Box = { left: number; top: number; width: number; height: number };

/** Geometria stron w układzie nakładki (piksele) i przestrzeni PDF (punkty). */
export interface LayerGeom {
  pageAt(lx: number, ly: number): number | null;
  toPdf(page: number, lx: number, ly: number): Pt | null;
  toLayer(page: number, x: number, y: number): Pt | null;
  scale(): number;
  pageCanvas(page: number): HTMLCanvasElement | null;
  pageBox(page: number): Box | null;
  currentPage(): number;
}

export interface TextStyle {
  fontSize: number;
  color: string;
  bold: boolean;
}

interface Props {
  mode: "objects" | "addtext";
  geom: LayerGeom;
  /** Zmienia się po każdym przeładowaniu Dokumentu (odświeża listy obiektów). */
  docKey: unknown;
  layout: number;
  textStyle: TextStyle;
  objectsFor(page: number): Promise<PageObject[]>;
  onOps(ops: EditOp[]): Promise<void>;
}

type Sel = { page: number; index: number };
type Drag = {
  kind: "move" | "resize";
  page: number;
  index: number;
  start: Pt;
  cur: Pt;
  /** Dla skalowania: narożnik nieruchomy (piksele nakładki). */
  anchor?: Pt;
  box: Box;
  ghost: string | null;
  shift: boolean;
  moved: boolean;
};
type Editing = { page: number; index: number | null; box: Box; value: string; fontPx: number; at?: Pt };

/**
 * Wybór koloru zatwierdzany raz — po zamknięciu okna wyboru (natywne zdarzenie „change”),
 * a nie przy każdym ruchu suwaka (React onChange = „input”).
 */
export function ColorInput({ value, title, onCommit }: { value: string; title: string; onCommit(color: string): void }) {
  const ref = useRef<HTMLInputElement>(null);
  const [local, setLocal] = useState(value);
  const commit = useRef(onCommit);
  commit.current = onCommit;
  useEffect(() => setLocal(value), [value]);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onChange = () => {
      if (el.value.toLowerCase() !== value.toLowerCase()) commit.current(el.value);
    };
    el.addEventListener("change", onChange);
    return () => el.removeEventListener("change", onChange);
  }, [value]);
  return <input ref={ref} type="color" value={local} title={title} onChange={(e) => setLocal(e.target.value)} />;
}

const PRESET_COLORS = ["#000000", "#444444", "#ffffff", "#d92525", "#e8710a", "#f2c200", "#1e8e3e", "#1a73e8", "#2f5bd3", "#9334e6"];

/** Kolor tekstu: gotowe kolory jednym kliknięciem i „Inny…” z pełnym wyborem. */
function ColorMenu({ value, onPick }: { value: string; onPick(color: string): void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);
  const pick = (c: string) => {
    setOpen(false);
    if (c.toLowerCase() !== value.toLowerCase()) onPick(c);
  };
  return (
    <div className="color-menu" ref={ref}>
      <button className="color-btn" title={t("obj.color")} onClick={() => setOpen((v) => !v)}>
        <span className="color-dot" style={{ background: value }} />▾
      </button>
      {open && (
        <div className="color-pop">
          {PRESET_COLORS.map((c) => (
            <button
              key={c}
              className={`swatch ${c.toLowerCase() === value.toLowerCase() ? "on" : ""}`}
              style={{ background: c }}
              title={c}
              onClick={() => pick(c)}
            />
          ))}
          <label className="color-other" title={t("obj.colorOther")}>
            <ColorInput value={value} title={t("obj.colorOther")} onCommit={pick} />
            {t("obj.colorOther")}
          </label>
        </div>
      )}
    </div>
  );
}

const area = (o: PageObject) => Math.max(0.01, (o.rect[2] - o.rect[0]) * (o.rect[3] - o.rect[1]));
const cleanFont = (name?: string) => (name ?? "").replace(/^[A-Z]{6}\+/, "");

export function ObjectLayer({ mode, geom, docKey, layout, textStyle, objectsFor, onOps }: Props) {
  const [objects, setObjects] = useState<Map<number, PageObject[]>>(new Map());
  const loading = useRef(new Set<number>());
  const [hover, setHover] = useState<Sel | null>(null);
  const [sel, setSel] = useState<Sel | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [sizeInput, setSizeInput] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const objectsRef = useRef(objects);
  objectsRef.current = objects;
  const dragRef = useRef(drag);
  dragRef.current = drag;

  // Nowy stan Dokumentu → listy obiektów od nowa.
  useEffect(() => {
    loading.current.clear();
    setObjects(new Map());
    setHover(null);
  }, [docKey]);

  // Wczytaj obiekty bieżącej strony i sąsiednich.
  const current = geom.currentPage();
  useEffect(() => {
    for (const p of [current, current - 1, current + 1]) {
      if (p < 1 || objectsRef.current.has(p) || loading.current.has(p) || !geom.pageBox(p)) continue;
      loading.current.add(p);
      objectsFor(p)
        .then((list) => setObjects((m) => new Map(m).set(p, list)))
        .catch(() => setObjects((m) => new Map(m).set(p, [])))
        .finally(() => loading.current.delete(p));
    }
  }, [current, docKey, objects, layout]); // eslint-disable-line react-hooks/exhaustive-deps

  // Właśnie dodany tekst zostaje zaznaczony, gdy pojawi się na liście obiektów strony
  // (od razu można go przesunąć albo zmienić mu kolor i rozmiar).
  const pendingNew = useRef<{ page: number; text: string; known: number } | null>(null);
  useEffect(() => {
    const want = pendingNew.current;
    const list = want && objects.get(want.page);
    if (!want || !list) return;
    const added = list.filter((o) => o.kind === "text" && o.text === want.text && o.index > want.known);
    if (!added.length) return;
    pendingNew.current = null;
    setSel({ page: want.page, index: Math.max(...added.map((o) => o.index)) });
  }, [objects]);

  // Zaznaczenie znika, gdy obiektu nie ma już na stronie (np. po cofnięciu zmiany).
  useEffect(() => {
    if (sel && objects.get(sel.page) && !objects.get(sel.page)!.some((o) => o.index === sel.index)) setSel(null);
  }, [objects]); // eslint-disable-line react-hooks/exhaustive-deps

  const objectOf = (s: Sel | null) => (s ? objects.get(s.page)?.find((o) => o.index === s.index) : undefined);

  const boxOf = (page: number, o: PageObject): Box | null => {
    const a = geom.toLayer(page, o.rect[0], o.rect[1]);
    const b = geom.toLayer(page, o.rect[2], o.rect[3]);
    if (!a || !b) return null;
    return { left: Math.min(a[0], b[0]), top: Math.min(a[1], b[1]), width: Math.abs(b[0] - a[0]), height: Math.abs(b[1] - a[1]) };
  };

  const local = (e: { clientX: number; clientY: number }): Pt => {
    const r = rootRef.current!.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };

  /**
   * Obiekt pod punktem. Tekst ma pierwszeństwo przed obrazami i tłem (i większy margines
   * chwytu), wśród obiektów tego samego rodzaju wygrywa najmniejszy.
   */
  const hitTest = (p: Pt, textOnly = false): Sel | null => {
    const page = geom.pageAt(p[0], p[1]);
    if (!page) return null;
    const pdf = geom.toPdf(page, p[0], p[1]);
    const list = objects.get(page);
    if (!pdf || !list) return null;
    let best: PageObject | null = null;
    for (const o of list) {
      const isText = o.kind === "text";
      if (textOnly && !isText) continue;
      const pad = (isText ? 6 : 2) / geom.scale();
      if (pdf[0] < o.rect[0] - pad || pdf[0] > o.rect[2] + pad || pdf[1] < o.rect[1] - pad || pdf[1] > o.rect[3] + pad) continue;
      const bestText = best?.kind === "text";
      if (!best || (isText && !bestText) || (isText === bestText && area(o) < area(best))) best = o;
    }
    return best ? { page, index: best.index } : null;
  };

  /** Wycinek wyrenderowanej strony pod obiektem — podgląd podczas przeciągania. */
  const makeGhost = (page: number, box: Box): string | null => {
    const canvas = geom.pageCanvas(page);
    const pb = geom.pageBox(page);
    if (!canvas || !pb || box.width < 1 || box.height < 1) return null;
    const ratio = canvas.width / pb.width;
    const c = document.createElement("canvas");
    c.width = Math.max(1, Math.round(box.width * ratio));
    c.height = Math.max(1, Math.round(box.height * ratio));
    try {
      c.getContext("2d")!.drawImage(canvas, (box.left - pb.left) * ratio, (box.top - pb.top) * ratio, c.width, c.height, 0, 0, c.width, c.height);
      return c.toDataURL();
    } catch {
      return null;
    }
  };

  const startEditing = (s: Sel) => {
    const o = objectOf(s);
    const box = o && boxOf(s.page, o);
    if (!o || o.kind !== "text" || !box) return;
    setEditing({ page: s.page, index: s.index, box, value: o.text ?? "", fontPx: (o.fontSize ?? 12) * geom.scale() });
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0 || editing) return;
    const p = local(e);
    // Klik w obrębie zaznaczonego obiektu zostawia zaznaczenie — chyba że pod kursorem jest
    // tekst, a zaznaczone jest coś innego (np. tło albo obraz pod tekstem).
    const cur = objectOf(sel);
    const curBox = cur && sel ? boxOf(sel.page, cur) : null;
    const grow = cur?.kind === "text" ? 6 : 0;
    const inside =
      curBox && p[0] >= curBox.left - grow && p[0] <= curBox.left + curBox.width + grow && p[1] >= curBox.top - grow && p[1] <= curBox.top + curBox.height + grow;
    const under = hitTest(p, mode === "addtext");
    const underText = objectOf(under)?.kind === "text";
    if (mode === "addtext" && !under && !inside) {
      // Nowy tekst tylko w pustym miejscu; istniejący tekst się zaznacza i przesuwa.
      const page = geom.pageAt(p[0], p[1]);
      setSel(null);
      if (!page) return;
      e.preventDefault();
      const fontPx = textStyle.fontSize * geom.scale();
      setEditing({ page, index: null, box: { left: p[0], top: p[1] - fontPx * 0.15, width: 0, height: fontPx * 1.3 }, value: "", fontPx, at: p });
      return;
    }
    const hit = inside && !(underText && cur?.kind !== "text") ? sel : under;
    setSel(hit);
    if (!hit) return;
    e.preventDefault();
    const o = objectOf(hit);
    const box = o && boxOf(hit.page, o);
    if (box) setDrag({ kind: "move", page: hit.page, index: hit.index, start: p, cur: p, box, ghost: null, shift: false, moved: false });
  };

  const startResize = (e: React.MouseEvent, corner: Pt) => {
    e.stopPropagation();
    e.preventDefault();
    const o = objectOf(sel);
    const box = o && sel && boxOf(sel.page, o);
    if (!o || !sel || !box) return;
    // Nieruchomy jest narożnik przeciwległy do chwyconego.
    const anchor: Pt = [box.left + (corner[0] ? 0 : box.width), box.top + (corner[1] ? 0 : box.height)];
    const p = local(e);
    setDrag({ kind: "resize", page: sel.page, index: sel.index, start: p, cur: p, anchor, box, ghost: makeGhost(sel.page, box), shift: false, moved: false });
  };

  /** Prostokąt w trakcie przeciągania (piksele nakładki). */
  const dragBox = (d: Drag): Box => {
    if (d.kind === "move") return { ...d.box, left: d.box.left + d.cur[0] - d.start[0], top: d.box.top + d.cur[1] - d.start[1] };
    const [ax, ay] = d.anchor!;
    let w = Math.abs(d.cur[0] - ax);
    let h = Math.abs(d.cur[1] - ay);
    if (!d.shift && d.box.width > 0 && d.box.height > 0) {
      const k = Math.max(w / d.box.width, h / d.box.height);
      w = d.box.width * k;
      h = d.box.height * k;
    }
    w = Math.max(4, w);
    h = Math.max(4, h);
    return { left: d.cur[0] < ax ? ax - w : ax, top: d.cur[1] < ay ? ay - h : ay, width: w, height: h };
  };

  useEffect(() => {
    if (!drag) return;
    const move = (e: MouseEvent) => {
      const p = local(e);
      setDrag((d) => {
        if (!d) return d;
        const moved = Math.hypot(p[0] - d.start[0], p[1] - d.start[1]) > 3;
        return { ...d, cur: p, shift: e.shiftKey, moved: d.moved || moved, ghost: d.ghost ?? (moved ? makeGhost(d.page, d.box) : null) };
      });
    };
    const up = () => {
      const d = dragRef.current;
      setDrag(null);
      if (!d) return;
      if (!d.moved) return;
      if (d.kind === "move") {
        const a = geom.toPdf(d.page, d.start[0], d.start[1]);
        const b = geom.toPdf(d.page, d.cur[0], d.cur[1]);
        if (a && b) onOps([{ op: "move", page: d.page, index: d.index, dx: b[0] - a[0], dy: b[1] - a[1] }]);
      } else {
        const nb = dragBox(d);
        const a = geom.toPdf(d.page, nb.left, nb.top);
        const b = geom.toPdf(d.page, nb.left + nb.width, nb.top + nb.height);
        if (a && b)
          onOps([{ op: "resize", page: d.page, index: d.index, rect: [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])] }]);
      }
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up, { once: true });
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [drag?.kind, drag?.index, drag?.page]); // eslint-disable-line react-hooks/exhaustive-deps

  // Klawisz Delete usuwa zaznaczony obiekt, Enter/F2 otwiera edycję tekstu.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (!sel || editing || (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable))) return;
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        const s = sel;
        setSel(null);
        onOps([{ op: "delete", page: s.page, index: s.index }]);
      } else if (e.key === "Enter" || e.key === "F2") {
        e.preventDefault();
        startEditing(sel);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  useEffect(() => {
    if (mode !== "objects") setSel(null);
    setEditing(null);
  }, [mode]);

  const selected = objectOf(sel);
  useEffect(() => setSizeInput(selected?.fontSize ? String(Math.round(selected.fontSize * 10) / 10) : ""), [selected?.fontSize, sel?.index]);

  const commitEditing = () => {
    const ed = editing;
    if (!ed) return;
    setEditing(null);
    const value = ed.value.replace(/\s*\n\s*/g, " ");
    if (ed.index === null) {
      if (!value.trim() || !ed.at) return;
      const top = geom.toPdf(ed.page, ed.at[0], ed.at[1]);
      if (!top) return;
      const known = (objects.get(ed.page) ?? []).filter((o) => o.kind === "text" && o.text === value).map((o) => o.index);
      pendingNew.current = { page: ed.page, text: value, known: Math.max(-1, ...known) };
      onOps([{ op: "addText", page: ed.page, x: top[0], y: top[1] - textStyle.fontSize * 0.8, text: value, fontSize: textStyle.fontSize, color: textStyle.color, bold: textStyle.bold }]);
      return;
    }
    const o = objects.get(ed.page)?.find((x) => x.index === ed.index);
    if (o && value === o.text) return;
    if (!value.trim()) onOps([{ op: "delete", page: ed.page, index: ed.index }]);
    else onOps([{ op: "setText", page: ed.page, index: ed.index, text: value }]);
  };

  const applyStyle = (patch: { fontSize?: number; color?: string }) => {
    if (!sel || !selected || selected.kind !== "text") return;
    onOps([{ op: "setText", page: sel.page, index: sel.index, text: selected.text ?? "", ...patch }]);
  };

  /**
   * Pasek nad zaznaczeniem (albo pod nim, gdy brak miejsca), zawsze w całości w obrębie
   * strony — nie wychodzi poza prawą krawędź i nie zasłania zaznaczonego tekstu.
   */
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarSize, setToolbarSize] = useState<[number, number]>([0, 0]);
  useLayoutEffect(() => {
    const el = toolbarRef.current;
    if (el && (el.offsetWidth !== toolbarSize[0] || el.offsetHeight !== toolbarSize[1])) setToolbarSize([el.offsetWidth, el.offsetHeight]);
  });
  const toolbarPos = (box: Box): React.CSSProperties => {
    const [w, h] = toolbarSize;
    const pb = sel ? geom.pageBox(sel.page) : null;
    const minX = pb ? pb.left : 0;
    const maxX = (pb ? pb.left + pb.width : rootRef.current?.clientWidth ?? 0) - w;
    const left = Math.max(minX, Math.min(box.left, maxX));
    const gap = 8;
    const pageTop = pb ? pb.top : 0;
    const above = box.top - h - gap;
    const top = above >= pageTop ? above : box.top + box.height + gap;
    return { left, top };
  };
  /** Menu kolorów otwiera się od zaznaczenia — nie zasłania tekstu, któremu zmieniamy kolor. */
  const popUp = (box: Box) => {
    const pageTop = sel ? geom.pageBox(sel.page)?.top ?? 0 : 0;
    const top = toolbarPos(box).top as number;
    return top < box.top && top - 110 >= pageTop;
  };

  const hoverObj = !drag && hover && (hover.page !== sel?.page || hover.index !== sel?.index) ? objectOf(hover) : undefined;
  const hoverBox = hoverObj && hover ? boxOf(hover.page, hoverObj) : null;
  const selBox = selected && sel ? boxOf(sel.page, selected) : null;
  const liveBox = drag?.moved ? dragBox(drag) : null;
  void layout;

  const kindLabel = (o: PageObject) =>
    o.kind === "text"
      ? `${t("obj.kind.text")} · ${cleanFont(o.fontName)} ${Math.round((o.fontSize ?? 0) * 10) / 10} pt`
      : t(`obj.kind.${o.kind}` as "obj.kind.text");

  return (
    <div
      ref={rootRef}
      className={`object-layer mode-${mode} ${hover || drag ? "over-obj" : ""}`}
      onMouseDown={onMouseDown}
      onMouseMove={(e) => {
        if (drag) return;
        const h = hitTest(local(e), mode === "addtext");
        if (h?.page !== hover?.page || h?.index !== hover?.index) setHover(h);
      }}
      onMouseLeave={() => setHover(null)}
      onDoubleClick={(e) => {
        const hit = hitTest(local(e));
        if (hit) {
          setSel(hit);
          startEditing(hit);
        }
      }}
    >
      {hoverBox && <div className="obj-box hover" style={hoverBox} />}
      {drag && liveBox && (
        <>
          <div className="obj-origin" style={drag.box} />
          <div className="obj-box dragging" style={{ ...liveBox, backgroundImage: drag.ghost ? `url(${drag.ghost})` : undefined }} />
        </>
      )}
      {!drag?.moved && selBox && selected && !editing && (
        <>
          <div className="obj-box selected" style={selBox}>
            {([[0, 0], [1, 0], [0, 1], [1, 1]] as Pt[]).map(([cx, cy]) => (
              <span
                key={`${cx}${cy}`}
                className={`obj-handle h${cx}${cy}`}
                onMouseDown={(e) => startResize(e, [cx, cy])}
              />
            ))}
          </div>
          <div
            ref={toolbarRef}
            className={`obj-toolbar ${popUp(selBox) ? "pop-up" : ""}`}
            style={toolbarPos(selBox)}
            onMouseDown={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <span className="obj-kind">{kindLabel(selected)}</span>
            {selected.kind === "text" && (
              <>
                <button onClick={() => sel && startEditing(sel)} title={t("obj.editText")}>
                  ✎
                </button>
                <input
                  className="obj-size"
                  value={sizeInput}
                  title={t("obj.fontSize")}
                  onChange={(e) => setSizeInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      const v = Number(sizeInput.replace(",", "."));
                      if (v >= 2 && v <= 300 && v !== selected.fontSize) applyStyle({ fontSize: v });
                    }
                  }}
                />
                <ColorMenu value={selected.color ?? "#000000"} onPick={(color) => applyStyle({ color })} />
              </>
            )}
            <button
              title={t("obj.delete")}
              onClick={() => {
                const s = sel!;
                setSel(null);
                onOps([{ op: "delete", page: s.page, index: s.index }]);
              }}
            >
              🗑
            </button>
          </div>
        </>
      )}
      {editing && (
        <input
          autoFocus
          className="obj-editor"
          style={{
            left: editing.box.left - 3,
            top: editing.box.top - 3,
            minWidth: Math.max(editing.box.width + 24, 140),
            height: Math.max(editing.box.height, editing.fontPx * 1.25) + 6,
            fontSize: editing.fontPx,
            fontWeight: editing.index === null && textStyle.bold ? 700 : undefined,
            color: editing.index === null ? textStyle.color : undefined,
          }}
          value={editing.value}
          placeholder={editing.index === null ? t("obj.newTextPlaceholder") : undefined}
          onMouseDown={(e) => e.stopPropagation()}
          onChange={(e) => setEditing({ ...editing, value: e.target.value })}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") commitEditing();
            if (e.key === "Escape") setEditing(null);
          }}
          onBlur={commitEditing}
        />
      )}
    </div>
  );
}
