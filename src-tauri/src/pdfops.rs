//! Operacje na strukturze PDF (lopdf): obrót stron, zamiana strony w obraz (Redakcja/Korekta)
//! i niewidoczna warstwa tekstu z OCR.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::Deserialize;

type Res<T> = Result<T, String>;

fn load(bytes: &[u8]) -> Res<Document> {
    let doc = Document::load_mem(bytes).map_err(|e| format!("Nie można odczytać PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("Dokument jest zaszyfrowany — edycja nie jest możliwa.".into());
    }
    Ok(doc)
}

fn save(mut doc: Document) -> Res<Vec<u8>> {
    doc.prune_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| format!("Nie można zapisać PDF: {e}"))?;
    Ok(out)
}

fn page_id(doc: &Document, page: u32) -> Res<ObjectId> {
    doc.get_pages().get(&page).copied().ok_or_else(|| format!("Nie ma strony {page}."))
}

/// Atrybut strony z dziedziczeniem po drzewie /Pages (MediaBox, CropBox, Resources, Rotate).
fn inherited(doc: &Document, id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = id;
    for _ in 0..32 {
        let dict = doc.get_dictionary(cur).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(match v {
                Object::Reference(r) => doc.get_object(*r).ok()?.clone(),
                other => other.clone(),
            });
        }
        cur = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn number(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

/// Widoczny obszar strony: CropBox albo MediaBox jako (x0, y0, x1, y1).
fn page_box(doc: &Document, id: ObjectId) -> (f32, f32, f32, f32) {
    let b = inherited(doc, id, b"CropBox").or_else(|| inherited(doc, id, b"MediaBox"));
    if let Some(Object::Array(a)) = b {
        if a.len() == 4 {
            let v: Vec<f32> = a.iter().map(number).collect();
            return (v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3]));
        }
    }
    (0.0, 0.0, 612.0, 792.0)
}

pub fn rotate_pages(bytes: &[u8], pages: &[u32], degrees: i64) -> Res<Vec<u8>> {
    let mut doc = load(bytes)?;
    for &p in pages {
        let id = page_id(&doc, p)?;
        let current = inherited(&doc, id, b"Rotate").map(|o| number(&o) as i64).unwrap_or(0);
        let next = (current + degrees).rem_euclid(360);
        doc.get_dictionary_mut(id).map_err(|e| e.to_string())?.set("Rotate", Object::Integer(next));
    }
    save(doc)
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RasterPage {
    pub page: u32,
    pub offset: usize,
    pub len: usize,
    pub width: i64,
    pub height: i64,
}

/// Zastępuje treść wskazanych stron obrazem JPEG (wyrenderowanym bez obrotu) i usuwa ich adnotacje.
/// Stare strumienie treści znikają z pliku przy zapisie (prune + pełne przepisanie).
pub fn rasterize_pages(bytes: &[u8], blob: &[u8], items: &[RasterPage]) -> Res<Vec<u8>> {
    let mut doc = load(bytes)?;
    for it in items {
        let jpeg = blob.get(it.offset..it.offset + it.len).ok_or("Uszkodzone dane obrazu.")?.to_vec();
        let id = page_id(&doc, it.page)?;
        let (x0, y0, x1, y1) = page_box(&doc, id);
        let media = inherited(&doc, id, b"MediaBox");
        let rotate = inherited(&doc, id, b"Rotate");
        let image = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => it.width,
                "Height" => it.height,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            jpeg,
        );
        let image_id = doc.add_object(image);
        let content = format!("q {} 0 0 {} {} {} cm /ImR Do Q", x1 - x0, y1 - y0, x0, y0);
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = doc.get_dictionary_mut(id).map_err(|e| e.to_string())?;
        page.set("Contents", Object::Reference(content_id));
        page.set("Resources", dictionary! { "XObject" => dictionary! { "ImR" => Object::Reference(image_id) } });
        page.remove(b"Annots");
        page.remove(b"StructParents");
        if let Some(m) = media {
            page.set("MediaBox", m);
        }
        if let Some(r) = rotate {
            page.set("Rotate", r);
        }
    }
    save(doc)
}

