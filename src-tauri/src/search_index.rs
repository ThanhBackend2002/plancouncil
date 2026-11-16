//! Index SQLite (`index.db`) cho lịch sử phiên: tìm kiếm full-text (FTS5) + liệt kê nhanh.
//!
//! File JSON trong `sessions/` vẫn là nguồn sự thật. Index chỉ là bản sao **dựng lại được**:
//! hỏng hoặc khác `SCHEMA_VERSION` → xoá `index.db` (+ `-wal`, `-shm`) rồi tạo mới, sau đó
//! `sync_dir` đọc lại JSON. Không migration, không rollback.
//!
//! Mọi hàm ở đây là **blocking** — caller gọi qua `spawn_blocking`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, ErrorCode};
use serde::Serialize;
use uuid::Uuid;

use crate::models::{SessionRecord, SessionStatus, SessionSummary, MAIN_SCOPE};
use crate::normalize::{fold, fold_query, word_spans};

/// Tăng khi đổi schema / cách dựng dữ liệu → index cũ tự bị xoá và dựng lại.
pub const SCHEMA_VERSION: i32 = 1;
pub const MAX_SEARCH_LIMIT: u32 = 50;
/// Độ dài (ký tự) xấp xỉ của đoạn trích quanh chỗ khớp đầu tiên.
const SNIPPET_CHARS: usize = 80;
/// Số ký tự ngữ cảnh đứng TRƯỚC chỗ khớp trong đoạn trích.
const SNIPPET_LEAD: usize = 30;
pub const HIT_OPEN: &str = "[[";
pub const HIT_CLOSE: &str = "]]";

