//! Hạ tầng test dùng chung: mock server OpenAI-compatible (HTTP/1.1 tối giản, 1 request / connection)
//! cùng các builder response. Chỉ biên dịch khi `cfg(test)`.

// Bộ công cụ dùng chung: không phải module test nào cũng dùng hết mọi helper.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Response mà mock server trả cho 1 request.
#[derive(Clone, Debug)]
pub(crate) struct MockResp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub delay: Duration,
    /// `Some(n)`: chỉ gửi `n` event SSE đầu tiên (event = đoạn kết thúc bằng `"\n\n"`, comment cũng tính)
    /// rồi đóng kết nối — không `Content-Length`, không `[DONE]`. Client thấy EOF "sạch" giữa chừng.
    pub abort_after: Option<usize>,
}

impl MockResp {
    /// Response tuỳ ý (thường dùng cho lỗi HTTP: 401, 429, 503...).
    pub(crate) fn http(status: u16, content_type: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
            delay: Duration::ZERO,
            abort_after: None,
        }
    }

    /// Chờ `d` trước khi gửi response.
    pub(crate) fn delayed(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }

    /// Gửi `n` event SSE đầu rồi cắt kết nối (xem `abort_after`).
    pub(crate) fn abort_after(mut self, n: usize) -> Self {
        self.abort_after = Some(n);
        self
    }
}

/// Handler nhận body JSON của request, trả response.
pub(crate) type Handler = Arc<dyn Fn(&Value) -> MockResp + Send + Sync>;

/// Mở mock server trên cổng ngẫu nhiên, trả về base URL dạng `http://127.0.0.1:PORT/v1`.
pub(crate) async fn spawn_mock(handler: Handler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 16 * 1024];
                let header_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let cl = headers
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < header_end + cl {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body: Value = serde_json::from_slice(&buf[header_end..header_end + cl]).unwrap();
                let r = handler(&body);
                tokio::time::sleep(r.delay).await;
                let out = match r.abort_after {
                    None => format!(
                        "HTTP/1.1 {} MOCK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        r.status,
                        r.content_type,
                        r.body.len(),
                        r.body
                    ),
                    Some(n) => {
                        // Body kết thúc bằng đóng kết nối → cắt sau n event là "rớt mạng" giữa stream.
                        let partial: String = r.body.split_inclusive("\n\n").take(n).collect();
                        format!(
                            "HTTP/1.1 {} MOCK\r\nContent-Type: {}\r\nConnection: close\r\n\r\n{}",
                            r.status, r.content_type, partial
                        )
                    }
                };
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

/// Handler biết số thứ tự lần gọi (bắt đầu từ 1), đếm trên toàn server.
pub(crate) fn counted(f: impl Fn(usize, &Value) -> MockResp + Send + Sync + 'static) -> Handler {
    let calls = AtomicUsize::new(0);
    Arc::new(move |body: &Value| f(calls.fetch_add(1, Ordering::SeqCst) + 1, body))
}

/// Kịch bản cố định: lần gọi i nhận `resps[i-1]`; hết kịch bản thì lặp lại response cuối.
/// Ví dụ "length 2 lần rồi stop": `scripted(vec![len.clone(), len, stop])`.
pub(crate) fn scripted(resps: Vec<MockResp>) -> Handler {
    assert!(!resps.is_empty(), "scripted cần ít nhất 1 response");
    counted(move |i, _| resps[(i - 1).min(resps.len() - 1)].clone())
}

/// Completion non-stream: `finish_reason` = "stop", usage 100/20.
pub(crate) fn completion(content: &str) -> MockResp {
    completion_with(content, Some("stop"), Some((100, 20)))
}

/// Completion non-stream; `None` → bỏ hẳn field tương ứng khỏi JSON.
pub(crate) fn completion_with(content: &str, finish_reason: Option<&str>, usage: Option<(u32, u32)>) -> MockResp {
    let mut choice = json!({"message":{"role":"assistant","content":content}});
    if let Some(fr) = finish_reason {
        choice["finish_reason"] = json!(fr);
    }
    let mut body = json!({ "choices": [choice] });
    if let Some((p, c)) = usage {
        body["usage"] = json!({"prompt_tokens":p,"completion_tokens":c});
    }
    MockResp::http(200, "application/json", body.to_string())
}

/// Lượt gọi tool (OpenAI function calling) với 1 tool call, usage 50/5.
pub(crate) fn tool_call(name: &str, args: Value) -> MockResp {
    MockResp::http(
        200,
        "application/json",
        json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":name,"arguments":args.to_string()}}]}}],
            "usage":{"prompt_tokens":50,"completion_tokens":5}})
        .to_string(),
    )
}

