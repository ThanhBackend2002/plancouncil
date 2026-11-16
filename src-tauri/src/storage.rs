//! Persist config + lịch sử phiên họp dưới dạng JSON file.
//!
//! Vị trí (qua `dirs::config_dir()`):
//! - Linux:   `~/.config/plancouncil/`
//! - Windows: `%APPDATA%\plancouncil\`
//!
//! Mọi hàm ở đây là **blocking** — caller (lib.rs) gọi qua `spawn_blocking`.
//! Ghi file theo kiểu atomic (tmp + fsync + rename) để crash giữa chừng không làm hỏng JSON.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::config::AppConfig;
use crate::models::{SessionRecord, SessionStatus, SessionSummary};

/// Lý do ghi vào phiên bị gián đoạn (app đóng / crash khi đang họp).
pub const INTERRUPTED_ERROR: &str = "Ứng dụng đóng khi phiên đang chạy; bản nháp đang stream không được lưu";

const APP_DIR: &str = "plancouncil";
const CONFIG_FILE: &str = "config.json";
const SESSIONS_DIR: &str = "sessions";

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("không xác định được thư mục cấu hình của hệ điều hành")]
    NoConfigDir,
    #[error("lỗi I/O: {0}")]
    Io(#[from] io::Error),
    #[error("lỗi JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("không tìm thấy phiên họp {0}")]
    NotFound(Uuid),
}

#[derive(Debug, Clone)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn new() -> Result<Self, StorageError> {
        let base = dirs::config_dir().ok_or(StorageError::NoConfigDir)?;
        Self::with_root(base.join(APP_DIR))
    }

    pub fn with_root(root: PathBuf) -> Result<Self, StorageError> {
        fs::create_dir_all(root.join(SESSIONS_DIR))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// Thư mục chứa các file `<uuid>.json`.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join(SESSIONS_DIR)
    }

    pub fn session_path(&self, id: Uuid) -> PathBuf {
        // `id` là Uuid đã parse → không thể path traversal.
        self.sessions_dir().join(format!("{id}.json"))
    }

    /// Không bao giờ fail: file thiếu → default; file hỏng → backup rồi default;
    /// file schema cũ → backup `config.pre-migration-backup.json` rồi ghi lại dạng mới.
    pub fn load_config(&self) -> AppConfig {
        let path = self.config_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return AppConfig::default(),
            Err(e) => {
                log::error!("không đọc được {}: {e}", path.display());
                return AppConfig::default();
            }
        };
        match AppConfig::from_json(&bytes) {
            Ok((c, false)) => c,
            Ok((c, true)) => {
                let backup = path.with_extension("pre-migration-backup.json");
                log::info!(
                    "migrate config lên schema v{} ({} provider, {} role); bản cũ lưu ở {}",
                    c.schema_version,
                    c.providers.len(),
                    c.roles.len(),
                    backup.display()
                );
                if let Err(e) = write_atomic(&backup, &bytes, true) {
                    log::warn!("không backup được config cũ: {e}");
                } else if let Err(e) = self.save_config(&c) {
                    log::warn!("không ghi được config mới: {e}");
                }
                c
            }
            Err(e) => {
                let backup = path.with_extension(format!(
                    "corrupt-{}.json",
                    chrono::Utc::now().format("%Y%m%d%H%M%S")
                ));
                log::error!("config.json hỏng ({e}); backup sang {}", backup.display());
                let _ = fs::rename(&path, &backup);
                AppConfig::default()
            }
        }
    }

    pub fn save_config(&self, cfg: &AppConfig) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(cfg)?;
        write_atomic(&self.config_path(), &bytes, true)?;
        Ok(())
    }

    pub fn save_session(&self, rec: &SessionRecord) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(rec)?;
        write_atomic(&self.session_path(rec.id), &bytes, false)?;
        Ok(())
    }

    pub fn load_session(&self, id: Uuid) -> Result<SessionRecord, StorageError> {
        match fs::read(self.session_path(id)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StorageError::NotFound(id)),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_session(&self, id: Uuid) -> Result<bool, StorageError> {
        match fs::remove_file(self.session_path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Khởi động: phiên còn `running` trên đĩa (app tắt / crash giữa chừng) → `interrupted` kèm
    /// lý do. Bỏ qua phiên `live` (đang chạy thật trong tiến trình này) và file hỏng.
    /// Trả về id các phiên đã sửa.
    pub fn recover_interrupted(&self, live: Option<Uuid>) -> Vec<Uuid> {
        /// Chỉ cần `status` để lọc — serde bỏ qua phần còn lại.
        #[derive(serde::Deserialize)]
        struct Head {
            status: SessionStatus,
        }
        let entries = match fs::read_dir(self.sessions_dir()) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("không quét được thư mục phiên để khôi phục: {e}");
                return vec![];
            }
        };
        let mut fixed = Vec::new();
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            if !matches!(serde_json::from_slice::<Head>(&bytes), Ok(h) if h.status == SessionStatus::Running) {
                continue;
            }
            let mut rec: SessionRecord = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("bỏ qua phiên hỏng {}: {e}", path.display());
                    continue;
                }
            };
            if Some(rec.id) == live || path != self.session_path(rec.id) {
                continue;
            }
            rec.status = SessionStatus::Interrupted;
            rec.error = Some(INTERRUPTED_ERROR.to_owned());
            match self.save_session(&rec) {
                Ok(()) => fixed.push(rec.id),
                Err(e) => log::warn!("không ghi được trạng thái interrupted cho {}: {e}", rec.id),
            }
        }
        fixed
    }

    /// O(n) file đọc mỗi lần list — đường dự phòng khi index SQLite (`search_index`) chưa sẵn sàng.
    /// Thứ tự: mới nhất trước, trùng `created_at` thì theo id tăng dần (giống `SearchIndex::list`).
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>, StorageError> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.sessions_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
                continue;
            }
            match read_session_file(&path) {
                Ok(rec) => out.push(rec.summary()),
                Err(e) => log::warn!("bỏ qua session hỏng {}: {e}", path.display()),
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
        out.truncate(limit);
        Ok(out)
    }
}

