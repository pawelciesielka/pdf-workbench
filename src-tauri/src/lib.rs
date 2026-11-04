mod chat;
mod db;
mod keys;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, Response};
use tauri::{Emitter, Manager, State};

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: reqwest::Client,
    /// Skrót Dokumentu → ostatnia znana ścieżka w tej sesji.
    pub paths: Mutex<HashMap<String, String>>,
    pub cancels: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl AppState {
    /// Ścieżka do pliku o danej treści: najpierw z tej sesji, potem ostatnia istniejąca z bazy.
    pub fn path_for(&self, conn: &Connection, hash: &str) -> Option<String> {
        if let Some(p) = self.paths.lock().unwrap().get(hash) {
            if std::path::Path::new(p).exists() {
                return Some(p.clone());
            }
        }
        let mut stmt = conn
            .prepare("SELECT path FROM document_paths WHERE hash = ?1 ORDER BY last_seen DESC")
            .ok()?;
        let paths: Vec<String> = stmt.query_map(params![hash], |r| r.get(0)).ok()?.flatten().collect();
        paths.into_iter().find(|p| std::path::Path::new(p).exists())
    }
}

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentInfo {
    hash: String,
    name: String,
    path: String,
    size: u64,
    page_count: Option<i64>,
    indexed: bool,
    conversations: Vec<db::ConversationInfo>,
    previous_conversations: Vec<db::ConversationInfo>,
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Jedna postać ścieżki dla tego samego pliku (ukośniki, wielkość liter dysku, „..”).
fn normalize_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
        })
        .unwrap_or_else(|_| path.replace('/', "\\"))
}

#[tauri::command]
async fn open_document(state: State<'_, AppState>, path: String) -> Res<DocumentInfo> {
    let path = normalize_path(&path);
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("Nie można otworzyć pliku: {e}"))?;
    if !bytes.starts_with(b"%PDF") && !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err("To nie wygląda na plik PDF.".into());
    }
    let hash = hex::encode(Sha256::digest(&bytes));
    let name = file_name(&path);
    let size = bytes.len() as u64;
    let now = db::now();

    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO documents(hash, name, size, first_seen, last_opened) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(hash) DO UPDATE SET name = excluded.name, last_opened = excluded.last_opened",
        params![hash, name, size as i64, now],
    )
    .map_err(err)?;
    conn.execute(
        "INSERT INTO document_paths(path, hash, last_seen) VALUES (?1, ?2, ?3)
         ON CONFLICT(path, hash) DO UPDATE SET last_seen = excluded.last_seen",
        params![path, hash, now],
    )
    .map_err(err)?;
    conn.execute(
        "INSERT INTO recent(path, hash, name, opened_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET hash = excluded.hash, name = excluded.name, opened_at = excluded.opened_at",
        params![path, hash, name, now],
    )
    .map_err(err)?;
    state.paths.lock().unwrap().insert(hash.clone(), path.clone());

    let (page_count, indexed): (Option<i64>, i64) = conn
        .query_row("SELECT page_count, indexed FROM documents WHERE hash = ?1", params![hash], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(err)?;
    let conversations = db::conversations_for(&conn, &hash, false).map_err(err)?;
    let mut previous_conversations = Vec::new();
    for prev in db::previous_versions(&conn, &path, &hash).map_err(err)? {
        previous_conversations.extend(db::conversations_for(&conn, &prev, true).map_err(err)?);
    }
    Ok(DocumentInfo {
        hash,
        name,
        path,
        size,
        page_count,
        indexed: indexed != 0,
        conversations,
        previous_conversations,
    })
}

#[tauri::command]
async fn read_document(path: String) -> Res<Response> {
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("Nie można odczytać pliku: {e}"))?;
    Ok(Response::new(bytes))
}

