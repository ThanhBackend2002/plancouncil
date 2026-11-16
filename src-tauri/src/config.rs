//! Cấu hình (schema v3):
//! - `providers`: nơi DUY NHẤT quản lý nguồn API (base URL + API key) + cache danh sách model.
//! - `roles`: danh sách vai trò do người dùng quản lý (thêm/sửa/xoá) — mỗi role có persona
//!   (`instructions`) và model riêng (provider + model + temperature).
//! - `departments`: phòng ban = 1 trưởng phòng (viết plan của phòng) + thành viên phản biện.
//! - `meeting`: cuộc họp chính = chủ trì (viết plan tổng) + reviewer + phòng ban tham gia mặc định.
//!
//! Bảo mật API key:
//! - `Debug` được implement thủ công để KHÔNG BAO GIỜ in key ra log.
//! - `redacted()` che key trước khi gửi lên webview; key thật chỉ sống ở Rust.
//! - `merge_secrets()`: giá trị mask (`••••abcd`) = giữ key cũ, nhưng chỉ khi base_url không đổi.
//! - Key thật KHÔNG BAO GIỜ được serialize (`ser_masked_only`): ra ngoài chỉ có giá trị mask hoặc
//!   `null`. Key nằm trong keyring / `secrets.json` / RAM theo `key_storage` — xem `secrets.rs`.
//!
//! Config v1 (endpoint mặc định + override 4 agent) và v2 (4 role cố định) được migrate tự động.

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::roster;

pub const MASK_PREFIX: &str = "••••";
pub const CONFIG_SCHEMA_VERSION: u32 = 3;
pub const DEFAULT_PROVIDER_ID: &str = "default";
const DEFAULT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_TEMPERATURE: f32 = 0.4;
pub const MAX_REVIEWERS: usize = 12;
/// Ngân sách token tối thiểu khi người dùng đặt `meeting.max_total_tokens`.
pub const MIN_TOTAL_TOKENS: u64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonMode {
    /// Chỉ dựa vào prompt + lenient parser (tương thích mọi proxy).
    Off,
    /// Gửi `response_format: {"type":"json_object"}`; tự fallback về Off nếu provider trả 400/422.
    #[default]
    JsonObject,
}

// ───────────────────────────── Nơi lưu API key ─────────────────────────────

/// Nơi lưu API key (key thật chỉ sống trong RAM của Rust, không nằm trong config.json).
/// Config cũ không có trường này → mặc định `keyring`; key plaintext còn sót trong config.json
/// được `secrets::Secrets::startup` migrate sang keyring (thất bại thì giữ nguyên chỗ cũ).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeyStorage {
    /// Keychain (macOS) / Credential Manager (Windows) / Secret Service (Linux).
    #[default]
    Keyring,
    /// `secrets.json` plaintext (0600) — chỉ khi người dùng đồng ý rõ ràng.
    File,
    /// Chỉ trong RAM — mất khi đóng app.
    Memory,
}

/// Chỉ cho phép serialize giá trị đã mask; key thật → `null` (config.json, webview, log JSON).
/// Key plaintext chưa migrate được do `secrets.rs` chèn lại vào config.json có chủ đích.
fn ser_masked_only<S: serde::Serializer>(key: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match key.as_deref() {
        Some(k) if k.starts_with(MASK_PREFIX) => s.serialize_some(k),
        _ => s.serialize_none(),
    }
}

// ───────────────────────────── Provider ─────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(serialize_with = "ser_masked_only")]
    pub api_key: Option<String>,
    pub key_storage: KeyStorage,
    /// Cache kết quả `GET /models` gần nhất để dropdown hiện ngay.
    pub models: Vec<String>,
    pub models_fetched_at: Option<DateTime<Utc>>,
}

impl Default for Provider {
    fn default() -> Self {
        Self {
            id: DEFAULT_PROVIDER_ID.into(),
            name: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            key_storage: KeyStorage::default(),
            models: vec![],
            models_fetched_at: None,
        }
    }
}

// ───────────────────────────── Role / Department / Meeting ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoleLevel {
    CLevel,
    Head,
    Manager,
    Lead,
    #[default]
    Engineer,
    Specialist,
}

/// Ngoại hình nhân vật pixel. Chuỗi rỗng = tự sinh từ id/cấp bậc (UI quyết định, backend chỉ lưu).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// "male" | "female" | "" (tự động)
    pub gender: String,
    /// Key tông da, vd "light", "tan", "deep".
    pub skin: String,
    pub hair_style: String,
    /// Key màu tóc hoặc mã hex.
    pub hair_color: String,
    pub outfit: String,
    /// Mã hex; rỗng = dùng màu của role.
    pub outfit_color: String,
    /// "none" | "glasses" | "sunglasses" | "" (tự động)
    pub eyewear: String,
    pub accessory: String,
}

impl Appearance {
    pub fn new(gender: &str, hair_style: &str, hair_color: &str, outfit: &str, eyewear: &str, accessory: &str) -> Self {
        Self {
            gender: gender.into(),
            hair_style: hair_style.into(),
            hair_color: hair_color.into(),
            outfit: outfit.into(),
            eyewear: eyewear.into(),
            accessory: accessory.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleDef {
    /// Định danh ổn định (slug/uuid) — phòng ban, cuộc họp và lịch sử tham chiếu qua id.
    pub id: String,
    pub name: String,
    pub title: String,
    pub level: RoleLevel,
    /// Tên icon (UI map sang lucide); không ảnh hưởng backend.
    pub icon: String,
    pub color: String,
    /// Mô tả ngắn hiển thị trên UI.
    pub description: String,
    /// Persona: role này quan tâm / soi điều gì. Được chèn vào system prompt.
    pub instructions: String,
    pub provider_id: String,
    pub model: String,
    pub temperature: f32,
    pub appearance: Appearance,
}

impl Default for RoleDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            title: String::new(),
            level: RoleLevel::default(),
            icon: "user".into(),
            color: "#a1a1aa".into(),
            description: String::new(),
            instructions: String::new(),
            provider_id: String::new(),
            model: String::new(),
            temperature: DEFAULT_TEMPERATURE,
            appearance: Appearance::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Department {
    pub id: String,
    pub name: String,
    /// Phạm vi trách nhiệm — chủ trì dựa vào đây để giao việc.
    pub mission: String,
    /// Trưởng phòng: viết plan của phòng ban.
    pub lead_id: String,
    /// Thành viên: phản biện plan của phòng ban (có thể rỗng → trưởng phòng viết 1 lần).
    pub member_ids: Vec<String>,
    /// Số vòng họp nội bộ tối đa.
    pub max_rounds: u32,
}

impl Default for Department {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            mission: String::new(),
            lead_id: String::new(),
            member_ids: vec![],
            max_rounds: 2,
        }
    }
}

/// Mức chi tiết của plan — quyết định định dạng & mức "enterprise" của prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanDepth {
    /// Bản đầu dùng được, ngắn gọn, không số liệu capacity/SLO.
    Mvp,
    /// Vừa đủ cho yêu cầu (mặc định).
    #[default]
    Standard,
    /// Production-grade: capacity, SLO, failure mode đầy đủ.
    Enterprise,
}

