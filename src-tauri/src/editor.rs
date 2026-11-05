//! Edycja treści stron (obiekty i tekst) przez PDFium — ADR 0004.
//!
//! Współrzędne są w przestrzeni strony PDF (punkty, bez obrotu strony), tak jak Oznaczenia w UI.

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Ładuje pdfium.dll z pierwszego katalogu, w którym jest.
pub fn init(dirs: &[PathBuf]) {
    PDFIUM.get_or_init(|| {
        let mut last = String::from("nie znaleziono pdfium.dll");
        for dir in dirs {
            let path = Pdfium::pdfium_platform_library_name_at_path(dir);
            if !path.exists() {
                continue;
            }
            match Pdfium::bind_to_library(&path) {
                Ok(b) => return Ok(Pdfium::new(b)),
                Err(e) => last = format!("{}: {e}", path.display()),
            }
        }
        Err(last)
    });
}

fn pdfium() -> Result<&'static Pdfium, String> {
    match PDFIUM.get() {
        Some(Ok(p)) => Ok(p),
        Some(Err(e)) => Err(format!("Silnik edycji PDF (PDFium) jest niedostępny: {e}")),
        None => Err("Silnik edycji PDF (PDFium) nie został zainicjowany.".into()),
    }
}

fn e(err: PdfiumError) -> String {
    format!("Błąd PDFium: {err}")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageObject {
    pub index: usize,
    /// text | image | path | shading | form
    pub kind: &'static str,
    /// [x0, y0, x1, y1]
    pub rect: [f32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

const MAX_OBJECTS: usize = 5000;

fn rect_of(obj: &PdfPageObject) -> Option<[f32; 4]> {
    let r = obj.bounds().ok()?.to_rect();
    let v = [r.left().value, r.bottom().value, r.right().value, r.top().value];
    if v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(v)
}

fn hex(c: PdfColor) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

fn parse_hex(s: &str) -> Option<PdfColor> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(PdfColor::new((v >> 16) as u8, (v >> 8) as u8, v as u8, 255))
}

/// Lista obiektów strony (1-based) do zaznaczania w UI. Niewidoczny tekst (warstwa OCR) jest pomijany.
pub fn page_objects(bytes: &[u8], page: u16) -> Result<Vec<PageObject>, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_byte_slice(bytes, None).map_err(e)?;
    let p = doc.pages().get(page as PdfPageIndex - 1).map_err(|_| format!("Brak strony {page}."))?;
    let text_page = p.text().map_err(e)?;
    let mut out = Vec::new();
    for (index, obj) in p.objects().iter().enumerate() {
        if out.len() >= MAX_OBJECTS {
            break;
        }
        let Some(rect) = rect_of(&obj) else { continue };
        let kind = match obj.object_type() {
            PdfPageObjectType::Text => "text",
            PdfPageObjectType::Image => "image",
            PdfPageObjectType::Path => "path",
            PdfPageObjectType::Shading => "shading",
            PdfPageObjectType::XObjectForm => "form",
            _ => continue,
        };
        let mut item = PageObject { index, kind, rect, text: None, font_size: None, font_name: None, color: None };
        if let Some(t) = obj.as_text_object() {
            if !t.is_visible() {
                continue;
            }
            let s = text_page.for_object(t);
            if s.trim().is_empty() {
                continue;
            }
            item.text = Some(s);
            item.font_size = Some(t.scaled_font_size().value);
            item.font_name = Some(t.font().name());
            item.color = obj.fill_color().ok().map(hex);
        }
        out.push(item);
    }
    Ok(out)
}

#[derive(Deserialize, Debug)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditOp {
    Move { page: u16, index: usize, dx: f32, dy: f32 },
    /// Nowy prostokąt obiektu [x0, y0, x1, y1].
    Resize { page: u16, index: usize, rect: [f32; 4] },
    Delete { page: u16, index: usize },
    SetText { page: u16, index: usize, text: String, font_size: Option<f32>, color: Option<String> },
    /// (x, y) = początek linii bazowej.
    AddText { page: u16, x: f32, y: f32, text: String, font_size: f32, color: Option<String>, bold: Option<bool> },
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditReport {
    /// Ile obiektów tekstowych zmieniono (replace_text).
    pub count: usize,
    /// Nazwy czcionek zastąpionych czcionką systemową (brakujące znaki).
    pub font_fallbacks: Vec<String>,
}

/// Czy tekst da się zapisać kodowaniem jednobajtowym wbudowanych czcionek PDF (bez polskich liter).
fn win_ansi(text: &str) -> bool {
    text.chars().all(|c| (c as u32) < 0x7f || ('\u{a0}'..='\u{ff}').contains(&c))
}

/// Rodzina kroju podobnego do czcionki Dokumentu: 0 = bezszeryfowa, 1 = szeryfowa, 2 = stałej szerokości.
fn family_of(name: &str, serif: bool, mono: bool) -> u8 {
    let n = name.to_lowercase();
    if mono || n.contains("courier") || n.contains("mono") || n.contains("consol") {
        2
    } else if serif || n.contains("times") || (n.contains("serif") && !n.contains("sans")) || n.contains("georgia") || n.contains("cambria") {
        1
    } else {
        0
    }
}

/// Czcionki wczytane w ramach jednej operacji (każdy plik czcionki osadzany raz).
type FontCache = std::collections::HashMap<String, PdfFontToken>;

const PS_NAMES: [[&str; 4]; 3] = [
    ["ArialMT", "Arial-BoldMT", "Arial-ItalicMT", "Arial-BoldItalicMT"],
    ["TimesNewRomanPSMT", "TimesNewRomanPS-BoldMT", "TimesNewRomanPS-ItalicMT", "TimesNewRomanPS-BoldItalicMT"],
    ["CourierNewPSMT", "CourierNewPS-BoldMT", "CourierNewPS-ItalicMT", "CourierNewPS-BoldItalicMT"],
];

fn style_index(bold: bool, italic: bool) -> usize {
    match (bold, italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

fn builtin_font(family: u8, bold: bool, italic: bool) -> PdfFontBuiltin {
    use PdfFontBuiltin::*;
    let fonts = [
        [Helvetica, HelveticaBold, HelveticaOblique, HelveticaBoldOblique],
        [TimesRoman, TimesBold, TimesItalic, TimesBoldItalic],
        [Courier, CourierBold, CourierOblique, CourierBoldOblique],
    ];
    fonts[family.min(2) as usize][style_index(bold, italic)]
}

fn system_font_path(family: u8, bold: bool, italic: bool) -> PathBuf {
    let base = ["arial", "times", "cour"][family.min(2) as usize];
    let suffix = ["", "bd", "i", "bi"][style_index(bold, italic)];
    let fonts = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let path = fonts.join(format!("{base}{suffix}.ttf"));
    if path.exists() {
        path
    } else {
        fonts.join("arial.ttf")
    }
}

/// Gdzie i jak umieścić nowy obiekt tekstowy.
enum Placement {
    /// Zastępuje obiekt o tym indeksie, przejmując jego macierz.
    Replace { index: usize, matrix: PdfMatrix },
    /// Nowy obiekt z linią bazową w (x, y).
    At { x: f32, y: f32 },
}

/// Wstawia tekst w Czcionce zastępczej i sprawdza, czy odczytuje się poprawnie.
/// Kolejność prób: wbudowana czcionka PDF (tekst bez polskich liter) → czcionka systemowa
/// osadzona już w Dokumencie przy wcześniejszej edycji → świeżo osadzony plik czcionki.
fn place_text(
    doc: &mut PdfDocument<'static>,
    cache: &mut FontCache,
    pidx: PdfPageIndex,
    family: u8,
    bold: bool,
    italic: bool,
    text: &str,
    unscaled_size: f32,
    fill: Option<PdfColor>,
    placement: Placement,
) -> Result<(), String> {
    let ps = PS_NAMES[family.min(2) as usize][style_index(bold, italic)];
    let path = system_font_path(family, bold, italic);
    let path_key = path.to_string_lossy().to_string();
    // Strona, z której pożyczamy czcionkę, musi żyć, dopóki nowy obiekt jej nie przejmie.
    let mut _keep_alive: Option<PdfPage> = None;
    let mut candidates: Vec<PdfFontToken> = Vec::new();
    if win_ansi(text) {
        let f = builtin_font(family, bold, italic);
        let key = format!("builtin:{f:?}");
        let t = match cache.get(&key) {
            Some(t) => *t,
            None => {
                let t = doc.fonts_mut().new_built_in(f);
                cache.insert(key, t);
                t
            }
        };
        candidates.push(t);
    }
    if let Some(t) = cache.get(&path_key) {
        candidates.push(*t);
    } else {
        let total = doc.pages().len();
        'outer: for i in 0..total {
            let Ok(p) = doc.pages().get(i) else { continue };
            let mut found = None;
            for obj in p.objects().iter() {
                if let Some(t) = obj.as_text_object() {
                    let f = t.font();
                    if f.name() == ps && f.is_embedded().unwrap_or(false) {
                        found = Some(f.token());
                        break;
                    }
                }
            }
            if let Some(t) = found {
                candidates.push(t);
                _keep_alive = Some(p);
                break 'outer;
            }
        }
    }
    let mut fresh_loaded = cache.contains_key(&path_key);
    let mut attempt = 0;
    loop {
        let token = if attempt < candidates.len() {
            candidates[attempt]
        } else if !fresh_loaded {
            fresh_loaded = true;
            let t = doc.fonts_mut().load_true_type_from_file(&path, true).map_err(e)?;
            cache.insert(path_key.clone(), t);
            t
        } else {
            return Err("Nie udało się zapisać tekstu żadną czcionką.".into());
        };
        attempt += 1;
        let mut obj = PdfPageTextObject::new(doc, text, token, PdfPoints::new(unscaled_size)).map_err(e)?;
        match &placement {
            Placement::Replace { matrix, .. } => obj.apply_matrix(*matrix).map_err(e)?,
            Placement::At { x, y } => obj.translate(PdfPoints::new(*x), PdfPoints::new(*y)).map_err(e)?,
        }
        obj.set_fill_color(fill.unwrap_or(PdfColor::new(0, 0, 0, 255))).map_err(e)?;
        let mut page = doc.pages().get(pidx).map_err(e)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let at = match &placement {
            Placement::Replace { index, .. } => *index + 1,
            Placement::At { .. } => page.objects().len(),
        };
        page.objects_mut().insert_object_at_index(at, PdfPageObject::Text(obj)).map_err(e)?;
        let ok = {
            let tp = page.text().map_err(e)?;
            let placed = page.objects().get(at).map_err(e)?;
            let back = placed.as_text_object().map(|t| tp.for_object(t)).unwrap_or_default();
            norm(&back) == norm(text)
        };
        if ok {
            if let Placement::Replace { index, .. } = placement {
                page.objects_mut().remove_object_at_index(index).map_err(e)?;
            }
            page.regenerate_content().map_err(e)?;
            return Ok(());
        }
        page.objects_mut().remove_object_at_index(at).map_err(e)?;
    }
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Znaki dostępne w czcionce o danej nazwie, ustalone z tekstu Dokumentu (czcionki osadzone
/// w podzbiorze mają tylko znaki użyte w pliku).
fn known_chars(doc: &PdfDocument, font_name: &str, page_hint: PdfPageIndex) -> HashSet<char> {
    let mut set = HashSet::new();
    let total = doc.pages().len();
    let range: Vec<PdfPageIndex> = if total <= 200 { (0..total).collect() } else { vec![page_hint] };
    for i in range {
        let Ok(p) = doc.pages().get(i) else { continue };
        let Ok(tp) = p.text() else { continue };
        for obj in p.objects().iter() {
            if let Some(t) = obj.as_text_object() {
                if t.font().name() == font_name {
                    set.extend(tp.for_object(t).chars());
                }
            }
        }
    }
    set
}

/// Czy osadzona/standardowa czcionka obiektu pokaże wszystkie znaki nowego tekstu.
fn font_can_render(doc: &PdfDocument, font: &PdfFont, page: PdfPageIndex, text: &str) -> bool {
    let name = font.name();
    let embedded = font.is_embedded().unwrap_or(false);
    let needed: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !embedded {
        // Czcionki nieosadzone (np. standardowe 14) mają kodowanie jednobajtowe: bez polskich liter.
        return win_ansi(text);
    }
    let subset = name.len() > 7 && name.as_bytes()[6] == b'+';
    if !subset {
        return true;
    }
    let known = known_chars(doc, &name, page);
    needed.iter().all(|c| known.contains(c))
}

/// Ustawia tekst obiektu; gdy czcionka nie ma znaków, zastępuje obiekt nowym w czcionce systemowej.
/// Zwraca nazwę zastąpionej czcionki.
fn set_text_at(
    doc: &mut PdfDocument<'static>,
    cache: &mut FontCache,
    page_no: u16,
    index: usize,
    text: &str,
    font_size: Option<f32>,
    color: Option<PdfColor>,
) -> Result<Option<String>, String> {
    let pidx = page_no as PdfPageIndex - 1;
    struct Info {
        font_name: String,
        fits: bool,
        unscaled: f32,
        ratio: f32,
        fill: Option<PdfColor>,
        bold: bool,
        italic: bool,
        serif: bool,
        mono: bool,
        matrix: PdfMatrix,
    }
    let info = {
        let page = doc.pages().get(pidx).map_err(|_| format!("Brak strony {page_no}."))?;
        let obj = page.objects().get(index).map_err(|_| "Obiekt nie istnieje (strona się zmieniła?).".to_string())?;
        let t = obj.as_text_object().ok_or("To nie jest obiekt tekstowy.")?;
        let font = t.font();
        let font_name = font.name();
        let lower = font_name.to_lowercase();
        let unscaled = t.unscaled_font_size().value;
        let scaled = t.scaled_font_size().value;
        let bold = lower.contains("bold")
            || font.is_bold_reenforced()
            || font
                .weight()
                .map(|w| matches!(w, PdfFontWeight::Weight600 | PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900))
                .unwrap_or(false);
        Info {
            fits: font_can_render(doc, &font, pidx, text),
            italic: font.is_italic() || lower.contains("italic") || lower.contains("oblique"),
            serif: font.is_serif(),
            mono: font.is_fixed_pitch(),
            bold,
            font_name,
            unscaled,
            ratio: if unscaled > 0.0 { scaled / unscaled } else { 1.0 },
            fill: color.or_else(|| obj.fill_color().ok()),
            matrix: obj.matrix().map_err(e)?,
        }
    };
    let new_unscaled = font_size.map(|s| s / info.ratio).unwrap_or(info.unscaled);

    if info.fits {
        let mut page = doc.pages().get(pidx).map_err(e)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let ok = {
            let mut obj = page.objects_mut().get(index).map_err(e)?;
            if let Some(t) = obj.as_text_object_mut() {
                t.set_text(text).map_err(e)?;
                if font_size.is_some() {
                    t.set_unscaled_font_size(PdfPoints::new(new_unscaled)).map_err(e)?;
                }
            }
            if let Some(c) = color {
                obj.set_fill_color(c).map_err(e)?;
            }
            // Weryfikacja: tekst odczytany po zmianie musi się zgadzać (inaczej kodowanie zgubiło znaki).
            let tp = page.text().map_err(e)?;
            let back = obj.as_text_object().map(|t| tp.for_object(t)).unwrap_or_default();
            norm(&back) == norm(text)
        };
        if ok {
            page.regenerate_content().map_err(e)?;
            return Ok(None);
        }
    }

    // Zastępstwo: nowy obiekt w podobnej czcionce systemowej w tym samym miejscu.
    let family = family_of(&info.font_name, info.serif, info.mono);
    place_text(doc, cache, pidx, family, info.bold, info.italic, text, new_unscaled, info.fill, Placement::Replace { index, matrix: info.matrix })?;
    Ok(Some(info.font_name))
}

fn transform_at(doc: &PdfDocument, page_no: u16, index: usize, a: f32, d: f32, ex: f32, fy: f32) -> Result<(), String> {
    let mut page = doc.pages().get(page_no as PdfPageIndex - 1).map_err(|_| format!("Brak strony {page_no}."))?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let mut obj = page.objects_mut().get(index).map_err(|_| "Obiekt nie istnieje (strona się zmieniła?).".to_string())?;
    obj.transform(a, 0.0, 0.0, d, ex, fy).map_err(e)?;
    drop(obj);
    page.regenerate_content().map_err(e)
}

fn load(bytes: &[u8]) -> Result<PdfDocument<'static>, String> {
    pdfium()?.load_pdf_from_byte_vec(bytes.to_vec(), None).map_err(e)
}

fn save(doc: &PdfDocument) -> Result<Vec<u8>, String> {
    doc.save_to_bytes().map_err(e)
}

/// Wykonuje operacje na obiektach. Usunięcia idą na końcu, od najwyższego indeksu.
pub fn edit_objects(bytes: &[u8], ops: Vec<EditOp>) -> Result<(Vec<u8>, EditReport), String> {
    let mut doc = load(bytes)?;
    let mut report = EditReport::default();
    let mut cache = FontCache::new();
    let (mut deletes, rest): (Vec<EditOp>, Vec<EditOp>) = ops.into_iter().partition(|o| matches!(o, EditOp::Delete { .. }));
    for op in rest {
        match op {
            EditOp::Move { page, index, dx, dy } => transform_at(&doc, page, index, 1.0, 1.0, dx, dy)?,
            EditOp::Resize { page, index, rect } => {
                let p = doc.pages().get(page as PdfPageIndex - 1).map_err(|_| format!("Brak strony {page}."))?;
                let obj = p.objects().get(index).map_err(|_| "Obiekt nie istnieje.".to_string())?;
                let [l, b, r, t] = rect_of(&obj).ok_or("Obiekt nie ma wymiarów.")?;
                drop(obj);
                drop(p);
                let (w, h) = (r - l, t - b);
                if w <= 0.01 || h <= 0.01 {
                    return Err("Obiekt jest zbyt mały, by zmienić jego rozmiar.".into());
                }
                let sx = (rect[2] - rect[0]) / w;
                let sy = (rect[3] - rect[1]) / h;
                if sx <= 0.0 || sy <= 0.0 {
                    return Err("Nieprawidłowy rozmiar.".into());
                }
                transform_at(&doc, page, index, sx, sy, rect[0] - l * sx, rect[1] - b * sy)?;
            }
            EditOp::SetText { page, index, text, font_size, color } => {
                if let Some(f) = set_text_at(&mut doc, &mut cache, page, index, &text, font_size, color.as_deref().and_then(parse_hex))? {
                    report.font_fallbacks.push(f);
                }
                report.count += 1;
            }
            EditOp::AddText { page, x, y, text, font_size, color, bold } => {
                if page == 0 || page as PdfPageIndex > doc.pages().len() {
                    return Err(format!("Brak strony {page}."));
                }
                let fill = color.as_deref().and_then(parse_hex);
                place_text(&mut doc, &mut cache, page as PdfPageIndex - 1, 0, bold.unwrap_or(false), false, &text, font_size, fill, Placement::At { x, y })?;
                report.count += 1;
            }
            EditOp::Delete { .. } => unreachable!(),
        }
    }
    deletes.sort_by_key(|o| match o {
        EditOp::Delete { page, index } => (std::cmp::Reverse(*page), std::cmp::Reverse(*index)),
        _ => unreachable!(),
    });
    for op in deletes {
        if let EditOp::Delete { page, index } = op {
            let mut p = doc.pages().get(page as PdfPageIndex - 1).map_err(|_| format!("Brak strony {page}."))?;
            p.objects_mut().remove_object_at_index(index).map_err(|_| "Obiekt nie istnieje (strona się zmieniła?).".to_string())?;
        }
    }
    Ok((save(&doc)?, report))
}

/// Podmienia tekst w obiektach tekstowych. `pages` puste = wszystkie strony;
/// `within` ogranicza do obiektów nachodzących na prostokąt (zaznaczenie w UI).
pub fn replace_text(
    bytes: &[u8],
    pages: &[u16],
    find: &str,
    replace: &str,
    within: Option<[f32; 4]>,
) -> Result<(Vec<u8>, EditReport), String> {
    if find.trim().is_empty() {
        return Err("Pusty tekst do znalezienia.".into());
    }
    let mut doc = load(bytes)?;
    let mut report = EditReport::default();
    let mut cache = FontCache::new();
    let total = doc.pages().len() as u16;
    let pages: Vec<u16> = if pages.is_empty() { (1..=total).collect() } else { pages.to_vec() };
    for page_no in pages {
        if page_no == 0 || page_no > total {
            return Err(format!("Dokument ma {total} stron."));
        }
        // Najpierw zbieramy trafienia (indeks → nowy tekst), potem zmieniamy.
        let hits: Vec<(usize, String)> = {
            let p = doc.pages().get(page_no as PdfPageIndex - 1).map_err(e)?;
            let tp = p.text().map_err(e)?;
            let mut v = Vec::new();
            for (i, obj) in p.objects().iter().enumerate() {
                let Some(t) = obj.as_text_object() else { continue };
                if !t.is_visible() {
                    continue;
                }
                if let Some(w) = within {
                    let Some(r) = rect_of(&obj) else { continue };
                    if r[2] < w[0] || r[0] > w[2] || r[3] < w[1] || r[1] > w[3] {
                        continue;
                    }
                }
                let s = tp.for_object(t);
                if s.contains(find) {
                    v.push((i, s.replace(find, replace)));
                }
            }
            v
        };
        for (index, new_text) in hits {
            if let Some(f) = set_text_at(&mut doc, &mut cache, page_no, index, &new_text, None, None)? {
                if !report.font_fallbacks.contains(&f) {
                    report.font_fallbacks.push(f);
                }
            }
            report.count += 1;
        }
    }
    let out = if report.count > 0 { save(&doc)? } else { Vec::new() };
    Ok((out, report))
}

/// Odpowiedź binarna: [u32 LE długość JSON][JSON raportu][PDF].
pub fn pack(bytes: Vec<u8>, report: &EditReport) -> Vec<u8> {
    let json = serde_json::to_vec(report).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + json.len() + bytes.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&bytes);
    out
}
