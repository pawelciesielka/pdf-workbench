//! Rozmowa z Claude o jednym lub kilku Dokumentach: budowa żądania, streaming SSE, Cytaty,
//! narzędzia edycji (wykonywane w UI), zapis i koszt.

use crate::{db, keys, AppState};
use base64::Engine;
use futures_util::StreamExt;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::Channel;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];

/// Dokumenty do tego łącznego rozmiaru idą do modelu w całości (limit API: 32 MB żądania i 600 stron;
/// base64 powiększa plik o 1/3, stąd zapas).
const FULL_MODE_MAX_BYTES: i64 = 22 * 1024 * 1024;
const FULL_MODE_MAX_PAGES: i64 = 550;
const FULL_MODE_MAX_DOCS: usize = 5;
const RETRIEVED_PAGES: usize = 14;
const MAX_PAGE_CHARS: usize = 6000;
const MAX_TOOL_ROUNDS: usize = 8;

const SYSTEM_PROMPT: &str = "You are the assistant inside PDF Workbench, a desktop app for reading and editing PDF documents. \
The user is looking at the attached document(s) and asks questions about them or asks for changes.\n\
- Base your answers on the documents and cite the passages you rely on.\n\
- If the documents do not contain the answer, say so plainly instead of guessing.\n\
- If you add information that does not come from the documents (general knowledge), put it in a separate paragraph \
that starts with the exact marker [[spoza-dokumentu]] so the app can label it.\n\
- When only selected pages of long documents are attached, each page starts with a line like \"[Strona 12]\"; \
mention if the answer may be on pages that were not provided.\n\
- You can change the documents with the provided tools, but only when the user asks for a change. Page numbers are 1-based \
and refer to the current state of the document; after deleting or moving pages, later page numbers shift. \
Deleting pages, moving pages, redaction and text replacement are shown to the user as a Proposal they must accept, so tell them what you proposed. \
Nothing is written to disk until the user saves.\n\
- Answer in the language of the user's question. Be concise and use Markdown (lists, bold, tables) where it helps.";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub page: u32,
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ChatEvent {
    Started { model: String },
    BlockStart { index: usize },
    TextDelta { index: usize, text: String },
    Citation { index: usize, citation: CitationView },
    Fallback { to_model: String },
    ToolCall { id: String, name: String, input: Value },
    Done { stop_reason: Option<String>, conversation: Option<db::ConversationInfo> },
    Error { message: String },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CitationView {
    pub page: Option<u32>,
    pub cited_text: String,
    pub document_hash: Option<String>,
    pub document_name: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Segment {
    pub text: String,
    pub citations: Vec<CitationView>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ToolOutcome {
    pub ok: bool,
    pub text: String,
}

#[derive(Default, Debug, Clone, Copy)]
struct Usage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
}

/// Ceny w $ za milion tokenów: (wejście, wyjście, odczyt cache, zapis cache).
fn prices(model: &str) -> (f64, f64, f64, f64) {
    if model.starts_with("claude-haiku-5") {
        (0.10, 0.50, 0.01, 0.125)
    } else if model.starts_with("claude-sonnet-5") {
        (2.0, 10.0, 0.20, 2.5)
    } else if model.starts_with("claude-opus-5-5") {
        (4.0, 20.0, 0.20, 5.0)
    } else {
        (5.0, 25.0, 0.50, 6.25)
    }
}

fn cost(model: &str, u: Usage) -> f64 {
    let (i, o, r, w) = prices(model);
    (u.input as f64 * i + u.output as f64 * o + u.cache_read as f64 * r + u.cache_write as f64 * w) / 1_000_000.0
}

// --- Narzędzia edycji ------------------------------------------------------------------------

fn doc_prop() -> Value {
    json!({"type": "string", "description": "File name of the document to change, exactly as in the document title."})
}

fn pages_prop() -> Value {
    json!({"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 1, "description": "1-based page numbers."})
}

fn tools() -> Value {
    json!([
        {
            "name": "highlight_text",
            "description": "Highlight an exact passage of text on a page (like a highlighter pen). Use the exact wording as it appears in the document; one call per passage. Applied immediately; the user can undo.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "page", "text", "color"], "properties": {
                "document": doc_prop(),
                "page": {"type": "integer", "minimum": 1},
                "text": {"type": "string", "description": "Exact text to highlight, ideally one sentence or phrase."},
                "color": {"type": "string", "enum": ["yellow", "green", "blue", "pink"]}
            }},
            "eager_input_streaming": true
        },
        {
            "name": "go_to_page",
            "description": "Scroll the viewer to a page so the user can see it.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "page"], "properties": {
                "document": doc_prop(), "page": {"type": "integer", "minimum": 1}
            }},
            "eager_input_streaming": true
        },
        {
            "name": "rotate_pages",
            "description": "Rotate pages clockwise. Applied immediately; the user can undo.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "pages", "degrees"], "properties": {
                "document": doc_prop(), "pages": pages_prop(), "degrees": {"type": "integer", "enum": [90, 180, 270]}
            }},
            "eager_input_streaming": true
        },
        {
            "name": "delete_pages",
            "description": "Propose deleting pages. The user must accept the Proposal.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "pages"], "properties": {
                "document": doc_prop(), "pages": pages_prop()
            }},
            "eager_input_streaming": true
        },
        {
            "name": "move_pages",
            "description": "Propose moving pages so they follow page `after_page` (0 = move to the beginning). Page numbers refer to the order before the move. The user must accept the Proposal.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "pages", "after_page"], "properties": {
                "document": doc_prop(), "pages": pages_prop(), "after_page": {"type": "integer", "minimum": 0}
            }},
            "eager_input_streaming": true
        },
        {
            "name": "redact_text",
            "description": "Propose permanently redacting (blacking out and removing) every occurrence of a text, e.g. a name or ID number. The user must accept the Proposal and save.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "text"], "properties": {
                "document": doc_prop(), "text": {"type": "string", "minLength": 2}
            }},
            "eager_input_streaming": true
        },
        {
            "name": "replace_text",
            "description": "Propose replacing a text with a new one directly in the page content (keeps the page as vector text; layout does not reflow, so keep replacements of similar length). Works on one text run at a time, so pass a short exact fragment as it appears in the document, e.g. a number, a date or a name. Omit `pages` to replace on all pages. The user must accept the Proposal and save.",
            "input_schema": {"type": "object", "additionalProperties": false, "required": ["document", "text", "replacement"], "properties": {
                "document": doc_prop(),
                "text": {"type": "string", "minLength": 1},
                "replacement": {"type": "string"},
                "pages": {"type": "array", "items": {"type": "integer", "minimum": 1}}
            }},
            "eager_input_streaming": true
        }
    ])
}

