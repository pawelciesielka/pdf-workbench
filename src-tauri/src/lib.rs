mod chat;
mod db;
mod editor;
mod keys;
mod ocr;
mod pdfops;
mod signing;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, InvokeBody, Request, Response};
use tauri::{Emitter, Manager, State};

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: reqwest::Client,
    /// Skrót Dokumentu → ostatnia znana ścieżka w tej sesji.
    pub paths: Mutex<HashMap<String, String>>,
    pub cancels: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Narzędzia czatu czekające na wynik z UI (id wywołania → odbiorca).
    pub pending_tools: Mutex<HashMap<String, tokio::sync::oneshot::Sender<chat::ToolOutcome>>>,
}

impl AppState {
    /// Ścieżka do pliku o danej treści: najpierw z tej sesji, potem ostatnia istniejąca z bazy.
    pub fn path_for(&self, conn: &Connection, hash: &str) -> Option<String> {
        if let Some(p) = self.paths.lock().unwrap().get(hash) {
            if std::path::Path::new(p).exists() {
                return Some(p.clone());
            }
        }
        let mut stmt = conn.prepare("SELECT path FROM document_paths WHERE hash = ?1 ORDER BY last_seen DESC").ok()?;
        let paths: Vec<String> = stmt.query_map(params![hash], |r| r.get(0)).ok()?.flatten().collect();
        paths.into_iter().find(|p| std::path::Path::new(p).exists())
    }
}

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// --- Dokumenty ---------------------------------------------------------------------------------

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
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
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

/// Rejestruje plik (treść + ścieżka) i zwraca informacje o Dokumencie.
fn register_document(state: &AppState, path: &str, bytes: &[u8], parent: Option<&str>) -> Res<DocumentInfo> {
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err("To nie wygląda na plik PDF.".into());
    }
    let hash = hex::encode(Sha256::digest(bytes));
    let name = file_name(path);
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
    if let Some(p) = parent.filter(|p| *p != hash) {
        conn.execute(
            "INSERT INTO document_versions(hash, parent_hash, edited_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(hash) DO UPDATE SET parent_hash = excluded.parent_hash, edited_at = excluded.edited_at",
            params![hash, p, now],
        )
        .map_err(err)?;
    }
    state.paths.lock().unwrap().insert(hash.clone(), path.to_string());

    let (page_count, indexed): (Option<i64>, i64) = conn
        .query_row("SELECT page_count, indexed FROM documents WHERE hash = ?1", params![hash], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?;
    let conversations = db::conversations_for(&conn, &hash, false).map_err(err)?;
    let mut previous_conversations = Vec::new();
    for (prev, edited_at) in db::previous_versions(&conn, path, &hash).map_err(err)? {
        for mut c in db::conversations_for(&conn, &prev, true).map_err(err)? {
            c.replaced_by_edit_at = edited_at;
            previous_conversations.push(c);
        }
    }
    Ok(DocumentInfo { hash, name, path: path.to_string(), size, page_count, indexed: indexed != 0, conversations, previous_conversations })
}

#[tauri::command]
async fn open_document(state: State<'_, AppState>, path: String) -> Res<DocumentInfo> {
    let path = normalize_path(&path);
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("Nie można otworzyć pliku: {e}"))?;
    register_document(&state, &path, &bytes, None)
}

#[tauri::command]
async fn read_document(path: String) -> Res<Response> {
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("Nie można odczytać pliku: {e}"))?;
    Ok(Response::new(bytes))
}

fn raw_body(request: &Request<'_>) -> Res<Vec<u8>> {
    match request.body() {
        InvokeBody::Raw(b) => Ok(b.clone()),
        _ => Err("Oczekiwano danych binarnych.".into()),
    }
}

