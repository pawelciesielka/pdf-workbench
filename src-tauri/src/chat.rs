//! Rozmowa z Claude o Dokumencie: budowa żądania, streaming SSE, Cytaty, zapis i koszt.

use crate::{db, keys, AppState};
use base64::Engine;
use futures_util::StreamExt;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];

/// Dokumenty do tego rozmiaru idą do modelu w całości (limit API: 32 MB żądania i 600 stron;
/// base64 powiększa plik o 1/3, stąd zapas).
const FULL_MODE_MAX_BYTES: u64 = 22 * 1024 * 1024;
const FULL_MODE_MAX_PAGES: i64 = 550;
const RETRIEVED_PAGES: usize = 12;
const MAX_PAGE_CHARS: usize = 6000;

const SYSTEM_PROMPT: &str = "You are the assistant inside PDF Workbench, a desktop app for reading PDF documents. \
The user is looking at the attached document and asks questions about it.\n\
- Base your answers on the document and cite the passages you rely on.\n\
- If the document does not contain the answer, say so plainly instead of guessing.\n\
- If you add information that does not come from the document (general knowledge), put it in a separate paragraph \
that starts with the exact marker [[spoza-dokumentu]] so the app can label it.\n\
- When only selected pages of a long document are attached, each page starts with a line like \"[Strona 12]\"; \
mention if the answer may be on pages that were not provided.\n\
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
    Citation { index: usize, page: Option<u32>, cited_text: String },
    Fallback { to_model: String },
    Done {
        stop_reason: Option<String>,
        conversation: Option<db::ConversationInfo>,
    },
    Error { message: String },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CitationView {
    pub page: Option<u32>,
    pub cited_text: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Segment {
    pub text: String,
    pub citations: Vec<CitationView>,
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
        // Opus 5 / 4.8 i inne modele zapasowe.
        (5.0, 25.0, 0.50, 6.25)
    }
}

fn cost(model: &str, u: Usage) -> f64 {
    let (i, o, r, w) = prices(model);
    (u.input as f64 * i + u.output as f64 * o + u.cache_read as f64 * r + u.cache_write as f64 * w) / 1_000_000.0
}

struct StoredMessage {
    role: String,
    content: Value,
    doc_pages: Option<Vec<u32>>,
}

fn user_text(text: &str, selection: &Option<Selection>) -> String {
    match selection {
        Some(s) => format!(
            "Zaznaczony fragment ze strony {}:\n\"\"\"\n{}\n\"\"\"\n\n{}",
            s.page,
            s.text.trim(),
            text.trim()
        ),
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
    if words.is_empty() {
        None
    } else {
        Some(words.iter().map(|w| format!("\"{w}\"*")).collect::<Vec<_>>().join(" OR "))
    }
}

/// Wybiera najtrafniejsze strony długiego Dokumentu z lokalnego indeksu.
fn retrieve_pages(conn: &rusqlite::Connection, hash: &str, query: &str, must: Option<u32>) -> Result<Vec<(u32, String)>, String> {
    let mut pages: Vec<(u32, String)> = Vec::new();
    if let Some(q) = fts_query(query) {
        let mut stmt = conn
            .prepare("SELECT page, text FROM pages_fts WHERE hash = ?1 AND pages_fts MATCH ?2 ORDER BY bm25(pages_fts) LIMIT ?3")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![hash, q, RETRIEVED_PAGES as i64], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for r in rows.flatten() {
            pages.push(r);
        }
    }
    if let Some(p) = must {
        if !pages.iter().any(|(n, _)| *n == p) {
            if let Ok(t) = conn.query_row(
                "SELECT text FROM pages_fts WHERE hash = ?1 AND page = ?2",
                params![hash, p as i64],
                |r| r.get::<_, String>(0),
            ) {
                pages.push((p, t));
            }
        }
    }
    if pages.is_empty() {
        let mut stmt = conn
            .prepare("SELECT page, text FROM pages_fts WHERE hash = ?1 ORDER BY page LIMIT ?2")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![hash, RETRIEVED_PAGES as i64], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        pages = rows.flatten().collect();
    }
    pages.sort_by_key(|(n, _)| *n);
    Ok(pages)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + " […]"
    }
}