/// Cách chọn thành phần họp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaffingMode {
    /// Dùng đúng chủ trì / reviewer / phòng ban đã cấu hình.
    Manual,
    /// AI đề xuất, người dùng duyệt trước khi họp (mặc định).
    #[default]
    Suggest,
    /// AI tự chọn rồi họp luôn.
    Auto,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingConfig {
    /// Chủ trì: giao việc cho phòng ban, viết + chỉnh plan tổng.
    pub chair_id: String,
    /// Reviewer của cuộc họp chính.
    pub reviewer_ids: Vec<String>,
    /// Phòng ban tham gia mặc định (người dùng có thể đổi khi bắt đầu phiên).
    pub department_ids: Vec<String>,
    /// Round 1 bắt buộc REJECT + ≥ 1 issue cụ thể (chống khen xã giao). Mặc định tắt.
    pub strict_first_round: bool,
    pub plan_depth: PlanDepth,
    pub staffing: StaffingMode,
    /// Role luôn có mặt khi AI chọn thành phần.
    pub required_role_ids: Vec<String>,
    /// Số reviewer tối đa AI được chọn cho cuộc họp chính.
    pub max_auto_reviewers: u32,
    /// Số phòng ban tối đa AI được chọn.
    pub max_auto_departments: u32,
    /// Reviewer được đề nghị mời thêm chuyên gia / tự rút khỏi cuộc họp.
    pub dynamic_membership: bool,
    /// Số người được mời thêm tối đa mỗi nơi họp.
    pub max_invites: u32,
    /// Người viết gắn tag reviewer cho từng section → reviewer chỉ đọc phần của mình.
    pub section_routing: bool,
    /// Ngân sách token MỀM cho cả phiên (None = không giới hạn; nếu đặt thì ≥ 1000).
    /// Hết chỗ giữ trước lần gọi kế tiếp → vòng họp dừng với `BudgetExhausted`.
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    /// Cách người viết chỉnh plan từ vòng 2: viết lại toàn bộ (mặc định) hay chỉ sửa section liên quan.
    pub revise_mode: ReviseMode,
    /// Section revise chỉ dùng khi token các section cần sửa ≤ tỉ lệ này × cả plan (0.1..=1.0; chỉ áp cho `auto`).
    pub section_mode_max_ratio: f32,
    /// Ai kiểm tra nhất quán sau section revise: "author" (người viết plan) hoặc id role reviewer.
    pub consistency_checker: String,
}

/// Chế độ chỉnh sửa plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviseMode {
    /// Luôn viết lại toàn bộ plan (hành vi cũ).
    #[default]
    Full,
    /// Section revise + mọi luật fallback (tỉ lệ, patch hỏng, 2 lần fallback liên tiếp).
    Auto,
    /// Section revise; chỉ quay về viết lại toàn bộ khi patch không hợp lệ.
    Section,
}

/// Giá trị `consistency_checker` mặc định: người viết plan.
pub const CONSISTENCY_CHECKER_AUTHOR: &str = "author";

impl Default for MeetingConfig {
    fn default() -> Self {
        roster::default_meeting()
    }
}

// ───────────────────────────── Bối cảnh & công cụ ─────────────────────────────

/// Thư mục dự án được cấp quyền ĐỌC cho các role (chọn qua hộp thoại của hệ điều hành).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    /// Không cần key (đọc trang HTML kết quả) — có thể bị chặn nếu truy vấn dồn dập.
    #[default]
    Duckduckgo,
    Tavily,
    Brave,
    /// SearXNG tự host (cần bật `format=json`).
    Searxng,
}

impl WebSearchProvider {
    /// Tên ổn định (khớp serde) — dùng làm account keyring `websearch:<provider>`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duckduckgo => "duckduckgo",
            Self::Tavily => "tavily",
            Self::Brave => "brave",
            Self::Searxng => "searxng",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    pub provider: WebSearchProvider,
    #[serde(serialize_with = "ser_masked_only")]
    pub api_key: Option<String>,
    pub key_storage: KeyStorage,
    /// Chỉ dùng cho SearXNG.
    pub base_url: String,
    pub max_results: u32,
    /// Bật tìm kiếm web mặc định cho phiên mới.
    pub enabled_by_default: bool,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: WebSearchProvider::Duckduckgo,
            api_key: None,
            key_storage: KeyStorage::default(),
            base_url: String::new(),
            max_results: 5,
            enabled_by_default: false,
        }
    }
}

/// Bối cảnh chung của công ty (tech stack, nội quy…) — mặc định áp dụng cho mọi role.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanyContext {
    pub text: String,
    pub enabled_by_default: bool,
}

impl Default for CompanyContext {
    fn default() -> Self {
        Self { text: String::new(), enabled_by_default: true }
    }
}

/// Ai được dùng công cụ (đọc workspace / web) trước khi viết hoặc review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    /// Chỉ người viết plan (chủ trì + trưởng phòng) — cân bằng chi phí/hiệu quả.
    #[default]
    Authors,
    /// Mọi role (reviewer cũng tự kiểm chứng) — tốn token hơn nhiều.
    All,
    Off,
}

/// Giá của 1 model (USD / 1 triệu token). Người dùng tự nhập — không có bảng giá sẵn, không tải từ mạng.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPrice {
    pub input_per_m: f64,
    /// Giá token prompt đọc từ cache.
    pub cached_per_m: f64,
    pub output_per_m: f64,
}

impl ModelPrice {
    /// Chi phí 1 lần gọi. `prompt` đã gồm phần `cached` (theo quy ước OpenAI).
    pub fn cost(&self, prompt: u64, cached: u64, completion: u64) -> f64 {
        let cached = cached.min(prompt);
        ((prompt - cached) as f64 * self.input_per_m
            + cached as f64 * self.cached_per_m
            + completion as f64 * self.output_per_m)
            / 1_000_000.0
    }
}

/// Bảng giá theo tên model (đúng chuỗi `model` của role).
pub type Pricing = std::collections::BTreeMap<String, ModelPrice>;

/// Tuỳ chọn riêng cho 1 phiên họp (chọn ở màn Phòng họp).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionOptions {
    pub workspace_id: Option<String>,
    pub web_search: bool,
    pub company_context: bool,
    /// Ghi đè chủ trì / reviewer cho phiên này (từ đề xuất thành phần đã duyệt).
    pub chair_id: Option<String>,
    pub reviewer_ids: Option<Vec<String>>,
    /// AI tự chọn thành phần ngay khi bắt đầu (chế độ Tự động).
    pub auto_staff: bool,
    pub plan_depth: Option<PlanDepth>,
}