/// Walidacja wejścia narzędzia po stronie klienta (przy eager_input_streaming API go nie sprawdza).
fn validate_tool_input(name: &str, input: &Value) -> Result<(), String> {
    let obj = input.as_object().ok_or("input is not an object")?;
    let need_str = |k: &str| obj.get(k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|_| ()).ok_or(format!("missing string `{k}`"));
    let need_int = |k: &str, min: i64| obj.get(k).and_then(|v| v.as_i64()).filter(|n| *n >= min).map(|_| ()).ok_or(format!("missing integer `{k}`"));
    let need_pages = || {
        obj.get("pages")
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty() && a.iter().all(|p| p.as_i64().is_some_and(|n| n >= 1)))
            .map(|_| ())
            .ok_or("missing `pages` array".to_string())
    };
    need_str("document")?;
    match name {
        "highlight_text" => {
            need_int("page", 1)?;
            need_str("text")?;
            need_str("color")
        }
        "go_to_page" => need_int("page", 1),
        "rotate_pages" => {
            need_pages()?;
            match obj.get("degrees").and_then(|v| v.as_i64()) {
                Some(90 | 180 | 270) => Ok(()),
                _ => Err("`degrees` must be 90, 180 or 270".into()),
            }
        }
        "delete_pages" => need_pages(),
        "move_pages" => {
            need_pages()?;
            need_int("after_page", 0)
        }
        "redact_text" => need_str("text"),
        "replace_text" => {
            need_str("text")?;
            obj.get("replacement").and_then(|v| v.as_str()).ok_or("missing string `replacement`")?;
            match obj.get("pages") {
                None => Ok(()),
                Some(v) if v.as_array().is_some_and(|a| a.iter().all(|p| p.as_i64().is_some_and(|n| n >= 1))) => Ok(()),
                _ => Err("`pages` must be an array of page numbers".into()),
            }
        }
        _ => Err(format!("unknown tool `{name}`")),
    }
}

// --- Dokumenty Rozmowy -----------------------------------------------------------------------

struct DocCtx {
    hash: String,
    name: String,
    size: i64,
    page_count: Option<i64>,
    indexed: bool,
    path: Option<String>,
}

/// Blok dokumentu w żądaniu: Dokument i (dla trybu stron) numery stron kolejnych bloków tekstu.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct DocBlockRef {
    hash: String,
    pages: Option<Vec<u32>>,
}

struct StoredMessage {
    role: String,
    content: Value,
    doc_refs: Vec<DocBlockRef>,
}

