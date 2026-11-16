//! Tauri glue: state, commands, event sink. Business logic nằm ở `orchestrator`.

mod budget;
mod config;
#[allow(dead_code)]
mod context_builder;
mod cost;
mod health;
mod history;
mod llm_client;
mod mcp;
mod models;
mod normalize;
mod orchestrator;
mod prompts;
mod rerun;
mod roster;
mod routing;
mod search_index;
#[allow(dead_code)]
mod sections;
mod secrets;
mod staffing;
mod storage;
#[cfg(test)] pub(crate) mod test_support;
mod tools;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use config::{api_root, AppConfig, SessionOptions, MASK_PREFIX};
use llm_client::{ChatMessage, Expect, LlmClient, LlmError};
use models::{EventEnvelope, ParentRef, SessionRecord, SessionSummary, COUNCIL_EVENT};
use history::{HistoryError, HistoryIndex, SessionWriter};
use orchestrator::{Checkpoint, ControlAction, Council, EventSink, MeetingControl, Seed, SessionExtras};
use rerun::{RerunError, RerunOverrides, RerunResult};
use secrets::{SecretError, Secrets, Target};
use storage::{Storage, StorageError};

const MAX_PROMPT_CHARS: usize = 50_000;
const MAX_EXPORT_BYTES: usize = 5 * 1024 * 1024;
const SESSION_SAVED_EVENT: &str = "council-session-saved";

// ───────────────────────────── State ─────────────────────────────

struct ActiveSession {
    id: Uuid,
    cancel: CancellationToken,
    /// Lệnh mời / mời ra của người dùng trong lúc họp.
    control: Arc<MeetingControl>,
}

pub struct AppState {
    /// `Arc` để lúc khởi động giữ write-lock (owned) trong lúc hydrate/migrate key ở nền.
    config: Arc<tokio::sync::RwLock<AppConfig>>,
    /// API key: keyring / secrets.json / RAM + trạng thái migrate (xem `secrets.rs`).
    secrets: Arc<Secrets>,
    storage: Storage,
    /// Index SQLite cho lịch sử (`Option<Arc<SearchIndex>>` bên trong); None nếu mở lỗi.
    history: HistoryIndex,
    http: reqwest::Client,
    /// v1: 1 phiên tại 1 thời điểm (tránh tự DDoS quota provider). Đổi thành HashMap nếu cần multi-session.
    active: Mutex<Option<ActiveSession>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII: luôn giải phóng slot `active` kể cả khi task panic → app không bị kẹt "đang chạy".
struct ActiveGuard {
    app: AppHandle,
    id: Uuid,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            let mut active = lock(&state.active);
            if active.as_ref().is_some_and(|s| s.id == self.id) {
                *active = None;
            }
        }
    }
}

struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, envelope: &EventEnvelope) {
        if let Err(e) = self.0.emit(COUNCIL_EVENT, envelope) {
            log::warn!("emit {COUNCIL_EVENT} thất bại: {e}");
        }
    }
}

/// Checkpoint → ghi file phiên ở `spawn_blocking` (không chặn vòng họp). Thứ tự do `SessionWriter` lo.
struct CheckpointSink {
    writer: Arc<SessionWriter>,
    seq: AtomicU64,
}

impl Checkpoint for CheckpointSink {
    fn save(&self, record: SessionRecord) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let writer = self.writer.clone();
        // tokio trực tiếp (không qua `tauri::async_runtime`): dùng chung cho app và MCP server.
        tokio::task::spawn_blocking(move || {
            writer.write_checkpoint(seq, &record);
        });
    }
}

// ───────────────────────────── Errors ─────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Invalid(String),
    #[error("Đã có một phiên họp đang chạy — hãy Stop trước khi bắt đầu phiên mới")]
    AlreadyRunning,
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    History(#[from] HistoryError),
    /// `keyring_unavailable` / `keyring_locked` / storage / invalid.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// Key chế độ RAM đã mất sau khi khởi động lại (hoặc keyring khoá) → cần nhập lại trước khi họp.
    #[error("Thiếu API key của {label} (chỉ lưu trong RAM hoặc keyring đang khoá) — vào Cài đặt để nhập lại")]
    SecretMissing { target: Target, label: String },
    /// `no_valid_version` / `missing_roles` / `unsupported_scope` / `invalid` (rerun / restart).
    #[error(transparent)]
    Rerun(#[from] RerunError),
    #[error("lỗi nội bộ: {0}")]
    Internal(String),
}

impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            kind: &'a str,
            message: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            target: Option<&'a Target>,
            /// Chỉ với `no_valid_version`: các phiên bản của scope.
            #[serde(skip_serializing_if = "Option::is_none")]
            available_versions: Option<&'a [u32]>,
            /// Chỉ với `missing_roles`.
            #[serde(skip_serializing_if = "Option::is_none")]
            missing_roles: Option<&'a [String]>,
        }
        let target = match self {
            CommandError::SecretMissing { target, .. } => Some(target),
            _ => None,
        };
        let kind = match self {
            CommandError::Secret(e) => e.code(),
            CommandError::SecretMissing { .. } => "secret_missing",
            CommandError::Invalid(_) => "invalid",
            CommandError::AlreadyRunning => "already_running",
            CommandError::Storage(StorageError::NotFound(_)) => "not_found",
            CommandError::Storage(_) => "storage",
            CommandError::Llm(_) => "llm",
            CommandError::History(HistoryError::Unavailable) => "index_unavailable",
            CommandError::History(HistoryError::Index(_)) => "index",
            CommandError::Rerun(e) => e.code(),
            CommandError::Internal(_) => "internal",
        };
        let (available_versions, missing_roles) = match self {
            CommandError::Rerun(RerunError::NoValidVersion { available, .. }) => (Some(available.as_slice()), None),
            CommandError::Rerun(RerunError::MissingRoles { roles }) => (None, Some(roles.as_slice())),
            _ => (None, None),
        };
        Wire {
            kind,
            message: self.to_string(),
            target,
            available_versions,
            missing_roles,
        }
        .serialize(s)
    }
}