/// Stream SSE: mỗi từ 1 chunk, 1 comment keep-alive, chunk usage 100/20, `[DONE]`. Không có finish_reason.
pub(crate) fn sse(content: &str) -> MockResp {
    sse_with(content, None, Some((100, 20)))
}

/// Stream SSE tuỳ biến. `finish_reason` nằm trong chunk riêng với delta rỗng (giống OpenAI),
/// đứng sau các chunk nội dung; usage (nếu có) nằm trong chunk `choices: []` trước `[DONE]`.
pub(crate) fn sse_with(content: &str, finish_reason: Option<&str>, usage: Option<(u32, u32)>) -> MockResp {
    let mut body = String::new();
    for piece in content.split_inclusive(' ') {
        body.push_str(&format!("data: {}\n\n", json!({"choices":[{"delta":{"content":piece}}]})));
    }
    if let Some(fr) = finish_reason {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":{},"finish_reason":fr}]})
        ));
    }
    body.push_str(": keep-alive comment\n\n");
    if let Some((p, c)) = usage {
        body.push_str(&format!(
            "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{p},\"completion_tokens\":{c}}}}}\n\n"
        ));
    }
    body.push_str("data: [DONE]\n\n");
    MockResp::http(200, "text/event-stream", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post(base: &str) -> (u16, Result<String, reqwest::Error>) {
        let resp = reqwest::Client::new()
            .post(format!("{base}/chat/completions"))
            .json(&json!({"model":"m"}))
            .send()
            .await
            .unwrap();
        (resp.status().as_u16(), resp.text().await)
    }

    #[test]
    fn sse_keeps_legacy_output() {
        assert_eq!(
            sse("Xin chào").body,
            "data: {\"choices\":[{\"delta\":{\"content\":\"Xin \"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"chào\"}}]}\n\n\
             : keep-alive comment\n\n\
             data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\n\
             data: [DONE]\n\n"
        );
    }

    #[test]
    fn builders_place_finish_reason_and_usage() {
        let s = sse_with("a b", Some("length"), None).body;
        let events: Vec<&str> = s.split("\n\n").filter(|e| !e.is_empty()).collect();
        // 2 chunk nội dung → chunk finish_reason (delta rỗng) → comment → [DONE].
        assert_eq!(events.len(), 5);
        let fin: Value = serde_json::from_str(events[2].strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(fin["choices"][0]["finish_reason"], "length");
        assert_eq!(fin["choices"][0]["delta"], json!({}));
        assert!(!s.contains("usage"));
        assert!(s.ends_with("data: [DONE]\n\n"));

        let c: Value = serde_json::from_str(&completion_with("x", Some("length"), Some((7, 3))).body).unwrap();
        assert_eq!(c["choices"][0]["finish_reason"], "length");
        assert_eq!(c["usage"]["completion_tokens"], 3);
        let bare: Value = serde_json::from_str(&completion_with("x", None, None).body).unwrap();
        assert!(bare["choices"][0].get("finish_reason").is_none() && bare.get("usage").is_none());
    }

    #[tokio::test]
    async fn abort_cuts_stream_without_done() {
        let base = spawn_mock(Arc::new(|_: &Value| sse("một hai ba").abort_after(2))).await;
        let (status, text) = post(&base).await;
        assert_eq!(status, 200);
        let text = text.unwrap();
        assert_eq!(text.matches("data:").count(), 2);
        assert!(text.contains("hai") && !text.contains("ba") && !text.contains("[DONE]"));
    }

    #[tokio::test]
    async fn scripted_returns_per_call_then_repeats_last() {
        let base = spawn_mock(scripted(vec![
            MockResp::http(503, "text/plain", "busy"),
            completion_with("cut", Some("length"), None),
            completion("ok"),
        ]))
        .await;
        let mut seen = Vec::new();
        for _ in 0..4 {
            let (status, text) = post(&base).await;
            seen.push((status, text.unwrap()));
        }
        assert_eq!(seen[0], (503, "busy".to_owned()));
        assert!(seen[1].1.contains("\"length\""));
        assert!(seen[2].1.contains("\"stop\"") && seen[3] == seen[2]);
    }
}