/// Đọc + parse 1 file phiên (dùng chung cho `list_sessions` và đồng bộ index).
pub fn read_session_file(path: &Path) -> Result<SessionRecord, StorageError> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// tmp (cùng thư mục) → fsync → rename. `rename` thay thế file đích atomically trên
/// cả ext4/NTFS (Rust dùng `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` trên Windows).
pub fn write_atomic(path: &Path, bytes: &[u8], secret: bool) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let result = (|| {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if secret {
                opts.mode(0o600); // chỉ owner đọc được file chứa API key
            }
        }
        #[cfg(not(unix))]
        let _ = secret; // Windows: %APPDATA% đã có ACL per-user
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_storage() -> Storage {
        let dir = std::env::temp_dir().join(format!("plancouncil-test-{}", Uuid::new_v4()));
        Storage::with_root(dir).unwrap()
    }

    #[test]
    fn config_roundtrip_and_corruption_recovery() {
        let s = tmp_storage();
        let c = AppConfig {
            max_rounds: 5,
            ..Default::default()
        };
        s.save_config(&c).unwrap();
        assert_eq!(s.load_config().max_rounds, 5);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(s.config_path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // v1 → được migrate, backup và ghi lại thành v3
        fs::write(
            s.config_path(),
            br#"{"default_endpoint":{"api_key":"sk-old-12345678","model":"m1"},"max_rounds":4}"#,
        )
        .unwrap();
        let migrated = s.load_config();
        assert_eq!(migrated.max_rounds, 4);
        assert_eq!(migrated.providers[0].api_key.as_deref(), Some("sk-old-12345678"));
        assert!(s.config_path().with_extension("pre-migration-backup.json").exists());
        let on_disk: serde_json::Value =
            serde_json::from_slice(&fs::read(s.config_path()).unwrap()).unwrap();
        assert_eq!(on_disk["schema_version"], 3);

        fs::write(s.config_path(), b"{not json").unwrap();
        assert_eq!(s.load_config().max_rounds, 3); // default
        assert!(!s.config_path().exists()); // đã bị move sang .corrupt-*
        fs::remove_dir_all(s.root()).ok();
    }

    #[test]
    fn running_sessions_become_interrupted_on_startup() {
        let s = tmp_storage();
        let mk = |status| {
            let mut r = SessionRecord::new(Uuid::new_v4(), "p".into(), 3);
            r.status = status;
            r.plans.push(crate::models::PlanVersion::new(1, "## 1. A\n".into(), Default::default()));
            s.save_session(&r).unwrap();
            r
        };
        let crashed = mk(SessionStatus::Running);
        let live = mk(SessionStatus::Running);
        let others: Vec<_> = [SessionStatus::Converged, SessionStatus::Cancelled, SessionStatus::Failed, SessionStatus::MaxRoundsReached]
            .into_iter()
            .map(mk)
            .collect();
        let before: Vec<_> = others.iter().map(|r| fs::read(s.session_path(r.id)).unwrap()).collect();
        fs::write(s.sessions_dir().join("broken.json"), b"{not json").unwrap();

        assert_eq!(s.recover_interrupted(Some(live.id)), vec![crashed.id]);
        let got = s.load_session(crashed.id).unwrap();
        assert_eq!(got.status, SessionStatus::Interrupted);
        assert_eq!(got.error.as_deref(), Some(INTERRUPTED_ERROR));
        assert_eq!(got.plans.len(), 1, "giữ nguyên plan đã lưu");
        assert_eq!(s.load_session(live.id).unwrap().status, SessionStatus::Running, "phiên đang chạy không bị đụng");
        for (r, b) in others.iter().zip(before) {
            assert_eq!(fs::read(s.session_path(r.id)).unwrap(), b, "{:?} không đổi", r.status);
        }
        // Chạy lại: không còn gì để sửa.
        assert!(s.recover_interrupted(Some(live.id)).is_empty());
        fs::remove_dir_all(s.root()).ok();
    }

    #[test]
    fn session_crud() {
        let s = tmp_storage();
        let mut r = SessionRecord::new(Uuid::new_v4(), "Design a chat system".into(), 3);
        r.status = SessionStatus::Converged;
        s.save_session(&r).unwrap();
        assert_eq!(s.list_sessions(10).unwrap().len(), 1);
        assert_eq!(s.load_session(r.id).unwrap().prompt, r.prompt);
        assert!(s.delete_session(r.id).unwrap());
        assert!(matches!(
            s.load_session(r.id),
            Err(StorageError::NotFound(_))
        ));
        fs::remove_dir_all(s.root()).ok();
    }
}
