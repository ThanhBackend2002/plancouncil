//! Lịch sử phiên: ghép `Storage` (JSON, nguồn sự thật) với `SearchIndex` (SQLite, dựng lại được).
//!
//! - Index mở lỗi → chạy không có index (list quét file như cũ, tìm kiếm báo `Unavailable`).
//! - `list` chỉ đọc từ index khi đã `sync` xong và chưa có lần ghi index nào thất bại;
//!   ngược lại quét thư mục `sessions/` → kết quả luôn giống `Storage::list_sessions`.
//!
//! Mọi hàm ở đây là **blocking** — caller (lib.rs) gọi qua `spawn_blocking`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use uuid::Uuid;

use crate::models::{SessionRecord, SessionSummary};
use crate::search_index::{file_stamp, IndexError, SearchHit, SearchIndex, SyncReport, MAX_SEARCH_LIMIT};
use crate::storage::{read_session_file, Storage, StorageError};

pub const INDEX_FILE: &str = "index.db";
pub const DEFAULT_SEARCH_LIMIT: u32 = 20;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("Chỉ mục tìm kiếm không khả dụng — xem log để biết lý do (danh sách lịch sử vẫn dùng được)")]
    Unavailable,
    #[error("lỗi chỉ mục tìm kiếm: {0}")]
    Index(#[from] IndexError),
}

#[derive(Default)]
struct Flags {
    /// `sync_dir` đã chạy xong và index khớp thư mục `sessions/`.
    synced: AtomicBool,
    /// Có lần ghi index thất bại → index có thể lệch JSON, không dùng cho `list` tới lần mở app sau.
    broken: AtomicBool,
}

/// Rẻ để clone (Arc) → dễ move vào `spawn_blocking`.
#[derive(Clone, Default)]
pub struct HistoryIndex {
    index: Option<Arc<SearchIndex>>,
    flags: Arc<Flags>,
}

impl HistoryIndex {
    /// Mở `<root>/index.db`. Lỗi → log rồi trả handle rỗng (app vẫn chạy bình thường).
    pub fn open(storage: &Storage) -> Self {
        let path = index_path(storage.root());
        match SearchIndex::open(&path) {
            Ok(i) => Self::with_index(Arc::new(i)),
            Err(e) => {
                log::error!("không mở được index {}: {e} — chạy không có tìm kiếm", path.display());
                Self::default()
            }
        }
    }

    pub fn with_index(index: Arc<SearchIndex>) -> Self {
        Self { index: Some(index), flags: Arc::default() }
    }

    /// Index dùng được cho `list` (đã sync, chưa có lỗi ghi).
    pub fn is_ready(&self) -> bool {
        self.index.is_some()
            && self.flags.synced.load(Ordering::Acquire)
            && !self.flags.broken.load(Ordering::Acquire)
    }

    fn mark_broken(&self, what: &str, e: &IndexError) {
        log::error!("{what} thất bại: {e} — list_sessions quay về quét file");
        self.flags.broken.store(true, Ordering::Release);
    }

    /// Đồng bộ index với thư mục `sessions/`. Chỉ đánh dấu `synced` khi mọi lỗi đều là file
    /// mà `Storage::list_sessions` cũng bỏ qua (JSON hỏng) → hai đường list cho cùng kết quả.
    pub fn sync(&self, storage: &Storage) -> Option<SyncReport> {
        let index = self.index.as_ref()?;
        let dir = storage.sessions_dir();
        let report = index.sync_dir(&dir, read_session_file);
        let consistent = report.failed.iter().all(|(name, _)| {
            name.ends_with(".json") && read_session_file(&dir.join(name)).is_err()
        });
        if consistent {
            self.flags.synced.store(true, Ordering::Release);
        } else {
            log::warn!("sync index chưa khớp JSON ({:?}) — list_sessions quét file", report.failed);
        }
        Some(report)
    }