const SCHEMA_SQL: &str = "
CREATE TABLE sessions(
    id           TEXT PRIMARY KEY,
    file_mtime   INTEGER NOT NULL,
    file_size    INTEGER NOT NULL,
    title        TEXT NOT NULL,
    status       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    versions     INTEGER NOT NULL,
    rounds       INTEGER NOT NULL,
    departments  INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL
);
CREATE INDEX sessions_by_created ON sessions(created_at DESC);
CREATE VIRTUAL TABLE search_fts USING fts5(
    heading_norm, content_norm,
    session_id UNINDEXED, scope UNINDEXED, version_no UNINDEXED, kind UNINDEXED,
    section_key UNINDEXED, heading_orig UNINDEXED, content_orig UNINDEXED,
    tokenize='unicode61'
);
-- FTS5 không index được cột UNINDEXED → bảng phụ để xoá theo session không phải quét toàn bộ.
CREATE TABLE search_rows(
    fts_rowid  INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL
);
CREATE INDEX search_rows_by_session ON search_rows(session_id);
";

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("lỗi SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("lỗi I/O: {0}")]
    Io(#[from] io::Error),
    /// Nội bộ: index cần xoá và dựng lại (schema khác / quick_check thất bại).
    #[error("index cần dựng lại: {0}")]
    NeedsRebuild(String),
}

impl IndexError {
    fn needs_rebuild(&self) -> bool {
        match self {
            IndexError::NeedsRebuild(_) => true,
            IndexError::Sqlite(rusqlite::Error::SqliteFailure(e, _)) => {
                matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SearchHit {
    pub session_id: String,
    pub session_title: String,
    /// "main" hoặc id phòng ban.
    pub scope: String,
    /// Plan version của section; 0 với `kind = "request"`.
    pub version_no: u32,
    /// "request" (đề bài) | "section" (1 section `##` của plan mới nhất).
    pub kind: String,
    pub section_key: Option<String>,
    pub heading: Option<String>,
    /// Đoạn trích từ văn bản GỐC (có dấu), chỗ khớp được bọc `[[` … `]]`.
    pub snippet: String,
    /// bm25: càng nhỏ (càng âm) càng liên quan.
    pub rank: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub indexed: usize,
    pub removed: usize,
    /// (tên file hoặc id, lỗi)
    pub failed: Vec<(String, String)>,
}

pub struct SearchIndex {
    conn: Mutex<Connection>,
}

fn lock(m: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// (mtime tính bằng nanosecond kể từ UNIX epoch, size byte) — đơn vị mà `index_session` mong đợi.
pub fn file_stamp(path: &Path) -> io::Result<(i64, i64)> {
    let meta = fs::metadata(path)?;
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Ok((mtime, i64::try_from(meta.len()).unwrap_or(i64::MAX)))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

fn remove_db_files(path: &Path) -> io::Result<()> {
    for p in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
        sidecar(path, "-journal"),
    ] {
        match fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn try_open(path: &Path) -> Result<Connection, IndexError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // File rác → lỗi NotADatabase ngay ở đây.
    let _mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == SCHEMA_VERSION {
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(IndexError::NeedsRebuild(format!("quick_check: {check}")));
        }
        return Ok(conn);
    }
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version != 0 || tables > 0 {
        return Err(IndexError::NeedsRebuild(format!(
            "schema v{version}, cần v{SCHEMA_VERSION}"
        )));
    }
    conn.execute_batch(&format!(
        "BEGIN; {SCHEMA_SQL} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
    ))?;
    Ok(conn)
}

fn status_str(s: SessionStatus) -> String {
    match serde_json::to_value(s) {
        Ok(serde_json::Value::String(v)) => v,
        other => format!("{other:?}"),
    }
}

fn parse_status(s: &str) -> Option<SessionStatus> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

fn ts_to_i64(t: DateTime<Utc>) -> i64 {
    // Ngoài khoảng 1677–2262 mới None; thời điểm tạo phiên luôn nằm trong khoảng này.
    t.timestamp_nanos_opt().unwrap_or(0)
}

// ───────────────────────────── Tách section ─────────────────────────────

#[derive(Debug, PartialEq)]
struct Section {
    /// None = phần mở đầu trước `##` đầu tiên.
    heading: Option<String>,
    body: String,
}

/// Dòng mở/đóng code fence: (ký tự, độ dài, phần còn lại sau dấu fence).
fn fence_marker(t: &str) -> Option<(char, usize, &str)> {
    let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|c| *c == ch).count();
    (n >= 3).then(|| (ch, n, &t[n..]))
}

/// Tách markdown theo tiêu đề `##` (không tính `###`), bỏ qua tiêu đề nằm trong code fence.
/// Dòng tag `<!-- reviewers: … -->` (chỉ có comment) bị bỏ để không làm nhiễu kết quả tìm.
fn split_sections(md: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut cur = Section { heading: None, body: String::new() };
    let mut fence: Option<(char, usize)> = None;

    for line in md.lines() {
        let t = line.trim_start();
        if let Some((ch, n, rest)) = fence_marker(t) {
            match fence {
                None => fence = Some((ch, n)),
                Some((c0, n0)) if ch == c0 && n >= n0 && rest.trim().is_empty() => fence = None,
                Some(_) => {}
            }
        } else if fence.is_none() {
            if let Some(h) = t.strip_prefix("## ") {
                let heading = h.trim().trim_end_matches('#').trim_end().to_owned();
                out.push(std::mem::replace(
                    &mut cur,
                    Section { heading: Some(heading), body: String::new() },
                ));
                continue;
            }
            let lt = t.trim_end();
            if lt.starts_with("<!--") && lt.ends_with("-->") {
                continue;
            }
        }
        cur.body.push_str(line);
        cur.body.push('\n');
    }
    out.push(cur);
    out.retain(|s| s.heading.is_some() || !s.body.trim().is_empty());
    for s in &mut out {
        s.body = s.body.trim().to_owned();
    }
    out
}

/// Khoá ổn định cho section (dạng anchor): "2. Kiến trúc" → "2-kien-truc", duy nhất trong 1 plan.
fn section_key(heading: &str, idx: usize, used: &mut HashSet<String>) -> String {
    let folded: Vec<char> = fold(heading).0.chars().collect();
    let base = word_spans(&folded)
        .into_iter()
        .map(|(a, b)| folded[a..b].iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() { format!("section-{}", idx + 1) } else { base };
    let mut key = base.clone();
    let mut n = 2;
    while !used.insert(key.clone()) {
        key = format!("{base}-{n}");
        n += 1;
    }
    key
}

// ───────────────────────────── Snippet ─────────────────────────────

/// Đoạn trích ~`SNIPPET_CHARS` ký tự từ `orig` quanh chỗ khớp đầu tiên, mọi từ khớp trong
/// cửa sổ được bọc `[[ ]]`. So khớp trên văn bản đã gấp rồi ánh xạ ngược về vị trí gốc.
fn build_snippet(orig: &str, terms: &HashSet<String>) -> String {
    let orig_chars: Vec<char> = orig.chars().collect();
    let (folded, map) = fold(orig);
    let folded: Vec<char> = folded.chars().collect();

    // Vị trí [start, end) trong `orig_chars` của từng từ khớp.
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (a, b) in word_spans(&folded) {
        let word: String = folded[a..b].iter().collect();
        if !terms.contains(&word) {
            continue;
        }
        let start = map[a];
        // Kết thúc = vị trí gốc của ký tự gấp kế tiếp → nuốt luôn dấu NFD rời đứng sau.
        let end = map.get(b).copied().unwrap_or(orig_chars.len()).max(map[b - 1] + 1);
        if hits.last().is_some_and(|&(_, e)| start < e) {
            continue; // 1 ký tự gốc giãn thành nhiều từ — không lồng `[[`.
        }
        hits.push((start, end));
    }

    let (win_start, win_end) = match hits.first() {
        Some(&(s, e)) => {
            let ws = s.saturating_sub(SNIPPET_LEAD);
            (ws, (ws + SNIPPET_CHARS).max(e).min(orig_chars.len()))
        }
        None => (0, SNIPPET_CHARS.min(orig_chars.len())),
    };

    let mut out = String::new();
    if win_start > 0 {
        out.push('…');
    }
    let mut hit_iter = hits.iter().filter(|&&(s, e)| s >= win_start && e <= win_end).peekable();
    let mut open_until: Option<usize> = None;
    let mut last_space = false;
    for (i, &c) in orig_chars.iter().enumerate().take(win_end).skip(win_start) {
        if open_until == Some(i) {
            out.push_str(HIT_CLOSE);
            open_until = None;
        }
        if let Some(&&(s, e)) = hit_iter.peek() {
            if s == i {
                out.push_str(HIT_OPEN);
                open_until = Some(e);
                hit_iter.next();
            }
        }
        // Gộp xuống dòng / khoảng trắng liên tiếp thành 1 dấu cách.
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    if open_until.is_some() {
        out.push_str(HIT_CLOSE);
    }
    let mut out = out.trim().to_owned();
    if win_end < orig_chars.len() {
        out.push('…');
    }
    out
}

/// Các từ (đã gấp) của câu tìm kiếm, cùng cách tách với `build_snippet`.
fn query_terms(query: &str) -> HashSet<String> {
    let folded: Vec<char> = fold(query).0.chars().collect();
    word_spans(&folded)
        .into_iter()
        .map(|(a, b)| folded[a..b].iter().collect())
        .collect()
}

// ───────────────────────────── Index ─────────────────────────────

struct Row<'a> {
    scope: &'a str,
    version_no: u32,
    kind: &'static str,
    section_key: Option<String>,
    heading: Option<String>,
    content: String,
}

/// Hàng FTS của 1 phiên: đề bài + từng section `##` của plan MỚI NHẤT mỗi scope (main + phòng ban).
fn rows_for(rec: &SessionRecord) -> Vec<Row<'_>> {
    let mut rows = vec![Row {
        scope: MAIN_SCOPE,
        version_no: 0,
        kind: "request",
        section_key: None,
        heading: None,
        content: rec.prompt.clone(),
    }];
    let scopes = std::iter::once((MAIN_SCOPE, &rec.plans))
        .chain(rec.departments.iter().map(|d| (d.id.as_str(), &d.plans)));
    for (scope, plans) in scopes {
        let Some(latest) = plans.iter().max_by_key(|p| p.version) else {
            continue;
        };
        let mut used = HashSet::new();
        for (idx, sec) in split_sections(&latest.markdown).into_iter().enumerate() {
            rows.push(Row {
                scope,
                version_no: latest.version,
                kind: "section",
                section_key: sec.heading.as_deref().map(|h| section_key(h, idx, &mut used)),
                heading: sec.heading,
                content: sec.body,
            });
        }
    }
    rows
}

fn insert_rows(tx: &rusqlite::Transaction<'_>, session_id: &str, rows: &[Row<'_>]) -> rusqlite::Result<()> {
    let mut fts = tx.prepare_cached(
        "INSERT INTO search_fts(heading_norm, content_norm, session_id, scope, version_no, kind,
            section_key, heading_orig, content_orig) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    let mut map = tx.prepare_cached("INSERT INTO search_rows(fts_rowid, session_id) VALUES (?1, ?2)")?;
    for r in rows {
        let heading_norm = r.heading.as_deref().map(|h| fold(h).0).unwrap_or_default();
        fts.execute(params![
            heading_norm,
            fold(&r.content).0,
            session_id,
            r.scope,
            r.version_no,
            r.kind,
            r.section_key,
            r.heading,
            r.content,
        ])?;
        map.execute(params![tx.last_insert_rowid(), session_id])?;
    }
    Ok(())
}

/// Xoá mọi dữ liệu của 1 phiên. Trả true nếu phiên có trong bảng `sessions`.
fn delete_session_rows(tx: &rusqlite::Transaction<'_>, id: &str) -> rusqlite::Result<bool> {
    tx.execute(
        "DELETE FROM search_fts WHERE rowid IN (SELECT fts_rowid FROM search_rows WHERE session_id = ?1)",
        [id],
    )?;
    tx.execute("DELETE FROM search_rows WHERE session_id = ?1", [id])?;
    Ok(tx.execute("DELETE FROM sessions WHERE id = ?1", [id])? > 0)
}

impl SearchIndex {
    /// Mở (hoặc tạo) index. Hỏng / khác schema → xoá file + `-wal`/`-shm` rồi tạo lại rỗng;
    /// caller nên `sync_dir` ngay sau đó (việc này cũng cần khi JSON đổi lúc app tắt).
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = match try_open(path) {
            Ok(c) => c,
            Err(e) if e.needs_rebuild() => {
                // `try_open` đã drop connection khi trả lỗi → file không còn bị giữ mở.
                log::warn!("index {} không dùng được ({e}); dựng lại từ đầu", path.display());
                remove_db_files(path)?;
                try_open(path)?
            }
            Err(e) => return Err(e),
        };
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Thay toàn bộ dữ liệu của phiên `rec` trong 1 transaction.
    /// `mtime`/`size` lấy từ `file_stamp` của file JSON vừa ghi.
    pub fn index_session(&self, rec: &SessionRecord, mtime: i64, size: i64) -> Result<(), IndexError> {
        let id = rec.id.to_string();
        let summary = rec.summary();
        let rows = rows_for(rec);
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        delete_session_rows(&tx, &id)?;
        tx.execute(
            "INSERT INTO sessions(id, file_mtime, file_size, title, status, created_at, finished_at,
                versions, rounds, departments, total_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                mtime,
                size,
                summary.prompt_preview,
                status_str(summary.status),
                ts_to_i64(summary.created_at),
                rec.finished_at.map(ts_to_i64),
                summary.versions,
                summary.rounds,
                summary.departments,
                i64::try_from(summary.total_tokens).unwrap_or(i64::MAX),
            ],
        )?;
        insert_rows(&tx, &id, &rows)?;
        tx.commit()?;
        Ok(())
    }

    /// Trả true nếu phiên có trong index.
    pub fn remove_session(&self, id: Uuid) -> Result<bool, IndexError> {
        self.remove_by_key(&id.to_string())
    }

    fn remove_by_key(&self, id: &str) -> Result<bool, IndexError> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        let existed = delete_session_rows(&tx, id)?;
        tx.commit()?;
        Ok(existed)
    }

    fn stamps(&self) -> Result<HashMap<String, (i64, i64)>, IndexError> {
        let conn = lock(&self.conn);
        let mut stmt = conn.prepare("SELECT id, file_mtime, file_size FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Đồng bộ index với thư mục `sessions/`: index lại file `*.json` đổi mtime/size,
    /// xoá phiên mà file đã mất. Bỏ qua file không phải `.json` và thư mục con.
    /// File không đọc được → vào `failed` (và dữ liệu cũ của nó bị xoá, giống `list_sessions`
    /// của storage bỏ qua file hỏng); lần sync sau sẽ thử lại.
    pub fn sync_dir<E: Display>(
        &self,
        sessions_dir: &Path,
        load: impl Fn(&Path) -> Result<SessionRecord, E>,
    ) -> SyncReport {
        let mut report = SyncReport::default();
        let known = match self.stamps() {
            Ok(k) => k,
            Err(e) => {
                report.failed.push(("index.db".into(), e.to_string()));
                return report;
            }
        };
        let entries = match fs::read_dir(sessions_dir) {
            Ok(e) => e,
            Err(e) => {
                report.failed.push((sessions_dir.display().to_string(), e.to_string()));
                return report;
            }
        };

        let mut seen: HashSet<String> = HashSet::new();
        for entry in entries {
            let path = match entry {
                Ok(e) => e.path(),
                Err(e) => {
                    report.failed.push((sessions_dir.display().to_string(), e.to_string()));
                    continue;
                }
            };
            if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            seen.insert(stem.clone());

            let (mtime, size) = match file_stamp(&path) {
                Ok(s) => s,
                Err(e) => {
                    report.failed.push((name, e.to_string()));
                    continue;
                }
            };
            if known.get(&stem) == Some(&(mtime, size)) {
                continue;
            }
            let result = match load(&path) {
                Ok(rec) if rec.id.to_string() == stem => {
                    self.index_session(&rec, mtime, size).map_err(|e| e.to_string())
                }
                Ok(rec) => Err(format!("id trong file ({}) khác tên file", rec.id)),
                Err(e) => Err(e.to_string()),
            };
            match result {
                Ok(()) => report.indexed += 1,
                Err(e) => {
                    if known.contains_key(&stem) {
                        if let Err(e2) = self.remove_by_key(&stem) {
                            log::warn!("không xoá được index cũ của {name}: {e2}");
                        }
                    }
                    report.failed.push((name, e));
                }
            }
        }

        for id in known.keys().filter(|id| !seen.contains(*id)) {
            match self.remove_by_key(id) {
                Ok(_) => report.removed += 1,
                Err(e) => report.failed.push((id.clone(), e.to_string())),
            }
        }
        report
    }

    /// Phiên mới nhất trước (giống `Storage::list_sessions`). Hàng không đọc được bị bỏ qua.
    pub fn list(&self, limit: u32, offset: u32) -> Result<Vec<SessionSummary>, IndexError> {
        let conn = lock(&self.conn);
        let mut stmt = conn.prepare_cached(
            "SELECT id, title, created_at, status, versions, rounds, departments, total_tokens
             FROM sessions ORDER BY created_at DESC, id LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, u32>(5)?,
                r.get::<_, u32>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, created, status, versions, rounds, departments, tokens) = row?;
            let (Ok(uuid), Some(status)) = (Uuid::parse_str(&id), parse_status(&status)) else {
                log::warn!("bỏ qua hàng index không hợp lệ: id={id} status={status}");
                continue;
            };
            out.push(SessionSummary {
                id: uuid,
                prompt_preview: title,
                created_at: DateTime::from_timestamp_nanos(created),
                status,
                versions,
                rounds,
                departments,
                total_tokens: u64::try_from(tokens).unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// Tìm full-text không phân biệt dấu / hoa thường, xếp theo bm25 (tiêu đề section nặng gấp đôi).
    /// Câu tìm bất kỳ đều an toàn (xem `fold_query`); câu rỗng / chỉ ký tự đặc biệt → rỗng.
    pub fn search(&self, query: &str, limit: u32, offset: u32) -> Result<Vec<SearchHit>, IndexError> {
        let fts_query = fold_query(query);
        let limit = limit.min(MAX_SEARCH_LIMIT);
        if fts_query.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let terms = query_terms(query);
        let conn = lock(&self.conn);
        let mut stmt = conn.prepare_cached(
            "SELECT search_fts.session_id, s.title, search_fts.scope, search_fts.version_no,
                    search_fts.kind, search_fts.section_key, search_fts.heading_orig,
                    search_fts.content_orig, bm25(search_fts, 2.0, 1.0) AS rank, s.created_at
             FROM search_fts JOIN sessions s ON s.id = search_fts.session_id
             WHERE search_fts MATCH ?1
             ORDER BY rank LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![fts_query, limit, offset], |r| {
            let content: String = r.get(7)?;
            Ok(SearchHit {
                session_id: r.get(0)?,
                session_title: r.get(1)?,
                scope: r.get(2)?,
                version_no: r.get(3)?,
                kind: r.get(4)?,
                section_key: r.get(5)?,
                heading: r.get(6)?,
                snippet: build_snippet(&content, &terms),
                rank: r.get(8)?,
                created_at: DateTime::from_timestamp_nanos(r.get(9)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DepartmentRecord, PlanVersion};
    use crate::storage::Storage;

    const PLAN: &str = "# Kế hoạch\n\
## 1. Tổng quan\n\
<!-- reviewers: all -->\n\
Kết quả mong đợi: hệ thống đang chạy ổn định.\n\
## 2. Kiến trúc\n\
```md\n\
## không phải tiêu đề\n\
```\n\
Dùng Redis làm cache.\n\
### 2.1 Chi tiết\n\
Giữ nguyên trong section 2.\n";

    fn plan(version: u32, markdown: &str) -> PlanVersion {
        PlanVersion { version, markdown: markdown.into(), created_at: Utc::now() }
    }

    fn record(prompt: &str, markdown: &str) -> SessionRecord {
        let mut r = SessionRecord::new(Uuid::new_v4(), prompt.into(), 3);
        r.status = SessionStatus::Converged;
        r.plans = vec![plan(1, "## Cũ\nbản nháp đầu tiên"), plan(2, markdown)];
        r
    }

    fn department(id: &str, plans: Vec<PlanVersion>) -> DepartmentRecord {
        DepartmentRecord {
            id: id.into(),
            name: "Phòng Dữ liệu".into(),
            task: "Thiết kế schema".into(),
            lead_id: "dba".into(),
            member_ids: vec![],
            max_rounds: 2,
            status: SessionStatus::Converged,
            plans,
            rounds: vec![],
            final_markdown: None,
            error: None,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        storage: Storage,
        db_path: PathBuf,
        index: SearchIndex,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf()).unwrap();
        let db_path = dir.path().join("index.db");
        let index = SearchIndex::open(&db_path).unwrap();
        Fixture { _dir: dir, storage, db_path, index }
    }

    impl Fixture {
        fn sessions_dir(&self) -> PathBuf {
            self.storage.root().join("sessions")
        }
        fn sync(&self) -> SyncReport {
            self.index.sync_dir(&self.sessions_dir(), |p| {
                fs::read(p)
                    .map_err(|e| e.to_string())
                    .and_then(|b| serde_json::from_slice::<SessionRecord>(&b).map_err(|e| e.to_string()))
            })
        }
        fn save_and_index(&self, rec: &SessionRecord) {
            self.storage.save_session(rec).unwrap();
            let (m, s) = file_stamp(&self.sessions_dir().join(format!("{}.json", rec.id))).unwrap();
            self.index.index_session(rec, m, s).unwrap();
        }
        fn count(&self, table: &str) -> i64 {
            lock(&self.index.conn)
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        }
    }

    #[test]
    fn bundled_sqlite_has_fts5() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE VIRTUAL TABLE t USING fts5(x, tokenize='unicode61'); INSERT INTO t VALUES ('xin chao');")
            .unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM t WHERE t MATCH '\"chao\"'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn split_sections_is_fence_aware() {
        let secs = split_sections(PLAN);
        let headings: Vec<_> = secs.iter().map(|s| s.heading.as_deref()).collect();
        assert_eq!(headings, vec![None, Some("1. Tổng quan"), Some("2. Kiến trúc")]);
        assert_eq!(secs[0].body, "# Kế hoạch");
        assert!(!secs[1].body.contains("reviewers"), "bỏ dòng tag reviewer");
        assert!(secs[2].body.contains("## không phải tiêu đề") && secs[2].body.contains("### 2.1"));
        // Fence ~~~ dài hơn chỉ đóng bởi fence cùng loại, đủ dài.
        let md = "## A\n~~~~\n```\n## x\n~~~\n~~~~\n## B\nb";
        let secs = split_sections(md);
        assert_eq!(secs.len(), 2);
        assert!(secs[0].body.contains("## x"));
        assert_eq!(secs[1].body, "b");

        let mut used = HashSet::new();
        assert_eq!(section_key("2. Kiến trúc", 0, &mut used), "2-kien-truc");
        assert_eq!(section_key("2. Kiến trúc", 1, &mut used), "2-kien-truc-2");
        assert_eq!(section_key("🚀", 4, &mut used), "section-5");
    }

    #[test]
    fn search_ignores_accents_and_case() {
        let f = fixture();
        let rec = record("Thiết kế hệ thống chat", PLAN);
        f.save_and_index(&rec);

        let hits = f.index.search("ket qua", 10, 0).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        let h = &hits[0];
        assert_eq!(h.session_id, rec.id.to_string());
        assert_eq!(h.session_title, "Thiết kế hệ thống chat");
        assert_eq!((h.scope.as_str(), h.kind.as_str(), h.version_no), (MAIN_SCOPE, "section", 2));
        assert_eq!(h.heading.as_deref(), Some("1. Tổng quan"));
        assert_eq!(h.section_key.as_deref(), Some("1-tong-quan"));
        assert!(h.snippet.contains("[[Kết]] [[quả]]"), "{}", h.snippet);
        assert_eq!(h.created_at, rec.created_at);

        let hits = f.index.search("DANG", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("[[đang]]"), "{}", hits[0].snippet);

        // Đề bài là 1 hàng riêng
        let hits = f.index.search("thiet ke", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].kind.as_str(), hits[0].heading.as_ref()), ("request", None));

        // Tiêu đề trong code fence vẫn thuộc section 2; chỉ plan mới nhất được index.
        let hits = f.index.search("khong phai tieu de", 10, 0).unwrap();
        assert_eq!(hits[0].heading.as_deref(), Some("2. Kiến trúc"));
        assert!(f.index.search("ban nhap", 10, 0).unwrap().is_empty());
        // Khớp ở tiêu đề section
        let hits = f.index.search("kien truc", 10, 0).unwrap();
        assert_eq!(hits[0].section_key.as_deref(), Some("2-kien-truc"));
        // Tag reviewer không bị index
        assert!(f.index.search("reviewers", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn department_scopes_use_latest_plan() {
        let f = fixture();
        let mut rec = record("Hệ thống thanh toán", PLAN);
        rec.departments.push(department(
            "data",
            vec![plan(1, "## Schema\nbảng ledger cũ"), plan(2, "## Schema\nbảng giao dịch mới")],
        ));
        rec.departments.push(department("empty", vec![]));
        f.save_and_index(&rec);

        let hits = f.index.search("giao dich", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].scope.as_str(), hits[0].version_no), ("data", 2));
        assert!(f.index.search("ledger", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn special_queries_never_error() {
        let f = fixture();
        f.save_and_index(&record("a b c OR NEAR", PLAN));
        for q in [
            "a OR b", "\"", "\"\"", "*", "-", ":", "-x", "ket*", "col:x", "NEAR(a b)", "(", ")",
            "AND", "NOT", "^a", "a + b", "'", "\"unterminated", "{a b}", "héllo \" OR \" world", "",
        ] {
            assert!(f.index.search(q, 10, 0).is_ok(), "query {q:?} lỗi");
        }
        // Toán tử chỉ là chữ thường: "a OR b" = a AND "or" AND b.
        assert_eq!(f.index.search("a OR b", 10, 0).unwrap().len(), 1);
        assert!(f.index.search("*", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn limit_is_clamped_and_offset_pages() {
        let f = fixture();
        let md: String = (1..=60).map(|i| format!("## Mục {i}\nnội dung chung số {i}\n")).collect();
        f.save_and_index(&record("p", &md));
        assert_eq!(f.index.search("noi dung", 1000, 0).unwrap().len(), MAX_SEARCH_LIMIT as usize);
        assert_eq!(f.index.search("noi dung", 50, 55).unwrap().len(), 5);
        assert!(f.index.search("noi dung", 0, 0).unwrap().is_empty());
    }

    #[test]
    fn reindex_is_idempotent() {
        let f = fixture();
        let rec = record("Thiết kế", PLAN);
        f.storage.save_session(&rec).unwrap();
        let r1 = f.sync();
        assert_eq!((r1.indexed, r1.removed, r1.failed.len()), (1, 0, 0));
        let (fts, map, sessions) = (f.count("search_fts"), f.count("search_rows"), f.count("sessions"));
        assert_eq!(fts, 4); // request + mở đầu + 2 section
        assert_eq!(map, fts);
        assert_eq!(sessions, 1);

        // File không đổi → không index lại
        let r2 = f.sync();
        assert_eq!((r2.indexed, r2.removed), (0, 0));
        // Gọi trực tiếp nhiều lần cũng không nhân đôi hàng
        f.save_and_index(&rec);
        f.save_and_index(&rec);
        assert_eq!((f.count("search_fts"), f.count("search_rows"), f.count("sessions")), (fts, map, 1));
        assert_eq!(f.index.search("ket qua", 10, 0).unwrap().len(), 1);
    }

    #[test]
    fn sync_removes_deleted_files_and_skips_junk() {
        let f = fixture();
        let a = record("phiên A", PLAN);
        let b = record("phiên B", "## X\nnội dung B");
        f.storage.save_session(&a).unwrap();
        f.storage.save_session(&b).unwrap();
        let dir = f.sessions_dir();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        fs::create_dir(dir.join("sub.json")).unwrap();
        fs::write(dir.join(format!("{}.tmp-123", Uuid::new_v4())), b"{").unwrap();
        assert_eq!(f.sync().indexed, 2);

        assert!(f.storage.delete_session(a.id).unwrap());
        fs::write(dir.join("broken.json"), b"{not json").unwrap();
        let r = f.sync();
        assert_eq!((r.indexed, r.removed), (0, 1));
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].0, "broken.json");
        assert!(f.index.search("ket qua", 10, 0).unwrap().is_empty());
        assert_eq!(f.count("search_rows"), f.count("search_fts"));
        let ids: Vec<Uuid> = f.index.list(10, 0).unwrap().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![b.id]);

        // Sửa file → mtime/size đổi → index lại
        let mut b2 = b.clone();
        b2.plans.push(plan(3, "## X\nnội dung mới hoàn toàn khác"));
        f.storage.save_session(&b2).unwrap();
        assert_eq!(f.sync().indexed, 1);
        assert_eq!(f.index.search("hoan toan", 10, 0).unwrap().len(), 1);

        assert!(f.index.remove_session(b.id).unwrap());
        assert!(!f.index.remove_session(b.id).unwrap());
        assert_eq!(f.count("search_fts"), 0);
    }

    #[test]
    fn corrupt_or_outdated_db_is_rebuilt() {
        let f = fixture();
        f.save_and_index(&record("p", PLAN));
        let path = f.db_path.clone();
        drop(f.index);

        fs::write(&path, b"this is definitely not a sqlite database, just garbage bytes ...").unwrap();
        fs::write(sidecar(&path, "-wal"), b"garbage wal").unwrap();
        let idx = SearchIndex::open(&path).unwrap();
        assert!(idx.list(10, 0).unwrap().is_empty());
        let rec = record("sau khi dựng lại", PLAN);
        f.storage.save_session(&rec).unwrap();
        let report = idx.sync_dir(&f.storage.root().join("sessions"), |p| {
            serde_json::from_slice::<SessionRecord>(&fs::read(p).unwrap())
        });
        assert_eq!(report.indexed, 2);
        let v: i32 = lock(&idx.conn).pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);

        // Schema version khác → xoá và dựng lại rỗng
        lock(&idx.conn).pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        drop(idx);
        let idx = SearchIndex::open(&path).unwrap();
        assert!(idx.list(10, 0).unwrap().is_empty());
    }

    #[test]
    fn list_returns_summaries_newest_first() {
        let f = fixture();
        let mut old = record("Phiên cũ", PLAN);
        old.created_at = Utc::now() - chrono::Duration::days(1);
        old.finished_at = Some(Utc::now());
        old.usage.total.prompt_tokens = 1200;
        old.usage.total.completion_tokens = 34;
        old.departments.push(department("data", vec![]));
        let mut new = record(&"dài ".repeat(100), PLAN);
        new.status = SessionStatus::MaxRoundsReached;
        f.save_and_index(&old);
        f.save_and_index(&new);

        let list = f.index.list(10, 0).unwrap();
        let expected = [new.summary(), old.summary()];
        assert_eq!(list.len(), 2);
        for (got, want) in list.iter().zip(expected.iter()) {
            assert_eq!(serde_json::to_value(got).unwrap(), serde_json::to_value(want).unwrap());
        }
        assert_eq!(f.index.list(1, 1).unwrap()[0].id, old.id);
        assert!(f.index.list(10, 2).unwrap().is_empty());
    }

    #[test]
    fn snippet_highlights_original_accented_text() {
        let terms = query_terms("ket qua");
        assert_eq!(build_snippet("Kết quả tốt", &terms), "[[Kết]] [[quả]] tốt");
        // NFD: dấu rời nằm trong vùng tô sáng
        assert_eq!(build_snippet("Ke\u{302}\u{301}t qua\u{309}!", &terms), "[[Ke\u{302}\u{301}t]] [[qua\u{309}]]!");
        // Không khớp nửa từ
        assert_eq!(build_snippet("ketqua kết", &terms), "ketqua [[kết]]");

        let long = format!("{}\n\nĐây là kết quả\tcuối. {}", "mở đầu ".repeat(20), "đuôi ".repeat(30));
        let s = build_snippet(&long, &terms);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("Đây là [[kết]] [[quả]] cuối."), "{s}");
        assert!(!s.contains('\n') && !s.contains('\t'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 2 + 4 * HIT_OPEN.len(), "{s}");
        // Không khớp trong nội dung (vd chỉ khớp tiêu đề) → phần đầu, không tô
        assert_eq!(build_snippet("abc", &terms), "abc");
        assert_eq!(build_snippet("", &terms), "");
    }
}