fn user_text(text: &str, selection: &Option<Selection>) -> String {
    match selection {
        Some(s) => format!("Zaznaczony fragment ze strony {}:\n\"\"\"\n{}\n\"\"\"\n\n{}", s.page, s.text.trim(), text.trim()),
        None => text.trim().to_string(),
    }
}

fn fts_query(text: &str) -> Option<String> {
    let mut words: Vec<String> = Vec::new();
    for w in text.split(|c: char| !c.is_alphanumeric()) {
        let w = w.to_lowercase();
        if w.chars().count() >= 3 && !words.contains(&w) {
            words.push(w);
        }
        if words.len() >= 30 {
            break;
        }
    }
    (!words.is_empty()).then(|| words.iter().map(|w| format!("\"{w}\"*")).collect::<Vec<_>>().join(" OR "))
}

/// Najtrafniejsze strony z lokalnego indeksu, po wszystkich Dokumentach Rozmowy.
fn retrieve_pages(conn: &rusqlite::Connection, hashes: &[String], query: &str, must: Option<(&str, u32)>) -> Result<Vec<(String, u32, String)>, String> {
    let mut pages: Vec<(String, u32, String)> = Vec::new();
    let in_list = hashes.iter().map(|h| format!("'{}'", h.replace('\'', ""))).collect::<Vec<_>>().join(",");
    if let Some(q) = fts_query(query) {
        let sql = format!("SELECT hash, page, text FROM pages_fts WHERE hash IN ({in_list}) AND pages_fts MATCH ?1 ORDER BY bm25(pages_fts) LIMIT ?2");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![q, RETRIEVED_PAGES as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32, r.get::<_, String>(2)?)))
            .map_err(|e| e.to_string())?;
        pages.extend(rows.flatten());
    }
    if let Some((h, p)) = must {
        if !pages.iter().any(|(ph, n, _)| ph == h && *n == p) {
            if let Ok(t) = conn.query_row("SELECT text FROM pages_fts WHERE hash = ?1 AND page = ?2", params![h, p as i64], |r| r.get::<_, String>(0)) {
                pages.push((h.to_string(), p, t));
            }
        }
    }
    if pages.is_empty() {
        let per_doc = (RETRIEVED_PAGES / hashes.len().max(1)).max(2) as i64;
        for h in hashes {
            let mut stmt = conn.prepare("SELECT page, text FROM pages_fts WHERE hash = ?1 ORDER BY page LIMIT ?2").map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![h, per_doc], |r| Ok((h.clone(), r.get::<_, i64>(0)? as u32, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?;
            pages.extend(rows.flatten());
        }
    }
    pages.sort_by(|a, b| (hashes.iter().position(|h| *h == a.0), a.1).cmp(&(hashes.iter().position(|h| *h == b.0), b.1)));
    Ok(pages)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + " […]"
    }
}

/// Surowy Cytat z API → strona i Dokument do pokazania w UI.
fn citation_view(c: &Value, refs: &[DocBlockRef], names: &(dyn Fn(&str) -> Option<String> + Sync)) -> CitationView {
    let cited_text = c["cited_text"].as_str().unwrap_or_default().to_string();
    let d = c["document_index"].as_u64().unwrap_or(0) as usize;
    let r = refs.get(d);
    let page = match c["type"].as_str() {
        Some("page_location") => c["start_page_number"].as_u64().map(|p| p as u32),
        Some("content_block_location") => {
            let b = c["start_block_index"].as_u64().unwrap_or(0) as usize;
            r.and_then(|r| r.pages.as_ref()).and_then(|p| p.get(b).copied())
        }
        _ => None,
    };
    CitationView {
        page,
        cited_text,
        document_hash: r.map(|r| r.hash.clone()),
        document_name: r.and_then(|r| names(&r.hash)),
    }
}

fn segments(blocks: &[Value], refs: &[DocBlockRef], names: &(dyn Fn(&str) -> Option<String> + Sync)) -> Vec<Segment> {
    blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .map(|b| Segment {
            text: b["text"].as_str().unwrap_or_default().to_string(),
            citations: b["citations"].as_array().map(|cs| cs.iter().map(|c| citation_view(c, refs, names)).collect()).unwrap_or_default(),
        })
        .collect()
}