// ───────────────────────────── AppConfig ─────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub providers: Vec<Provider>,
    pub roles: Vec<RoleDef>,
    pub departments: Vec<Department>,
    pub meeting: MeetingConfig,
    /// Áp cho mọi role; None = mặc định của model.
    pub max_tokens: Option<u32>,
    /// Số vòng tối đa của cuộc họp chính.
    pub max_rounds: u32,
    pub request_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
    pub max_retries: u32,
    pub json_mode: JsonMode,
    /// Stream plan (của chủ trì và trưởng phòng) theo thời gian thực.
    pub stream_plans: bool,
    /// Giới hạn số request LLM chạy đồng thời (nhiều phòng ban × nhiều reviewer dễ dính 429).
    pub max_parallel_calls: u32,
    pub company_context: CompanyContext,
    pub workspaces: Vec<Workspace>,
    pub web_search: WebSearchConfig,
    pub tool_policy: ToolPolicy,
    /// Số lượt gọi công cụ tối đa mỗi lần nghiên cứu (1..=12); 0 = không giới hạn
    /// (vẫn dừng khi model tự viết ghi chú, hết ngân sách token, bị dừng, hoặc chạm chặn an toàn).
    pub max_tool_steps: u32,
    /// Bảng giá model để ước tính chi phí (mặc định rỗng → chi phí `null`).
    pub pricing: Pricing,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            // Không tạo sẵn provider: người dùng tự thêm (provider đầu tiên được gán cho mọi role).
            providers: vec![],
            roles: roster::default_roles("", "", DEFAULT_TEMPERATURE),
            departments: roster::default_departments(),
            meeting: roster::default_meeting(),
            max_tokens: None,
            max_rounds: 3,
            request_timeout_secs: 300,
            stream_idle_timeout_secs: 90,
            max_retries: 3,
            json_mode: JsonMode::JsonObject,
            stream_plans: true,
            max_parallel_calls: 6,
            company_context: CompanyContext::default(),
            workspaces: vec![],
            web_search: WebSearchConfig::default(),
            tool_policy: ToolPolicy::Authors,
            max_tool_steps: 6,
            pricing: Pricing::new(),
        }
    }
}

/// Endpoint đã "phẳng hoá" cho 1 role — thứ duy nhất `llm_client` cần biết.
#[derive(Clone)]
pub struct ResolvedEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub json_mode: JsonMode,
}

/// Bỏ `/chat/completions` thừa ở cuối để ghép được cả `/chat/completions` lẫn `/models`.
pub fn api_root(base_url: &str) -> &str {
    let base = base_url.trim().trim_end_matches('/');
    base.strip_suffix("/chat/completions").unwrap_or(base)
}

