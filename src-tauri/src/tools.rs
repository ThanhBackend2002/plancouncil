//! Công cụ cho role trong lúc "nghiên cứu" trước khi viết/review plan:
//! - Workspace: CHỈ ĐỌC, mọi đường dẫn bị khoá trong thư mục được cấp quyền (chặn `..`, symlink
//!   trỏ ra ngoài, thư mục rác như node_modules/.git, file nhạy cảm như .env/*.pem/id_rsa).
//! - Web: tìm kiếm (DuckDuckGo không cần key / Tavily / Brave / SearXNG) + đọc nội dung URL,
//!   chặn SSRF (không cho truy cập IP nội bộ/loopback/LAN, kiểm tra lại sau mỗi redirect).

use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{WebSearchConfig, WebSearchProvider};
use crate::models::truncate_chars;

const IGNORED_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", "out", ".next", ".nuxt", ".svelte-kit", ".venv", "venv",
    "__pycache__", ".idea", ".vscode", ".gradle", "Pods", ".dart_tool", "coverage", ".cache", ".turbo", "bin", "obj",
    ".terraform", ".pytest_cache", ".mypy_cache",
];
const MAX_READ_BYTES: u64 = 1_000_000;
const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
const MAX_SEARCH_FILES: usize = 5_000;
const MAX_TOOL_OUTPUT: usize = 24_000;

/// File chứa bí mật: không bao giờ đưa cho LLM (kể cả khi người dùng đã cấp quyền thư mục).
pub fn is_sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let harmless = ["example", "sample", "template", "dist"];
    if n.starts_with(".env") {
        return !harmless.iter().any(|h| n.contains(h));
    }
    [".pem", ".key", ".p12", ".pfx", ".keystore", ".jks", ".secret", ".kdbx"]
        .iter()
        .any(|ext| n.ends_with(ext))
        || n.starts_with("id_rsa")
        || n.starts_with("id_ed25519")
        || n.starts_with("id_ecdsa")
        || matches!(n.as_str(), ".npmrc" | ".netrc" | ".pgpass" | ".pypirc" | "credentials" | "credentials.json")
        || n.starts_with("secrets.")
}