/// Po przełączeniu na model zapasowy w trakcie odpowiedzi pomijamy bloki myślenia i narzędzi sprzed
/// ostatniego znacznika `fallback` (wymóg API). Sam znacznik też pomijamy.
fn sanitize_for_replay(blocks: Vec<Value>) -> Vec<Value> {
    let last_fallback = blocks.iter().rposition(|b| b["type"] == "fallback");
    blocks
        .into_iter()
        .enumerate()
        .filter(|(i, b)| {
            let t = b["type"].as_str().unwrap_or_default();
            if t == "fallback" {
                return false;
            }
            match last_fallback {
                Some(f) if *i < f => !matches!(t, "thinking" | "redacted_thinking" | "tool_use" | "server_tool_use"),
                _ => true,
            }
        })
        .map(|(_, b)| b)
        .collect()
}

fn parse_doc_refs(raw: Option<String>, first_hash: &str) -> Vec<DocBlockRef> {
    let Some(raw) = raw else { return vec![] };
    if let Ok(refs) = serde_json::from_str::<Vec<DocBlockRef>>(&raw) {
        return refs;
    }
    // Format v1: same numery stron jednego Dokumentu.
    serde_json::from_str::<Vec<u32>>(&raw)
        .map(|pages| vec![DocBlockRef { hash: first_hash.to_string(), pages: Some(pages) }])
        .unwrap_or_default()
}

pub async fn send_message(
    state: &AppState,
    conversation_id: i64,
    text: String,
    selection: Option<Selection>,
    action: Option<String>,
    channel: Channel<ChatEvent>,
) -> Result<(), String> {
    let result = run(state, conversation_id, text, selection, action, &channel).await;
    state.cancels.lock().unwrap().remove(&conversation_id);
    if let Err(message) = &result {
        let _ = channel.send(ChatEvent::Error { message: message.clone() });
    }
    Ok(())
}

struct TurnResult {
    blocks: Vec<Value>,
    usage: Usage,
    model: String,
    stop_reason: Option<String>,
    error: Option<String>,
    cancelled: bool,
}