    /// Ghi JSON (nguồn sự thật) rồi cập nhật index. Lỗi index chỉ log — không làm hỏng việc lưu.
    pub fn save_session(&self, storage: &Storage, rec: &SessionRecord) -> Result<(), StorageError> {
        storage.save_session(rec)?;
        if let Some(index) = &self.index {
            let result = file_stamp(&storage.session_path(rec.id))
                .map_err(IndexError::from)
                .and_then(|(mtime, size)| index.index_session(rec, mtime, size));
            if let Err(e) = result {
                self.mark_broken(&format!("index phiên {}", rec.id), &e);
            }
        }
        Ok(())
    }

    /// Xoá file JSON rồi xoá phiên khỏi index (kể cả khi file đã mất từ trước).
    pub fn delete_session(&self, storage: &Storage, id: Uuid) -> Result<bool, StorageError> {
        let existed = storage.delete_session(id)?;
        if let Some(index) = &self.index {
            if let Err(e) = index.remove_session(id) {
                self.mark_broken(&format!("xoá index phiên {id}"), &e);
            }
        }
        Ok(existed)
    }

    /// Như `Storage::list_sessions`, nhưng đọc từ index khi sẵn sàng.
    pub fn list_sessions(&self, storage: &Storage, limit: usize) -> Result<Vec<SessionSummary>, StorageError> {
        if let (Some(index), true) = (&self.index, self.is_ready()) {
            match index.list(u32::try_from(limit).unwrap_or(u32::MAX), 0) {
                Ok(list) => return Ok(list),
                Err(e) => log::warn!("đọc danh sách từ index thất bại: {e} — quét file"),
            }
        }
        storage.list_sessions(limit)
    }

    /// Câu rỗng → `[]` (kể cả khi không có index). Không có index → `HistoryError::Unavailable`.
    pub fn search(&self, query: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<SearchHit>, HistoryError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let index = self.index.as_ref().ok_or(HistoryError::Unavailable)?;
        let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT);
        Ok(index.search(query, limit, offset.unwrap_or(0))?)
    }
}