fn is_ignored_dir(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

fn is_binary(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else { return true };
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

// ───────────────────────────── Workspace ─────────────────────────────

pub struct WorkspaceFs {
    root: PathBuf,
    name: String,
}

impl WorkspaceFs {
    pub fn open(name: &str, path: &str) -> Result<Self, String> {
        let root = fs::canonicalize(path.trim()).map_err(|e| format!("không mở được thư mục '{path}': {e}"))?;
        if !root.is_dir() {
            return Err(format!("'{path}' không phải thư mục"));
        }
        Ok(Self { root, name: name.to_owned() })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Đường dẫn tương đối (từ gốc workspace) → đường dẫn thật, đã kiểm tra quyền.
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim().trim_start_matches("./");
        let rel = if rel.is_empty() || rel == "." || rel == "/" { "" } else { rel.trim_start_matches('/') };
        let candidate = Path::new(rel);
        for c in candidate.components() {
            match c {
                Component::Normal(name) => {
                    let n = name.to_string_lossy();
                    if is_ignored_dir(&n) {
                        return Err(format!("'{rel}' nằm trong thư mục bị bỏ qua ({n})"));
                    }
                }
                Component::CurDir => {}
                _ => return Err(format!("đường dẫn '{rel}' không hợp lệ (chỉ dùng đường dẫn tương đối trong workspace)")),
            }
        }
        let full = fs::canonicalize(self.root.join(candidate)).map_err(|_| format!("không tìm thấy '{rel}'"))?;
        // canonicalize đã giải symlink → chặn symlink trỏ ra ngoài workspace.
        if !full.starts_with(&self.root) {
            return Err(format!("'{rel}' nằm ngoài workspace"));
        }
        if full.file_name().is_some_and(|n| is_sensitive(&n.to_string_lossy())) {
            return Err(format!("'{rel}' là file nhạy cảm (secret/khoá), không được đọc"));
        }
        Ok(full)
    }

    pub fn list_dir(&self, rel: &str, depth: u32) -> Result<String, String> {
        let dir = self.resolve(rel)?;
        if !dir.is_dir() {
            return Err(format!("'{rel}' không phải thư mục"));
        }
        let mut out = format!("{}/\n", if rel.trim().is_empty() || rel.trim() == "." { self.name.clone() } else { self.rel(&dir) });
        let mut count = 0usize;
        self.tree(&dir, 1, depth.clamp(1, 3), &mut out, &mut count, 300);
        if count >= 300 {
            out.push_str("… (đã cắt bớt)\n");
        }
        Ok(out)
    }

    fn tree(&self, dir: &Path, level: u32, max: u32, out: &mut String, count: &mut usize, limit: usize) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name().to_ascii_lowercase()));
        for e in entries {
            if *count >= limit {
                return;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            let indent = "  ".repeat(level as usize);
            if path.is_dir() {
                if is_ignored_dir(&name) {
                    continue;
                }
                *count += 1;
                let _ = writeln!(out, "{indent}{name}/");
                if level < max {
                    self.tree(&path, level + 1, max, out, count, limit);
                }
            } else {
                if is_sensitive(&name) {
                    continue;
                }
                *count += 1;
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = writeln!(out, "{indent}{name} ({})", human_size(size));
            }
        }
    }

    pub fn read_file(&self, rel: &str, start_line: usize, max_lines: usize) -> Result<String, String> {
        let path = self.resolve(rel)?;
        if !path.is_file() {
            return Err(format!("'{rel}' không phải file"));
        }
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_READ_BYTES {
            return Err(format!("'{rel}' quá lớn ({}), hãy dùng search_code để tìm đoạn cần đọc", human_size(size)));
        }
        if is_binary(&path) {
            return Err(format!("'{rel}' là file nhị phân"));
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("không đọc được '{rel}': {e}"))?;
        let lines: Vec<&str> = text.lines().collect();
        let start = start_line.max(1);
        let max = max_lines.clamp(1, 400);
        let end = (start - 1 + max).min(lines.len());
        let mut out = format!("{} (dòng {start}-{end} / {})\n", self.rel(&path), lines.len());
        for (i, l) in lines.iter().enumerate().take(end).skip(start - 1) {
            let _ = writeln!(out, "{:>5} | {}", i + 1, truncate_chars(l, 400));
        }
        if end < lines.len() {
            let _ = writeln!(out, "… còn {} dòng (gọi lại với start_line={})", lines.len() - end, end + 1);
        }
        Ok(out)
    }

    /// Tìm chuỗi (không phân biệt hoa thường) trong file text của workspace.
    pub fn search(&self, query: &str, ext: Option<&str>) -> Result<String, String> {
        let q = query.trim().to_lowercase();
        if q.len() < 2 {
            return Err("query quá ngắn".into());
        }
        let ext = ext
            .map(|e| e.trim().trim_start_matches('*').trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty());
        let mut files = Vec::new();
        self.walk(&self.root, &mut files);
        let mut out = String::new();
        let mut hits = 0usize;
        for f in &files {
            if let Some(ext) = &ext {
                if f.extension().and_then(|x| x.to_str()).map(str::to_lowercase).as_deref() != Some(ext.as_str()) {
                    continue;
                }
            }
            if fs::metadata(f).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_SEARCH_FILE_BYTES || is_binary(f) {
                continue;
            }
            let Ok(text) = fs::read_to_string(f) else { continue };
            for (i, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(&q) {
                    let _ = writeln!(out, "{}:{}: {}", self.rel(f), i + 1, truncate_chars(line.trim(), 200));
                    hits += 1;
                    if hits >= 60 {
                        out.push_str("… (đã đủ 60 kết quả, hãy thu hẹp query)\n");
                        return Ok(out);
                    }
                }
            }
        }
        Ok(if hits == 0 { format!("Không tìm thấy '{query}' trong {} file.", files.len()) } else { out })
    }

    fn walk(&self, dir: &Path, out: &mut Vec<PathBuf>) {
        if out.len() >= MAX_SEARCH_FILES {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue; // không đi theo symlink khi duyệt
            }
            if ft.is_dir() {
                if !is_ignored_dir(&name) {
                    self.walk(&e.path(), out);
                }
            } else if ft.is_file() && !is_sensitive(&name) {
                out.push(e.path());
                if out.len() >= MAX_SEARCH_FILES {
                    return;
                }
            }
        }
    }

    /// Tóm tắt workspace: cây thư mục + manifest + README (đưa sẵn vào prompt người viết plan).
    pub fn overview(&self) -> String {
        let mut out = format!("Workspace \"{}\"\n\n## Cây thư mục (2 cấp)\n", self.name);
        let mut count = 0;
        self.tree(&self.root, 1, 2, &mut out, &mut count, 150);
        const MANIFESTS: &[&str] = &[
            "package.json", "Cargo.toml", "go.mod", "pom.xml", "build.gradle", "build.gradle.kts", "requirements.txt",
            "pyproject.toml", "composer.json", "Gemfile", "pubspec.yaml", "docker-compose.yml", "docker-compose.yaml",
            "Dockerfile", "tsconfig.json", "CLAUDE.md", "AGENTS.md",
        ];
        let mut budget = 9_000usize;
        let mut add = |title: &str, path: &Path, lines: usize, out: &mut String| {
            if budget == 0 || !path.is_file() || is_binary(path) {
                return;
            }
            let Ok(text) = fs::read_to_string(path) else { return };
            let head: String = text.lines().take(lines).collect::<Vec<_>>().join("\n");
            let head = truncate_chars(&head, budget.min(3_000));
            budget = budget.saturating_sub(head.len());
            // Ngày sửa giúp model nhận ra tài liệu có thể đã cũ so với code.
            let modified = fs::metadata(path)
                .and_then(|m| m.modified())
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).format(" (sửa lần cuối %Y-%m-%d)").to_string())
                .unwrap_or_default();
            let _ = write!(out, "\n## {title}{modified}\n```\n{head}\n```\n");
        };
        for m in MANIFESTS {
            add(m, &self.root.join(m), 60, &mut out);
        }
        for readme in ["README.md", "readme.md", "README.MD", "README"] {
            let p = self.root.join(readme);
            if p.is_file() {
                add(readme, &p, 80, &mut out);
                break;
            }
        }
        truncate_chars(&out, 14_000)
    }
}