impl ResolvedEndpoint {
    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", api_root(&self.base_url))
    }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl AppConfig {
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    fn provider_mut(&mut self, id: &str) -> Option<&mut Provider> {
        self.providers.iter_mut().find(|p| p.id == id)
    }

    pub fn role(&self, id: &str) -> Option<&RoleDef> {
        self.roles.iter().find(|r| r.id == id)
    }

    pub fn department(&self, id: &str) -> Option<&Department> {
        self.departments.iter().find(|d| d.id == id)
    }

    /// Endpoint của role. None nếu role không tồn tại hoặc chưa gán provider hợp lệ
    /// (`validate_ready()` chặn trường hợp này trước khi bắt đầu phiên).
    pub fn resolve(&self, role_id: &str) -> Option<ResolvedEndpoint> {
        let r = self.role(role_id)?;
        let p = self.provider(&r.provider_id)?;
        Some(ResolvedEndpoint {
            base_url: p.base_url.trim().to_owned(),
            api_key: non_empty(&p.api_key).map(str::to_owned),
            model: r.model.trim().to_owned(),
            temperature: r.temperature,
            max_tokens: self.max_tokens,
            json_mode: self.json_mode,
        })
    }

    pub fn redacted(&self) -> AppConfig {
        let mut c = self.clone();
        for p in &mut c.providers {
            p.api_key = p.api_key.as_deref().map(mask_key);
        }
        c.web_search.api_key = c.web_search.api_key.as_deref().map(mask_key);
        c
    }

    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// Ghép config từ webview với key thật đang lưu (khớp provider theo `id`).
    pub fn merge_secrets(mut self, existing: &AppConfig) -> AppConfig {
        for p in &mut self.providers {
            let old = existing.provider(&p.id);
            // Nơi lưu key chỉ đổi qua `set_secret` (cần người dùng đồng ý) → lấy theo bản đang lưu.
            p.key_storage = old.map(|o| o.key_storage).unwrap_or_default();
            p.api_key = match p.api_key.take() {
                Some(k) if k.starts_with(MASK_PREFIX) => match old {
                    Some(o) if api_root(&o.base_url) == api_root(&p.base_url) => o.api_key.clone(),
                    Some(_) => {
                        log::warn!("provider '{}' đổi base_url → bỏ key cũ, cần nhập lại", p.name);
                        None
                    }
                    None => None,
                },
                Some(k) if k.trim().is_empty() => None,
                Some(k) => Some(k.trim().to_owned()),
                None => None,
            };
        }
        // Key dịch vụ tìm kiếm: giữ key cũ chỉ khi vẫn cùng dịch vụ.
        let old = &existing.web_search;
        self.web_search.key_storage =
            if old.provider == self.web_search.provider { old.key_storage } else { KeyStorage::default() };
        self.web_search.api_key = match self.web_search.api_key.take() {
            Some(k) if k.starts_with(MASK_PREFIX) => {
                (old.provider == self.web_search.provider).then(|| old.api_key.clone()).flatten()
            }
            Some(k) if k.trim().is_empty() => None,
            Some(k) => Some(k.trim().to_owned()),
            None => None,
        };
        self.schema_version = CONFIG_SCHEMA_VERSION;
        self
    }

    pub fn set_models(&mut self, provider_id: &str, models: Vec<String>, at: DateTime<Utc>) -> bool {
        match self.provider_mut(provider_id) {
            Some(p) => {
                p.models = models;
                p.models_fetched_at = Some(at);
                true
            }
            None => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut errs = Vec::new();
        if !(1..=10).contains(&self.max_rounds) {
            errs.push("max_rounds phải trong khoảng 1..=10".to_owned());
        }
        if !(10..=1800).contains(&self.request_timeout_secs) {
            errs.push("request_timeout_secs phải trong khoảng 10..=1800".to_owned());
        }
        if !(10..=600).contains(&self.stream_idle_timeout_secs) {
            errs.push("stream_idle_timeout_secs phải trong khoảng 10..=600".to_owned());
        }
        if self.max_retries > 6 {
            errs.push("max_retries tối đa 6".to_owned());
        }
        if !(1..=16).contains(&self.max_parallel_calls) {
            errs.push("max_parallel_calls phải trong khoảng 1..=16".to_owned());
        }
        if self.max_tokens == Some(0) {
            errs.push("max_tokens phải ≥ 1 hoặc để trống".to_owned());
        }
        if self.max_tool_steps != 0 && !(1..=12).contains(&self.max_tool_steps) {
            errs.push("max_tool_steps phải trong khoảng 1..=12 hoặc 0 (không giới hạn)".to_owned());
        }
        if !(1..=10).contains(&self.web_search.max_results) {
            errs.push("web_search.max_results phải trong khoảng 1..=10".to_owned());
        }
        for (model, p) in &self.pricing {
            if model.trim().is_empty() {
                errs.push("bảng giá: tên model không được để trống".to_owned());
            }
            if [p.input_per_m, p.cached_per_m, p.output_per_m].iter().any(|v| !v.is_finite() || *v < 0.0) {
                errs.push(format!("bảng giá '{model}': giá phải là số hữu hạn ≥ 0"));
            }
        }
        if self.company_context.text.chars().count() > 20_000 {
            errs.push("bối cảnh công ty tối đa 20.000 ký tự".to_owned());
        }
        let mut seen = HashSet::new();
        for w in &self.workspaces {
            if w.id.trim().is_empty() || !seen.insert(w.id.as_str()) {
                errs.push(format!("workspace '{}': id trống hoặc bị trùng", w.name));
            }
            if w.name.trim().is_empty() || w.path.trim().is_empty() {
                errs.push("workspace cần tên và đường dẫn".to_owned());
            }
        }

        // Providers
        let mut seen = HashSet::new();
        for p in &self.providers {
            let label = if p.name.trim().is_empty() { &p.id } else { &p.name };
            if p.id.trim().is_empty() {
                errs.push("provider thiếu id".to_owned());
            } else if !seen.insert(p.id.as_str()) {
                errs.push(format!("provider id '{}' bị trùng", p.id));
            }
            if p.name.trim().is_empty() {
                errs.push(format!("provider '{}': tên không được để trống", p.id));
            }
            if let Err(e) = validate_url(&p.base_url) {
                errs.push(format!("provider '{label}': base_url không hợp lệ ({e})"));
            }
        }

        // Roles
        let mut seen = HashSet::new();
        for r in &self.roles {
            let label = if r.name.trim().is_empty() { &r.id } else { &r.name };
            if r.id.trim().is_empty() {
                errs.push("có role thiếu id".to_owned());
            } else if !seen.insert(r.id.as_str()) {
                errs.push(format!("role id '{}' bị trùng", r.id));
            }
            if r.name.trim().is_empty() {
                errs.push(format!("role '{}': tên không được để trống", r.id));
            }
            // Cho phép lưu role chưa gán provider/model; chỉ bắt buộc khi role tham gia họp.
            if !r.provider_id.is_empty() && self.provider(&r.provider_id).is_none() {
                errs.push(format!("{label}: provider '{}' không tồn tại", r.provider_id));
            }
            if !(0.0..=2.0).contains(&r.temperature) {
                errs.push(format!("{label}: temperature phải trong 0..=2"));
            }
        }
        let role_ok = |id: &str| self.role(id).is_some();

        // Departments
        let mut seen = HashSet::new();
        for d in &self.departments {
            let label = if d.name.trim().is_empty() { &d.id } else { &d.name };
            if d.id.trim().is_empty() {
                errs.push("có phòng ban thiếu id".to_owned());
            } else if !seen.insert(d.id.as_str()) {
                errs.push(format!("phòng ban id '{}' bị trùng", d.id));
            }
            if d.name.trim().is_empty() {
                errs.push(format!("phòng ban '{}': tên không được để trống", d.id));
            }
            if !role_ok(&d.lead_id) {
                errs.push(format!("phòng ban {label}: chưa chọn trưởng phòng hợp lệ"));
            }
            for m in &d.member_ids {
                if !role_ok(m) {
                    errs.push(format!("phòng ban {label}: thành viên '{m}' không tồn tại"));
                } else if *m == d.lead_id {
                    errs.push(format!("phòng ban {label}: trưởng phòng không được đồng thời là thành viên"));
                }
            }
            if d.member_ids.len() > MAX_REVIEWERS {
                errs.push(format!("phòng ban {label}: tối đa {MAX_REVIEWERS} thành viên"));
            }
            if !(1..=5).contains(&d.max_rounds) {
                errs.push(format!("phòng ban {label}: số vòng phải trong 1..=5"));
            }
        }

        // Meeting
        let m = &self.meeting;
        if !role_ok(&m.chair_id) {
            errs.push("cuộc họp chính: chưa chọn chủ trì hợp lệ".to_owned());
        }
        if m.reviewer_ids.is_empty() {
            errs.push("cuộc họp chính: cần ít nhất 1 reviewer".to_owned());
        }
        if m.reviewer_ids.len() > MAX_REVIEWERS {
            errs.push(format!("cuộc họp chính: tối đa {MAX_REVIEWERS} reviewer"));
        }
        for r in &m.reviewer_ids {
            if !role_ok(r) {
                errs.push(format!("cuộc họp chính: reviewer '{r}' không tồn tại"));
            } else if *r == m.chair_id {
                errs.push("cuộc họp chính: chủ trì không được đồng thời là reviewer".to_owned());
            }
        }
        for d in &m.department_ids {
            if self.department(d).is_none() {
                errs.push(format!("cuộc họp chính: phòng ban '{d}' không tồn tại"));
            }
        }
        for r in &m.required_role_ids {
            if !role_ok(r) {
                errs.push(format!("cuộc họp chính: role bắt buộc '{r}' không tồn tại"));
            }
        }
        if !(1..=MAX_REVIEWERS as u32).contains(&m.max_auto_reviewers) {
            errs.push(format!("số reviewer AI được chọn phải trong 1..={MAX_REVIEWERS}"));
        }
        if m.max_auto_departments > 8 {
            errs.push("số phòng ban AI được chọn tối đa 8".to_owned());
        }
        if m.max_invites > 6 {
            errs.push("số người được mời thêm tối đa 6".to_owned());
        }
        if m.max_total_tokens.is_some_and(|t| t < MIN_TOTAL_TOKENS) {
            errs.push(format!("ngân sách token của phiên phải ≥ {MIN_TOTAL_TOKENS} hoặc để trống"));
        }
        if !(0.1..=1.0).contains(&m.section_mode_max_ratio) {
            errs.push("tỉ lệ tối đa của section revise phải trong 0.1..=1.0".to_owned());
        }
        if m.consistency_checker != CONSISTENCY_CHECKER_AUTHOR && !role_ok(&m.consistency_checker) {
            errs.push(format!("người kiểm tra nhất quán '{}' không tồn tại", m.consistency_checker));
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// Role tham gia phiên (chủ trì, reviewer, trưởng phòng + thành viên của phòng ban được chọn).
    pub fn participants(&self) -> Vec<&str> {
        let m = &self.meeting;
        let mut ids: Vec<&str> = vec![m.chair_id.as_str()];
        ids.extend(m.reviewer_ids.iter().map(String::as_str));
        for d in m.department_ids.iter().filter_map(|id| self.department(id)) {
            ids.push(&d.lead_id);
            ids.extend(d.member_ids.iter().map(String::as_str));
        }
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(*id));
        ids
    }

    /// Kiểm tra trước khi họp: cấu trúc hợp lệ + mọi role tham gia đã có provider và model.
    pub fn validate_ready(&self) -> Result<(), String> {
        self.validate()?;
        if self.providers.is_empty() {
            return Err("Chưa có provider nào — vào Cài đặt → Provider để thêm API key".into());
        }
        let mut no_provider = Vec::new();
        let mut no_model = Vec::new();
        for id in self.participants() {
            let Some(r) = self.role(id) else { continue };
            if self.provider(&r.provider_id).is_none() {
                no_provider.push(r.name.as_str());
            }
            if r.model.trim().is_empty() {
                no_model.push(r.name.as_str());
            }
        }
        let list = |v: &[&str]| {
            let head = v.iter().take(6).copied().collect::<Vec<_>>().join(", ");
            if v.len() > 6 { format!("{head}, …") } else { head }
        };
        let mut errs = Vec::new();
        if !no_provider.is_empty() {
            errs.push(format!("{} role tham gia chưa chọn provider: {}", no_provider.len(), list(&no_provider)));
        }
        if !no_model.is_empty() {
            errs.push(format!("{} role tham gia chưa chọn model: {}", no_model.len(), list(&no_model)));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// Đọc config từ JSON, tự migrate v1/v2 → v3. Trả về `(config, đã_migrate)`.
    pub fn from_json(bytes: &[u8]) -> Result<(AppConfig, bool), serde_json::Error> {
        let v: Value = serde_json::from_slice(bytes)?;
        let version = v.get("schema_version").and_then(Value::as_u64).unwrap_or(1);
        if v.get("default_endpoint").is_some() || version <= 1 {
            let legacy: LegacyV1 = serde_json::from_value(v)?;
            return Ok((legacy.migrate().migrate(), true));
        }
        if version == 2 {
            let v2: ConfigV2 = serde_json::from_value(v)?;
            return Ok((v2.migrate(), true));
        }
        // Role lưu trước khi có `appearance` → điền ngoại hình mẫu của danh mục (nếu có).
        let missing: HashSet<String> = v
            .get("roles")
            .and_then(Value::as_array)
            .map(|roles| {
                roles
                    .iter()
                    .filter(|r| r.get("appearance").is_none())
                    .filter_map(|r| r.get("id").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let mut cfg: AppConfig = serde_json::from_value(v)?;
        let mut filled = false;
        for r in cfg.roles.iter_mut().filter(|r| missing.contains(&r.id)) {
            if let Some(a) = roster::seed_appearance(&r.id) {
                r.appearance = a;
                filled = true;
            }
        }
        Ok((cfg, filled))
    }
}

pub fn validate_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| e.to_string())?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host_str().unwrap_or_default();
            if !matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
                log::warn!("base_url dùng http:// tới host không phải loopback ({host}) — API key sẽ đi plaintext");
            }
            Ok(())
        }
        s => Err(format!("scheme '{s}' không được hỗ trợ")),
    }
}

pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() > 8 {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{MASK_PREFIX}{tail}")
    } else {
        MASK_PREFIX.to_owned()
    }
}

// ───────────────────────────── Migrate v2 → v3 ─────────────────────────────

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct AgentModelV2 {
    provider_id: String,
    model: String,
    temperature: f32,
}

impl Default for AgentModelV2 {
    fn default() -> Self {
        Self {
            provider_id: DEFAULT_PROVIDER_ID.into(),
            model: DEFAULT_MODEL.into(),
            temperature: DEFAULT_TEMPERATURE,
        }
    }
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default)]
struct AgentModelsV2 {
    architect: AgentModelV2,
    scale_reviewer: AgentModelV2,
    chaos_reviewer: AgentModelV2,
    pragmatic_reviewer: AgentModelV2,
}

