//! Lokalna baza SQLite: Dokumenty, ich ścieżki, ostatnio otwierane, Rozmowy, wiadomości i indeks stron.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS documents (
    hash        TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    page_count  INTEGER,
    indexed     INTEGER NOT NULL DEFAULT 0,
    first_seen  INTEGER NOT NULL,
    last_opened INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS document_paths (
    path      TEXT NOT NULL,
    hash      TEXT NOT NULL REFERENCES documents(hash) ON DELETE CASCADE,
    last_seen INTEGER NOT NULL,
    PRIMARY KEY (path, hash)
);

CREATE TABLE IF NOT EXISTS recent (
    path      TEXT PRIMARY KEY,
    hash      TEXT NOT NULL,
    name      TEXT NOT NULL,
    opened_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    document_hash      TEXT NOT NULL REFERENCES documents(hash) ON DELETE CASCADE,
    title              TEXT NOT NULL,
    model              TEXT NOT NULL,
    mode               TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd           REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content_json    TEXT NOT NULL,
    display_json    TEXT NOT NULL,
    doc_pages_json  TEXT,
    created_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts USING fts5(
    hash UNINDEXED,
    page UNINDEXED,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInfo {
    pub id: i64,
    pub document_hash: String,
    pub title: String,
    pub model: String,
    pub mode: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
    pub read_only: bool,
}

const CONV_COLS: &str = "id, document_hash, title, model, mode, created_at, updated_at, \
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd";

fn conv_from_row(row: &rusqlite::Row, read_only: bool) -> rusqlite::Result<ConversationInfo> {
    Ok(ConversationInfo {
        id: row.get(0)?,
        document_hash: row.get(1)?,
        title: row.get(2)?,
        model: row.get(3)?,
        mode: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        input_tokens: row.get(7)?,
        output_tokens: row.get(8)?,
        cache_read_tokens: row.get(9)?,
        cache_write_tokens: row.get(10)?,
        cost_usd: row.get(11)?,
        read_only,
    })
}

pub fn conversations_for(conn: &Connection, hash: &str, read_only: bool) -> rusqlite::Result<Vec<ConversationInfo>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CONV_COLS} FROM conversations WHERE document_hash = ?1 ORDER BY updated_at DESC"
    ))?;
    let rows = stmt.query_map(params![hash], |r| conv_from_row(r, read_only))?;
    rows.collect()
}

pub fn conversation(conn: &Connection, id: i64) -> rusqlite::Result<Option<ConversationInfo>> {
    conn.query_row(
        &format!("SELECT {CONV_COLS} FROM conversations WHERE id = ?1"),
        params![id],
        |r| conv_from_row(r, false),
    )
    .optional()
}

/// Dokumenty widziane wcześniej pod tą samą ścieżką, ale z inną treścią (Poprzednie wersje).
pub fn previous_versions(conn: &Connection, path: &str, hash: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT hash FROM document_paths WHERE path = ?1 AND hash <> ?2 ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map(params![path, hash], |r| r.get::<_, String>(0))?;
    rows.collect()
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