/// `<storage root>/index.db`.
pub fn index_path(root: &Path) -> std::path::PathBuf {
    root.join(INDEX_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PlanVersion, SessionStatus};
    use chrono::{Duration, Utc};
    use std::fs;

    fn record(prompt: &str, markdown: &str) -> SessionRecord {
        let mut r = SessionRecord::new(Uuid::new_v4(), prompt.into(), 3);
        r.status = SessionStatus::Converged;
        r.plans = vec![PlanVersion { version: 1, markdown: markdown.into(), created_at: Utc::now(), revise_mode: Default::default(), fallback_reason: None }];
        r
    }

    struct Fx {
        _dir: tempfile::TempDir,
        storage: Storage,
    }

    fn fx() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf()).unwrap();
        Fx { _dir: dir, storage }
    }

    fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn list_via_index_matches_storage_scan() {
        let f = fx();
        let base = Utc::now() - Duration::days(3);
        let mut recs = Vec::new();
        for i in 0..8 {
            let mut r = record(&format!("Phiên số {i} — thiết kế hệ thống"), "## A\nnội dung");
            // 3 phiên trùng created_at → kiểm tra tie-break theo id.
            r.created_at = if i < 3 { base } else { base + Duration::minutes(i) };
            r.status = if i % 2 == 0 { SessionStatus::Converged } else { SessionStatus::Failed };
            r.usage.total.prompt_tokens = 100 * i as u64;
            if i == 5 {
                r.finished_at = Some(Utc::now());
                r.created_at += Duration::nanoseconds(123_456_789); // độ chính xác nano
            }
            recs.push(r);
        }
        // Một nửa ghi TRƯỚC khi mở index (như phiên từ lần chạy trước), nửa sau ghi qua handle.
        for r in &recs[..4] {
            f.storage.save_session(r).unwrap();
        }
        fs::write(f.storage.sessions_dir().join("broken.json"), b"{not json").unwrap();
        fs::write(f.storage.sessions_dir().join("note.txt"), b"x").unwrap();

        let h = HistoryIndex::open(&f.storage);
        assert!(h.index.is_some() && !h.is_ready());
        let report = h.sync(&f.storage).unwrap();
        assert_eq!((report.indexed, report.failed.len()), (4, 1));
        assert!(h.is_ready(), "file JSON hỏng không chặn index (storage cũng bỏ qua)");
        for r in &recs[4..] {
            h.save_session(&f.storage, r).unwrap();
        }
        assert!(h.delete_session(&f.storage, recs[6].id).unwrap());

        for limit in [1, 3, 5, 100] {
            let via_index = h.list_sessions(&f.storage, limit).unwrap();
            let via_scan = f.storage.list_sessions(limit).unwrap();
            assert_eq!(json(&via_index), json(&via_scan), "limit={limit}");
        }
        let all = f.storage.list_sessions(100).unwrap();
        assert_eq!(all.len(), 7);
        let ids: Vec<_> = all.iter().map(|s| s.id).collect();
        let mut ties: Vec<_> = recs[..3].iter().map(|r| r.id).collect();
        ties.sort();
        assert_eq!(&ids[4..], &ties[..], "trùng created_at → id tăng dần");
    }

    #[test]
    fn id_mismatch_keeps_storage_fallback() {
        let f = fx();
        let r = record("lệch tên file", "## A\nx");
        // File mang tên khác id bên trong: storage vẫn liệt kê, index thì không → không được dùng index.
        fs::write(f.storage.sessions_dir().join(format!("{}.json", Uuid::new_v4())), serde_json::to_vec(&r).unwrap())
            .unwrap();
        let h = HistoryIndex::open(&f.storage);
        h.sync(&f.storage);
        assert!(!h.is_ready());
        assert_eq!(h.list_sessions(&f.storage, 10).unwrap().len(), 1);
    }

    #[test]
    fn search_after_save_and_gone_after_delete() {
        let f = fx();
        let h = HistoryIndex::open(&f.storage);
        h.sync(&f.storage);
        let r = record("Hệ thống thanh toán", "## 1. Kiến trúc\nDùng Redis làm bộ đệm.");
        h.save_session(&f.storage, &r).unwrap();

        let hits = h.search("redis", None, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, r.id.to_string());
        assert_eq!(hits[0].heading.as_deref(), Some("1. Kiến trúc"));
        assert!(hits[0].snippet.contains("[[Redis]]"), "{}", hits[0].snippet);
        assert_eq!(h.search("thanh toan", Some(5), Some(0)).unwrap()[0].kind, "request");
        assert!(h.search("   ", None, None).unwrap().is_empty());

        assert!(h.delete_session(&f.storage, r.id).unwrap());
        assert!(h.search("redis", None, None).unwrap().is_empty());
        assert!(h.list_sessions(&f.storage, 10).unwrap().is_empty());
        // Xoá lần 2: file không còn → false, không lỗi.
        assert!(!h.delete_session(&f.storage, r.id).unwrap());
    }

    #[test]
    fn without_index_falls_back_and_search_is_typed_error() {
        let f = fx();
        let h = HistoryIndex::default();
        let r = record("không index", "## A\nx");
        h.save_session(&f.storage, &r).unwrap();
        assert!(h.sync(&f.storage).is_none());
        assert!(!h.is_ready());
        assert_eq!(h.list_sessions(&f.storage, 10).unwrap().len(), 1);
        assert!(h.search("", None, None).unwrap().is_empty(), "câu rỗng → [] kể cả khi không có index");
        assert!(matches!(h.search("x", None, None), Err(HistoryError::Unavailable)));
    }

    #[test]
    fn unopenable_index_is_disabled() {
        let f = fx();
        // `index.db` là thư mục → SQLite không mở được.
        fs::create_dir(index_path(f.storage.root())).unwrap();
        let h = HistoryIndex::open(&f.storage);
        assert!(h.index.is_none());
        assert!(matches!(h.search("x", None, None), Err(HistoryError::Unavailable)));
    }
}