#[derive(Deserialize)]
#[serde(default)]
struct ConfigV2 {
    providers: Vec<Provider>,
    agents: AgentModelsV2,
    max_tokens: Option<u32>,
    max_rounds: u32,
    request_timeout_secs: u64,
    stream_idle_timeout_secs: u64,
    max_retries: u32,
    json_mode: JsonMode,
    stream_architect: bool,
}

impl Default for ConfigV2 {
    fn default() -> Self {
        let d = AppConfig::default();
        Self {
            providers: vec![Provider::default()],
            agents: AgentModelsV2::default(),
            max_tokens: None,
            max_rounds: d.max_rounds,
            request_timeout_secs: d.request_timeout_secs,
            stream_idle_timeout_secs: d.stream_idle_timeout_secs,
            max_retries: d.max_retries,
            json_mode: d.json_mode,
            stream_architect: true,
        }
    }
}

impl ConfigV2 {
    /// 4 role cũ giữ model đã gán; role mới trong danh mục dùng model của reviewer cũ.
    fn migrate(self) -> AppConfig {
        let base = &self.agents.scale_reviewer;
        let mut roles = roster::default_roles(&base.provider_id, &base.model, base.temperature);
        let legacy = [
            (roster::LEGACY_ARCHITECT, &self.agents.architect),
            ("scale_reviewer", &self.agents.scale_reviewer),
            ("chaos_reviewer", &self.agents.chaos_reviewer),
            ("pragmatic_reviewer", &self.agents.pragmatic_reviewer),
        ];
        for (id, a) in legacy {
            if let Some(r) = roles.iter_mut().find(|r| r.id == id) {
                r.provider_id = a.provider_id.clone();
                r.model = a.model.clone();
                r.temperature = a.temperature;
            }
        }
        let providers = if self.providers.is_empty() {
            vec![Provider::default()]
        } else {
            self.providers
        };
        AppConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            providers,
            roles,
            departments: roster::default_departments(),
            meeting: roster::default_meeting(),
            max_tokens: self.max_tokens,
            max_rounds: self.max_rounds,
            request_timeout_secs: self.request_timeout_secs,
            stream_idle_timeout_secs: self.stream_idle_timeout_secs,
            max_retries: self.max_retries,
            json_mode: self.json_mode,
            stream_plans: self.stream_architect,
            ..AppConfig::default()
        }
    }
}

