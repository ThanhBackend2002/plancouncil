//! Kiểm tra "sức khoẻ" model của các role: gọi thử 1 request rất ngắn cho mỗi cặp
//! (provider, model) rồi phân loại lỗi (hết quota, sai key, model không tồn tại…).
//!
//! API chuẩn OpenAI không có endpoint xem số dư → cách đáng tin nhất là gọi thử và đọc lỗi.
//! Riêng OpenRouter có `GET /api/v1/key` trả về hạn mức/credit còn lại.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{api_root, AppConfig};
use crate::llm_client::{ChatMessage, Expect, LlmClient, LlmError};
use crate::models::truncate_chars;

/// Trạng thái của 1 cặp provider + model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
    /// Hết quota / credit / số dư (402, insufficient_quota…).
    Quota,
    /// Sai hoặc thiếu API key (401/403).
    Auth,
    /// Model không tồn tại hoặc không được phép dùng.
    NotFound,
    /// Bị giới hạn tốc độ (429 nhưng không phải hết quota) — thường vẫn dùng được sau ít phút.
    RateLimit,
    /// Request bị từ chối (400) — tham số/model không tương thích.
    BadRequest,
    Server,
    Timeout,
    Network,
    /// Role chưa gán provider/model.
    Unassigned,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelHealth {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub role_ids: Vec<String>,
    pub status: HealthStatus,
    pub message: String,
    pub latency_ms: Option<u64>,
    /// Số dư / hạn mức nếu provider có API (hiện hỗ trợ OpenRouter).
    pub balance: Option<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

/// Phân loại lỗi của provider thành trạng thái dễ hiểu.
pub fn classify(err: &LlmError) -> (HealthStatus, String) {
    match err {
        LlmError::Http { status, body } => {
            let b = body.to_lowercase();
            let quota_words = ["insufficient_quota", "quota", "credit", "billing", "balance", "payment", "exceeded your current", "insufficient funds", "余额"];
            let is_quota = quota_words.iter().any(|w| b.contains(w));
            let st = match status {
                402 => HealthStatus::Quota,
                429 if is_quota => HealthStatus::Quota,
                429 => HealthStatus::RateLimit,
                401 | 403 if is_quota => HealthStatus::Quota,
                401 | 403 => HealthStatus::Auth,
                404 => HealthStatus::NotFound,
                400 | 422 if b.contains("model") && (b.contains("not found") || b.contains("does not exist") || b.contains("not_found") || b.contains("invalid model") || b.contains("no such model") || b.contains("not available")) => {
                    HealthStatus::NotFound
                }
                400 | 422 if is_quota => HealthStatus::Quota,
                400 | 422 => HealthStatus::BadRequest,
                500..=599 => HealthStatus::Server,
                _ => HealthStatus::Other,
            };
            (st, format!("HTTP {status}: {}", truncate_chars(body, 300)))
        }
        LlmError::Timeout => (HealthStatus::Timeout, "Quá 30 giây không phản hồi".into()),
        LlmError::Network(e) => (HealthStatus::Network, truncate_chars(e, 300)),
        LlmError::InvalidResponse(e) | LlmError::JsonParse(e) => {
            let l = e.to_lowercase();
            if l.contains("quota") || l.contains("credit") || l.contains("balance") {
                (HealthStatus::Quota, truncate_chars(e, 300))
            } else {
                (HealthStatus::Other, truncate_chars(e, 300))
            }
        }
        LlmError::Cancelled => (HealthStatus::Other, "đã huỷ".into()),
    }
}

/// OpenRouter: `GET /api/v1/key` → `{data:{limit, usage, limit_remaining}}` (USD).
async fn openrouter_balance(http: &reqwest::Client, base_url: &str, key: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    if url.host_str()? != "openrouter.ai" {
        return None;
    }
    let v: Value = http
        .get(format!("{}/key", api_root(base_url)))
        .bearer_auth(key)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let d = v.get("data")?;
    let usage = d.get("usage").and_then(Value::as_f64).unwrap_or(0.0);
    Some(match (d.get("limit_remaining").and_then(Value::as_f64), d.get("limit").and_then(Value::as_f64)) {
        (Some(rem), Some(limit)) => format!("Còn ${rem:.2} / hạn mức ${limit:.2} (đã dùng ${usage:.2})"),
        _ => format!("Không giới hạn hạn mức key (đã dùng ${usage:.2})"),
    })
}

/// Kiểm tra model của các role (None = mọi role). Mỗi cặp provider+model chỉ gọi 1 lần.
pub async fn check_roles(http: &reqwest::Client, cfg: &AppConfig, role_ids: Option<&[String]>) -> Vec<ModelHealth> {
    let now = chrono::Utc::now();
    // (provider_id, model) → role ids
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut out = Vec::new();
    for r in cfg.roles.iter().filter(|r| role_ids.map_or(true, |ids| ids.contains(&r.id))) {
        if cfg.provider(&r.provider_id).is_none() || r.model.trim().is_empty() {
            out.push(ModelHealth {
                provider_id: r.provider_id.clone(),
                provider_name: String::new(),
                model: r.model.trim().to_owned(),
                role_ids: vec![r.id.clone()],
                status: HealthStatus::Unassigned,
                message: "Role chưa chọn provider/model".into(),
                latency_ms: None,
                balance: None,
                checked_at: now,
            });
            continue;
        }
        groups.entry((r.provider_id.clone(), r.model.trim().to_owned())).or_default().push(r.id.clone());
    }

    // Không retry, timeout ngắn để báo lỗi ngay.
    let llm = LlmClient::new(http.clone(), cfg).with_limits(0, Duration::from_secs(30));
    let msgs = [
        ChatMessage::system("You are a health check endpoint."),
        ChatMessage::user("Reply with exactly: OK"),
    ];
    let mut balances: BTreeMap<String, Option<String>> = BTreeMap::new();
    for pid in groups.keys().map(|(p, _)| p.clone()).collect::<std::collections::BTreeSet<_>>() {
        let p = cfg.provider(&pid).expect("đã lọc");
        let bal = match p.api_key.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => openrouter_balance(http, &p.base_url, k).await,
            None => None,
        };
        balances.insert(pid, bal);
    }

    let checks = stream::iter(groups.into_iter().map(|((pid, model), roles)| {
        let llm = &llm;
        let msgs = &msgs;
        let balances = &balances;
        async move {
            let p = cfg.provider(&pid).expect("đã lọc");
            let mut ep = cfg.resolve(&roles[0]).expect("role có provider");
            ep.max_tokens = Some(8);
            let t0 = Instant::now();
            let res = llm.chat(&ep, msgs, Expect::Text, &CancellationToken::new()).await;
            let latency = t0.elapsed().as_millis() as u64;
            let (status, message, latency_ms) = match res {
                Ok(c) => (HealthStatus::Ok, format!("Phản hồi: \"{}\"", truncate_chars(c.text.trim(), 60)), Some(latency)),
                // Model trả rỗng với max_tokens nhỏ vẫn chứng tỏ key + model dùng được.
                Err(LlmError::InvalidResponse(e)) if e.contains("content rỗng") => {
                    (HealthStatus::Ok, "Phản hồi rỗng (key và model vẫn dùng được)".into(), Some(latency))
                }
                Err(e) => {
                    let (s, m) = classify(&e);
                    (s, m, None)
                }
            };
            ModelHealth {
                provider_id: pid.clone(),
                provider_name: p.name.clone(),
                model,
                role_ids: roles,
                status,
                message,
                latency_ms,
                balance: balances.get(&pid).cloned().flatten(),
                checked_at: chrono::Utc::now(),
            }
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    out.extend(checks);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16, body: &str) -> LlmError {
        LlmError::Http { status, body: body.into() }
    }

    #[test]
    fn classify_common_provider_errors() {
        let cases = [
            (http(429, r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#), HealthStatus::Quota),
            (http(402, "Payment Required"), HealthStatus::Quota),
            (http(403, r#"{"error":"Key limit exceeded: insufficient credits"}"#), HealthStatus::Quota),
            (http(429, "Rate limit reached for requests"), HealthStatus::RateLimit),
            (http(401, "Incorrect API key provided"), HealthStatus::Auth),
            (http(404, "The model `gpt-9` does not exist"), HealthStatus::NotFound),
            (http(400, r#"{"error":"Invalid model: foo/bar is not a valid model ID","code":"model_not_found"}"#), HealthStatus::NotFound),
            (http(400, "max_tokens must be > 0"), HealthStatus::BadRequest),
            (http(503, "overloaded"), HealthStatus::Server),
            (LlmError::Timeout, HealthStatus::Timeout),
            (LlmError::Network("dns error".into()), HealthStatus::Network),
        ];
        for (err, want) in cases {
            assert_eq!(classify(&err).0, want, "{err}");
        }
    }

    #[tokio::test]
    async fn unassigned_roles_are_reported_without_network() {
        let mut cfg = AppConfig::default(); // không có provider
        cfg.roles.truncate(2);
        let res = check_roles(&reqwest::Client::new(), &cfg, None).await;
        assert_eq!(res.len(), 2);
        assert!(res.iter().all(|r| r.status == HealthStatus::Unassigned));
    }
}