type CmdResult<T> = Result<T, CommandError>;

async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::Internal(e.to_string()))?
        .map_err(Into::into)
}

/// Như `blocking` nhưng cho thao tác keyring / secrets (cũng blocking, có thể hiện hộp thoại OS).
async fn secret_blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SecretError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::Internal(e.to_string()))?
        .map_err(Into::into)
}

// ───────────────────────────── Commands ─────────────────────────────

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> CmdResult<AppConfig> {
    Ok(state.config.read().await.redacted())
}

#[tauri::command]
async fn save_config(state: State<'_, AppState>, config: AppConfig) -> CmdResult<AppConfig> {
    // Giữ write-lock suốt quá trình merge + persist → 2 lần save đồng thời không ghi đè lẫn nhau.
    let mut guard = state.config.write().await;
    // Mask không đổi = giữ key; "" = xoá key; key mới → keyring (lỗi → chỉ giữ trong RAM, UI hỏi tiếp).
    let merged = config.merge_secrets(&guard);
    merged.validate().map_err(CommandError::Invalid)?;
    let secrets = state.secrets.clone();
    let old = guard.clone();
    let merged = secret_blocking(move || {
        let mut m = merged;
        secrets.persist(&mut m, &old)?;
        Ok(m)
    })
    .await?;
    *guard = merged;
    log::info!("config saved: {:?}", guard.providers); // Debug đã redact key
    Ok(guard.redacted())
}

/// Dựng công cụ + bối cảnh cho phiên từ tuỳ chọn người dùng chọn ở màn Phòng họp.
async fn build_extras(cfg: &AppConfig, opts: &SessionOptions) -> CmdResult<SessionExtras> {
    let mut extras = SessionExtras::default();

    if let Some(id) = opts.workspace_id.as_deref().filter(|s| !s.is_empty()) {
        let w = cfg
            .workspace(id)
            .ok_or_else(|| CommandError::Invalid("Workspace đã chọn không còn trong cấu hình".into()))?
            .clone();
        let fs = tokio::task::spawn_blocking(move || tools::WorkspaceFs::open(&w.name, &w.path))
            .await
            .map_err(|e| CommandError::Internal(e.to_string()))?
            .map_err(CommandError::Invalid)?;
        let fs = Arc::new(fs);
        let f2 = fs.clone();
        extras.overview = Some(
            tokio::task::spawn_blocking(move || f2.overview())
                .await
                .map_err(|e| CommandError::Internal(e.to_string()))?,
        );
        extras.info.workspace = Some(fs.name().to_owned());
        extras.toolbox.workspace = Some(fs);
    }

    if opts.web_search {
        let ws = &cfg.web_search;
        let has_key = ws.api_key.as_deref().is_some_and(|k| !k.trim().is_empty());
        match ws.provider {
            config::WebSearchProvider::Tavily | config::WebSearchProvider::Brave if !has_key => {
                return Err(CommandError::Invalid("Dịch vụ tìm kiếm web cần API key (Cài đặt → Bối cảnh & Công cụ)".into()));
            }
            config::WebSearchProvider::Searxng if config::validate_url(&ws.base_url).is_err() => {
                return Err(CommandError::Invalid("SearXNG cần base URL hợp lệ (Cài đặt → Bối cảnh & Công cụ)".into()));
            }
            _ => {}
        }
        extras.toolbox.web = Some(Arc::new(tools::WebSearch::new(ws.clone()).map_err(CommandError::Internal)?));
        extras.info.web_search = true;
    }

    let text = cfg.company_context.text.trim();
    extras.company = if text.is_empty() {
        prompts::CompanyMode::None
    } else if opts.company_context {
        extras.info.company_context = true;
        prompts::CompanyMode::Enforced(text.to_owned())
    } else {
        prompts::CompanyMode::Lifted
    };
    Ok(extras)
}