async fn stream_turn(
    state: &AppState,
    api_key: &str,
    body: &Value,
    with_fallback: bool,
    refs: &[DocBlockRef],
    names: &(dyn Fn(&str) -> Option<String> + Sync),
    cancel: &AtomicBool,
    channel: &Channel<ChatEvent>,
) -> Result<TurnResult, String> {
    let mut req = state
        .http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    if with_fallback {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = req.json(body).send().await.map_err(|e| format!("Błąd połączenia z Claude API: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(text);
        return Err(match status.as_u16() {
            401 => "Nieprawidłowy klucz API. Sprawdź go w Ustawieniach.".into(),
            429 => format!("Przekroczony limit zapytań. Spróbuj za chwilę. ({msg})"),
            529 | 503 => "Claude API jest chwilowo przeciążone. Spróbuj ponownie.".into(),
            _ => format!("Claude API zwróciło błąd {status}: {msg}"),
        });
    }

    let mut blocks: Vec<Option<Value>> = Vec::new();
    let mut partial_json: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut t = TurnResult { blocks: vec![], usage: Usage::default(), model: body["model"].as_str().unwrap_or_default().to_string(), stop_reason: None, error: None, cancelled: false };
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            t.cancelled = true;
            break;
        }
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                t.error = Some(format!("Przerwane połączenie: {e}"));
                break;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find("\n\n") {
            let event: String = buf.drain(..pos + 2).collect();
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data: ") else { continue };
                let Ok(ev) = serde_json::from_str::<Value>(data) else { continue };
                match ev["type"].as_str().unwrap_or_default() {
                    "message_start" => {
                        let m = &ev["message"];
                        if let Some(s) = m["model"].as_str() {
                            t.model = s.to_string();
                        }
                        let u = &m["usage"];
                        t.usage.input = u["input_tokens"].as_i64().unwrap_or(0);
                        t.usage.cache_read = u["cache_read_input_tokens"].as_i64().unwrap_or(0);
                        t.usage.cache_write = u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
                        t.usage.output = u["output_tokens"].as_i64().unwrap_or(0);
                        let _ = channel.send(ChatEvent::Started { model: t.model.clone() });
                    }
                    "content_block_start" => {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        let cb = ev["content_block"].clone();
                        if blocks.len() <= idx {
                            blocks.resize(idx + 1, None);
                        }
                        match cb["type"].as_str() {
                            Some("text") => {
                                let _ = channel.send(ChatEvent::BlockStart { index: idx });
                            }
                            Some("fallback") => {
                                let to = cb["to"]["model"].as_str().unwrap_or_default().to_string();
                                let _ = channel.send(ChatEvent::Fallback { to_model: to });
                            }
                            _ => {}
                        }
                        blocks[idx] = Some(cb);
                    }
                    "content_block_delta" => {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        let Some(Some(block)) = blocks.get_mut(idx) else { continue };
                        let d = &ev["delta"];
                        match d["type"].as_str().unwrap_or_default() {
                            "text_delta" => {
                                let s = d["text"].as_str().unwrap_or_default();
                                let cur = block["text"].as_str().unwrap_or_default().to_string();
                                block["text"] = Value::String(cur + s);
                                let _ = channel.send(ChatEvent::TextDelta { index: idx, text: s.to_string() });
                            }
                            "citations_delta" => {
                                let c = d["citation"].clone();
                                let view = citation_view(&c, refs, names);
                                if !block["citations"].is_array() {
                                    block["citations"] = json!([]);
                                }
                                block["citations"].as_array_mut().unwrap().push(c);
                                let _ = channel.send(ChatEvent::Citation { index: idx, citation: view });
                            }
                            "thinking_delta" => {
                                let cur = block["thinking"].as_str().unwrap_or_default().to_string();
                                block["thinking"] = Value::String(cur + d["thinking"].as_str().unwrap_or_default());
                            }
                            "signature_delta" => block["signature"] = d["signature"].clone(),
                            "input_json_delta" => partial_json.entry(idx).or_default().push_str(d["partial_json"].as_str().unwrap_or_default()),
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(Some(block)) = blocks.get_mut(idx) {
                            if block["type"] == "tool_use" {
                                let raw = partial_json.remove(&idx).unwrap_or_default();
                                block["input"] = if raw.trim().is_empty() {
                                    json!({})
                                } else {
                                    serde_json::from_str(&raw).unwrap_or_else(|_| json!({"__invalid_json": raw}))
                                };
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(s) = ev["delta"]["stop_reason"].as_str() {
                            t.stop_reason = Some(s.to_string());
                        }
                        let u = &ev["usage"];
                        if let Some(o) = u["output_tokens"].as_i64() {
                            t.usage.output = o;
                        }
                        if let Some(i) = u["input_tokens"].as_i64() {
                            t.usage.input = i;
                        }
                        if let Some(r) = u["cache_read_input_tokens"].as_i64() {
                            t.usage.cache_read = r;
                        }
                        if let Some(w) = u["cache_creation_input_tokens"].as_i64() {
                            t.usage.cache_write = w;
                        }
                    }
                    "error" => {
                        t.error = Some(ev["error"]["message"].as_str().unwrap_or("Nieznany błąd").to_string());
                        break 'outer;
                    }
                    _ => {}
                }
            }
        }
    }

    let mut out: Vec<Value> = blocks.into_iter().flatten().collect();
    if t.cancelled || t.error.is_some() {
        // Niedokończona odpowiedź: tylko tekst (bloki myślenia bez podpisu i niedokończone narzędzia są nieważne).
        out.retain(|b| b["type"] == "text" && !b["text"].as_str().unwrap_or_default().is_empty());
        for b in out.iter_mut() {
            if let Some(o) = b.as_object_mut() {
                o.remove("citations");
            }
        }
        if t.cancelled {
            t.stop_reason = Some("cancelled".into());
        }
    }
    t.blocks = out;
    Ok(t)
}

/// Czeka na wynik narzędzia z UI (z obsługą przerwania).
async fn await_tool(state: &AppState, id: &str, cancel: &AtomicBool) -> ToolOutcome {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    state.pending_tools.lock().unwrap().insert(id.to_string(), tx);
    let outcome = loop {
        match tokio::time::timeout(Duration::from_millis(400), &mut rx).await {
            Ok(Ok(o)) => break o,
            Ok(Err(_)) => break ToolOutcome { ok: false, text: "Narzędzie nie odpowiedziało.".into() },
            Err(_) if cancel.load(Ordering::Relaxed) => break ToolOutcome { ok: false, text: "Przerwane przez użytkownika.".into() },
            Err(_) => continue,
        }
    };
    state.pending_tools.lock().unwrap().remove(id);
    outcome
}

async fn run(
    state: &AppState,
    conversation_id: i64,
    text: String,
    selection: Option<Selection>,
    action: Option<String>,
    channel: &Channel<ChatEvent>,
) -> Result<(), String> {
    let (api_key, _) = keys::load().ok_or("Brak klucza API. Dodaj go w Ustawieniach.")?;
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(conversation_id, cancel.clone());

    // --- Rozmowa, Dokumenty, historia ---------------------------------------------------------
    let (conv, docs, mode, mut history, context_note) = {
        let conn = state.db.lock().unwrap();
        let conv = db::conversation(&conn, conversation_id).map_err(|e| e.to_string())?.ok_or("Nie znaleziono Rozmowy.")?;
        let hashes = db::conversation_documents(&conn, conversation_id).map_err(|e| e.to_string())?;
        if hashes.is_empty() {
            return Err("Rozmowa nie ma Dokumentów.".into());
        }
        let mut docs = Vec::new();
        for h in &hashes {
            let (name, size, page_count, indexed): (String, i64, Option<i64>, i64) = conn
                .query_row("SELECT name, size, page_count, indexed FROM documents WHERE hash = ?1", params![h], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map_err(|e| e.to_string())?;
            docs.push(DocCtx { hash: h.clone(), name, size, page_count, indexed: indexed != 0, path: state.path_for(&conn, h) });
        }
        let mode = match &conv.mode {
            Some(m) => m.clone(),
            None => {
                let total_size: i64 = docs.iter().map(|d| d.size).sum();
                let total_pages: i64 = docs.iter().map(|d| d.page_count.unwrap_or(0)).sum();
                let full = docs.len() <= FULL_MODE_MAX_DOCS && total_size <= FULL_MODE_MAX_BYTES && total_pages <= FULL_MODE_MAX_PAGES;
                if !full && docs.iter().any(|d| !d.indexed) {
                    return Err("Dokumenty są duże i wciąż są indeksowane. Spróbuj za chwilę.".into());
                }
                let m = if full { "full" } else { "pages" }.to_string();
                conn.execute("UPDATE conversations SET mode = ?1 WHERE id = ?2", params![m, conversation_id]).map_err(|e| e.to_string())?;
                m
            }
        };
        if mode == "full" {
            if let Some(d) = docs.iter().find(|d| d.path.is_none()) {
                return Err(format!("Nie można znaleźć pliku „{}” na dysku. Otwórz go ponownie.", d.name));
            }
        }
        let mut stmt = conn
            .prepare("SELECT role, content_json, doc_pages_json FROM messages WHERE conversation_id = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let first = hashes[0].clone();
        let history: Vec<StoredMessage> = stmt
            .query_map(params![conversation_id], |r| {
                let content: String = r.get(1)?;
                Ok(StoredMessage { role: r.get(0)?, content: serde_json::from_str(&content).unwrap_or(Value::Null), doc_refs: parse_doc_refs(r.get(2)?, &first) })
            })
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        let note: Option<String> = conn
            .query_row("SELECT context_note FROM conversations WHERE id = ?1", params![conversation_id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        (conv, docs, mode, history, note)
    };
    let names_owned: Vec<(String, String)> = docs.iter().map(|d| (d.hash.clone(), d.name.clone())).collect();
    let names = move |h: &str| names_owned.iter().find(|(x, _)| x == h).map(|(_, n)| n.clone());

    // --- Nowa wiadomość użytkownika -------------------------------------------------------------
    let (user_content, user_refs) = {
        let conn = state.db.lock().unwrap();
        let mut content = Vec::new();
        let mut refs = Vec::new();
        if history.is_empty() {
            if let Some(note) = &context_note {
                content.push(json!({"type": "text", "text": format!("Kontekst z poprzedniej Rozmowy (o wcześniejszej wersji Dokumentu):\n{note}")}));
            }
        }
        if mode == "pages" {
            let hashes: Vec<String> = docs.iter().map(|d| d.hash.clone()).collect();
            let query = format!("{} {}", text, selection.as_ref().map(|s| s.text.as_str()).unwrap_or(""));
            let must = selection.as_ref().map(|s| (docs[0].hash.as_str(), s.page));
            let pages = retrieve_pages(&conn, &hashes, &query, must)?;
            for d in &docs {
                let mine: Vec<&(String, u32, String)> = pages.iter().filter(|(h, _, _)| *h == d.hash).collect();
                if mine.is_empty() {
                    continue;
                }
                let blocks: Vec<Value> = mine
                    .iter()
                    .map(|(_, n, t)| json!({"type": "text", "text": format!("[Strona {n}]\n{}", truncate_chars(t, MAX_PAGE_CHARS))}))
                    .collect();
                content.push(json!({
                    "type": "document",
                    "source": {"type": "content", "content": blocks},
                    "title": format!("{} (wybrane strony)", d.name),
                    "citations": {"enabled": true}
                }));
                refs.push(DocBlockRef { hash: d.hash.clone(), pages: Some(mine.iter().map(|(_, n, _)| *n).collect()) });
            }
        }
        content.push(json!({"type": "text", "text": user_text(&text, &selection)}));
        (Value::Array(content), refs)
    };
    let user_display = json!({
        "text": text.trim(),
        "selection": selection.as_ref().map(|s| json!({"page": s.page, "text": s.text})),
        "action": action,
    });

    let pdf_blocks: Vec<Value> = if mode == "full" {
        let mut v = Vec::new();
        for d in &docs {
            let bytes = tokio::fs::read(d.path.as_ref().unwrap()).await.map_err(|e| format!("Nie można odczytać pliku „{}”: {e}", d.name))?;
            v.push(json!({
                "type": "document",
                "source": {"type": "base64", "media_type": "application/pdf", "data": base64::engine::general_purpose::STANDARD.encode(bytes)},
                "title": d.name,
                "citations": {"enabled": true}
            }));
        }
        v
    } else {
        vec![]
    };

    let model = conv.model.clone();
    let with_fallback = !model.starts_with("claude-haiku");
    let mut pending_user: Option<(Value, Vec<DocBlockRef>, Value)> = Some((user_content, user_refs, user_display));
    let mut total = Usage::default();
    let mut total_cost = 0.0;
    let mut last_stop: Option<String> = None;
    let mut error: Option<String> = None;

    for _round in 0..MAX_TOOL_ROUNDS {
        // Żądanie: historia + (w pierwszej rundzie) nowa wiadomość; kolejne wiadomości tej samej roli scalane.
        let mut messages: Vec<Value> = Vec::new();
        let mut refs: Vec<DocBlockRef> = Vec::new();
        let new_msg = pending_user.as_ref().map(|(c, r, _)| StoredMessage { role: "user".into(), content: c.clone(), doc_refs: r.clone() });
        for (i, m) in history.iter().chain(new_msg.iter()).enumerate() {
            let mut blocks = m.content.as_array().cloned().unwrap_or_default();
            if i == 0 && mode == "full" {
                for (k, b) in pdf_blocks.iter().enumerate() {
                    blocks.insert(k, b.clone());
                    refs.push(DocBlockRef { hash: docs[k].hash.clone(), pages: None });
                }
            }
            if m.role == "user" {
                refs.extend(m.doc_refs.iter().cloned());
            }
            match messages.last_mut() {
                Some(last) if last["role"] == m.role.as_str() => last["content"].as_array_mut().unwrap().extend(blocks),
                _ => messages.push(json!({"role": m.role, "content": blocks})),
            }
        }

        let mut body = json!({
            "model": model,
            "max_tokens": 32000,
            "stream": true,
            "system": SYSTEM_PROMPT,
            "messages": messages,
            "tools": tools(),
            "cache_control": {"type": "ephemeral"},
            "output_config": {"effort": "medium"},
        });
        if with_fallback {
            body["fallbacks"] = json!("default");
        }

        let turn = stream_turn(state, &api_key, &body, with_fallback, &refs, &names, &cancel, channel).await?;
        total.input += turn.usage.input;
        total.output += turn.usage.output;
        total.cache_read += turn.usage.cache_read;
        total.cache_write += turn.usage.cache_write;
        total_cost += cost(&turn.model, turn.usage);
        last_stop = turn.stop_reason.clone();
        error = turn.error.clone();

        let replay = sanitize_for_replay(turn.blocks.clone());
        let segs = segments(&turn.blocks, &refs, &names);
        let tool_uses: Vec<Value> = if turn.stop_reason.as_deref() == Some("tool_use") && !turn.cancelled && turn.error.is_none() {
            replay.iter().filter(|b| b["type"] == "tool_use").cloned().collect()
        } else {
            vec![]
        };
        let has_content = segs.iter().any(|s| !s.text.trim().is_empty()) || !tool_uses.is_empty();
        if !has_content || replay.is_empty() {
            break;
        }

        // Zapis rundy: (wiadomość użytkownika) + odpowiedź.
        let assistant_id = {
            let conn = state.db.lock().unwrap();
            let now = db::now();
            if let Some((content, refs_u, display)) = pending_user.take() {
                conn.execute(
                    "INSERT INTO messages(conversation_id, role, content_json, display_json, doc_pages_json, created_at) VALUES (?1, 'user', ?2, ?3, ?4, ?5)",
                    params![conversation_id, content.to_string(), display.to_string(), (!refs_u.is_empty()).then(|| serde_json::to_string(&refs_u).unwrap()), now],
                )
                .map_err(|e| e.to_string())?;
                history.push(StoredMessage { role: "user".into(), content, doc_refs: refs_u });
                if history.len() == 1 {
                    let title = truncate_chars(action.as_deref().unwrap_or(text.trim()), 60);
                    conn.execute("UPDATE conversations SET title = ?1 WHERE id = ?2", params![title, conversation_id]).map_err(|e| e.to_string())?;
                }
            }
            let display = json!({"segments": segs, "stopReason": turn.stop_reason, "model": turn.model, "tools": []});
            conn.execute(
                "INSERT INTO messages(conversation_id, role, content_json, display_json, created_at) VALUES (?1, 'assistant', ?2, ?3, ?4)",
                params![conversation_id, Value::Array(replay.clone()).to_string(), display.to_string(), now + 1],
            )
            .map_err(|e| e.to_string())?;
            history.push(StoredMessage { role: "assistant".into(), content: Value::Array(replay), doc_refs: vec![] });
            conn.last_insert_rowid()
        };

        if tool_uses.is_empty() {
            break;
        }

        // Narzędzia: wykonanie w UI, wyniki w jednej wiadomości użytkownika.
        let mut results = Vec::new();
        let mut tool_views = Vec::new();
        for tu in &tool_uses {
            let id = tu["id"].as_str().unwrap_or_default().to_string();
            let name = tu["name"].as_str().unwrap_or_default().to_string();
            let input = tu["input"].clone();
            let outcome = match validate_tool_input(&name, &input) {
                Err(e) => ToolOutcome { ok: false, text: format!("INVALID_JSON: {e}") },
                Ok(()) if cancel.load(Ordering::Relaxed) => ToolOutcome { ok: false, text: "Przerwane przez użytkownika.".into() },
                Ok(()) => {
                    let _ = channel.send(ChatEvent::ToolCall { id: id.clone(), name: name.clone(), input: input.clone() });
                    await_tool(state, &id, &cancel).await
                }
            };
            results.push(json!({"type": "tool_result", "tool_use_id": id, "content": outcome.text, "is_error": !outcome.ok}));
            tool_views.push(json!({"name": name, "input": input, "ok": outcome.ok, "result": outcome.text}));
        }
        {
            let conn = state.db.lock().unwrap();
            let display_raw: String = conn
                .query_row("SELECT display_json FROM messages WHERE id = ?1", params![assistant_id], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let mut display: Value = serde_json::from_str(&display_raw).unwrap_or(json!({}));
            display["tools"] = Value::Array(tool_views);
            conn.execute("UPDATE messages SET display_json = ?1 WHERE id = ?2", params![display.to_string(), assistant_id]).map_err(|e| e.to_string())?;
            let content = Value::Array(results);
            conn.execute(
                "INSERT INTO messages(conversation_id, role, content_json, display_json, created_at) VALUES (?1, 'user', ?2, ?3, ?4)",
                params![conversation_id, content.to_string(), json!({"hidden": true}).to_string(), db::now()],
            )
            .map_err(|e| e.to_string())?;
            history.push(StoredMessage { role: "user".into(), content, doc_refs: vec![] });
        }
        if cancel.load(Ordering::Relaxed) {
            last_stop = Some("cancelled".into());
            break;
        }
    }

    let conv_info = {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET updated_at = ?1, input_tokens = input_tokens + ?2, output_tokens = output_tokens + ?3, \
             cache_read_tokens = cache_read_tokens + ?4, cache_write_tokens = cache_write_tokens + ?5, cost_usd = cost_usd + ?6 WHERE id = ?7",
            params![db::now(), total.input, total.output, total.cache_read, total.cache_write, total_cost, conversation_id],
        )
        .map_err(|e| e.to_string())?;
        db::conversation(&conn, conversation_id).map_err(|e| e.to_string())?
    };
    if let Some(e) = error {
        return Err(format!("Odpowiedź przerwana: {e}"));
    }
    let _ = channel.send(ChatEvent::Done { stop_reason: last_stop, conversation: conv_info });
    Ok(())
}

/// Krótki zapis Rozmowy do przeniesienia jako kontekst do nowej Rozmowy.
pub fn summarize_for_continuation(conn: &rusqlite::Connection, conversation_id: i64) -> Result<String, String> {
    let mut stmt = conn
        .prepare("SELECT role, display_json FROM messages WHERE conversation_id = ?1 ORDER BY id DESC LIMIT 12")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt.query_map(params![conversation_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.flatten().collect();
    let mut parts = Vec::new();
    for (role, display) in rows.into_iter().rev() {
        let d: Value = serde_json::from_str(&display).unwrap_or(Value::Null);
        if d["hidden"] == true {
            continue;
        }
        let text = if role == "user" {
            format!("Użytkownik: {}", d["action"].as_str().or(d["text"].as_str()).unwrap_or_default())
        } else {
            let t: String = d["segments"].as_array().map(|s| s.iter().filter_map(|x| x["text"].as_str()).collect()).unwrap_or_default();
            format!("Asystent: {t}")
        };
        parts.push(truncate_chars(&text, 1200));
    }
    Ok(truncate_chars(&parts.join("\n\n"), 6000))
}