// ───────────────────────────── Migrate v1 → v2 ─────────────────────────────

#[derive(Deserialize)]
#[serde(default)]
struct LegacyEndpoint {
    base_url: String,
    api_key: Option<String>,
    model: String,
    temperature: f32,
    max_tokens: Option<u32>,
}

impl Default for LegacyEndpoint {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            model: DEFAULT_MODEL.into(),
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: None,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyOverride {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyOverrides {
    architect: LegacyOverride,
    scale_reviewer: LegacyOverride,
    chaos_reviewer: LegacyOverride,
    pragmatic_reviewer: LegacyOverride,
}

#[derive(Deserialize)]
#[serde(default)]
struct LegacyV1 {
    default_endpoint: LegacyEndpoint,
    agents: LegacyOverrides,
    max_rounds: u32,
    request_timeout_secs: u64,
    stream_idle_timeout_secs: u64,
    max_retries: u32,
    json_mode: JsonMode,
    stream_architect: bool,
}

impl Default for LegacyV1 {
    fn default() -> Self {
        let d = AppConfig::default();
        Self {
            default_endpoint: LegacyEndpoint::default(),
            agents: LegacyOverrides::default(),
            max_rounds: d.max_rounds,
            request_timeout_secs: d.request_timeout_secs,
            stream_idle_timeout_secs: d.stream_idle_timeout_secs,
            max_retries: d.max_retries,
            json_mode: d.json_mode,
            stream_architect: true,
        }
    }
}

fn host_name(base_url: &str) -> String {
    match reqwest::Url::parse(base_url.trim()) {
        Ok(u) => match u.host_str() {
            Some("api.openai.com") => "OpenAI".into(),
            Some("openrouter.ai") => "OpenRouter".into(),
            Some(h) => match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_owned(),
            },
            None => base_url.to_owned(),
        },
        Err(_) => base_url.to_owned(),
    }
}

impl LegacyV1 {
    /// Mỗi cặp (base_url, api_key) khác nhau → 1 provider. Role giữ nguyên model/temperature cũ.
    fn migrate(self) -> ConfigV2 {
        let d = &self.default_endpoint;
        let mut providers = vec![Provider {
            id: DEFAULT_PROVIDER_ID.into(),
            name: host_name(&d.base_url),
            base_url: d.base_url.trim().to_owned(),
            api_key: non_empty(&d.api_key).map(str::to_owned),
            key_storage: KeyStorage::default(),
            models: vec![],
            models_fetched_at: None,
        }];
        let mut pick = |o: &LegacyOverride, label: &str| -> AgentModelV2 {
            let base = non_empty(&o.base_url).unwrap_or(d.base_url.trim()).to_owned();
            let key = non_empty(&o.api_key).or(non_empty(&d.api_key)).map(str::to_owned);
            let pid = match providers
                .iter()
                .find(|p| api_root(&p.base_url) == api_root(&base) && p.api_key == key)
            {
                Some(p) => p.id.clone(),
                None => {
                    let id = format!("migrated-{}", providers.len());
                    let mut name = host_name(&base);
                    if providers.iter().any(|p| p.name == name) {
                        name = format!("{name} ({label})");
                    }
                    providers.push(Provider {
                        id: id.clone(),
                        name,
                        base_url: base,
                        api_key: key,
                        key_storage: KeyStorage::default(),
                        models: vec![],
                        models_fetched_at: None,
                    });
                    id
                }
            };
            AgentModelV2 {
                provider_id: pid,
                model: non_empty(&o.model).unwrap_or(d.model.trim()).to_owned(),
                temperature: o.temperature.unwrap_or(d.temperature),
            }
        };
        let agents = AgentModelsV2 {
            architect: pick(&self.agents.architect, "Architect"),
            scale_reviewer: pick(&self.agents.scale_reviewer, "Scale"),
            chaos_reviewer: pick(&self.agents.chaos_reviewer, "Chaos"),
            pragmatic_reviewer: pick(&self.agents.pragmatic_reviewer, "Pragmatic"),
        };
        ConfigV2 {
            providers,
            agents,
            max_tokens: d.max_tokens,
            max_rounds: self.max_rounds,
            request_timeout_secs: self.request_timeout_secs,
            stream_idle_timeout_secs: self.stream_idle_timeout_secs,
            max_retries: self.max_retries,
            json_mode: self.json_mode,
            stream_architect: self.stream_architect,
        }
    }
}

// ── Debug thủ công: tuyệt đối không lộ key ──

fn fmt_key(k: &Option<String>) -> &'static str {
    if k.as_deref().is_some_and(|s| !s.is_empty()) {
        "<redacted>"
    } else {
        "<none>"
    }
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &fmt_key(&self.api_key))
            .field("key_storage", &self.key_storage)
            .field("models", &self.models.len())
            .finish()
    }
}