/// `departments`: phòng ban tham gia riêng phiên này (None = theo `meeting.department_ids`).
/// `options`: workspace / web / bối cảnh công ty cho phiên này (None = theo mặc định trong cấu hình).
#[tauri::command]
async fn start_council(
    app: AppHandle,
    state: State<'_, AppState>,
    prompt: String,
    departments: Option<Vec<String>>,
    options: Option<SessionOptions>,
) -> CmdResult<Uuid> {
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(CommandError::Invalid("Prompt không được để trống".into()));
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(CommandError::Invalid(format!(
            "Prompt vượt quá {MAX_PROMPT_CHARS} ký tự"
        )));
    }

    // Snapshot config: đổi setting giữa phiên không ảnh hưởng phiên đang chạy.
    let mut cfg = state.config.read().await.clone();
    if let Some(mut ids) = departments {
        let mut seen = std::collections::HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        cfg.meeting.department_ids = ids;
    }
    let opts = options.unwrap_or_else(|| SessionOptions {
        web_search: cfg.web_search.enabled_by_default,
        company_context: cfg.company_context.enabled_by_default,
        auto_staff: cfg.meeting.staffing == config::StaffingMode::Auto,
        ..SessionOptions::default()
    });
    // Thành phần đã duyệt từ đề xuất (hoặc người dùng tự chọn) ghi đè cấu hình cho riêng phiên này.
    if let Some(chair) = opts.chair_id.as_deref().filter(|s| !s.is_empty()) {
        cfg.meeting.chair_id = chair.to_owned();
    }
    if let Some(ids) = &opts.reviewer_ids {
        let mut seen = std::collections::HashSet::new();
        cfg.meeting.reviewer_ids = ids.iter().filter(|id| seen.insert(id.as_str())).cloned().collect();
    }
    if let Some(depth) = opts.plan_depth {
        cfg.meeting.plan_depth = depth;
    }
    launch(app, &state, LaunchSpec { prompt, cfg, opts, seed: None, parent: None }).await
}

/// Thông số đã chuẩn bị để khởi chạy 1 phiên — dùng chung cho start / rerun / restart.
struct LaunchSpec {
    prompt: String,
    /// Snapshot config của riêng phiên này (đã áp thành phần / override).
    cfg: AppConfig,
    opts: SessionOptions,
    seed: Option<Seed>,
    parent: Option<ParentRef>,
}

impl From<rerun::RerunPlan> for LaunchSpec {
    fn from(p: rerun::RerunPlan) -> Self {
        Self { prompt: p.prompt, cfg: p.cfg, opts: p.options, seed: p.seed, parent: Some(p.parent) }
    }
}

/// Chưa có phiên nào chạy (kiểm tra sớm trước khi làm việc nặng; `claim_slot` mới là chốt chặn thật).
fn ensure_idle(active: &Mutex<Option<ActiveSession>>) -> CmdResult<()> {
    if lock(active).is_some() {
        return Err(CommandError::AlreadyRunning);
    }
    Ok(())
}

/// Giữ slot phiên duy nhất; đã có phiên chạy → `already_running`.
fn claim_slot(active: &Mutex<Option<ActiveSession>>, session: ActiveSession) -> CmdResult<()> {
    let mut active = lock(active);
    if active.is_some() {
        return Err(CommandError::AlreadyRunning);
    }
    *active = Some(session);
    Ok(())
}

/// Kiểm tra sẵn sàng → giữ slot → chạy hội đồng ở nền; checkpoint + bản ghi cuối qua `SessionWriter`.
async fn launch(app: AppHandle, state: &AppState, spec: LaunchSpec) -> CmdResult<Uuid> {
    let LaunchSpec { prompt, cfg, opts, seed, parent } = spec;
    cfg.validate_ready().map_err(CommandError::Invalid)?;
    check_secrets_present(&state.secrets, &cfg, opts.web_search)?;
    let mut extras = build_extras(&cfg, &opts).await?;
    extras.auto_staff = opts.auto_staff;
    extras.seed = seed;
    extras.parent = parent;
    let control = extras.control.clone();

    let session_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    claim_slot(&state.active, ActiveSession { id: session_id, cancel: cancel.clone(), control })?;

    let writer = Arc::new(SessionWriter::new(state.storage.clone(), state.history.clone()));
    extras.checkpoint = Some(Arc::new(CheckpointSink { writer: writer.clone(), seq: AtomicU64::new(0) }));

    let llm = LlmClient::new(state.http.clone(), &cfg);
    let council = Council::with_extras(
        llm,
        cfg,
        Arc::new(TauriSink(app.clone())),
        session_id,
        cancel,
        extras,
    );

    tauri::async_runtime::spawn(async move {
        let guard = ActiveGuard {
            app: app.clone(),
            id: session_id,
        };
        let record = council.run(prompt).await;
        drop(guard); // giải phóng slot ngay khi họp xong, trước khi I/O persist

        // Bản ghi cuối chặn mọi checkpoint đến muộn (xem `SessionWriter`).
        let ok = match tauri::async_runtime::spawn_blocking(move || writer.write_final(&record))
            .await
        {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                log::error!("lưu session {session_id} thất bại: {e}");
                false
            }
            Err(e) => {
                log::error!("task lưu session {session_id} panic: {e}");
                false
            }
        };
        let _ = app.emit(
            SESSION_SAVED_EVENT,
            serde_json::json!({ "session_id": session_id, "ok": ok }),
        );
    });

    Ok(session_id)
}

