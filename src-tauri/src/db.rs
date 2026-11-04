//! Lokalna baza SQLite: Dokumenty, ich ścieżki i Wersje, ostatnio otwierane, Przestrzenie robocze,
//! Rozmowy, wiadomości i indeks stron.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

const SCHEMA_V1: &str = r#"
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

/// v2: Rozmowy o wielu Dokumentach (Przestrzenie robocze), Wersje z edycji, kontekst kontynuacji.
const MIGRATION_V2: &str = r#"
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE workspaces (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE workspace_documents (
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    hash         TEXT NOT NULL REFERENCES documents(hash) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, hash)
);
CREATE TABLE conversations_v2 (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    document_hash      TEXT REFERENCES documents(hash) ON DELETE CASCADE,
    workspace_id       INTEGER REFERENCES workspaces(id) ON DELETE CASCADE,
    title              TEXT NOT NULL,
    model              TEXT NOT NULL,
    mode               TEXT,
    context_note       TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd           REAL NOT NULL DEFAULT 0
);
INSERT INTO conversations_v2 (id, document_hash, title, model, mode, created_at, updated_at,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd)
SELECT id, document_hash, title, model, mode, created_at, updated_at,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd FROM conversations;
DROP TABLE conversations;
ALTER TABLE conversations_v2 RENAME TO conversations;
CREATE TABLE conversation_documents (
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    hash            TEXT NOT NULL REFERENCES documents(hash) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, hash)
);
INSERT INTO conversation_documents (conversation_id, hash, position)
SELECT id, document_hash, 0 FROM conversations WHERE document_hash IS NOT NULL;
CREATE TABLE document_versions (
    hash        TEXT PRIMARY KEY REFERENCES documents(hash) ON DELETE CASCADE,
    parent_hash TEXT NOT NULL,
    edited_at   INTEGER NOT NULL
);
PRAGMA user_version = 2;
COMMIT;
PRAGMA foreign_keys = ON;
"#;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA_V1)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 2 {
        conn.execute_batch(MIGRATION_V2)?;
    }
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
    pub document_hash: Option<String>,
    pub workspace_id: Option<i64>,
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
    /// Dla Rozmów o Wersji z edycji: kiedy użytkownik zapisał zmiany, które ją zastąpiły.
    pub replaced_by_edit_at: Option<i64>,
}

const CONV_COLS: &str = "id, document_hash, workspace_id, title, model, mode, created_at, updated_at, \
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd";

fn conv_from_row(row: &rusqlite::Row, read_only: bool) -> rusqlite::Result<ConversationInfo> {
    Ok(ConversationInfo {
        id: row.get(0)?,
        document_hash: row.get(1)?,
        workspace_id: row.get(2)?,
        title: row.get(3)?,
        model: row.get(4)?,
        mode: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        input_tokens: row.get(8)?,
        output_tokens: row.get(9)?,
        cache_read_tokens: row.get(10)?,
        cache_write_tokens: row.get(11)?,
        cost_usd: row.get(12)?,
        read_only,
        replaced_by_edit_at: None,
    })
}

pub fn conversations_for(conn: &Connection, hash: &str, read_only: bool) -> rusqlite::Result<Vec<ConversationInfo>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CONV_COLS} FROM conversations WHERE document_hash = ?1 AND workspace_id IS NULL ORDER BY updated_at DESC"
    ))?;
    let rows = stmt.query_map(params![hash], |r| conv_from_row(r, read_only))?;
    rows.collect()
}

pub fn conversations_for_workspace(conn: &Connection, workspace_id: i64) -> rusqlite::Result<Vec<ConversationInfo>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CONV_COLS} FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at DESC"
    ))?;
    let rows = stmt.query_map(params![workspace_id], |r| conv_from_row(r, false))?;
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

/// Skróty Dokumentów Rozmowy w ustalonej kolejności.
pub fn conversation_documents(conn: &Connection, id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT hash FROM conversation_documents WHERE conversation_id = ?1 ORDER BY position")?;
    let rows = stmt.query_map(params![id], |r| r.get(0))?;
    rows.collect()
}

/// Poprzednie wersje: łańcuch Wersji z edycji oraz Dokumenty widziane pod tą samą ścieżką z inną treścią.
/// Zwraca (skrót, kiedy zastąpiony własną edycją).
pub fn previous_versions(conn: &Connection, path: &str, hash: &str) -> rusqlite::Result<Vec<(String, Option<i64>)>> {
    let mut out: Vec<(String, Option<i64>)> = Vec::new();
    let mut cur = hash.to_string();
    for _ in 0..50 {
        let parent: Option<(String, i64)> = conn
            .query_row(
                "SELECT parent_hash, edited_at FROM document_versions WHERE hash = ?1",
                params![cur],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match parent {
            Some((p, at)) if p != hash && !out.iter().any(|(h, _)| *h == p) => {
                out.push((p.clone(), Some(at)));
                cur = p;
            }
            _ => break,
        }
    }
    let mut stmt = conn.prepare("SELECT hash FROM document_paths WHERE path = ?1 AND hash <> ?2 ORDER BY last_seen DESC")?;
    let same_path: Vec<String> = stmt.query_map(params![path, hash], |r| r.get(0))?.flatten().collect();
    for h in same_path {
        if !out.iter().any(|(x, _)| *x == h) {
            out.push((h, None));
        }
    }
    Ok(out)
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
