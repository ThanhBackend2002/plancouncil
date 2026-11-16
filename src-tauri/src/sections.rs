//! Tách / ghép plan Markdown thành các leaf section (`##` / `###`) để revise từng phần.
//!
//! Module thuần (không I/O). Bất biến quan trọng nhất: `join(&split(md)) == md` với MỌI input
//! (kể cả CRLF, fence chưa đóng, heading lạ) — vì `text` là slice nguyên bản của `md`.

use std::collections::HashMap;

/// Một leaf block của plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Id ổn định giữa các vòng revise: "s1", "s2"…
    pub id: String,
    /// 0 = phần intro trước heading đầu tiên; còn lại là 2 hoặc 3.
    pub level: u8,
    /// Số đầu heading: "## 3. Data Flow" → "3", "### 3.3 Foo" → "3.3".
    pub number: Option<String>,
    /// Nội dung heading (đã bỏ `#`, khoảng trắng, closing `#`); intro = "".
    pub heading: String,
    /// Slice nguyên bản, gồm cả dòng heading và line ending.
    pub text: String,
    /// blake3 của `text` sau khi gộp khoảng trắng.
    pub hash: [u8; 32],
}

// ───────────────────────────── Quét dòng / fence ─────────────────────────────

/// Một dòng của tài liệu, kèm trạng thái fence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line<'a> {
    /// Byte offset đầu dòng.
    pub start: usize,
    /// Byte offset sau line ending.
    pub end: usize,
    /// Nội dung dòng, KHÔNG gồm `\n` / `\r\n`.
    pub content: &'a str,
    /// true nếu dòng là fence delimiter hoặc nằm trong code fence.
    pub in_fence: bool,
}

/// Quét từng dòng, theo dõi code fence kiểu CommonMark. Trả về (các dòng, fence còn mở ở cuối?).
///
/// Mở fence: thụt ≤3 space, ≥3 ký tự ``` ` ``` hoặc `~` (info string của backtick fence không
/// được chứa backtick). Đóng fence: cùng ký tự, độ dài ≥ lúc mở, thụt ≤3 space, sau đó chỉ có
/// khoảng trắng. Fence không đóng thì kéo tới hết tài liệu.
pub(crate) fn scan_lines(md: &str) -> (Vec<Line<'_>>, bool) {
    let mut out = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut pos = 0;
    for raw in md.split_inclusive('\n') {
        let start = pos;
        pos += raw.len();
        let content = raw.strip_suffix('\n').unwrap_or(raw);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let in_fence = match fence {
            None => match opening_fence(content) {
                Some(f) => {
                    fence = Some(f);
                    true
                }
                None => false,
            },
            Some((c, n)) => {
                if is_closing_fence(content, c, n) {
                    fence = None;
                }
                true
            }
        };
        out.push(Line {
            start,
            end: pos,
            content,
            in_fence,
        });
    }
    (out, fence.is_some())
}

/// Đếm space đầu dòng (tab KHÔNG tính → dòng bắt đầu bằng tab coi như thụt ≥4).
fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

/// (ký tự fence, độ dài run, phần còn lại) nếu dòng có dạng fence delimiter.
fn fence_run(content: &str) -> Option<(u8, usize, &str)> {
    let indent = leading_spaces(content);
    if indent > 3 {
        return None;
    }
    let rest = &content[indent..];
    let c = *rest.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = rest.bytes().take_while(|&b| b == c).count();
    if n < 3 {
        return None;
    }
    Some((c, n, &rest[n..]))
}

fn opening_fence(content: &str) -> Option<(u8, usize)> {
    let (c, n, info) = fence_run(content)?;
    if c == b'`' && info.contains('`') {
        return None;
    }
    Some((c, n))
}

fn is_closing_fence(content: &str, c: u8, n: usize) -> bool {
    matches!(fence_run(content), Some((c2, n2, rest)) if c2 == c && n2 >= n && rest.trim().is_empty())
}