/// Chạy lại từ 1 phiên bản plan đã lưu → phiên MỚI (bắt đầu bằng review plan đó, kèm issue còn mở).
/// `dry_run` chỉ kiểm tra và trả `uncovered_sections` / `missing_roles`, không khởi chạy (và không
/// bị chặn khi đang có phiên chạy). Lỗi: `no_valid_version` (+ `available_versions`), `missing_roles`
/// (+ `missing_roles`), `unsupported_scope`, `already_running`, `not_found`, `invalid`.
#[tauri::command(rename_all = "snake_case")]
async fn rerun_from_version(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    scope: String,
    version_no: u32,
    dry_run: Option<bool>,
    overrides: Option<RerunOverrides>,
) -> CmdResult<RerunResult> {
    let dry = dry_run.unwrap_or(false);
    if !dry {
        ensure_idle(&state.active)?;
    }
    let storage = state.storage.clone();
    let rec = blocking(move || storage.load_session(session_id)).await?;
    let cur = state.config.read().await.clone();
    let plan = rerun::plan_rerun(&cur, &rec, scope.trim(), version_no, &overrides.unwrap_or_default())?;
    if dry {
        return Ok(plan.preview);
    }
    if !plan.blocking.is_empty() {
        return Err(RerunError::MissingRoles { roles: plan.preview.missing_roles }.into());
    }
    let preview = plan.preview.clone();
    let id = launch(app, &state, plan.into()).await?;
    log::info!("rerun {session_id} v{version_no} → phiên mới {id}");
    Ok(RerunResult { new_session_id: Some(id), ..preview })
}

#[derive(Serialize)]
struct RestartResult {
    new_session_id: Uuid,
}

/// Chạy lại yêu cầu gốc (phiên MỚI) với thành phần + tuỳ chọn đã chụp của phiên cũ — dùng khi
/// phiên không có phiên bản plan nào. Lỗi như `rerun_from_version` (trừ `no_valid_version`).
#[tauri::command(rename_all = "snake_case")]
async fn restart_from_request(app: AppHandle, state: State<'_, AppState>, session_id: Uuid) -> CmdResult<RestartResult> {
    ensure_idle(&state.active)?;
    let storage = state.storage.clone();
    let rec = blocking(move || storage.load_session(session_id)).await?;
    let cur = state.config.read().await.clone();
    let plan = rerun::plan_restart(&cur, &rec)?;
    let id = launch(app, &state, plan.into()).await?;
    log::info!("restart {session_id} từ yêu cầu → phiên mới {id}");
    Ok(RestartResult { new_session_id: id })
}

/// Token + chi phí của 1 phiên. Chi phí tính bằng bảng giá HIỆN TẠI; model chưa có giá → `null`.
#[tauri::command(rename_all = "snake_case")]
async fn get_usage(state: State<'_, AppState>, session_id: Uuid) -> CmdResult<cost::UsageBreakdown> {
    let storage = state.storage.clone();
    let rec = blocking(move || storage.load_session(session_id)).await?;
    let pricing = state.config.read().await.pricing.clone();
    Ok(cost::usage_breakdown(&rec, &pricing))
}

/// Đề xuất thành phần họp cho yêu cầu (chế độ "Đề xuất rồi duyệt"). Dùng cấu hình đã lưu.
#[tauri::command]
async fn suggest_staffing(
    state: State<'_, AppState>,
    prompt: String,
    company_context: Option<bool>,
) -> CmdResult<StaffingProposal> {
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(CommandError::Invalid("Hãy nhập yêu cầu trước".into()));
    }
    let cfg = state.config.read().await.clone();
    if cfg.resolve(&cfg.meeting.chair_id).is_none() {
        return Err(CommandError::Invalid("Chủ trì chưa có provider/model — vào Cài đặt → Vai trò".into()));
    }
    let text = cfg.company_context.text.trim();
    let company = match (text.is_empty(), company_context.unwrap_or(cfg.company_context.enabled_by_default)) {
        (true, _) => prompts::CompanyMode::None,
        (false, true) => prompts::CompanyMode::Enforced(text.to_owned()),
        (false, false) => prompts::CompanyMode::Lifted,
    };
    let llm = LlmClient::new(state.http.clone(), &cfg).with_limits(1, Duration::from_secs(90));
    let (staffing, usage, pc, oc) = staffing::propose(&llm, &cfg, &prompt, &company, &CancellationToken::new()).await?;
    let tokens = usage.map_or_else(|| ((pc + oc) as u64).div_ceil(3), |u| u.prompt_tokens + u.completion_tokens);
    Ok(StaffingProposal { staffing, tokens })
}

#[derive(Serialize)]
struct StaffingProposal {
    staffing: models::StaffingInfo,
    /// Token đã dùng cho lần đề xuất (ước lượng nếu provider không trả usage).
    tokens: u64,
}

/// Mời 1 role vào nơi họp (`scope` = "main" hoặc id phòng ban) — áp dụng từ vòng kế tiếp.
#[tauri::command]
fn invite_member(state: State<'_, AppState>, session_id: Uuid, scope: String, role_id: String) -> bool {
    control_push(&state, session_id, ControlAction::Invite { scope, role_id })
}