#[derive(Deserialize, Debug)]
pub struct TextLine {
    pub text: String,
    /// Lewy dolny róg linii w przestrzeni strony PDF (bez obrotu).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Deserialize, Debug)]
pub struct PageText {
    pub page: u32,
    pub lines: Vec<TextLine>,
}

/// Kodowanie: WinAnsi + polskie litery w kodach 128–143 (przez /Differences), żeby wyszukiwanie
/// i kopiowanie zwracały poprawne znaki.
const POLISH: [(char, u8, &str); 16] = [
    ('Ą', 128, "Aogonek"),
    ('ą', 129, "aogonek"),
    ('Ć', 130, "Cacute"),
    ('ć', 131, "cacute"),
    ('Ę', 132, "Eogonek"),
    ('ę', 133, "eogonek"),
    ('Ł', 134, "Lslash"),
    ('ł', 135, "lslash"),
    ('Ń', 136, "Nacute"),
    ('ń', 137, "nacute"),
    ('Ś', 138, "Sacute"),
    ('ś', 139, "sacute"),
    ('Ź', 140, "Zacute"),
    ('ź', 141, "zacute"),
    ('Ż', 142, "Zdotaccent"),
    ('ż', 143, "zdotaccent"),
];

fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if let Some((_, code, _)) = POLISH.iter().find(|(ch, _, _)| *ch == c) {
                *code
            } else if (c as u32) < 128 || ((c as u32) >= 0xA0 && (c as u32) <= 0xFF) {
                c as u32 as u8
            } else {
                b'?'
            }
        })
        .collect()
}

fn ocr_font(doc: &mut Document) -> ObjectId {
    let mut diffs = vec![Object::Integer(128)];
    diffs.extend(POLISH.iter().map(|(_, _, name)| Object::Name(name.as_bytes().to_vec())));
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => dictionary! {
            "Type" => "Encoding",
            "BaseEncoding" => "WinAnsiEncoding",
            "Differences" => Object::Array(diffs),
        },
    })
}

/// Dokleja niewidoczny tekst (tryb renderowania 3) — strona staje się przeszukiwalna, wygląd się nie zmienia.
pub fn add_text_layer(bytes: &[u8], pages: &[PageText]) -> Res<Vec<u8>> {
    let mut doc = load(bytes)?;
    let font_id = ocr_font(&mut doc);
    for pt in pages {
        if pt.lines.is_empty() {
            continue;
        }
        let id = page_id(&doc, pt.page)?;
        let mut content = Vec::new();
        content.extend_from_slice(b"BT 3 Tr\n");
        for l in &pt.lines {
            // Spacja na końcu słowa: kopiowanie i wyszukiwanie fraz widzą odstępy między słowami.
            let word = l.text.trim();
            if word.is_empty() || l.h <= 0.0 {
                continue;
            }
            let text = format!("{word} ");
            let text = text.as_str();
            let size = (l.h * 0.85).max(1.0);
            let natural = 0.5 * size * text.chars().count() as f32;
            let tz = if natural > 0.0 { (l.w / natural * 100.0).clamp(10.0, 500.0) } else { 100.0 };
            content.extend_from_slice(format!("/FOcr {size:.2} Tf {tz:.1} Tz 1 0 0 1 {:.2} {:.2} Tm ", l.x, l.y + l.h * 0.2).as_bytes());
            content.push(b'(');
            for b in encode(text) {
                if matches!(b, b'(' | b')' | b'\\') {
                    content.push(b'\\');
                }
                content.push(b);
            }
            content.extend_from_slice(b") Tj\n");
        }
        content.extend_from_slice(b"ET\n");

        // Zasoby: kopia odziedziczonych + nasza czcionka.
        let mut resources = match inherited(&doc, id, b"Resources") {
            Some(Object::Dictionary(d)) => d,
            _ => Dictionary::new(),
        };
        let mut fonts = match resources.get(b"Font") {
            Ok(Object::Dictionary(d)) => d.clone(),
            Ok(Object::Reference(r)) => doc.get_dictionary(*r).cloned().unwrap_or_default(),
            _ => Dictionary::new(),
        };
        fonts.set("FOcr", Object::Reference(font_id));
        resources.set("Font", Object::Dictionary(fonts));

        // Istniejąca treść w q…Q, żeby jej stan graficzny nie wpływał na nasz tekst.
        let existing: Vec<Object> = match doc.get_dictionary(id).ok().and_then(|d| d.get(b"Contents").ok()).cloned() {
            Some(Object::Array(a)) => a,
            Some(o @ Object::Reference(_)) => vec![o],
            _ => vec![],
        };
        let q_id = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        let big_q_id = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
        let ours_id = doc.add_object(Stream::new(Dictionary::new(), content));
        let mut contents = vec![Object::Reference(q_id)];
        contents.extend(existing);
        contents.push(Object::Reference(big_q_id));
        contents.push(Object::Reference(ours_id));

        let page = doc.get_dictionary_mut(id).map_err(|e| e.to_string())?;
        page.set("Contents", Object::Array(contents));
        page.set("Resources", Object::Dictionary(resources));
    }
    save(doc)
}