impl fmt::Debug for WebSearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchConfig")
            .field("provider", &self.provider)
            .field("api_key", &fmt_key(&self.api_key))
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field("base_url", &self.base_url)
            .field("api_key", &fmt_key(&self.api_key))
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Config mặc định + 1 provider "default" được gán cho mọi role.
    pub(crate) fn with_default_provider(key: &str, model: &str) -> AppConfig {
        let mut c = AppConfig::default();
        c.providers.push(Provider {
            api_key: Some(key.into()),
            ..Provider::default()
        });
        for r in &mut c.roles {
            r.provider_id = DEFAULT_PROVIDER_ID.into();
            r.model = model.into();
        }
        c
    }

    fn with_openrouter() -> AppConfig {
        let mut c = with_default_provider("sk-default-123456", "gpt-4.1-mini");
        c.providers.push(Provider {
            id: "or".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1/".into(),
            api_key: Some("sk-or-abcdefgh9999".into()),
            ..Provider::default()
        });
        let cto = c.roles.iter_mut().find(|r| r.id == "cto").unwrap();
        cto.provider_id = "or".into();
        cto.model = "anthropic/claude-big".into();
        c
    }

    #[test]
    fn max_tool_steps_zero_means_unlimited() {
        let mut c = AppConfig::default();
        c.max_tool_steps = 0;
        c.validate().unwrap();
        c.max_tool_steps = 13;
        assert!(c.validate().unwrap_err().contains("max_tool_steps"));
    }

    #[test]
    fn default_config_is_valid_and_has_full_roster() {
        let c = AppConfig::default();
        c.validate().unwrap();
        // Không còn provider mặc định → chưa họp được cho tới khi người dùng thêm provider.
        assert!(c.providers.is_empty());
        assert!(c.validate_ready().unwrap_err().contains("Chưa có provider"));
        let genders: HashSet<&str> = c.roles.iter().map(|r| r.appearance.gender.as_str()).collect();
        assert!(genders.contains("male") && genders.contains("female"), "danh mục role phải có cả nam và nữ");
        for id in ["ceo", "cto", "cfo", "tech_lead_missing"] {
            assert_eq!(c.role(id).is_some(), id != "tech_lead_missing");
        }
        for id in ["qa_engineer", "qc_specialist", "tester", "ua_manager", "monetization_manager", "ui_ux_designer"] {
            assert!(c.role(id).is_some(), "thiếu role {id}");
        }
        assert!(c.departments.len() >= 6);
        assert!(c.meeting.department_ids.is_empty());
    }

    #[test]
    fn resolve_uses_role_provider_and_model() {
        let c = with_openrouter();
        let a = c.resolve("cto").unwrap();
        assert_eq!(a.model, "anthropic/claude-big");
        assert_eq!(a.chat_url(), "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(a.api_key.as_deref(), Some("sk-or-abcdefgh9999"));
        let s = c.resolve("tester").unwrap();
        assert_eq!(s.model, "gpt-4.1-mini");
        assert_eq!(s.api_key.as_deref(), Some("sk-default-123456"));
        assert!(c.resolve("nobody").is_none());
        assert_eq!(api_root("https://x/v1/chat/completions/"), "https://x/v1");
    }

    #[test]
    fn redact_and_merge_roundtrip_keeps_secret() {
        let stored = with_openrouter();
        let view = stored.redacted();
        assert_eq!(view.providers[0].api_key.as_deref(), Some("••••3456"));
        assert!(!format!("{view:?}{stored:?}").contains("sk-default"));

        let mut incoming = view.clone();
        incoming.providers[1].api_key = Some(String::new());
        incoming.providers.push(Provider {
            id: "new".into(),
            name: "Local".into(),
            base_url: "http://localhost:11434/v1".into(),
            api_key: Some(" sk-new ".into()),
            ..Provider::default()
        });
        let merged = incoming.merge_secrets(&stored);
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-default-123456"));
        assert_eq!(merged.providers[1].api_key, None);
        assert_eq!(merged.providers[2].api_key.as_deref(), Some("sk-new"));
    }

    #[test]
    fn real_keys_are_never_serialized_and_storage_comes_from_saved_config() {
        let mut stored = with_openrouter();
        stored.providers[1].key_storage = KeyStorage::File;
        stored.web_search.provider = WebSearchProvider::Tavily;
        stored.web_search.api_key = Some("tvly-secret-123456".into());
        stored.web_search.key_storage = KeyStorage::Memory;
        let json = serde_json::to_string(&stored).unwrap();
        assert!(!json.contains("sk-default") && !json.contains("sk-or-") && !json.contains("tvly-"), "{json}");
        assert!(json.contains(r#""key_storage":"file""#));
        // Bản mask vẫn đi ra webview được.
        let view = serde_json::to_value(stored.redacted()).unwrap();
        assert_eq!(view["providers"][0]["api_key"], "••••3456");
        assert_eq!(view["web_search"]["key_storage"], "memory");

        // Webview gửi key_storage khác → bỏ qua, chỉ `set_secret` mới đổi được nơi lưu.
        let mut incoming = stored.redacted();
        incoming.providers[0].key_storage = KeyStorage::File;
        incoming.providers[1].key_storage = KeyStorage::Keyring;
        incoming.web_search.key_storage = KeyStorage::File;
        let merged = incoming.merge_secrets(&stored);
        assert_eq!(merged.providers[0].key_storage, KeyStorage::Keyring);
        assert_eq!(merged.providers[1].key_storage, KeyStorage::File);
        assert_eq!(merged.web_search.key_storage, KeyStorage::Memory);
        assert_eq!(merged.web_search.api_key.as_deref(), Some("tvly-secret-123456"));

        // Config cũ (không có key_storage) → mặc định keyring, key plaintext vẫn đọc được để migrate.
        let (c, _) = AppConfig::from_json(
            br#"{"schema_version":3,"providers":[{"id":"a","name":"A","base_url":"https://x/v1","api_key":"sk-legacy-1234"}]}"#,
        )
        .unwrap();
        assert_eq!(c.providers[0].key_storage, KeyStorage::Keyring);
        assert_eq!(c.providers[0].api_key.as_deref(), Some("sk-legacy-1234"));
    }

    #[test]
    fn masked_key_is_dropped_when_base_url_changes() {
        let stored = with_openrouter();
        let mut incoming = stored.redacted();
        incoming.providers[0].base_url = "https://evil.example.com/v1".into();
        let merged = incoming.merge_secrets(&stored);
        assert_eq!(merged.providers[0].api_key, None);
    }

    #[test]
    fn validate_rejects_bad_roles_departments_and_meeting() {
        let mut c = with_default_provider("sk-x", "m");
        c.max_rounds = 0;
        c.providers[0].base_url = "ftp://x".into();
        c.roles.iter_mut().find(|r| r.id == "tester").unwrap().provider_id = "ghost-provider".into();
        c.roles.push(RoleDef { id: "ceo".into(), name: "Dup".into(), ..RoleDef::default() });
        c.departments[0].lead_id = "ghost".into();
        let lead = c.departments[1].lead_id.clone();
        c.departments[1].member_ids.push(lead);
        c.meeting.reviewer_ids.push(c.meeting.chair_id.clone());
        c.meeting.department_ids.push("nope".into());
        let e = c.validate().unwrap_err();
        for needle in [
            "max_rounds",
            "base_url",
            "Tester: provider 'ghost-provider' không tồn tại",
            "role id 'ceo' bị trùng",
            "chưa chọn trưởng phòng",
            "trưởng phòng không được đồng thời là thành viên",
            "chủ trì không được đồng thời là reviewer",
            "phòng ban 'nope' không tồn tại",
        ] {
            assert!(e.contains(needle), "thiếu lỗi '{needle}' trong: {e}");
        }
    }

    #[test]
    fn max_total_tokens_defaults_to_none_and_must_be_at_least_1000() {
        let c = with_default_provider("sk-x", "m");
        assert_eq!(c.meeting.max_total_tokens, None);
        let m: MeetingConfig = serde_json::from_str(r#"{"chair_id":"cto"}"#).unwrap();
        assert_eq!(m.max_total_tokens, None, "config cũ không có field → None");
        let mut c2 = c.clone();
        c2.meeting.max_total_tokens = Some(999);
        assert!(c2.validate().unwrap_err().contains("ngân sách token"));
        c2.meeting.max_total_tokens = Some(1000);
        c2.validate().unwrap();
    }

    #[test]
    fn pricing_defaults_empty_and_is_validated() {
        let c = with_default_provider("sk-x", "m");
        assert!(c.pricing.is_empty());
        // Config cũ không có field `pricing` → rỗng.
        let old: AppConfig = serde_json::from_str(r#"{"schema_version":3,"max_rounds":3}"#).unwrap();
        assert!(old.pricing.is_empty());
        let mut c2 = c.clone();
        c2.pricing.insert("m".into(), ModelPrice { input_per_m: 2.0, cached_per_m: 0.5, output_per_m: 8.0 });
        c2.validate().unwrap();
        // 1M prompt (trong đó 400k cache) + 100k output.
        let cost = c2.pricing["m"].cost(1_000_000, 400_000, 100_000);
        assert!((cost - (1.2 + 0.2 + 0.8)).abs() < 1e-9, "{cost}");
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let mut c3 = c.clone();
            c3.pricing.insert("m".into(), ModelPrice { input_per_m: 1.0, cached_per_m: bad, output_per_m: 1.0 });
            assert!(c3.validate().unwrap_err().contains("bảng giá"), "{bad}");
        }
        let mut c4 = c.clone();
        c4.pricing.insert(" ".into(), ModelPrice::default());
        assert!(c4.validate().is_err());
    }

    #[test]
    fn save_allows_unassigned_roles_but_start_requires_participants_ready() {
        let mut c = with_default_provider("sk-x", "gpt-4.1-mini");
        // Role không tham gia họp được phép chưa gán provider/model.
        let tester = c.roles.iter_mut().find(|r| r.id == "tester").unwrap();
        tester.provider_id.clear();
        tester.model.clear();
        c.validate().unwrap();
        c.validate_ready().unwrap();

        // Khi tester tham gia (qua phòng QA) → báo lỗi gọn, có tên role.
        c.meeting.department_ids = vec!["qa".into()];
        let e = c.validate_ready().unwrap_err();
        assert!(e.contains("1 role tham gia chưa chọn provider: Tester"), "{e}");
        assert!(e.contains("1 role tham gia chưa chọn model: Tester"), "{e}");
        assert!(c.resolve("tester").is_none());
    }

    #[test]
    fn v3_roles_without_appearance_get_seed_look() {
        let json = br#"{"schema_version":3,"roles":[
            {"id":"ceo","name":"CEO"},
            {"id":"custom","name":"X"},
            {"id":"cto","name":"CTO","appearance":{"gender":"female"}}
        ]}"#;
        let (c, migrated) = AppConfig::from_json(json).unwrap();
        assert!(migrated);
        assert_eq!(c.role("ceo").unwrap().appearance.gender, "female");
        assert_eq!(c.role("ceo").unwrap().appearance.accessory, "crown");
        assert_eq!(c.role("custom").unwrap().appearance, Appearance::default());
        // Không đè ngoại hình người dùng đã chỉnh.
        assert_eq!(c.role("cto").unwrap().appearance.gender, "female");
        assert_eq!(c.role("cto").unwrap().appearance.outfit, "");
    }

    #[test]
    fn v3_files_deserialize_with_defaults() {
        let (c, migrated) = AppConfig::from_json(br#"{"schema_version":3,"max_rounds":2}"#).unwrap();
        assert!(!migrated);
        assert_eq!(c.max_rounds, 2);
        assert!(c.role("ceo").is_some());
        c.validate().unwrap();
    }

    #[test]
    fn v2_config_is_migrated_keeping_role_models() {
        let v2 = br#"{
            "schema_version": 2,
            "providers": [
                {"id":"default","name":"OpenAI","base_url":"https://api.openai.com/v1","api_key":"sk-main-12345678"},
                {"id":"or","name":"OpenRouter","base_url":"https://openrouter.ai/api/v1","api_key":"sk-or-xxxxxxxx"}
            ],
            "agents": {
                "architect": {"provider_id":"or","model":"anthropic/claude","temperature":0.2},
                "scale_reviewer": {"provider_id":"default","model":"gpt-4.1-mini","temperature":0.3},
                "chaos_reviewer": {"provider_id":"default","model":"gpt-4.1","temperature":0.4},
                "pragmatic_reviewer": {"provider_id":"or","model":"qwen","temperature":0.5}
            },
            "max_rounds": 4,
            "stream_architect": false
        }"#;
        let (c, migrated) = AppConfig::from_json(v2).unwrap();
        assert!(migrated);
        assert_eq!(c.schema_version, 3);
        assert_eq!((c.max_rounds, c.stream_plans), (4, false));
        let arch = c.resolve("architect").unwrap();
        assert_eq!((arch.model.as_str(), arch.temperature), ("anthropic/claude", 0.2));
        assert_eq!(arch.api_key.as_deref(), Some("sk-or-xxxxxxxx"));
        assert_eq!(c.resolve("chaos_reviewer").unwrap().model, "gpt-4.1");
        // role mới dùng model của reviewer cũ
        assert_eq!(c.resolve("ceo").unwrap().model, "gpt-4.1-mini");
        assert_eq!(c.meeting.chair_id, "architect");
        c.validate().unwrap();
    }

    #[test]
    fn v1_config_is_migrated_all_the_way() {
        let v1 = br#"{
            "schema_version": 1,
            "default_endpoint": {"base_url":"https://api.openai.com/v1","api_key":"sk-main-12345678","model":"gpt-4.1","temperature":0.5,"max_tokens":4000},
            "agents": {
                "architect": {"base_url":"https://openrouter.ai/api/v1","api_key":"sk-or-xxxxxxxx","model":"anthropic/claude"},
                "scale_reviewer": {"model":"gpt-4.1-mini","temperature":0.1},
                "pragmatic_reviewer": {"base_url":"https://openrouter.ai/api/v1/","api_key":"sk-or-xxxxxxxx"}
            },
            "max_rounds": 4
        }"#;
        let (c, migrated) = AppConfig::from_json(v1).unwrap();
        assert!(migrated);
        assert_eq!(c.schema_version, 3);
        assert_eq!(c.max_tokens, Some(4000));
        assert_eq!(c.providers.len(), 2, "{:?}", c.providers);
        let arch = c.resolve("architect").unwrap();
        assert_eq!(arch.model, "anthropic/claude");
        assert_eq!(arch.api_key.as_deref(), Some("sk-or-xxxxxxxx"));
        assert_eq!(
            c.role("pragmatic_reviewer").unwrap().provider_id,
            c.role("architect").unwrap().provider_id
        );
        let scale = c.resolve("scale_reviewer").unwrap();
        assert_eq!((scale.model.as_str(), scale.temperature), ("gpt-4.1-mini", 0.1));
        c.validate().unwrap();
    }

    #[test]
    fn legacy_file_without_version_is_migrated() {
        let (c, migrated) = AppConfig::from_json(br#"{"max_rounds":2}"#).unwrap();
        assert!(migrated);
        assert_eq!(c.max_rounds, 2);
        assert!(c.stream_plans);
        c.validate().unwrap();
    }
}