#[tauri::command]
fn save_page_texts(state: State<'_, AppState>, hash: String, pages: Vec<String>) -> Res<()> {
    let mut conn = state.db.lock().unwrap();
    let indexed: i64 = conn
        .query_row("SELECT indexed FROM documents WHERE hash = ?1", params![hash], |r| r.get(0))
        .optional()
        .map_err(err)?
        .unwrap_or(1);
    if indexed != 0 {
        return Ok(());
    }
    let tx = conn.transaction().map_err(err)?;
    tx.execute("DELETE FROM pages_fts WHERE hash = ?1", params![hash]).map_err(err)?;
    {
        let mut stmt = tx.prepare("INSERT INTO pages_fts(hash, page, text) VALUES (?1, ?2, ?3)").map_err(err)?;
        for (i, text) in pages.iter().enumerate() {
            stmt.execute(params![hash, (i + 1) as i64, text]).map_err(err)?;
        }
    }
    tx.execute(
        "UPDATE documents SET page_count = ?1, indexed = 1 WHERE hash = ?2",
        params![pages.len() as i64, hash],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentItem {
    path: String,
    name: String,
    opened_at: i64,
    exists: bool,
}

#[tauri::command]
fn list_recent(state: State<'_, AppState>) -> Res<Vec<RecentItem>> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT path, name, opened_at FROM recent ORDER BY opened_at DESC LIMIT 15")
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| {
            let path: String = r.get(0)?;
            Ok(RecentItem { exists: std::path::Path::new(&path).exists(), path, name: r.get(1)?, opened_at: r.get(2)? })
        })
        .map_err(err)?;
    Ok(rows.flatten().collect())
}

#[tauri::command]
fn remove_recent(state: State<'_, AppState>, path: String) -> Res<()> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM recent WHERE path = ?1", params![path]).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn create_conversation(state: State<'_, AppState>, hash: String) -> Res<db::ConversationInfo> {
    let conn = state.db.lock().unwrap();
    let model = db::get_setting(&conn, "model").unwrap_or_else(|| chat::DEFAULT_MODEL.to_string());
    let now = db::now();
    conn.execute(
        "INSERT INTO conversations(document_hash, title, model, created_at, updated_at) VALUES (?1, 'Nowa rozmowa', ?2, ?3, ?3)",
        params![hash, model, now],
    )
    .map_err(err)?;
    let id = conn.last_insert_rowid();
    db::conversation(&conn, id).map_err(err)?.ok_or_else(|| "Nie udało się utworzyć Rozmowy.".into())
}

#[tauri::command]
fn delete_conversation(state: State<'_, AppState>, id: i64) -> Res<()> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM conversations WHERE id = ?1", params![id]).map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    id: i64,
    role: String,
    display: Value,
    created_at: i64,
}

#[tauri::command]
fn get_messages(state: State<'_, AppState>, conversation_id: i64) -> Res<Vec<MessageView>> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, role, display_json, created_at FROM messages WHERE conversation_id = ?1 ORDER BY id")
        .map_err(err)?;
    let rows = stmt
        .query_map(params![conversation_id], |r| {
            let display: String = r.get(2)?;
            Ok(MessageView {
                id: r.get(0)?,
                role: r.get(1)?,
                display: serde_json::from_str(&display).unwrap_or(Value::Null),
                created_at: r.get(3)?,
            })
        })
        .map_err(err)?;
    Ok(rows.flatten().collect())
}

#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    conversation_id: i64,
    text: String,
    selection: Option<chat::Selection>,
    action: Option<String>,
    on_event: Channel<chat::ChatEvent>,
) -> Res<()> {
    chat::send_message(&state, conversation_id, text, selection, action, on_event).await
}

#[tauri::command]
fn stop_generation(state: State<'_, AppState>, conversation_id: i64) {
    if let Some(flag) = state.cancels.lock().unwrap().get(&conversation_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    model: String,
    models: Vec<String>,
    api_key_source: Option<String>,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    let conn = state.db.lock().unwrap();
    Settings {
        model: db::get_setting(&conn, "model").unwrap_or_else(|| chat::DEFAULT_MODEL.to_string()),
        models: chat::MODELS.iter().map(|m| m.to_string()).collect(),
        api_key_source: keys::load().map(|(_, s)| s.to_string()),
    }
}

#[tauri::command]
fn set_model(state: State<'_, AppState>, model: String) -> Res<()> {
    if !chat::MODELS.contains(&model.as_str()) {
        return Err("Nieznany model.".into());
    }
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, "model", &model).map_err(err)
}

#[tauri::command]
fn set_api_key(key: String) -> Res<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Klucz API jest pusty.".into());
    }
    keys::store(key)
}

#[tauri::command]
fn clear_api_key() -> Res<()> {
    keys::clear()
}

fn pdf_args(args: &[String]) -> Vec<String> {
    args.iter()
        .skip(1)
        .filter(|a| a.to_lowercase().ends_with(".pdf"))
        .cloned()
        .collect()
}

#[tauri::command]
fn get_initial_files() -> Vec<String> {
    pdf_args(&std::env::args().collect::<Vec<_>>())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let files = pdf_args(&argv);
            if !files.is_empty() {
                let _ = app.emit("open-files", files);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("workbench.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
                http: reqwest::Client::builder().build()?,
                paths: Mutex::new(HashMap::new()),
                cancels: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_document,
            read_document,
            save_page_texts,
            list_recent,
            remove_recent,
            create_conversation,
            delete_conversation,
            get_messages,
            send_message,
            stop_generation,
            get_settings,
            set_model,
            set_api_key,
            clear_api_key,
            get_initial_files
        ])
        .run(tauri::generate_context!())
        .expect("błąd uruchamiania PDF Workbench");
}