/// Mời 1 reviewer ra khỏi nơi họp — áp dụng từ vòng kế tiếp.
#[tauri::command]
fn dismiss_member(state: State<'_, AppState>, session_id: Uuid, scope: String, role_id: String) -> bool {
    control_push(&state, session_id, ControlAction::Dismiss { scope, role_id })
}

fn control_push(state: &State<'_, AppState>, session_id: Uuid, action: ControlAction) -> bool {
    match lock(&state.active).as_ref() {
        Some(s) if s.id == session_id => {
            s.control.push(action);
            true
        }
        _ => false,
    }
}

#[tauri::command]
fn stop_council(state: State<'_, AppState>, session_id: Uuid) -> bool {
    let active = lock(&state.active);
    match active.as_ref() {
        Some(s) if s.id == session_id => {
            s.cancel.cancel();
            true
        }
        _ => false,
    }
}

#[tauri::command]
fn active_session(state: State<'_, AppState>) -> Option<Uuid> {
    lock(&state.active).as_ref().map(|s| s.id)
}

#[tauri::command]
async fn list_sessions(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<SessionSummary>> {
    let storage = state.storage.clone();
    let history = state.history.clone();
    let limit = limit.unwrap_or(100).clamp(1, 500);
    blocking(move || history.list_sessions(&storage, limit)).await
}

/// Tìm trong lịch sử (không phân biệt dấu / hoa thường). Câu rỗng → `[]`;
/// không có index → lỗi `index_unavailable`.
#[tauri::command]
async fn search_history(
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> CmdResult<Vec<search_index::SearchHit>> {
    let history = state.history.clone();
    tauri::async_runtime::spawn_blocking(move || history.search(&query, limit, offset))
        .await
        .map_err(|e| CommandError::Internal(e.to_string()))?
        .map_err(Into::into)
}

#[tauri::command]
async fn load_session(state: State<'_, AppState>, session_id: Uuid) -> CmdResult<SessionRecord> {
    let storage = state.storage.clone();
    blocking(move || storage.load_session(session_id)).await
}

#[tauri::command]
async fn delete_session(state: State<'_, AppState>, session_id: Uuid) -> CmdResult<bool> {
    if lock(&state.active)
        .as_ref()
        .is_some_and(|s| s.id == session_id)
    {
        return Err(CommandError::Invalid(
            "Không thể xoá phiên đang chạy".into(),
        ));
    }
    if state.storage.lease_alive(session_id) {
        return Err(CommandError::Invalid("Phiên đang chạy ở tiến trình khác (Claude Code / MCP) — không thể xoá".into()));
    }
    let storage = state.storage.clone();
    let history = state.history.clone();
    blocking(move || history.delete_session(&storage, session_id)).await
}

#[derive(Serialize)]
struct ConnectionTestResult {
    agent: String,
    model: String,
    latency_ms: u128,
    reply: String,
}

/// Ping nhanh endpoint của 1 agent (dùng ở màn Settings). Không retry để báo lỗi ngay.
#[tauri::command]
async fn test_connection(
    state: State<'_, AppState>,
    agent: String,
) -> CmdResult<ConnectionTestResult> {
    let cfg = state.config.read().await.clone();
    let mut ep = cfg
        .resolve(&agent)
        .ok_or_else(|| {
            CommandError::Invalid(format!(
                "role '{agent}' không tồn tại hoặc chưa chọn provider (hãy lưu cấu hình trước)"
            ))
        })?;
    ep.max_tokens = Some(32);
    let llm = LlmClient::new(state.http.clone(), &cfg).with_limits(0, Duration::from_secs(30));
    let msgs = [
        ChatMessage::system("You are a health check endpoint."),
        ChatMessage::user("Reply with exactly: OK"),
    ];
    let t0 = Instant::now();
    let reply = llm
        .chat(&ep, &msgs, Expect::Text, &CancellationToken::new())
        .await?;
    Ok(ConnectionTestResult {
        agent,
        model: ep.model,
        latency_ms: t0.elapsed().as_millis(),
        reply: models::truncate_chars(reply.text.trim(), 100),
    })
}

#[derive(Serialize)]
struct ModelList {
    provider_id: String,
    models: Vec<String>,
    fetched_at: DateTime<Utc>,
    /// true nếu danh sách đã được lưu vào cache của provider trong config.
    cached: bool,
}

/// Lấy danh sách model của 1 provider qua `GET {base_url}/models`.
///
/// `base_url` / `api_key` cho phép thử provider đang sửa dở trên form (chưa lưu):
/// - `api_key` None hoặc giá trị mask → dùng key đã lưu, CHỈ KHI base_url trùng với bản đã lưu
///   (không gửi key cũ tới host khác);
/// - `api_key` "" → gọi không có key.
///
/// Nếu gọi đúng bằng credential đã lưu → cập nhật cache `models` của provider trên đĩa.
#[tauri::command]
async fn list_models(
    state: State<'_, AppState>,
    provider_id: String,
    base_url: Option<String>,
    api_key: Option<String>,
) -> CmdResult<ModelList> {
    let stored = state.config.read().await.provider(&provider_id).cloned();
    let base = base_url
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| stored.as_ref().map(|p| p.base_url.trim().to_owned()))
        .ok_or_else(|| CommandError::Invalid("Provider chưa có base URL".into()))?;
    config::validate_url(&base)
        .map_err(|e| CommandError::Invalid(format!("base_url không hợp lệ: {e}")))?;

    let same_base = stored
        .as_ref()
        .is_some_and(|p| api_root(&p.base_url) == api_root(&base));
    let stored_key = stored.as_ref().and_then(|p| p.api_key.clone());
    let key = match api_key.map(|k| k.trim().to_owned()) {
        Some(k) if k.starts_with(MASK_PREFIX) => same_base.then(|| stored_key.clone()).flatten(),
        Some(k) if k.is_empty() => None,
        Some(k) => Some(k),
        None => same_base.then(|| stored_key.clone()).flatten(),
    };

    let models =
        llm_client::list_models(&state.http, &base, key.as_deref(), Duration::from_secs(20))
            .await?;
    let fetched_at = Utc::now();

    let mut cached = false;
    if same_base && key == stored_key {
        let mut guard = state.config.write().await;
        let still_same = guard
            .provider(&provider_id)
            .is_some_and(|p| api_root(&p.base_url) == api_root(&base) && p.api_key == key);
        if still_same {
            let mut next = guard.clone();
            next.set_models(&provider_id, models.clone(), fetched_at);
            let secrets = state.secrets.clone();
            let to_save = next.clone();
            match secret_blocking(move || secrets.save_config(&to_save)).await {
                Ok(()) => {
                    *guard = next;
                    cached = true;
                }
                Err(e) => log::warn!("không lưu được cache model của '{provider_id}': {e}"),
            }
        }
    }
    log::info!(
        "list_models '{provider_id}': {} model (cached={cached})",
        models.len()
    );
    Ok(ModelList {
        provider_id,
        models,
        fetched_at,
        cached,
    })
}

/// Hộp thoại chọn thư mục mở **từ Rust** (webview không có quyền dialog/fs).
#[tauri::command]
async fn pick_folder(app: AppHandle) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let Some(picked) = rx.await.map_err(|e| CommandError::Internal(e.to_string()))? else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| CommandError::Invalid(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Kiểm tra model của các role (None = mọi role) bằng cấu hình đã lưu.
/// Mỗi cặp provider + model chỉ gọi thử 1 lần, tối đa 4 request song song.
#[tauri::command]
async fn check_models(
    state: State<'_, AppState>,
    role_ids: Option<Vec<String>>,
) -> CmdResult<Vec<health::ModelHealth>> {
    let cfg = state.config.read().await.clone();
    let res = health::check_roles(&state.http, &cfg, role_ids.as_deref()).await;
    let bad = res.iter().filter(|h| h.status != health::HealthStatus::Ok).count();
    log::info!("check_models: {} cặp provider/model, {bad} lỗi", res.len());
    Ok(res)
}

/// Thử dịch vụ tìm kiếm web với cấu hình đã lưu.
#[tauri::command]
async fn test_web_search(state: State<'_, AppState>, query: String) -> CmdResult<Vec<tools::SearchHit>> {
    let cfg = state.config.read().await.web_search.clone();
    let web = tools::WebSearch::new(cfg).map_err(CommandError::Internal)?;
    web.search(&query).await.map_err(CommandError::Invalid)
}

/// Danh mục role mặc định (không chứa bí mật) — UI dùng để áp ngoại hình mẫu / khôi phục role.
#[tauri::command]
fn default_roles() -> Vec<config::RoleDef> {
    roster::default_roles("", "", 0.4)
}

/// Save dialog được mở **từ Rust** → webview không thể tự chọn path tuỳ ý để ghi đè file hệ thống.
#[tauri::command]
async fn export_markdown(
    app: AppHandle,
    content: String,
    suggested_name: String,
) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    if content.len() > MAX_EXPORT_BYTES {
        return Err(CommandError::Invalid(
            "Nội dung export vượt quá 5 MiB".into(),
        ));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Markdown", &["md"])
        .set_file_name(sanitize_file_name(&suggested_name))
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx
        .await
        .map_err(|e| CommandError::Internal(e.to_string()))?
    else {
        return Ok(None); // user bấm Cancel
    };
    let mut path = picked
        .into_path()
        .map_err(|e| CommandError::Invalid(e.to_string()))?;
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("md")
    {
        path.set_extension("md");
    }
    let display = path.display().to_string();
    blocking(move || storage::write_atomic(&path, content.as_bytes(), false).map_err(Into::into))
        .await?;
    Ok(Some(display))
}

fn sanitize_file_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_matches(['.', '_']).to_owned();
    if cleaned.is_empty() {
        "plan.md".into()
    } else if cleaned.to_ascii_lowercase().ends_with(".md") {
        cleaned
    } else {
        format!("{cleaned}.md")
    }
}

// ───────────────────────────── API key (secrets) ─────────────────────────────

/// Trước khi họp: key của provider mà role tham gia dùng (và key tìm web nếu bật) phải còn.
/// Key chế độ RAM mất sau khi khởi động lại → báo `secret_missing {target}` thay vì lỗi 401 giữa chừng.
fn check_secrets_present(secrets: &Secrets, cfg: &AppConfig, web_search: bool) -> CmdResult<()> {
    let mut wanted: Vec<Target> = Vec::new();
    for id in cfg.participants() {
        if let Some(r) = cfg.role(id) {
            let t = Target::provider(&r.provider_id);
            if !wanted.contains(&t) {
                wanted.push(t);
            }
        }
    }
    if web_search {
        wanted.push(Target::websearch(cfg));
    }
    match secrets.first_missing(cfg, &wanted) {
        None => Ok(()),
        Some(target) => {
            let label = match target.kind {
                secrets::TargetKind::Provider => {
                    let name = cfg.provider(&target.id).map_or(target.id.as_str(), |p| p.name.as_str());
                    format!("provider \"{name}\"")
                }
                secrets::TargetKind::Websearch => format!("dịch vụ tìm web \"{}\"", target.id),
            };
            Err(CommandError::SecretMissing { target, label })
        }
    }
}

#[derive(Serialize)]
struct SetSecretResult {
    storage: config::KeyStorage,
    /// Giá trị mask mới (`••••abcd`) để form cập nhật — KHÔNG phải key.
    masked: String,
}

/// Lưu key cho 1 target. `api_key` = giá trị mask → dùng key hiện tại (đổi nơi lưu / thử lại
/// migrate / mở khoá keyring). `storage: "file"` chỉ được dùng khi frontend gửi rõ (sau khi
/// người dùng đồng ý). Lỗi: `keyring_unavailable`, `keyring_locked`, `invalid`, `storage`.
#[tauri::command(rename_all = "snake_case")]
async fn set_secret(
    state: State<'_, AppState>,
    target: Target,
    api_key: String,
    storage: config::KeyStorage,
) -> CmdResult<SetSecretResult> {
    let mut guard = state.config.write().await;
    let secrets = state.secrets.clone();
    let mut cfg = guard.clone();
    let t = target.clone();
    let (cfg, storage) = secret_blocking(move || {
        let s = secrets.set_secret(&mut cfg, &t, &api_key, storage)?;
        Ok((cfg, s))
    })
    .await?;
    *guard = cfg;
    let key = match target.kind {
        secrets::TargetKind::Provider => guard.provider(&target.id).and_then(|p| p.api_key.as_deref()),
        secrets::TargetKind::Websearch => guard.web_search.api_key.as_deref(),
    };
    log::info!("đã lưu key '{}' ({storage:?})", target.account());
    Ok(SetSecretResult { storage, masked: key.map(config::mask_key).unwrap_or_default() })
}

/// Xoá key của target khỏi keyring, secrets.json, RAM (và plaintext còn trong config.json).
#[tauri::command]
async fn delete_secret(state: State<'_, AppState>, target: Target) -> CmdResult<()> {
    let mut guard = state.config.write().await;
    let secrets = state.secrets.clone();
    let mut cfg = guard.clone();
    let t = target.clone();
    let cfg = secret_blocking(move || {
        secrets.delete_secret(&mut cfg, &t)?;
        Ok(cfg)
    })
    .await?;
    *guard = cfg;
    log::info!("đã xoá key '{}'", target.account());
    Ok(())
}

/// Trạng thái từng key (nơi lưu, đã migrate chưa, lỗi) + file sót lại còn chứa key. Không trả key.
#[tauri::command]
async fn secret_status(state: State<'_, AppState>) -> CmdResult<secrets::SecretStatusReport> {
    let cfg = state.config.read().await.clone();
    let secrets = state.secrets.clone();
    secret_blocking(move || Ok(secrets.status(&cfg))).await
}

/// Xoá file backup/hỏng còn chứa key (chỉ file đang nằm trong `leftover_files`). Trả danh sách còn lại.
#[tauri::command]
async fn delete_leftover_files(state: State<'_, AppState>, files: Vec<String>) -> CmdResult<Vec<String>> {
    let secrets = state.secrets.clone();
    secret_blocking(move || secrets.delete_leftovers(&files)).await
}

// ───────────────────────────── Entry ─────────────────────────────

/// `plancouncil --mcp`: MCP server qua stdio cho Claude Code (không mở cửa sổ). Trả mã thoát.
pub fn run_mcp() -> i32 {
    mcp::serve()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("hyper", log::LevelFilter::Warn)
                .level_for("reqwest", log::LevelFilter::Warn)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .max_file_size(5 * 1024 * 1024)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let storage = Storage::new()?;
            let config = storage.load_config();
            log::info!("PlanCouncil data dir: {}", storage.root().display());
            let history = HistoryIndex::open(&storage);
            {
                // Đồng bộ index nền — không chặn khởi động; trong lúc chờ, list_sessions quét file.
                // Trước đó: phiên còn `running` trên đĩa (app tắt giữa chừng) → `interrupted`.
                // Lúc khởi động chưa thể có phiên đang chạy trong tiến trình này → không loại trừ id nào.
                let (history, storage) = (history.clone(), storage.clone());
                tauri::async_runtime::spawn(async move {
                    let t0 = Instant::now();
                    match tauri::async_runtime::spawn_blocking(move || {
                        let fixed = storage.recover_interrupted(None);
                        if !fixed.is_empty() {
                            log::warn!("{} phiên bị gián đoạn lần trước → interrupted: {fixed:?}", fixed.len());
                        }
                        history.sync(&storage)
                    })
                    .await
                    {
                        Ok(Some(r)) => log::info!(
                            "sync index: {} index lại, {} xoá, {} lỗi ({} ms)",
                            r.indexed,
                            r.removed,
                            r.failed.len(),
                            t0.elapsed().as_millis()
                        ),
                        Ok(None) => {}
                        Err(e) => log::error!("task sync index panic: {e}"),
                    }
                });
            }
            let secrets = Arc::new(Secrets::new(Arc::new(secrets::KeyringStore), storage.root()));
            let config = Arc::new(tokio::sync::RwLock::new(config));
            {
                // Hydrate key từ keyring + migrate key plaintext ở nền (keyring có thể hiện hộp thoại
                // mở khoá). Lấy write-lock NGAY tại đây → mọi command đọc config đều chờ xong bước này.
                let mut guard = config.clone().try_write_owned().expect("config lock chưa ai giữ lúc setup");
                let secrets = secrets.clone();
                tauri::async_runtime::spawn(async move {
                    let mut cfg = guard.clone();
                    match tauri::async_runtime::spawn_blocking(move || {
                        secrets.startup(&mut cfg);
                        cfg
                    })
                    .await
                    {
                        Ok(cfg) => *guard = cfg,
                        Err(e) => log::error!("task nạp API key panic: {e}"),
                    }
                });
            }
            app.manage(AppState {
                config,
                secrets,
                storage,
                history,
                http: llm_client::build_http_client()?,
                active: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            start_council,
            stop_council,
            active_session,
            list_sessions,
            load_session,
            delete_session,
            search_history,
            test_connection,
            list_models,
            default_roles,
            pick_folder,
            test_web_search,
            check_models,
            suggest_staffing,
            rerun_from_version,
            restart_from_request,
            get_usage,
            invite_member,
            dismiss_member,
            export_markdown,
            set_secret,
            delete_secret,
            secret_status,
            delete_leftover_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PlanCouncil");
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name;

    #[test]
    fn secret_errors_have_stable_codes_and_target() {
        use super::{secrets::SecretError, secrets::Target, CommandError};
        let v = serde_json::to_value(CommandError::SecretMissing { target: Target::provider("p1"), label: "x".into() }).unwrap();
        assert_eq!(v["kind"], "secret_missing");
        assert_eq!(v["target"], serde_json::json!({"kind": "provider", "id": "p1"}));
        let v = serde_json::to_value(CommandError::Secret(SecretError::Unavailable("no dbus".into()))).unwrap();
        assert_eq!(v["kind"], "keyring_unavailable");
        assert!(v.get("target").is_none());
        let v = serde_json::to_value(CommandError::Secret(SecretError::Locked("cancel".into()))).unwrap();
        assert_eq!(v["kind"], "keyring_locked");
    }

    #[test]
    fn rerun_errors_have_stable_codes_and_payloads() {
        use super::{CommandError, RerunError};
        let v = serde_json::to_value(CommandError::from(RerunError::NoValidVersion { version: 4, available: vec![1, 2] })).unwrap();
        assert_eq!(v["kind"], "no_valid_version");
        assert_eq!(v["available_versions"], serde_json::json!([1, 2]));
        assert!(v.get("missing_roles").is_none());
        let v = serde_json::to_value(CommandError::from(RerunError::MissingRoles { roles: vec!["dba".into()] })).unwrap();
        assert_eq!(v["kind"], "missing_roles");
        assert_eq!(v["missing_roles"], serde_json::json!(["dba"]));
        let v = serde_json::to_value(CommandError::from(RerunError::UnsupportedScope("qa".into()))).unwrap();
        assert_eq!(v["kind"], "unsupported_scope");
        let v = serde_json::to_value(CommandError::from(RerunError::Invalid("x".into()))).unwrap();
        assert_eq!(v["kind"], "invalid");
    }

    #[test]
    fn rerun_is_rejected_while_a_session_runs() {
        use super::{claim_slot, ensure_idle, ActiveSession, CommandError};
        use std::sync::{Arc, Mutex};
        let active = Mutex::new(None);
        let mk = || ActiveSession {
            id: uuid::Uuid::new_v4(),
            cancel: tokio_util::sync::CancellationToken::new(),
            control: Arc::default(),
        };
        ensure_idle(&active).unwrap();
        claim_slot(&active, mk()).unwrap();
        // Cùng lỗi với start_council.
        assert!(matches!(ensure_idle(&active), Err(CommandError::AlreadyRunning)));
        assert!(matches!(claim_slot(&active, mk()), Err(CommandError::AlreadyRunning)));
        assert_eq!(serde_json::to_value(ensure_idle(&active).unwrap_err()).unwrap()["kind"], "already_running");
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "etc_passwd.md");
        assert_eq!(sanitize_file_name("Plan v3 Final.md"), "Plan_v3_Final.md");
        assert_eq!(sanitize_file_name(""), "plan.md");
    }
}