/// Parametry przekazywane w nagłówku `x-meta` jako JSON zakodowany przez encodeURIComponent.
fn meta<T: for<'de> Deserialize<'de>>(request: &Request<'_>) -> Res<T> {
    let raw = request.headers().get("x-meta").and_then(|v| v.to_str().ok()).ok_or("Brak parametrów.")?;
    let decoded = urlencoding::decode(raw).map_err(err)?;
    serde_json::from_str(&decoded).map_err(err)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveMeta {
    path: String,
    parent_hash: Option<String>,
    backup: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveResult {
    document: DocumentInfo,
    backup_path: Option<String>,
}

/// Zapis Dokumentu (Ctrl+S / Zapisz jako). Przy pierwszym nadpisaniu robi kopię `nazwa.bak.pdf`.
#[tauri::command]
async fn save_document(state: State<'_, AppState>, request: Request<'_>) -> Res<SaveResult> {
    let bytes = raw_body(&request)?;
    let m: SaveMeta = meta(&request)?;
    let path = m.path.replace('/', "\\");
    let target = std::path::Path::new(&path);
    let mut backup_path = None;
    if m.backup && target.exists() {
        let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let bak = target.with_file_name(format!("{stem}.bak.pdf"));
        if !bak.exists() {
            std::fs::copy(target, &bak).map_err(|e| format!("Nie można utworzyć kopii zapasowej: {e}"))?;
            backup_path = Some(bak.to_string_lossy().to_string());
        }
    }
    let tmp = target.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("Nie można zapisać pliku: {e}"))?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Nie można zastąpić pliku (może jest otwarty w innym programie?): {e}")
    })?;
    let path = normalize_path(&path);
    let document = register_document(&state, &path, &bytes, m.parent_hash.as_deref())?;
    Ok(SaveResult { document, backup_path })
}

#[derive(Deserialize)]
struct RotateMeta {
    pages: Vec<u32>,
    degrees: i64,
}