/// Zmienia surowy Cytat z API na numer strony do pokazania w UI.
fn citation_view(c: &Value, doc_pages: &[Option<Vec<u32>>]) -> CitationView {
    let cited_text = c["cited_text"].as_str().unwrap_or_default().to_string();
    let page = match c["type"].as_str() {
        Some("page_location") => c["start_page_number"].as_u64().map(|p| p as u32),
        Some("content_block_location") => {
            let d = c["document_index"].as_u64().unwrap_or(0) as usize;
            let b = c["start_block_index"].as_u64().unwrap_or(0) as usize;
            doc_pages.get(d).and_then(|p| p.as_ref()).and_then(|p| p.get(b).copied())
        }
        _ => None,
    };
    CitationView { page, cited_text }
}

fn segments(blocks: &[Value], doc_pages: &[Option<Vec<u32>>]) -> Vec<Segment> {
    blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .map(|b| Segment {
            text: b["text"].as_str().unwrap_or_default().to_string(),
            citations: b["citations"]
                .as_array()
                .map(|cs| cs.iter().map(|c| citation_view(c, doc_pages)).collect())
                .unwrap_or_default(),
        })
        .collect()
}

/// Po przełączeniu na model zapasowy w trakcie odpowiedzi API wymaga pominięcia bloków
/// myślenia i narzędzi sprzed ostatniego znacznika `fallback`. Sam znacznik też pomijamy.
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

    // --- Wczytanie Rozmowy, Dokumentu i historii -------------------------------------------
    let (conv, doc_name, mode, history, pdf_path, user_content, user_pages, user_display) = {
        let conn = state.db.lock().unwrap();
        let conv = db::conversation(&conn, conversation_id)
            .map_err(|e| e.to_string())?
            .ok_or("Nie znaleziono Rozmowy.")?;
        let (doc_name, size, page_count, indexed): (String, i64, Option<i64>, i64) = conn
            .query_row(
                "SELECT name, size, page_count, indexed FROM documents WHERE hash = ?1",
                params![conv.document_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| e.to_string())?;

        let mode = match &conv.mode {
            Some(m) => m.clone(),
            None => {
                let full = size as u64 <= FULL_MODE_MAX_BYTES && page_count.map_or(true, |p| p <= FULL_MODE_MAX_PAGES);
                if !full && indexed == 0 {
                    return Err("Dokument jest duży i wciąż jest indeksowany. Spróbuj za chwilę.".into());
                }
                let m = if full { "full" } else { "pages" }.to_string();
                conn.execute("UPDATE conversations SET mode = ?1 WHERE id = ?2", params![m, conversation_id])
                    .map_err(|e| e.to_string())?;
                m
            }
        };

        let mut stmt = conn
            .prepare("SELECT role, content_json, doc_pages_json FROM messages WHERE conversation_id = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let history: Vec<StoredMessage> = stmt
            .query_map(params![conversation_id], |r| {
                let content: String = r.get(1)?;
                let pages: Option<String> = r.get(2)?;
                Ok(StoredMessage {
                    role: r.get(0)?,
                    content: serde_json::from_str(&content).unwrap_or(Value::Null),
                    doc_pages: pages.and_then(|p| serde_json::from_str(&p).ok()),
                })
            })
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();

        let pdf_path = if mode == "full" { state.path_for(&conn, &conv.document_hash) } else { None };
        if mode == "full" && pdf_path.is_none() {
            return Err("Nie można znaleźć pliku Dokumentu na dysku. Otwórz go ponownie.".into());
        }

        let prompt = user_text(&text, &selection);
        let mut content = Vec::new();
        let mut user_pages = None;
        if mode == "pages" {
            let query = format!("{} {}", text, selection.as_ref().map(|s| s.text.as_str()).unwrap_or(""));
            let pages = retrieve_pages(&conn, &conv.document_hash, &query, selection.as_ref().map(|s| s.page))?;
            let blocks: Vec<Value> = pages
                .iter()
                .map(|(n, t)| json!({"type": "text", "text": format!("[Strona {n}]\n{}", truncate_chars(t, MAX_PAGE_CHARS))}))
                .collect();
            content.push(json!({
                "type": "document",
                "source": {"type": "content", "content": blocks},
                "title": format!("{doc_name} (wybrane strony)"),
                "citations": {"enabled": true}
            }));
            user_pages = Some(pages.iter().map(|(n, _)| *n).collect::<Vec<u32>>());
        }
        content.push(json!({"type": "text", "text": prompt}));

        let display = json!({
            "text": text.trim(),
            "selection": selection.as_ref().map(|s| json!({"page": s.page, "text": s.text})),
            "action": action,
        });
        (conv, doc_name, mode, history, pdf_path, Value::Array(content), user_pages, display)
    };

    // --- Budowa żądania ----------------------------------------------------------------------
    let pdf_b64 = match &pdf_path {
        Some(p) => {
            let bytes = tokio::fs::read(p).await.map_err(|e| format!("Nie można odczytać pliku: {e}"))?;
            Some(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        None => None,
    };

    let mut messages: Vec<Value> = Vec::new();
    let mut doc_pages: Vec<Option<Vec<u32>>> = Vec::new();
    let all = history
        .iter()
        .map(|m| (m.role.as_str(), &m.content, &m.doc_pages))
        .chain(std::iter::once(("user", &user_content, &user_pages)));
    for (i, (role, content, pages)) in all.enumerate() {
        let mut blocks = content.as_array().cloned().unwrap_or_default();
        if i == 0 {
            if let Some(b64) = &pdf_b64 {
                blocks.insert(
                    0,
                    json!({
                        "type": "document",
                        "source": {"type": "base64", "media_type": "application/pdf", "data": b64},
                        "title": doc_name,
                        "citations": {"enabled": true}
                    }),
                );
                doc_pages.push(None);
            }
        }
        if role == "user" && mode == "pages" {
            doc_pages.push(pages.clone());
        }
        messages.push(json!({"role": role, "content": blocks}));
    }

    let model = conv.model.clone();
    let has_fallback = !model.starts_with("claude-haiku");
    let mut body = json!({
        "model": model,
        "max_tokens": 32000,
        "stream": true,
        "system": SYSTEM_PROMPT,
        "messages": messages,
        "cache_control": {"type": "ephemeral"},
        "output_config": {"effort": "medium"},
    });
    if has_fallback {
        body["fallbacks"] = json!("default");
    }

    let mut req = state
        .http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    if has_fallback {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("Błąd połączenia z Claude API: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(match status.as_u16() {
            401 => "Nieprawidłowy klucz API. Sprawdź go w Ustawieniach.".into(),
            429 => format!("Przekroczony limit zapytań. Spróbuj za chwilę. ({msg})"),
            529 | 503 => "Claude API jest chwilowo przeciążone. Spróbuj ponownie.".into(),
            _ => format!("Claude API zwróciło błąd {status}: {msg}"),
        });
    }

    // --- Streaming ---------------------------------------------------------------------------
    let mut blocks: Vec<Option<Value>> = Vec::new();
    let mut usage = Usage::default();
    let mut served_model = model.clone();
    let mut stop_reason: Option<String> = None;
    let mut stream_error: Option<String> = None;
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    let mut cancelled = false;

    'outer: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| format!("Przerwane połączenie: {e}"))?;
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
                            served_model = s.to_string();
                        }
                        let u = &m["usage"];
                        usage.input = u["input_tokens"].as_i64().unwrap_or(0);
                        usage.cache_read = u["cache_read_input_tokens"].as_i64().unwrap_or(0);
                        usage.cache_write = u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
                        usage.output = u["output_tokens"].as_i64().unwrap_or(0);
                        let _ = channel.send(ChatEvent::Started { model: served_model.clone() });
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
                                let t = d["text"].as_str().unwrap_or_default();
                                let cur = block["text"].as_str().unwrap_or_default().to_string();
                                block["text"] = Value::String(cur + t);
                                let _ = channel.send(ChatEvent::TextDelta { index: idx, text: t.to_string() });
                            }
                            "citations_delta" => {
                                let c = d["citation"].clone();
                                let view = citation_view(&c, &doc_pages);
                                if !block["citations"].is_array() {
                                    block["citations"] = json!([]);
                                }
                                block["citations"].as_array_mut().unwrap().push(c);
                                let _ = channel.send(ChatEvent::Citation { index: idx, page: view.page, cited_text: view.cited_text });
                            }
                            "thinking_delta" => {
                                let cur = block["thinking"].as_str().unwrap_or_default().to_string();
                                block["thinking"] = Value::String(cur + d["thinking"].as_str().unwrap_or_default());
                            }
                            "signature_delta" => {
                                block["signature"] = d["signature"].clone();
                            }
                            _ => {}
                        }
                    }
                    "message_delta" => {
                        if let Some(s) = ev["delta"]["stop_reason"].as_str() {
                            stop_reason = Some(s.to_string());
                        }
                        let u = &ev["usage"];
                        if let Some(o) = u["output_tokens"].as_i64() {
                            usage.output = o;
                        }
                        if let Some(i) = u["input_tokens"].as_i64() {
                            usage.input = i;
                        }
                        if let Some(r) = u["cache_read_input_tokens"].as_i64() {
                            usage.cache_read = r;
                        }
                        if let Some(w) = u["cache_creation_input_tokens"].as_i64() {
                            usage.cache_write = w;
                        }
                    }
                    "error" => {
                        stream_error = Some(ev["error"]["message"].as_str().unwrap_or("Nieznany błąd").to_string());
                        break 'outer;
                    }
                    _ => {}
                }
            }
        }
    }

    // --- Zapis -------------------------------------------------------------------------------
    let mut blocks: Vec<Value> = blocks.into_iter().flatten().collect();
    if cancelled || stream_error.is_some() {
        // Niedokończona odpowiedź: zostawiamy tylko tekst (bloki myślenia bez podpisu są nieważne).
        blocks.retain(|b| b["type"] == "text" && !b["text"].as_str().unwrap_or_default().is_empty());
        for b in blocks.iter_mut() {
            if let Some(o) = b.as_object_mut() {
                o.remove("citations");
            }
        }
        if cancelled {
            stop_reason = Some("cancelled".into());
        }
    }
    let replay = sanitize_for_replay(blocks.clone());
    let segs = segments(&blocks, &doc_pages);
    let has_text = segs.iter().any(|s| !s.text.trim().is_empty());

    let cost_usd = cost(&served_model, usage);
    let conv_info = {
        let conn = state.db.lock().unwrap();
        let now = db::now();
        if has_text && !replay.is_empty() {
            conn.execute(
                "INSERT INTO messages(conversation_id, role, content_json, display_json, doc_pages_json, created_at) VALUES (?1, 'user', ?2, ?3, ?4, ?5)",
                params![
                    conversation_id,
                    user_content.to_string(),
                    user_display.to_string(),
                    user_pages.as_ref().map(|p| serde_json::to_string(p).unwrap()),
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
            let display = json!({"segments": segs, "stopReason": stop_reason, "model": served_model});
            conn.execute(
                "INSERT INTO messages(conversation_id, role, content_json, display_json, created_at) VALUES (?1, 'assistant', ?2, ?3, ?4)",
                params![conversation_id, Value::Array(replay).to_string(), display.to_string(), now + 1],
            )
            .map_err(|e| e.to_string())?;
            if history.is_empty() {
                let title = truncate_chars(action.as_deref().unwrap_or(text.trim()), 60);
                conn.execute("UPDATE conversations SET title = ?1 WHERE id = ?2", params![title, conversation_id])
                    .map_err(|e| e.to_string())?;
            }
        }
        conn.execute(
            "UPDATE conversations SET updated_at = ?1, input_tokens = input_tokens + ?2, output_tokens = output_tokens + ?3, \
             cache_read_tokens = cache_read_tokens + ?4, cache_write_tokens = cache_write_tokens + ?5, cost_usd = cost_usd + ?6 WHERE id = ?7",
            params![now, usage.input, usage.output, usage.cache_read, usage.cache_write, cost_usd, conversation_id],
        )
        .map_err(|e| e.to_string())?;
        db::conversation(&conn, conversation_id).map_err(|e| e.to_string())?
    };

    if let Some(e) = stream_error {
        return Err(format!("Odpowiedź przerwana: {e}"));
    }
    let _ = channel.send(ChatEvent::Done { stop_reason, conversation: conv_info });
    Ok(())
}