/// ATX heading (level 1..=6) → (level, text đã làm sạch). Không xét fence — caller tự lọc.
pub(crate) fn atx_heading(content: &str) -> Option<(u8, &str)> {
    let indent = leading_spaces(content);
    if indent > 3 {
        return None;
    }
    let rest = &content[indent..];
    let n = rest.bytes().take_while(|&b| b == b'#').count();
    if n == 0 || n > 6 {
        return None;
    }
    let after = &rest[n..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    Some((n as u8, clean_heading_text(after)))
}

/// Bỏ khoảng trắng hai đầu và closing sequence `#` (chỉ khi đứng sau khoảng trắng hoặc trống).
fn clean_heading_text(s: &str) -> &str {
    let t = s.trim_matches([' ', '\t']);
    let stripped = t.trim_end_matches('#');
    if stripped.len() == t.len() {
        t
    } else if stripped.is_empty() {
        ""
    } else if stripped.ends_with([' ', '\t']) {
        stripped.trim_end_matches([' ', '\t'])
    } else {
        t
    }
}

/// Số đầu heading: "3. Data" → "3", "3.3 Foo" → "3.3", "10.2.1. X" → "10.2.1", "3.Data" → "3".
pub(crate) fn leading_number(heading: &str) -> Option<String> {
    let end = heading
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(heading.len());
    let token = &heading[..end];
    let tail = &heading[end..];
    let ok_tail = tail.is_empty() || tail.starts_with(char::is_whitespace) || token.ends_with('.');
    if !ok_tail {
        return None;
    }
    let num = token.trim_end_matches('.');
    if num.is_empty()
        || !num
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(num.to_string())
}

/// blake3 của text sau khi gộp mọi khoảng trắng (kể cả CRLF) thành 1 space.
pub(crate) fn content_hash(text: &str) -> [u8; 32] {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    *blake3::hash(collapsed.as_bytes()).as_bytes()
}

fn make_section(id: String, level: u8, heading: &str, text: &str) -> Section {
    Section {
        id,
        level,
        number: if level == 0 {
            None
        } else {
            leading_number(heading)
        },
        heading: heading.to_string(),
        text: text.to_string(),
        hash: content_hash(text),
    }
}

// ───────────────────────────── API ─────────────────────────────

/// Tách plan thành leaf block: intro (nếu không rỗng), mỗi `##` tới `###`/`##` kế tiếp, mỗi `###`.
/// Heading trong code fence không tính. Id gán "s1", "s2"… theo thứ tự.
pub fn split(md: &str) -> Vec<Section> {
    let (lines, _) = scan_lines(md);
    let starts: Vec<(usize, u8, &str)> = lines
        .iter()
        .filter(|l| !l.in_fence)
        .filter_map(|l| match atx_heading(l.content) {
            Some((lvl @ (2 | 3), h)) => Some((l.start, lvl, h)),
            _ => None,
        })
        .collect();

    let mut out = Vec::with_capacity(starts.len() + 1);
    let first = starts.first().map_or(md.len(), |s| s.0);
    if first > 0 {
        out.push(make_section(String::new(), 0, "", &md[..first]));
    }
    for (i, &(start, lvl, h)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(md.len(), |s| s.0);
        out.push(make_section(String::new(), lvl, h, &md[start..end]));
    }
    for (i, s) in out.iter_mut().enumerate() {
        s.id = format!("s{}", i + 1);
    }
    out
}

/// Ghép lại nguyên văn.
pub fn join(sections: &[Section]) -> String {
    sections.iter().map(|s| s.text.as_str()).collect()
}

/// Heading chuẩn hoá để so khớp giữa 2 phiên bản: lowercase, gộp khoảng trắng, GIỮ số.
fn normalize_heading(h: &str) -> String {
    h.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn id_num(id: &str) -> Option<u64> {
    id.strip_prefix('s')?.parse().ok()
}

/// Kế thừa id từ phiên bản trước: section mới giữ id cũ CHỈ khi heading chuẩn hoá xuất hiện
/// đúng 1 lần ở cả `prev` và `next`. Còn lại nhận id mới, lớn hơn mọi id của `prev`.
pub fn inherit_ids(prev: &[Section], mut next: Vec<Section>) -> Vec<Section> {
    fn counts(secs: &[Section]) -> HashMap<String, usize> {
        let mut m = HashMap::new();
        for s in secs {
            *m.entry(normalize_heading(&s.heading)).or_insert(0) += 1;
        }
        m
    }
    let prev_counts = counts(prev);
    let next_counts = counts(&next);
    let prev_ids: HashMap<String, &str> = prev
        .iter()
        .map(|s| (normalize_heading(&s.heading), s.id.as_str()))
        .collect();

    let mut fresh = prev.iter().filter_map(|s| id_num(&s.id)).max().unwrap_or(0) + 1;
    for s in next.iter_mut() {
        let key = normalize_heading(&s.heading);
        let unique = prev_counts.get(&key) == Some(&1) && next_counts.get(&key) == Some(&1);
        match prev_ids.get(&key) {
            Some(id) if unique => s.id = (*id).to_string(),
            _ => {
                s.id = format!("s{fresh}");
                fresh += 1;
            }
        }
    }
    next
}

/// Tìm section theo số. Một `##` khớp số thì trả kèm các `###` con ngay sau nó (theo cấu trúc,
/// không theo tiền tố số). Thứ tự theo tài liệu.
pub fn find_by_number<'a>(sections: &'a [Section], number: &str) -> Vec<&'a Section> {
    let want = number.trim().trim_end_matches('.');
    if want.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut in_parent = false;
    for s in sections {
        let hit = s.number.as_deref() == Some(want);
        match s.level {
            2 => {
                in_parent = hit;
                if hit {
                    out.push(s);
                }
            }
            3 => {
                if in_parent || hit {
                    out.push(s);
                }
            }
            _ => in_parent = false,
        }
    }
    out
}

