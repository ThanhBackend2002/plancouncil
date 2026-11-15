//! OpenAI-compatible Chat Completions client (`POST {base_url}/chat/completions`).
//!
//! - Retry exponential backoff + jitter cho 408/425/429/5xx/529/timeout/network, tôn trọng `Retry-After`.
//! - Mọi await đều `select!` với `CancellationToken` → bấm Stop là dừng ngay, không chờ hết request.
//! - `chat_json`: parse khoan dung (bỏ code fence, `<think>`, text thừa) + 1 lần "repair prompt".
//! - `chat_stream`: SSE parser tự viết (không thêm dependency), idle-timeout từng chunk,
//!   tự fallback nếu provider phớt lờ `stream: true` và trả JSON thường.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, CONTENT_TYPE, RETRY_AFTER};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::config::{api_root, AppConfig, JsonMode, ResolvedEndpoint};
use crate::models::truncate_chars;

const MAX_ERROR_BODY_CHARS: usize = 500;
const MAX_SSE_LINE_BYTES: usize = 1 << 20; // 1 MiB: chống server lỗi đẩy 1 dòng vô hạn
const MAX_BACKOFF: Duration = Duration::from_secs(30);

// ───────────────────────────── Types ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(c: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: c.into(),
        }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: c.into(),
        }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: c.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Text,
    Json,
}

pub enum StreamSignal<'a> {
    /// Bắt đầu (lại) 1 attempt → consumer phải xoá buffer cũ.
    Reset,
    Delta(&'a str),
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("đã huỷ")]
    Cancelled,
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("timeout khi gọi LLM")]
    Timeout,
    #[error("lỗi mạng: {0}")]
    Network(String),
    #[error("phản hồi không hợp lệ: {0}")]
    InvalidResponse(String),
    #[error("không parse được JSON từ model: {0}")]
    JsonParse(String),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Http { status, .. } => {
                matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
            }
            LlmError::Timeout | LlmError::Network(_) => true,
            _ => false,
        }
    }

    fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            LlmError::Timeout
        } else if e.is_decode() {
            LlmError::InvalidResponse(e.to_string())
        } else {
            LlmError::Network(e.to_string())
        }
    }
}

/// Token tiêu thụ của 1 lần gọi (theo `usage` provider trả về).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

impl Usage {
    pub fn add(self, o: Usage) -> Usage {
        Usage {
            prompt_tokens: self.prompt_tokens + o.prompt_tokens,
            completion_tokens: self.completion_tokens + o.completion_tokens,
        }
    }
}

/// Cộng 2 usage có thể thiếu (provider không trả usage → None).
pub fn sum_usage(a: Option<Usage>, b: Option<Usage>) -> Option<Usage> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.add(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Kết quả 1 lần gọi chat: nội dung + token (nếu provider báo).
#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub usage: Option<Usage>,
}

/// 1 lượt trong vòng gọi công cụ.
#[derive(Debug, Clone)]
pub struct ToolTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Message assistant nguyên bản (giữ `tool_calls`) để nối lại vào hội thoại.
    pub assistant_message: Value,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<ResponseMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Option<StreamDelta>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

// ───────────────────────────── Client ─────────────────────────────

/// Rẻ để clone (`reqwest::Client` bên trong là `Arc`) → share connection pool giữa 3 reviewer.
#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    max_retries: u32,
    request_timeout: Duration,
    stream_idle_timeout: Duration,
    /// Provider từ chối `stream_options` → tắt cho các lần stream sau (dùng chung giữa các clone).
    stream_usage_off: Arc<AtomicBool>,
}

pub fn build_http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent(concat!("PlanCouncil/", env!("CARGO_PKG_VERSION")))
        .build()
}