fn human_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

// ───────────────────────────── Web ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub struct WebSearch {
    http: reqwest::Client,
    /// Client không tự follow redirect (để kiểm tra SSRF từng bước).
    fetcher: reqwest::Client,
    cfg: WebSearchConfig,
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 (CGNAT, Tailscale)
                || (o[0] == 198 && (o[1] & 0xFE) == 18) // 198.18.0.0/15 benchmark
                || o[0] >= 224) // multicast + reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80) // link-local
        }
    }
}

/// Chặn SSRF: chỉ http(s) tới host mà MỌI địa chỉ phân giải đều là IP công cộng.
pub async fn ensure_public_url(url: &reqwest::Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("chỉ hỗ trợ http/https, không hỗ trợ '{}'", url.scheme()));
    }
    let host = url.host_str().ok_or("URL không có host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".internal") {
        return Err(format!("không cho phép truy cập host nội bộ '{host}'"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_public_ip(ip) { Ok(()) } else { Err(format!("không cho phép truy cập IP nội bộ {ip}")) };
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("không phân giải được '{host}': {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("không phân giải được '{host}'"));
    }
    if let Some(bad) = addrs.iter().find(|a| !is_public_ip(a.ip())) {
        return Err(format!("'{host}' trỏ tới địa chỉ nội bộ {}", bad.ip()));
    }
    Ok(())
}

impl WebSearch {
    pub fn new(cfg: WebSearchConfig) -> Result<Self, String> {
        let build = |redirect: reqwest::redirect::Policy| {
            reqwest::Client::builder()
                .use_rustls_tls()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .redirect(redirect)
                .user_agent("Mozilla/5.0 (compatible; PlanCouncil/0.1; +research)")
                .build()
                .map_err(|e| e.to_string())
        };
        Ok(Self {
            http: build(reqwest::redirect::Policy::limited(5))?,
            fetcher: build(reqwest::redirect::Policy::none())?,
            cfg,
        })
    }

    fn limit(&self) -> usize {
        self.cfg.max_results.clamp(1, 10) as usize
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Err("query rỗng".into());
        }
        let key = self.cfg.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let mut hits = match self.cfg.provider {
            WebSearchProvider::Duckduckgo => self.duckduckgo(q).await?,
            WebSearchProvider::Tavily => {
                let key = key.ok_or("Tavily cần API key (Cài đặt → Bối cảnh & Công cụ)")?;
                let v: Value = self
                    .http
                    .post("https://api.tavily.com/search")
                    .json(&json!({ "api_key": key, "query": q, "max_results": self.limit() }))
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("Tavily lỗi: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("Tavily trả dữ liệu lạ: {e}"))?;
                hits_from(&v["results"], "title", "url", "content")
            }
            WebSearchProvider::Brave => {
                let key = key.ok_or("Brave Search cần API key (Cài đặt → Bối cảnh & Công cụ)")?;
                let url = reqwest::Url::parse_with_params(
                    "https://api.search.brave.com/res/v1/web/search",
                    &[("q", q), ("count", &self.limit().to_string())],
                )
                .map_err(|e| e.to_string())?;
                let v: Value = self
                    .http
                    .get(url)
                    .header("X-Subscription-Token", key)
                    .header("Accept", "application/json")
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("Brave lỗi: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("Brave trả dữ liệu lạ: {e}"))?;
                hits_from(&v["web"]["results"], "title", "url", "description")
            }
            WebSearchProvider::Searxng => {
                let base = self.cfg.base_url.trim().trim_end_matches('/');
                if base.is_empty() {
                    return Err("SearXNG cần base URL (Cài đặt → Bối cảnh & Công cụ)".into());
                }
                let url = reqwest::Url::parse_with_params(&format!("{base}/search"), &[("q", q), ("format", "json")])
                    .map_err(|e| e.to_string())?;
                let v: Value = self
                    .http
                    .get(url)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("SearXNG lỗi: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("SearXNG trả dữ liệu lạ (đã bật format json chưa?): {e}"))?;
                hits_from(&v["results"], "title", "url", "content")
            }
        };
        hits.truncate(self.limit());
        Ok(hits)
    }

    async fn duckduckgo(&self, q: &str) -> Result<Vec<SearchHit>, String> {
        let url = reqwest::Url::parse_with_params("https://html.duckduckgo.com/html/", &[("q", q)]).map_err(|e| e.to_string())?;
        let html = self
            .http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("DuckDuckGo lỗi: {e}"))?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let hits = parse_duckduckgo(&html);
        if hits.is_empty() && html.contains("anomaly") {
            return Err("DuckDuckGo đang chặn truy vấn tự động; hãy dùng Tavily/Brave/SearXNG".into());
        }
        Ok(hits)
    }

    /// Tải 1 trang web công khai và trả về text thuần (đã chặn SSRF ở mọi bước redirect).
    pub async fn fetch(&self, raw: &str) -> Result<String, String> {
        let mut url = reqwest::Url::parse(raw.trim()).map_err(|e| format!("URL không hợp lệ: {e}"))?;
        for _ in 0..5 {
            ensure_public_url(&url).await?;
            let resp = self.fetcher.get(url.clone()).send().await.map_err(|e| format!("không tải được: {e}"))?;
            if resp.status().is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("redirect không có Location")?;
                url = url.join(loc).map_err(|e| e.to_string())?;
                continue;
            }
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !(ct.is_empty() || ct.contains("text/") || ct.contains("json") || ct.contains("xml")) {
                return Err(format!("không đọc loại nội dung '{ct}'"));
            }
            let bytes = read_capped(resp, 2_000_000).await?;
            let body = String::from_utf8_lossy(&bytes);
            let text = if ct.contains("html") || body.trim_start().starts_with('<') { html_to_text(&body) } else { body.to_string() };
            return Ok(truncate_chars(&format!("{url}\n\n{}", text.trim()), 12_000));
        }
        Err("quá nhiều redirect".into())
    }
}

async fn read_capped(resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(chunk) = s.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        out.extend_from_slice(&chunk);
        if out.len() >= cap {
            out.truncate(cap);
            break;
        }
    }
    Ok(out)
}

fn hits_from(arr: &Value, title: &str, url: &str, snippet: &str) -> Vec<SearchHit> {
    arr.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(SearchHit {
                        title: r.get(title)?.as_str()?.trim().to_owned(),
                        url: r.get(url)?.as_str()?.trim().to_owned(),
                        snippet: truncate_chars(r.get(snippet).and_then(Value::as_str).unwrap_or("").trim(), 400),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse trang kết quả HTML của DuckDuckGo (html.duckduckgo.com) — không cần API key.
pub fn parse_duckduckgo(html: &str) -> Vec<SearchHit> {
    let mut out = Vec::new();
    let marker = "class=\"result__a\"";
    let mut pos = 0;
    while let Some(i) = html[pos..].find(marker) {
        let at = pos + i;
        pos = at + marker.len();
        let Some(tag_start) = html[..at].rfind("<a") else { continue };
        let Some(tag_end_rel) = html[at..].find('>') else { break };
        let tag = &html[tag_start..at + tag_end_rel];
        let Some(href) = attr(tag, "href") else { continue };
        let Some(close_rel) = html[at + tag_end_rel..].find("</a>") else { break };
        let title = html_to_text(&html[at + tag_end_rel + 1..at + tag_end_rel + close_rel]);

        let next = html[pos..].find(marker).map_or(html.len(), |n| pos + n);
        let snippet = html[pos..next]
            .find("result__snippet")
            .and_then(|s| {
                let s = pos + s;
                let open = html[s..next].find('>')? + s + 1;
                let close = html[open..next].find("</").map_or(next, |c| open + c);
                Some(html_to_text(&html[open..close]))
            })
            .unwrap_or_default();

        let href = decode_entities(&href);
        let full = if href.starts_with("//") { format!("https:{href}") } else { href };
        let Ok(u) = reqwest::Url::parse(&full) else { continue };
        let target = if u.path().starts_with("/l/") {
            u.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.to_string())
        } else {
            Some(full.clone())
        };
        let Some(target) = target else { continue };
        if target.contains("duckduckgo.com/y.js") {
            continue; // quảng cáo
        }
        out.push(SearchHit { title: title.trim().to_owned(), url: target, snippet: truncate_chars(snippet.trim(), 400) });
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let s = tag.find(&needle)? + needle.len();
    let e = tag[s..].find('"')? + s;
    Some(tag[s..e].to_owned())
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail[..tail.len().min(10)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" | "#x27" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// HTML → text thuần: bỏ script/style/svg/comment, xuống dòng theo block, gộp khoảng trắng.
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase(); // giữ nguyên vị trí byte
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < html.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            let skip_block = ["script", "style", "noscript", "svg", "head", "template"]
                .iter()
                .find(|t| rest.starts_with(&format!("<{t}")) && rest[1 + t.len()..].starts_with(|c: char| c == '>' || c.is_whitespace()));
            if let Some(t) = skip_block {
                let close = format!("</{t}");
                i = lower[i..].find(&close).map_or(html.len(), |e| i + e);
                i = lower[i..].find('>').map_or(html.len(), |e| i + e + 1);
                continue;
            }
            if rest.starts_with("<!--") {
                i = lower[i..].find("-->").map_or(html.len(), |e| i + e + 3);
                continue;
            }
            let end = lower[i..].find('>').map_or(html.len(), |e| i + e + 1);
            let tag = &rest[..(end - i).min(rest.len())];
            if ["<br", "<p", "</p", "<div", "</div", "<li", "</li", "<h1", "<h2", "<h3", "<h4", "</h", "<tr", "</tr", "<section", "<article"]
                .iter()
                .any(|b| tag.starts_with(b))
            {
                out.push('\n');
            }
            i = end;
        } else {
            let next = html[i..].find('<').map_or(html.len(), |n| i + n);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = decode_entities(&out);
    let mut lines: Vec<String> = Vec::new();
    for l in decoded.lines() {
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if !l.is_empty() {
            lines.push(l);
        }
    }
    lines.join("\n")
}

// ───────────────────────────── Toolbox ─────────────────────────────

/// Tập công cụ của 1 phiên: workspace (nếu có) + web (nếu bật).
#[derive(Clone, Default)]
pub struct Toolbox {
    pub workspace: Option<Arc<WorkspaceFs>>,
    pub web: Option<Arc<WebSearch>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Args {
    path: String,
    depth: Option<u32>,
    start_line: Option<usize>,
    max_lines: Option<usize>,
    query: String,
    file_extension: Option<String>,
    url: String,
}

impl Toolbox {
    pub fn is_empty(&self) -> bool {
        self.workspace.is_none() && self.web.is_none()
    }

    /// Định nghĩa tool theo chuẩn OpenAI function calling.
    pub fn definitions(&self) -> Vec<Value> {
        let f = |name: &str, desc: &str, props: Value, required: &[&str]| {
            json!({"type":"function","function":{"name":name,"description":desc,
                "parameters":{"type":"object","properties":props,"required":required}}})
        };
        let mut out = Vec::new();
        if self.workspace.is_some() {
            out.push(f(
                "list_files",
                "List files and folders of the project workspace (read-only). Use '.' for the root.",
                json!({"path":{"type":"string","description":"Relative path inside the workspace"},
                       "depth":{"type":"integer","minimum":1,"maximum":3}}),
                &["path"],
            ));
            out.push(f(
                "read_file",
                "Read a text file of the project workspace with line numbers (max 400 lines per call).",
                json!({"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},
                       "max_lines":{"type":"integer","minimum":1,"maximum":400}}),
                &["path"],
            ));
            out.push(f(
                "search_code",
                "Case-insensitive text search across the project workspace. Returns path:line: text.",
                json!({"query":{"type":"string"},"file_extension":{"type":"string","description":"Optional, e.g. 'ts' or 'rs'"}}),
                &["query"],
            ));
        }
        if self.web.is_some() {
            out.push(f(
                "web_search",
                "Search the public web for up-to-date information. Returns titles, URLs and snippets.",
                json!({"query":{"type":"string"}}),
                &["query"],
            ));
            out.push(f(
                "fetch_url",
                "Fetch a public web page (http/https) and return its text content.",
                json!({"url":{"type":"string"}}),
                &["url"],
            ));
        }
        out
    }

    /// Chạy 1 tool. Trả về (kết quả cho model, mô tả ngắn cho UI, thành công?). Lỗi không làm hỏng vòng lặp.
    pub async fn execute(&self, name: &str, raw_args: &str) -> (String, String, bool) {
        let args: Args = serde_json::from_str(raw_args).unwrap_or_default();
        let ws = self.workspace.clone();
        let blocking = |f: Box<dyn FnOnce(&WorkspaceFs) -> Result<String, String> + Send>| async move {
            match ws {
                Some(ws) => tokio::task::spawn_blocking(move || f(&ws)).await.unwrap_or_else(|e| Err(e.to_string())),
                None => Err("workspace chưa được cấp quyền cho phiên này".into()),
            }
        };
        let (summary, result) = match name {
            "list_files" => {
                let path = if args.path.trim().is_empty() { ".".to_owned() } else { args.path.clone() };
                let depth = args.depth.unwrap_or(2);
                (format!("xem thư mục {path}"), blocking(Box::new(move |w| w.list_dir(&path, depth))).await)
            }
            "read_file" => {
                let path = args.path.clone();
                let (start, max) = (args.start_line.unwrap_or(1), args.max_lines.unwrap_or(200));
                (format!("đọc {}", args.path), blocking(Box::new(move |w| w.read_file(&path, start, max))).await)
            }
            "search_code" => {
                let (q, ext) = (args.query.clone(), args.file_extension.clone());
                (format!("tìm \"{}\" trong code", args.query), blocking(Box::new(move |w| w.search(&q, ext.as_deref()))).await)
            }
            "web_search" => {
                let r = match &self.web {
                    Some(web) => web.search(&args.query).await.map(|hits| {
                        if hits.is_empty() {
                            "Không có kết quả.".to_owned()
                        } else {
                            hits.iter()
                                .enumerate()
                                .map(|(i, h)| format!("{}. {}\n   {}\n   {}", i + 1, h.title, h.url, h.snippet))
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    }),
                    None => Err("tìm kiếm web chưa được bật cho phiên này".into()),
                };
                (format!("tìm web: {}", args.query), r)
            }
            "fetch_url" => {
                let r = match &self.web {
                    Some(web) => web.fetch(&args.url).await,
                    None => Err("tìm kiếm web chưa được bật cho phiên này".into()),
                };
                (format!("đọc trang {}", truncate_chars(&args.url, 80)), r)
            }
            other => (format!("tool lạ '{other}'"), Err(format!("không có tool '{other}'"))),
        };
        match result {
            Ok(text) => (truncate_chars(&text, MAX_TOOL_OUTPUT), summary, true),
            Err(e) => (format!("ERROR: {e}"), format!("{summary} — lỗi: {}", truncate_chars(&e, 120)), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws() -> (PathBuf, WorkspaceFs) {
        let dir = std::env::temp_dir().join(format!("pc-ws-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("node_modules/lib")).unwrap();
        fs::write(dir.join("README.md"), "# Demo\nLeaderboard service").unwrap();
        fs::write(dir.join("package.json"), r#"{"name":"demo","dependencies":{"react":"19"}}"#).unwrap();
        fs::write(dir.join("src/main.ts"), "const a = 1;\nexport function leaderboard() {}\n").unwrap();
        fs::write(dir.join(".env"), "SECRET=abc").unwrap();
        fs::write(dir.join(".env.example"), "SECRET=").unwrap();
        fs::write(dir.join("node_modules/lib/x.js"), "leaderboard").unwrap();
        fs::write(dir.join("logo.bin"), [0u8, 1, 2, 3]).unwrap();
        let ws = WorkspaceFs::open("demo", dir.to_str().unwrap()).unwrap();
        (dir, ws)
    }

    #[test]
    fn workspace_reads_and_blocks_escapes_and_secrets() {
        let (dir, ws) = temp_ws();
        assert!(ws.read_file("src/main.ts", 1, 50).unwrap().contains("export function leaderboard"));
        assert!(ws.read_file("./README.md", 1, 5).unwrap().contains("Leaderboard"));
        for bad in ["../", "../../etc/passwd", "/etc/passwd", "C:\\Windows\\win.ini", "src/../../x"] {
            assert!(ws.read_file(bad, 1, 5).is_err(), "phải chặn {bad}");
        }
        assert!(ws.read_file(".env", 1, 5).unwrap_err().contains("nhạy cảm"));
        assert!(ws.read_file(".env.example", 1, 5).is_ok());
        assert!(ws.read_file("node_modules/lib/x.js", 1, 5).is_err());
        assert!(ws.read_file("logo.bin", 1, 5).unwrap_err().contains("nhị phân"));

        let tree = ws.list_dir(".", 2).unwrap();
        assert!(tree.contains("src/") && tree.contains("main.ts"));
        assert!(!tree.contains("node_modules") && !tree.contains(".env (") && tree.contains(".env.example"));

        let found = ws.search("LEADERBOARD", None).unwrap();
        assert!(found.contains("src/main.ts:2:"), "{found}");
        assert!(!found.contains("node_modules"));
        assert!(ws.search("leaderboard", Some("*.md")).unwrap().contains("README.md:2:"));

        let ov = ws.overview();
        assert!(ov.contains("package.json") && ov.contains("Leaderboard service"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sensitive_names() {
        for n in [".env", ".env.local", "id_rsa", "server.pem", "prod.key", ".npmrc", "secrets.yaml"] {
            assert!(is_sensitive(n), "{n}");
        }
        for n in [".env.example", "main.rs", "keyboard.ts", "README.md"] {
            assert!(!is_sensitive(n), "{n}");
        }
    }

    #[test]
    fn ssrf_ip_checks() {
        for ip in ["127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.1", "169.254.169.254", "100.100.204.55", "0.0.0.0", "::1", "fe80::1", "fd00::1"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} phải bị chặn");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn ensure_public_rejects_internal_hosts() {
        for u in ["http://localhost:8080/x", "http://127.0.0.1/", "http://[::1]/", "file:///etc/passwd", "http://169.254.169.254/latest"] {
            let url = reqwest::Url::parse(u).unwrap();
            assert!(ensure_public_url(&url).await.is_err(), "{u}");
        }
    }

    #[test]
    fn html_text_and_duckduckgo_parsing() {
        let t = html_to_text("<html><head><title>x</title><style>a{}</style></head><body><h1>Tiêu đề</h1><script>evil()</script><p>A &amp; B&nbsp;C</p><!-- c --></body></html>");
        assert_eq!(t, "Tiêu đề\nA & B C");

        let html = r#"<div class="result"><h2><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs&amp;rut=abc">Example <b>Docs</b></a></h2>
            <a class="result__snippet" href="x">Fast &amp; simple</a></div>
            <div class="result"><a class="result__a" href="https://duckduckgo.com/y.js?ad=1">Ad</a></div>
            <div class="result"><a class="result__a" href="https://rust-lang.org/">Rust</a><a class="result__snippet">Lang</a></div>"#;
        let hits = parse_duckduckgo(html);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0], SearchHit { title: "Example Docs".into(), url: "https://example.com/docs".into(), snippet: "Fast & simple".into() });
        assert_eq!(hits[1].url, "https://rust-lang.org/");
    }
}