/// Plan bị cắt cụt: bỏ section cuối (heading `##`/`###` cuối cùng nằm ngoài fence trở đi).
/// Trả về (phần giữ lại, dòng heading cuối không kèm line ending); None nếu không có heading.
pub fn cut_last_incomplete(md: &str) -> Option<(String, String)> {
    let (lines, _) = scan_lines(md);
    let last = lines
        .iter()
        .rev()
        .find(|l| !l.in_fence && matches!(atx_heading(l.content), Some((2 | 3, _))))?;
    Some((md[..last.start].to_string(), last.content.to_string()))
}

/// Ước lượng token thô: (số ký tự + 3) / 4.
pub fn approx_tokens(s: &str) -> usize {
    // = (chars + 3) / 4
    s.chars().count().div_ceil(4)
}

/// Section Changelog (heading chứa "changelog", không phân biệt hoa thường).
pub fn is_changelog(s: &Section) -> bool {
    s.heading.to_lowercase().contains("changelog")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../tests-data/sections_vectors.json");

    #[derive(serde::Deserialize)]
    struct Expected {
        level: u8,
        number: Option<String>,
        heading: String,
    }

    #[derive(serde::Deserialize)]
    struct Vector {
        name: String,
        md: String,
        expected: Vec<Expected>,
    }

    #[test]
    fn shared_vectors() {
        let vectors: Vec<Vector> = serde_json::from_str(VECTORS).expect("vectors JSON hợp lệ");
        assert!(vectors.len() >= 10);
        for v in &vectors {
            let secs = split(&v.md);
            assert_eq!(join(&secs), v.md, "round trip: {}", v.name);
            let got: Vec<(u8, Option<&str>, &str)> = secs
                .iter()
                .map(|s| (s.level, s.number.as_deref(), s.heading.as_str()))
                .collect();
            let want: Vec<(u8, Option<&str>, &str)> = v
                .expected
                .iter()
                .map(|e| (e.level, e.number.as_deref(), e.heading.as_str()))
                .collect();
            assert_eq!(got, want, "vector: {}", v.name);
            for (i, s) in secs.iter().enumerate() {
                assert_eq!(s.id, format!("s{}", i + 1), "id: {}", v.name);
            }
        }
    }

    /// Fuzz tất định: ghép ngẫu nhiên các mảnh khó (fence, heading, CRLF, unicode) → round trip.
    #[test]
    fn round_trip_fuzz() {
        const FRAGS: &[&str] = &[
            "## A\n",
            "### 1.1 B\r\n",
            "```\n",
            "~~~~\n",
            "    ```\n",
            "text\n",
            "\n",
            "\r\n",
            "## Mục tiêu",
            "# Top\n",
            "``` x`y\n",
            "````\n",
            "  ### c ##\n",
            "ư",
            "\t## tab\n",
            "#",
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..500 {
            let mut md = String::new();
            let n = (seed % 20) as usize;
            for _ in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                md.push_str(FRAGS[(seed >> 33) as usize % FRAGS.len()]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let secs = split(&md);
            assert_eq!(join(&secs), md);
            assert!(secs.iter().all(|s| !s.text.is_empty()));
        }
    }

    #[test]
    fn hash_ignores_whitespace_layout() {
        let a = split("## A\nfoo  bar\n");
        let b = split("## A\r\nfoo bar\r\n\r\n");
        assert_eq!(a[0].hash, b[0].hash);
        let c = split("## A\nfoo baz\n");
        assert_ne!(a[0].hash, c[0].hash);
    }

    #[test]
    fn text_includes_heading_line_and_h2_stops_at_first_h3() {
        let md = "## 2. Design\nD\n### 2.1 API\nA\n";
        let s = split(md);
        assert_eq!(s[0].text, "## 2. Design\nD\n");
        assert_eq!(s[1].text, "### 2.1 API\nA\n");
    }

    #[test]
    fn leading_number_variants() {
        assert_eq!(leading_number("3. Data Flow").as_deref(), Some("3"));
        assert_eq!(leading_number("3.3 Foo").as_deref(), Some("3.3"));
        assert_eq!(leading_number("10.2.1. Deep").as_deref(), Some("10.2.1"));
        assert_eq!(leading_number("3.Data").as_deref(), Some("3"));
        assert_eq!(leading_number("7").as_deref(), Some("7"));
        assert_eq!(leading_number("v2 Plan"), None);
        assert_eq!(leading_number("3x Faster"), None);
        assert_eq!(leading_number("1..2 Odd"), None);
        assert_eq!(leading_number(". Dot"), None);
    }

    #[test]
    fn inherit_ids_keeps_unique_and_refreshes_duplicates() {
        let prev = split("Intro\n## 1. Goals\na\n## Notes\nb\n## Notes\nc\n## 2. Design\nd\n");
        // prev: s1 intro, s2 Goals, s3 Notes, s4 Notes, s5 Design
        let next =
            split("Intro v2\n## 2. Design\nd2\n## Notes\nx\n## 1.  GOALS\na\n## 3. New\nn\n");
        let got = inherit_ids(&prev, next);
        let ids: Vec<(&str, &str)> = got
            .iter()
            .map(|s| (s.heading.as_str(), s.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("", "s1"),
                ("2. Design", "s5"),
                // "Notes" trùng ở prev → id mới dù next chỉ có 1.
                ("Notes", "s6"),
                // So khớp không phân biệt hoa thường + gộp khoảng trắng.
                ("1.  GOALS", "s2"),
                ("3. New", "s7"),
            ]
        );
    }

    #[test]
    fn inherit_ids_duplicate_in_next_gets_fresh_ids() {
        let prev = split("## A\n## B\n");
        let next = split("## A\n## A\n## B\n");
        let got = inherit_ids(&prev, next);
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["s3", "s4", "s2"]);
    }

    #[test]
    fn inherit_ids_renumbered_heading_is_new() {
        // Số được giữ trong key → đổi số = section mới.
        let prev = split("## 1. A\n## 2. B\n");
        let next = split("## 1. A\n## 3. B\n");
        let got = inherit_ids(&prev, next);
        assert_eq!(got[0].id, "s1");
        assert_eq!(got[1].id, "s3");
    }

    #[test]
    fn find_by_number_includes_h3_children() {
        let secs =
            split("## 1. A\n### 1.1 X\n## 2. B\n### 2.1 Y\n### 2.2 Z\n## 3. C\n### 9.9 Odd\n");
        let h = |v: Vec<&Section>| v.iter().map(|s| s.heading.clone()).collect::<Vec<_>>();
        assert_eq!(
            h(find_by_number(&secs, "2")),
            vec!["2. B", "2.1 Y", "2.2 Z"]
        );
        assert_eq!(h(find_by_number(&secs, "2.2")), vec!["2.2 Z"]);
        assert_eq!(h(find_by_number(&secs, " 1. ")), vec!["1. A", "1.1 X"]);
        assert_eq!(h(find_by_number(&secs, "3")), vec!["3. C", "9.9 Odd"]);
        assert!(find_by_number(&secs, "4").is_empty());
        assert!(find_by_number(&secs, "").is_empty());
    }

    #[test]
    fn cut_last_incomplete_basic() {
        let md = "Intro\n## 1. A\na\n### 1.1 B\nhalf a sent";
        let (kept, head) = cut_last_incomplete(md).unwrap();
        assert_eq!(kept, "Intro\n## 1. A\na\n");
        assert_eq!(head, "### 1.1 B");
    }

    #[test]
    fn cut_last_incomplete_inside_fence() {
        // Bị cắt giữa code fence: "## fake" trong fence không được tính.
        let md = "## 1. A\na\r\n## 2. B\r\n```rust\n## fake\nfn main() {";
        let (kept, head) = cut_last_incomplete(md).unwrap();
        assert_eq!(kept, "## 1. A\na\r\n");
        assert_eq!(head, "## 2. B");
    }

    #[test]
    fn cut_last_incomplete_none_without_heading() {
        assert_eq!(cut_last_incomplete("just text\n```\n## in fence\n"), None);
        assert_eq!(cut_last_incomplete(""), None);
    }

    #[test]
    fn approx_tokens_counts_chars() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("a"), 1);
        assert_eq!(approx_tokens("abcd"), 1);
        assert_eq!(approx_tokens("abcde"), 2);
        // Đếm ký tự, không đếm byte.
        assert_eq!(approx_tokens("ữữữữ"), 1);
    }

    #[test]
    fn is_changelog_case_insensitive() {
        let secs = split("## 9. CHANGELOG\n## Change log\n### ChangeLog v2\n");
        assert!(is_changelog(&secs[0]));
        assert!(!is_changelog(&secs[1]));
        assert!(is_changelog(&secs[2]));
    }
}