impl LlmClient {
    pub fn new(http: reqwest::Client, cfg: &AppConfig) -> Self {
        Self {
            http,
            max_retries: cfg.max_retries,
            request_timeout: Duration::from_secs(cfg.request_timeout_secs),
            stream_idle_timeout: Duration::from_secs(cfg.stream_idle_timeout_secs),
            stream_usage_off: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_limits(mut self, max_retries: u32, request_timeout: Duration) -> Self {
        self.max_retries = max_retries;
        self.request_timeout = request_timeout;
        self
    }

    fn build_body(
        &self,
        ep: &ResolvedEndpoint,
        messages: &[ChatMessage],
        stream: bool,
        json_rf: bool,
    ) -> Value {
        let mut body = json!({
            "model": ep.model,
            "messages": messages,
            "temperature": ep.temperature,
            "stream": stream,
        });
        if let Some(mt) = ep.max_tokens {
            body["max_tokens"] = json!(mt);
        }
        if json_rf {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if stream && !self.stream_usage_off.load(Ordering::Relaxed) {
            // Để chunk cuối có `usage` (OpenAI và đa số proxy hỗ trợ).
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }

    /// Non-streaming completion, trả về `choices[0].message.content` + usage.
    pub async fn chat(
        &self,
        ep: &ResolvedEndpoint,
        messages: &[ChatMessage],
        expect: Expect,
        cancel: &CancellationToken,
    ) -> Result<Completion, LlmError> {
        let mut use_rf = expect == Expect::Json && ep.json_mode == JsonMode::JsonObject;
        loop {
            let body = self.build_body(ep, messages, false, use_rf);
            match self
                .send_with_retry(ep, &body, cancel, Some(self.request_timeout))
                .await
            {
                Ok(resp) => return read_completion(resp, cancel).await,
                // Nhiều proxy/local model không hiểu `response_format` → hạ cấp 1 lần.
                Err(LlmError::Http {
                    status: 400 | 422,
                    ref body,
                }) if use_rf => {
                    log::warn!(
                        "model {} từ chối response_format=json_object ({}); thử lại không có response_format",
                        ep.model,
                        truncate_chars(body, 120)
                    );
                    use_rf = false;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Gọi model và parse JSON thành `T`. Trả về cả raw text (để đưa lại vào hội thoại khi cần sửa).
    /// Nếu parse fail → gửi lại 1 "repair prompt" kèm lỗi cụ thể.
    pub async fn chat_json<T: DeserializeOwned>(
        &self,
        ep: &ResolvedEndpoint,
        messages: &[ChatMessage],
        cancel: &CancellationToken,
    ) -> Result<(T, String, Option<Usage>), LlmError> {
        let first = self.chat(ep, messages, Expect::Json, cancel).await?;
        let text = first.text;
        let first_err = match parse_json_lenient::<T>(&text) {
            Ok(v) => return Ok((v, text, first.usage)),
            Err(e) => e,
        };
        log::warn!(
            "model {} trả JSON không hợp lệ ({first_err}); gửi repair prompt",
            ep.model
        );

        let mut repair = messages.to_vec();
        repair.push(ChatMessage::assistant(truncate_chars(&text, 8_000)));
        repair.push(ChatMessage::user(format!(
            "Your previous reply was NOT valid JSON for the required schema. Parser error: {first_err}. \
             Reply again with ONLY one valid JSON object matching the schema. No markdown fences, no prose."
        )));
        let second = self.chat(ep, &repair, Expect::Json, cancel).await?;
        let usage = sum_usage(first.usage, second.usage);
        parse_json_lenient::<T>(&second.text)
            .map(|v| (v, second.text, usage))
            .map_err(LlmError::JsonParse)
    }

    /// 1 lượt gọi có công cụ (OpenAI function calling, non-stream).
    /// `messages` là JSON thô vì cần chứa `tool_calls` / `role: "tool"`.
    pub async fn chat_tools(
        &self,
        ep: &ResolvedEndpoint,
        messages: &[Value],
        tools: &[Value],
        allow_tools: bool,
        cancel: &CancellationToken,
    ) -> Result<ToolTurn, LlmError> {
        let mut body = json!({
            "model": ep.model,
            "messages": messages,
            "temperature": ep.temperature,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!(if allow_tools { "auto" } else { "none" });
        }
        if let Some(mt) = ep.max_tokens {
            body["max_tokens"] = json!(mt);
        }
        let resp = self
            .send_with_retry(ep, &body, cancel, Some(self.request_timeout))
            .await?;
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            t = resp.text() => t.map_err(LlmError::from_reqwest)?,
        };
        parse_tool_turn(&text)
    }

    /// Streaming completion. `on` nhận `Reset` trước mỗi attempt và `Delta` cho từng mẩu text.
    pub async fn chat_stream<F>(
        &self,
        ep: &ResolvedEndpoint,
        messages: &[ChatMessage],
        cancel: &CancellationToken,
        mut on: F,
    ) -> Result<Completion, LlmError>
    where
        F: FnMut(StreamSignal<'_>) + Send,
    {
        let mut body = self.build_body(ep, messages, true, false);
        let mut attempt = 0u32;
        loop {
            on(StreamSignal::Reset);
            // Không đặt total-timeout cho stream: plan dài có thể chạy vài phút. Dùng idle-timeout.
            let resp = match self.send_with_retry(ep, &body, cancel, None).await {
                Ok(r) => r,
                // Provider không hiểu `stream_options` → bỏ đi, gửi lại (1 lần cho cả phiên).
                Err(LlmError::Http { status: 400 | 422, body: ref err })
                    if body.get("stream_options").is_some() =>
                {
                    log::warn!(
                        "model {} từ chối stream_options ({}); stream không kèm usage",
                        ep.model,
                        truncate_chars(err, 120)
                    );
                    self.stream_usage_off.store(true, Ordering::Relaxed);
                    if let Some(o) = body.as_object_mut() {
                        o.remove("stream_options");
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            match self.consume_stream(resp, cancel, &mut on).await {
                Ok(text) => return Ok(text),
                Err(LlmError::Cancelled) => return Err(LlmError::Cancelled),
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    attempt += 1;
                    let delay = backoff(attempt);
                    log::warn!(
                        "stream {} đứt giữa chừng ({e}); retry #{attempt} sau {delay:?}",
                        ep.model
                    );
                    sleep_cancellable(delay, cancel).await?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn send_with_retry(
        &self,
        ep: &ResolvedEndpoint,
        body: &Value,
        cancel: &CancellationToken,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, LlmError> {
        let url = ep.chat_url();
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let mut req = self.http.post(&url).json(body);
            if let Some(key) = ep.api_key.as_deref().filter(|k| !k.is_empty()) {
                req = req.bearer_auth(key);
            }
            if let Some(t) = timeout {
                req = req.timeout(t);
            }

            let outcome = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                r = req.send() => r,
            };

            let (err, retry_after) = match outcome {
                Ok(resp) if resp.status().is_success() => return Ok(resp),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = parse_retry_after(resp.headers());
                    let text = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                        t = resp.text() => t.unwrap_or_default(),
                    };
                    (
                        LlmError::Http {
                            status,
                            body: truncate_chars(text.trim(), MAX_ERROR_BODY_CHARS),
                        },
                        retry_after,
                    )
                }
                Err(e) => (LlmError::from_reqwest(e), None),
            };

            attempt += 1;
            if !err.is_retryable() || attempt > self.max_retries {
                return Err(err);
            }
            let delay = retry_after
                .unwrap_or_else(|| backoff(attempt))
                .min(MAX_BACKOFF);
            log::warn!(
                "LLM {} lỗi ({err}); retry {attempt}/{} sau {delay:?}",
                ep.model,
                self.max_retries
            );
            sleep_cancellable(delay, cancel).await?;
        }
    }

    async fn consume_stream<F>(
        &self,
        resp: reqwest::Response,
        cancel: &CancellationToken,
        on: &mut F,
    ) -> Result<Completion, LlmError>
    where
        F: FnMut(StreamSignal<'_>) + Send,
    {
        let is_json = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/json"));
        if is_json {
            // Provider bỏ qua stream=true → đọc như response thường.
            let c = read_completion(resp, cancel).await?;
            on(StreamSignal::Delta(&c.text));
            return Ok(c);
        }

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
        let mut out = String::new();
        let mut usage: Option<Usage> = None;

        'outer: loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                n = tokio::time::timeout(self.stream_idle_timeout, stream.next()) => n,
            };
            let chunk = match next {
                Err(_) => return Err(LlmError::Timeout),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(LlmError::from_reqwest(e)),
                Ok(Some(Ok(b))) => b,
            };
            buf.extend_from_slice(&chunk);

            // '\n' không bao giờ nằm giữa 1 ký tự UTF-8 multi-byte → split theo byte an toàn.
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                match parse_sse_line(&line)? {
                    SseLine::Delta(d) => {
                        out.push_str(&d);
                        on(StreamSignal::Delta(&d));
                    }
                    SseLine::Usage(u) => usage = Some(u),
                    SseLine::Done => break 'outer,
                    SseLine::Skip => {}
                }
            }
            if buf.len() > MAX_SSE_LINE_BYTES {
                return Err(LlmError::InvalidResponse("dòng SSE vượt quá 1 MiB".into()));
            }
        }
        if !buf.is_empty() {
            match parse_sse_line(&buf)? {
                SseLine::Delta(d) => {
                    out.push_str(&d);
                    on(StreamSignal::Delta(&d));
                }
                SseLine::Usage(u) => usage = Some(u),
                _ => {}
            }
        }
        if out.trim().is_empty() {
            return Err(LlmError::InvalidResponse(
                "stream kết thúc mà không có nội dung".into(),
            ));
        }
        Ok(Completion { text: out, usage })
    }
}

/// Parse 1 response non-stream có thể chứa `tool_calls`.
fn parse_tool_turn(text: &str) -> Result<ToolTurn, LlmError> {
    let v: Value = serde_json::from_str(text).map_err(|e| {
        LlmError::InvalidResponse(format!("{e}; body: {}", truncate_chars(text, 200)))
    })?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(LlmError::InvalidResponse(format!(
            "provider error: {}",
            truncate_chars(&err.to_string(), MAX_ERROR_BODY_CHARS)
        )));
    }
    let usage = v.get("usage").and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
    let msg = v
        .pointer("/choices/0/message")
        .cloned()
        .ok_or_else(|| LlmError::InvalidResponse("không có choices".into()))?;
    let content = msg
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty());
    let tool_calls: Vec<ToolCall> = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|c| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: c.get("id").and_then(Value::as_str).unwrap_or("call").to_owned(),
                        name: f.get("name")?.as_str()?.to_owned(),
                        arguments: match f.get("arguments") {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => "{}".into(),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Giữ message gốc nhưng bỏ field null/thừa để provider khác nhận lại được.
    let mut assistant = json!({ "role": "assistant", "content": content.clone().unwrap_or_default() });
    if let Some(tc) = msg.get("tool_calls").filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        assistant["tool_calls"] = tc.clone();
    }
    Ok(ToolTurn {
        content,
        tool_calls,
        assistant_message: assistant,
        usage,
    })
}

async fn read_completion(
    resp: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<Completion, LlmError> {
    let text = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
        t = resp.text() => t.map_err(LlmError::from_reqwest)?,
    };
    let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
        LlmError::InvalidResponse(format!("{e}; body: {}", truncate_chars(&text, 200)))
    })?;
    if let Some(err) = parsed.error {
        return Err(LlmError::InvalidResponse(format!(
            "provider error: {}",
            truncate_chars(&err.to_string(), MAX_ERROR_BODY_CHARS)
        )));
    }
    let usage = parsed.usage;
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::InvalidResponse("không có choices".into()))?;
    if choice.finish_reason.as_deref() == Some("length") {
        log::warn!("output bị cắt do max_tokens (finish_reason=length)");
    }
    let content = choice.message.and_then(|m| m.content).unwrap_or_default();
    if content.trim().is_empty() {
        return Err(LlmError::InvalidResponse("content rỗng".into()));
    }
    Ok(Completion { text: content, usage })
}

// ───────────────────────────── SSE ─────────────────────────────

#[derive(Debug, PartialEq)]
enum SseLine {
    Delta(String),
    /// Chunk cuối (khi gửi `stream_options.include_usage`).
    Usage(Usage),
    Done,
    Skip,
}

fn parse_sse_line(line: &[u8]) -> Result<SseLine, LlmError> {
    let s = std::str::from_utf8(line)
        .map_err(|e| LlmError::InvalidResponse(format!("SSE không phải UTF-8: {e}")))?
        .trim_end_matches(['\r', '\n']);
    // Bỏ qua comment (`: OPENROUTER PROCESSING`), `event:`, `id:`, dòng trống.
    let Some(data) = s.strip_prefix("data:") else {
        return Ok(SseLine::Skip);
    };
    let data = data.trim_start();
    if data.is_empty() {
        return Ok(SseLine::Skip);
    }
    if data == "[DONE]" {
        return Ok(SseLine::Done);
    }
    let chunk: StreamChunk = serde_json::from_str(data).map_err(|e| {
        LlmError::InvalidResponse(format!(
            "SSE chunk lỗi: {e}; data: {}",
            truncate_chars(data, 200)
        ))
    })?;
    if let Some(err) = chunk.error {
        return Err(LlmError::InvalidResponse(format!(
            "provider error giữa stream: {}",
            truncate_chars(&err.to_string(), MAX_ERROR_BODY_CHARS)
        )));
    }
    let usage = chunk.usage;
    let delta = chunk
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta)
        .and_then(|d| d.content)
        .unwrap_or_default();
    Ok(match (delta.is_empty(), usage) {
        (false, _) => SseLine::Delta(delta),
        (true, Some(u)) => SseLine::Usage(u),
        (true, None) => SseLine::Skip,
    })
}

// ───────────────────────────── JSON extraction ─────────────────────────────

/// Thứ tự thử: nguyên văn → bỏ `<think>` → bỏ code fence → object `{...}` cân bằng đầu tiên.
pub fn parse_json_lenient<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let original = text.trim();
    let mut last_err = match serde_json::from_str::<T>(original) {
        Ok(v) => return Ok(v),
        Err(e) => e.to_string(),
    };
    let no_think = strip_think_blocks(original);
    let candidates = [
        Some(no_think.trim()),
        Some(strip_code_fence(no_think.trim())),
        extract_first_json_object(&no_think),
    ];
    for c in candidates.into_iter().flatten() {
        match serde_json::from_str::<T>(c) {
            Ok(v) => return Ok(v),
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(last_err)
}

fn strip_think_blocks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out, // think không đóng → bỏ phần còn lại
        }
    }
    out.push_str(rest);
    out
}

fn strip_code_fence(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let rest = rest.split_once('\n').map_or("", |(_, body)| body); // bỏ "json"/"markdown" tag
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Quét byte, theo dõi string/escape để không nhầm `{` `}` nằm trong chuỗi.
fn extract_first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, &b) in s.as_bytes().iter().enumerate().skip(start) {
        if in_str {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Bỏ code fence bao ngoài toàn bộ document (model hay wrap plan trong ```markdown).
pub fn unwrap_markdown_fence(s: &str) -> String {
    let t = s.trim();
    let lowered_head: String = t.chars().take(16).collect::<String>().to_ascii_lowercase();
    if (lowered_head.starts_with("```markdown") || lowered_head.starts_with("```md"))
        && t.ends_with("```")
    {
        strip_code_fence(t).to_owned()
    } else {
        t.to_owned()
    }
}

// ───────────────────────────── Model list ─────────────────────────────

/// `GET {base_url}/models` (OpenAI-compatible). Không retry — dùng ở màn Settings, báo lỗi ngay.
pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>, LlmError> {
    let url = format!("{}/models", api_root(base_url));
    let mut req = http.get(&url).timeout(timeout);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
        // Anthropic: endpoint OpenAI-compatible cho chat nhận Bearer, nhưng /v1/models cần x-api-key.
        if reqwest::Url::parse(&url).is_ok_and(|u| u.host_str() == Some("api.anthropic.com")) {
            req = req.header("x-api-key", key).header("anthropic-version", "2023-06-01");
        }
    }
    let resp = req.send().await.map_err(LlmError::from_reqwest)?;
    let status = resp.status();
    let text = resp.text().await.map_err(LlmError::from_reqwest)?;
    if !status.is_success() {
        return Err(LlmError::Http {
            status: status.as_u16(),
            body: truncate_chars(text.trim(), MAX_ERROR_BODY_CHARS),
        });
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        LlmError::InvalidResponse(format!("{e}: {}", truncate_chars(text.trim(), 120)))
    })?;
    let models = parse_model_list(&v);
    if models.is_empty() {
        return Err(LlmError::InvalidResponse(
            "endpoint /models không trả về model nào".into(),
        ));
    }
    Ok(models)
}

/// Chấp nhận `{"data":[{"id":..}]}` (OpenAI/OpenRouter/Gemini/Groq), `{"models":[{"name":..}]}`
/// (Ollama `/api/tags`) hoặc mảng thuần. Sắp xếp + bỏ trùng.
pub fn parse_model_list(v: &Value) -> Vec<String> {
    let arr = v
        .get("data")
        .or_else(|| v.get("models"))
        .unwrap_or(v)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<String> = arr
        .iter()
        .filter_map(|it| match it {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => ["id", "name", "model"]
                .iter()
                .find_map(|k| o.get(*k).and_then(Value::as_str))
                .map(str::to_owned),
            _ => None,
        })
        // Gemini trả "models/gemini-2.5-pro" — chat completions dùng tên không có tiền tố.
        .map(|s| s.trim().trim_start_matches("models/").to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort_by_key(|s| s.to_lowercase());
    out.dedup();
    out
}

// ───────────────────────────── Backoff ─────────────────────────────

fn parse_retry_after(h: &HeaderMap) -> Option<Duration> {
    let v = h.get(RETRY_AFTER)?.to_str().ok()?.trim();
    v.parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

/// 0.5s, 1s, 2s, 4s, 8s (cap) + jitter 0–250ms — tránh thundering herd khi 3 reviewer cùng bị 429.
fn backoff(attempt: u32) -> Duration {
    let base_ms = 500u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) % 250)
        .unwrap_or(0);
    Duration::from_millis(base_ms + jitter)
}

async fn sleep_cancellable(d: Duration, cancel: &CancellationToken) -> Result<(), LlmError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(LlmError::Cancelled),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Debug)]
    struct T {
        a: i32,
    }

    #[test]
    fn model_list_shapes() {
        let openai = json!({"object":"list","data":[{"id":"gpt-4.1"},{"id":"gpt-4.1-mini"},{"id":"gpt-4.1"}]});
        assert_eq!(parse_model_list(&openai), ["gpt-4.1", "gpt-4.1-mini"]);
        let ollama = json!({"models":[{"name":"qwen3:8b"},{"name":"Llama3:8b"}]});
        assert_eq!(parse_model_list(&ollama), ["Llama3:8b", "qwen3:8b"]);
        let gemini = json!({"data":[{"id":"models/gemini-2.5-pro"}]});
        assert_eq!(parse_model_list(&gemini), ["gemini-2.5-pro"]);
        assert!(parse_model_list(&json!({"error":"x"})).is_empty());
    }

    #[test]
    fn lenient_json() {
        assert_eq!(parse_json_lenient::<T>(r#"{"a":1}"#).unwrap().a, 1);
        assert_eq!(
            parse_json_lenient::<T>("```json\n{\"a\":2}\n```")
                .unwrap()
                .a,
            2
        );
        assert_eq!(
            parse_json_lenient::<T>("Sure! Here: {\"a\":3, \"s\":\"}{\"} thanks")
                .unwrap()
                .a,
            3
        );
        assert_eq!(
            parse_json_lenient::<T>("<think>{ draft }</think>\n{\"a\":4}")
                .unwrap()
                .a,
            4
        );
        assert!(parse_json_lenient::<T>("no json here").is_err());
    }

    #[test]
    fn markdown_fence_unwrap() {
        assert_eq!(unwrap_markdown_fence("```markdown\n## Hi\n```"), "## Hi");
        // Fence mermaid bên trong document phải được giữ nguyên.
        let doc = "## A\n```mermaid\ngraph TD\n```";
        assert_eq!(unwrap_markdown_fence(doc), doc);
    }

    #[test]
    fn sse_lines() {
        assert_eq!(
            parse_sse_line(b"data: {\"choices\":[{\"delta\":{\"content\":\"Xin\"}}]}\r\n").unwrap(),
            SseLine::Delta("Xin".into())
        );
        assert_eq!(parse_sse_line(b"data: [DONE]\n").unwrap(), SseLine::Done);
        assert_eq!(
            parse_sse_line(b": OPENROUTER PROCESSING\n").unwrap(),
            SseLine::Skip
        );
        assert_eq!(
            parse_sse_line(b"data: {\"choices\":[{\"delta\":{}}]}\n").unwrap(),
            SseLine::Skip
        );
        assert!(parse_sse_line(b"data: {\"error\":{\"message\":\"overloaded\"}}\n").is_err());
        assert_eq!(
            parse_sse_line(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n").unwrap(),
            SseLine::Usage(Usage { prompt_tokens: 12, completion_tokens: 3 })
        );
    }

    #[test]
    fn tool_turn_parsing() {
        let t = parse_tool_turn(
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}}]}}],
                "usage":{"prompt_tokens":50,"completion_tokens":5}}"#,
        )
        .unwrap();
        assert_eq!(t.tool_calls.len(), 1);
        assert_eq!(t.tool_calls[0].name, "read_file");
        assert_eq!(t.tool_calls[0].arguments, r#"{"path":"README.md"}"#);
        assert_eq!(t.usage, Some(Usage { prompt_tokens: 50, completion_tokens: 5 }));
        assert!(t.content.is_none());
        assert_eq!(t.assistant_message["tool_calls"][0]["id"], "c1");

        let done = parse_tool_turn(r#"{"choices":[{"message":{"content":"notes"}}]}"#).unwrap();
        assert!(done.tool_calls.is_empty());
        assert_eq!(done.content.as_deref(), Some("notes"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert!(backoff(1) < Duration::from_millis(800));
        assert!(backoff(3) >= Duration::from_millis(2000));
        assert!(backoff(20) <= Duration::from_millis(8250));
    }

    #[test]
    fn retryability() {
        assert!(LlmError::Http {
            status: 429,
            body: String::new()
        }
        .is_retryable());
        assert!(!LlmError::Http {
            status: 401,
            body: String::new()
        }
        .is_retryable());
        assert!(!LlmError::Cancelled.is_retryable());
    }
}