#[tauri::command]
async fn rotate_pages(request: Request<'_>) -> Res<Response> {
    let bytes = raw_body(&request)?;
    let m: RotateMeta = meta(&request)?;
    let out = tokio::task::spawn_blocking(move || pdfops::rotate_pages(&bytes, &m.pages, m.degrees)).await.map_err(err)??;
    Ok(Response::new(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RasterMeta {
    pdf_len: usize,
    items: Vec<pdfops::RasterPage>,
}

/// Body: [PDF][JPEG 1][JPEG 2]…; offsety obrazów liczone od końca PDF.
#[tauri::command]
async fn rasterize_pages(request: Request<'_>) -> Res<Response> {
    let body = raw_body(&request)?;
    let m: RasterMeta = meta(&request)?;
    let out = tokio::task::spawn_blocking(move || {
        let (pdf, images) = body.split_at(m.pdf_len.min(body.len()));
        pdfops::rasterize_pages(pdf, images, &m.items)
    })
    .await
    .map_err(err)??;
    Ok(Response::new(out))
}

#[tauri::command]
async fn add_text_layer(request: Request<'_>) -> Res<Response> {
    let bytes = raw_body(&request)?;
    let pages: Vec<pdfops::PageText> = meta(&request)?;
    let out = tokio::task::spawn_blocking(move || pdfops::add_text_layer(&bytes, &pages)).await.map_err(err)??;
    Ok(Response::new(out))
}

#[derive(Deserialize)]
struct ObjectsMeta {
    page: u16,
}

#[tauri::command]
async fn page_objects(request: Request<'_>) -> Res<Vec<editor::PageObject>> {
    let bytes = raw_body(&request)?;
    let m: ObjectsMeta = meta(&request)?;
    tokio::task::spawn_blocking(move || editor::page_objects(&bytes, m.page)).await.map_err(err)?
}

#[tauri::command]
async fn edit_objects(request: Request<'_>) -> Res<Response> {
    let bytes = raw_body(&request)?;
    let ops: Vec<editor::EditOp> = meta(&request)?;
    let (out, report) = tokio::task::spawn_blocking(move || editor::edit_objects(&bytes, ops)).await.map_err(err)??;
    Ok(Response::new(editor::pack(out, &report)))
}

#[derive(Deserialize)]
struct ReplaceMeta {
    #[serde(default)]
    pages: Vec<u16>,
    find: String,
    replace: String,
    within: Option<[f32; 4]>,
}

#[tauri::command]
async fn replace_text(request: Request<'_>) -> Res<Response> {
    let bytes = raw_body(&request)?;
    let m: ReplaceMeta = meta(&request)?;
    let (out, report) =
        tokio::task::spawn_blocking(move || editor::replace_text(&bytes, &m.pages, &m.find, &m.replace, m.within)).await.map_err(err)??;
    Ok(Response::new(editor::pack(out, &report)))
}

/// Podpisy elektroniczne w pliku (puste, gdy brak) z oceną integralności.
#[tauri::command]
async fn pdf_signatures(request: Request<'_>) -> Res<Vec<signing::SignatureInfo>> {
    let bytes = raw_body(&request)?;
    tokio::task::spawn_blocking(move || signing::inspect(&bytes)).await.map_err(err)
}

/// Certyfikaty z kluczem prywatnym w magazynie Windows (np. z SimplySign Desktop).
#[tauri::command]
async fn signing_certs() -> Res<Vec<signing::SigningCert>> {
    tokio::task::spawn_blocking(signing::list_certs).await.map_err(err)?
}

#[tauri::command]
async fn sign_pdf(request: Request<'_>) -> Res<Response> {
    let bytes = raw_body(&request)?;
    let o: signing::SignOptions = meta(&request)?;
    let (out, report) = tokio::task::spawn_blocking(move || signing::sign_pdf(&bytes, &o)).await.map_err(err)??;
    Ok(Response::new(signing::pack(out, &report)))
}

/// Zaufanie do podpisów w pliku: łańcuch, unieważnienie, unijna lista zaufanych (pobierana i
/// przechowywana w danych aplikacji), zaufany czas.
#[tauri::command]
async fn signature_trust(app: tauri::AppHandle, state: State<'_, AppState>, request: Request<'_>) -> Res<Vec<signing::TrustInfo>> {
    let bytes = raw_body(&request)?;
    let probe = bytes.clone();
    let countries = tokio::task::spawn_blocking(move || signing::issuer_countries(&probe)).await.map_err(err)?;
    let dir = app.path().app_data_dir().map_err(err)?.join("tsl");
    let mut lists = HashMap::new();
    let mut errors = Vec::new();
    for cc in countries {
        match signing::trust::tl_services(&state.http, &dir, &cc).await {
            Ok(s) => {
                lists.insert(cc, s);
            }
            Err(e) => errors.push(format!("Nie udało się sprawdzić unijnej listy zaufanych ({cc}): {e}.")),
        }
    }
    tokio::task::spawn_blocking(move || signing::check_trust(&bytes, &lists, &errors)).await.map_err(err)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BatchProgress {
    index: usize,
    item: signing::BatchItem,
}

/// Podpisanie wielu plików jednym certyfikatem; postęp na kanale po każdym pliku.
#[tauri::command]
async fn sign_files(options: signing::BatchOptions, on_progress: Channel<BatchProgress>) -> Res<Vec<signing::BatchItem>> {
    tokio::task::spawn_blocking(move || {
        signing::sign_files(&options, |index, item| {
            let _ = on_progress.send(BatchProgress { index, item: item.clone() });
        })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn ocr_image(request: Request<'_>) -> Res<ocr::OcrResult> {
    let bytes = raw_body(&request)?;
    tokio::task::spawn_blocking(move || ocr::recognize(&bytes, "pl")).await.map_err(err)?
}

#[tauri::command]
fn save_page_texts(state: State<'_, AppState>, hash: String, pages: Vec<String>, force: Option<bool>) -> Res<()> {
    let mut conn = state.db.lock().unwrap();
    let indexed: i64 = conn
        .query_row("SELECT indexed FROM documents WHERE hash = ?1", params![hash], |r| r.get(0))
        .optional()
        .map_err(err)?
        .unwrap_or(1);
    if indexed != 0 && !force.unwrap_or(false) {
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
    tx.execute("UPDATE documents SET page_count = ?1, indexed = 1 WHERE hash = ?2", params![pages.len() as i64, hash]).map_err(err)?;
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
    let mut stmt = conn.prepare("SELECT path, name, opened_at FROM recent ORDER BY opened_at DESC LIMIT 15").map_err(err)?;
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

// --- Rozmowy -----------------------------------------------------------------------------------

fn insert_conversation(conn: &Connection, hashes: &[String], workspace_id: Option<i64>, title: &str, note: Option<&str>) -> Res<db::ConversationInfo> {
    let model = db::get_setting(conn, "model").unwrap_or_else(|| chat::DEFAULT_MODEL.to_string());
    let now = db::now();
    let doc_hash = if workspace_id.is_none() { hashes.first().cloned() } else { None };
    conn.execute(
        "INSERT INTO conversations(document_hash, workspace_id, title, model, context_note, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![doc_hash, workspace_id, title, model, note, now],
    )
    .map_err(err)?;
    let id = conn.last_insert_rowid();
    for (i, h) in hashes.iter().enumerate() {
        conn.execute("INSERT INTO conversation_documents(conversation_id, hash, position) VALUES (?1, ?2, ?3)", params![id, h, i as i64]).map_err(err)?;
    }
    db::conversation(conn, id).map_err(err)?.ok_or_else(|| "Nie udało się utworzyć Rozmowy.".into())
}

#[tauri::command]
fn create_conversation(state: State<'_, AppState>, hash: Option<String>, workspace_id: Option<i64>) -> Res<db::ConversationInfo> {
    let conn = state.db.lock().unwrap();
    let hashes = match (hash, workspace_id) {
        (_, Some(ws)) => {
            let mut stmt = conn.prepare("SELECT hash FROM workspace_documents WHERE workspace_id = ?1 ORDER BY position").map_err(err)?;
            let v: Vec<String> = stmt.query_map(params![ws], |r| r.get(0)).map_err(err)?.flatten().collect();
            v
        }
        (Some(h), None) => vec![h],
        _ => return Err("Brak Dokumentu.".into()),
    };
    if hashes.is_empty() {
        return Err("Przestrzeń robocza nie ma Dokumentów.".into());
    }
    insert_conversation(&conn, &hashes, workspace_id, "Nowa rozmowa", None)
}

/// „Kontynuuj w nowej Rozmowie”: nowa Rozmowa o bieżącym Dokumencie z krótkim zapisem starej jako kontekstem.
#[tauri::command]
fn continue_conversation(state: State<'_, AppState>, old_id: i64, hash: String) -> Res<db::ConversationInfo> {
    let conn = state.db.lock().unwrap();
    let old = db::conversation(&conn, old_id).map_err(err)?.ok_or("Nie znaleziono Rozmowy.")?;
    let note = chat::summarize_for_continuation(&conn, old_id)?;
    insert_conversation(&conn, &[hash], None, &format!("Kontynuacja: {}", old.title), Some(&note))
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
            Ok(MessageView { id: r.get(0)?, role: r.get(1)?, display: serde_json::from_str(&display).unwrap_or(Value::Null), created_at: r.get(3)? })
        })
        .map_err(err)?;
    Ok(rows.flatten().filter(|m| m.display["hidden"] != true).collect())
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
fn tool_result(state: State<'_, AppState>, id: String, ok: bool, text: String) {
    if let Some(tx) = state.pending_tools.lock().unwrap().remove(&id) {
        let _ = tx.send(chat::ToolOutcome { ok, text });
    }
}

#[tauri::command]
fn stop_generation(state: State<'_, AppState>, conversation_id: i64) {
    if let Some(flag) = state.cancels.lock().unwrap().get(&conversation_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

// --- Przestrzenie robocze ----------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceDoc {
    hash: String,
    name: String,
    path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceInfo {
    id: i64,
    name: String,
    updated_at: i64,
    documents: Vec<WorkspaceDoc>,
    conversations: Vec<db::ConversationInfo>,
}

fn workspace_info(state: &AppState, conn: &Connection, id: i64) -> Res<WorkspaceInfo> {
    let (name, updated_at): (String, i64) =
        conn.query_row("SELECT name, updated_at FROM workspaces WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
    let mut stmt = conn
        .prepare("SELECT d.hash, d.name FROM workspace_documents w JOIN documents d ON d.hash = w.hash WHERE w.workspace_id = ?1 ORDER BY w.position")
        .map_err(err)?;
    let docs: Vec<(String, String)> = stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.flatten().collect();
    let documents = docs.into_iter().map(|(hash, name)| WorkspaceDoc { path: state.path_for(conn, &hash), hash, name }).collect();
    let conversations = db::conversations_for_workspace(conn, id).map_err(err)?;
    Ok(WorkspaceInfo { id, name, updated_at, documents, conversations })
}

#[tauri::command]
fn list_workspaces(state: State<'_, AppState>) -> Res<Vec<WorkspaceInfo>> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn.prepare("SELECT id FROM workspaces ORDER BY updated_at DESC").map_err(err)?;
    let ids: Vec<i64> = stmt.query_map([], |r| r.get(0)).map_err(err)?.flatten().collect();
    ids.into_iter().map(|id| workspace_info(&state, &conn, id)).collect()
}

#[tauri::command]
fn save_workspace(state: State<'_, AppState>, id: Option<i64>, name: String, hashes: Vec<String>) -> Res<WorkspaceInfo> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Podaj nazwę Przestrzeni roboczej.".into());
    }
    if hashes.is_empty() {
        return Err("Wybierz co najmniej jeden Dokument.".into());
    }
    let mut conn = state.db.lock().unwrap();
    let now = db::now();
    let tx = conn.transaction().map_err(err)?;
    let id = match id {
        Some(id) => {
            tx.execute("UPDATE workspaces SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name, now, id]).map_err(err)?;
            tx.execute("DELETE FROM workspace_documents WHERE workspace_id = ?1", params![id]).map_err(err)?;
            id
        }
        None => {
            tx.execute("INSERT INTO workspaces(name, created_at, updated_at) VALUES (?1, ?2, ?2)", params![name, now]).map_err(err)?;
            tx.last_insert_rowid()
        }
    };
    for (i, h) in hashes.iter().enumerate() {
        tx.execute("INSERT OR IGNORE INTO workspace_documents(workspace_id, hash, position) VALUES (?1, ?2, ?3)", params![id, h, i as i64]).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    workspace_info(&state, &conn, id)
}

#[tauri::command]
fn delete_workspace(state: State<'_, AppState>, id: i64) -> Res<()> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM workspaces WHERE id = ?1", params![id]).map_err(err)?;
    Ok(())
}

// --- Ustawienia --------------------------------------------------------------------------------

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
    args.iter().skip(1).filter(|a| a.to_lowercase().ends_with(".pdf")).cloned().collect()
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("workbench.db"))?;
            // pdfium.dll: obok exe (instalator), w zasobach albo w katalogu źródeł (dev).
            let mut dirs = vec![];
            if let Ok(exe) = std::env::current_exe() {
                if let Some(d) = exe.parent() {
                    dirs.push(d.to_path_buf());
                }
            }
            if let Ok(r) = app.path().resource_dir() {
                dirs.push(r.join("pdfium"));
                dirs.push(r);
            }
            dirs.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pdfium"));
            std::thread::spawn(move || editor::init(&dirs));
            app.manage(AppState {
                db: Mutex::new(conn),
                http: reqwest::Client::builder().build()?,
                paths: Mutex::new(HashMap::new()),
                cancels: Mutex::new(HashMap::new()),
                pending_tools: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_document,
            read_document,
            save_document,
            rotate_pages,
            rasterize_pages,
            add_text_layer,
            page_objects,
            edit_objects,
            replace_text,
            pdf_signatures,
            signing_certs,
            sign_pdf,
            signature_trust,
            sign_files,
            ocr_image,
            save_page_texts,
            list_recent,
            remove_recent,
            create_conversation,
            continue_conversation,
            delete_conversation,
            get_messages,
            send_message,
            tool_result,
            stop_generation,
            list_workspaces,
            save_workspace,
            delete_workspace,
            get_settings,
            set_model,
            set_api_key,
            clear_api_key,
            get_initial_files
        ])
        .run(tauri::generate_context!())
        .expect("błąd uruchamiania PDF Workbench");
}
