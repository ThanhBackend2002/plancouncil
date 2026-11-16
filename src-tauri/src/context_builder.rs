//! Dựng context cho "section revise" và kiểm tra / merge patch tác giả trả về. Hàm thuần.
//!
//! Luồng: `required_set` → `section_mode_ok` → gửi `section_index` + `required_payload` →
//! `parse_patch` → `validate_and_merge`. Mọi lỗi trả `FallbackReason` để orchestrator quay về
//! chế độ viết lại toàn bộ.

use std::collections::{HashMap, HashSet};

use crate::sections::{
    approx_tokens, atx_heading, content_hash, find_by_number, is_changelog, leading_number,
    scan_lines, Section,
};

/// Tác giả chèn marker này (ngoài fence) khi thấy cần đổi cấu trúc → fallback viết lại toàn bộ.
pub const STRUCTURAL_CHANGE_MARKER: &str = "<!-- structural-change -->";

/// Marker đóng mỗi block patch. Marker mở: `<!-- section id="sX" base_version="N" -->`.
pub const SECTION_END_MARKER: &str = "<!-- /section -->";

/// Lý do không dùng được section mode (fallback về full rewrite).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    MissingIds,
    ExtraIds,
    DuplicateIds,
    BaseMismatch,
    BadStructure,
    StructuralChange,
    Ratio,
    Truncated,
}

/// Một block trong output của tác giả.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchBlock {
    pub id: String,
    pub base_version: u32,
    /// Nội dung giữa marker mở và marker đóng (nguyên văn).
    pub text: String,
}

/// Id các section cần sửa: khớp theo số (kèm `###` con của `##`) + LUÔN kèm Changelog nếu có,
/// để tác giả cập nhật Changelog. Thứ tự theo tài liệu, không trùng.
pub fn required_set(sections: &[Section], referenced_numbers: &[String]) -> Vec<String> {
    let mut wanted: HashSet<&str> = HashSet::new();
    for n in referenced_numbers {
        for s in find_by_number(sections, n) {
            wanted.insert(s.id.as_str());
        }
    }
    for s in sections.iter().filter(|s| is_changelog(s)) {
        wanted.insert(s.id.as_str());
    }
    sections
        .iter()
        .filter(|s| wanted.contains(s.id.as_str()))
        .map(|s| s.id.clone())
        .collect()
}

/// Section mode đáng dùng khi tổng token các section cần sửa ≤ `max_ratio` × tổng plan.
/// Trả false nếu `required` rỗng hoặc plan rỗng (không có gì để patch).
pub fn section_mode_ok(sections: &[Section], required: &[String], max_ratio: f32) -> bool {
    let req: HashSet<&str> = required.iter().map(String::as_str).collect();
    let total: usize = sections.iter().map(|s| approx_tokens(&s.text)).sum();
    let needed: usize = sections
        .iter()
        .filter(|s| req.contains(s.id.as_str()))
        .map(|s| approx_tokens(&s.text))
        .sum();
    if req.is_empty() || total == 0 || needed == 0 {
        return false;
    }
    needed as f64 <= f64::from(max_ratio) * total as f64
}

/// Câu đầu tiên của phần thân (bỏ dòng heading, code fence), tối đa 160 ký tự.
fn first_sentence(s: &Section) -> String {
    const MAX: usize = 160;
    let body = if s.level == 0 {
        s.text.as_str()
    } else {
        s.text.split_once('\n').map_or("", |(_, rest)| rest)
    };
    let (lines, _) = scan_lines(body);
    let words: Vec<&str> = lines
        .iter()
        .filter(|l| !l.in_fence && atx_heading(l.content).is_none())
        .flat_map(|l| l.content.split_whitespace())
        .collect();
    let flat = words.join(" ");

    // Cắt ở dấu kết câu đứng trước khoảng trắng / cuối chuỗi ("3.3" không bị cắt).
    let mut end = flat.len();
    let mut it = flat.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '.' | '!' | '?' | '。') {
            match it.peek() {
                None => break,
                Some(&(_, n)) if n.is_whitespace() => {
                    end = i + c.len_utf8();
                    break;
                }
                _ => {}
            }
        }
    }
    let sentence = &flat[..end];
    if sentence.chars().count() <= MAX {
        sentence.to_string()
    } else {
        let mut cut: String = sentence.chars().take(MAX - 1).collect();
        cut.push('…');
        cut
    }
}

/// Mục lục gọn của toàn plan, mỗi section 1 dòng:
/// `id | heading | ~N tokens | câu đầu (≤160 ký tự)`. Heading kèm `##`/`###`; intro = "(intro)".
pub fn section_index(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        let heading = if s.level == 0 {
            "(intro)".to_string()
        } else {
            format!("{} {}", "#".repeat(usize::from(s.level)), s.heading)
        };
        out.push_str(&format!(
            "{} | {} | ~{} tokens | {}\n",
            s.id,
            heading,
            approx_tokens(&s.text),
            first_sentence(s)
        ));
    }
    out
}

fn open_marker(id: &str, base_version: u32) -> String {
    format!("<!-- section id=\"{id}\" base_version=\"{base_version}\" -->")
}

/// Nguyên văn các section cần sửa (thứ tự tài liệu), mỗi section bọc trong marker mở / đóng.
pub fn required_payload(sections: &[Section], required: &[String], base_version: u32) -> String {
    let req: HashSet<&str> = required.iter().map(String::as_str).collect();
    let mut out = String::new();
    for s in sections.iter().filter(|s| req.contains(s.id.as_str())) {
        out.push_str(&open_marker(&s.id, base_version));
        out.push('\n');
        out.push_str(s.text.trim_end());
        out.push('\n');
        out.push_str(SECTION_END_MARKER);
        out.push_str("\n\n");
    }
    out
}

fn is_end_marker(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "<!--/section-->"
}

/// Ok(None) = không phải marker mở; Err = trông như marker mở nhưng thiếu / sai thuộc tính.
fn parse_open_marker(t: &str) -> Result<Option<(String, u32)>, FallbackReason> {
    let Some(inner) = t.strip_prefix("<!--").and_then(|r| r.strip_suffix("-->")) else {
        return Ok(None);
    };
    let inner = inner.trim();
    let Some(attrs) = inner.strip_prefix("section") else {
        return Ok(None);
    };
    if !attrs.starts_with(char::is_whitespace) {
        return Ok(None);
    }
    let mut id = None;
    let mut base = None;
    for tok in attrs.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            return Err(FallbackReason::BadStructure);
        };
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        match k {
            "id" => id = Some(v.to_string()),
            "base_version" => base = v.parse::<u32>().ok(),
            _ => {}
        }
    }
    match (id, base) {
        (Some(id), Some(b)) if !id.is_empty() => Ok(Some((id, b))),
        _ => Err(FallbackReason::BadStructure),
    }
}

/// Tách output của tác giả thành các block. Marker phải nằm trên dòng riêng, ngoài code fence;
/// text ngoài marker bị bỏ qua.
/// - Có `STRUCTURAL_CHANGE_MARKER` (ngoài fence) → `StructuralChange`.
/// - Không có block nào, marker lồng nhau, marker đóng lẻ, marker mở hỏng → `BadStructure`.
/// - Block mở mà hết output chưa đóng (bị cắt cụt) → `Truncated`.
pub fn parse_patch(output: &str) -> Result<Vec<PatchBlock>, FallbackReason> {
    let (lines, _) = scan_lines(output);
    if lines
        .iter()
        .any(|l| !l.in_fence && l.content.contains(STRUCTURAL_CHANGE_MARKER))
    {
        return Err(FallbackReason::StructuralChange);
    }

    let mut blocks = Vec::new();
    let mut open: Option<(String, u32, usize)> = None;
    for l in lines.iter().filter(|l| !l.in_fence) {
        let t = l.content.trim();
        if is_end_marker(t) {
            let Some((id, base_version, start)) = open.take() else {
                return Err(FallbackReason::BadStructure);
            };
            blocks.push(PatchBlock {
                id,
                base_version,
                text: output[start..l.start].to_string(),
            });
        } else if let Some((id, b)) = parse_open_marker(t)? {
            if open.is_some() {
                return Err(FallbackReason::BadStructure);
            }
            open = Some((id, b, l.end));
        }
    }
    if open.is_some() {
        return Err(FallbackReason::Truncated);
    }
    if blocks.is_empty() {
        return Err(FallbackReason::BadStructure);
    }
    Ok(blocks)
}

/// Kiểm tra cấu trúc block mới; trả (heading, number) lấy từ dòng heading mới.
fn check_block(text: &str, level: u8) -> Result<(String, Option<String>), FallbackReason> {
    let (lines, fence_open) = scan_lines(text);
    if fence_open {
        return Err(FallbackReason::BadStructure);
    }
    // Intro: không được chứa heading cấp 2/3 (cho phép `# Title`).
    if level == 0 {
        let has_section_heading = lines
            .iter()
            .any(|l| !l.in_fence && matches!(atx_heading(l.content), Some((2 | 3, _))));
        return if has_section_heading {
            Err(FallbackReason::BadStructure)
        } else {
            Ok((String::new(), None))
        };
    }
    let first = lines.first().ok_or(FallbackReason::BadStructure)?;
    let heading = match (first.in_fence, atx_heading(first.content)) {
        (false, Some((lvl, h))) if lvl == level => h.to_string(),
        _ => return Err(FallbackReason::BadStructure),
    };
    let extra_heading = lines[1..]
        .iter()
        .any(|l| !l.in_fence && matches!(atx_heading(l.content), Some((1..=3, _))));
    if extra_heading {
        return Err(FallbackReason::BadStructure);
    }
    let number = leading_number(&heading);
    Ok((heading, number))
}

/// Text block đã trim + giữ nguyên khoảng trắng cuối của section gốc (để khoảng cách giữa các
/// section không đổi sau khi join).
fn normalized_block_text(block: &str, original: &str) -> String {
    let suffix = &original[original.trim_end().len()..];
    let mut t = block.trim().to_string();
    t.push_str(suffix);
    t
}

/// Kiểm tra patch và merge vào plan. Id/thứ tự giữ nguyên; section không nằm trong `required`
/// giữ byte-identical.
pub fn validate_and_merge(
    sections: &[Section],
    required: &[String],
    patch: &[PatchBlock],
    base_version: u32,
) -> Result<Vec<Section>, FallbackReason> {
    let req: HashSet<&str> = required.iter().map(String::as_str).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    for b in patch {
        if !seen.insert(b.id.as_str()) {
            return Err(FallbackReason::DuplicateIds);
        }
    }
    if req.iter().any(|id| !seen.contains(id)) {
        return Err(FallbackReason::MissingIds);
    }
    if seen.iter().any(|id| !req.contains(id)) {
        return Err(FallbackReason::ExtraIds);
    }
    if patch.iter().any(|b| b.base_version != base_version) {
        return Err(FallbackReason::BaseMismatch);
    }

    let index: HashMap<&str, usize> = sections
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut out = sections.to_vec();
    for b in patch {
        // Id có trong required nhưng không có trong plan → coi như id thừa.
        let &i = index.get(b.id.as_str()).ok_or(FallbackReason::ExtraIds)?;
        let orig = &sections[i];
        let text = normalized_block_text(&b.text, &orig.text);
        let (heading, number) = check_block(&text, orig.level)?;
        out[i] = Section {
            id: orig.id.clone(),
            level: orig.level,
            number,
            heading,
            hash: content_hash(&text),
            text,
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::{join, split};

    const PLAN: &str = "# Kế hoạch\n\nMô tả chung. Câu thứ hai.\n\n\
## 1. Goals\nMục tiêu v1.\n\n\
## 2. Design\nThiết kế tổng 3.3 lần nhanh hơn. Chi tiết.\n\
### 2.1 API\nREST.\n```\n## not heading\n```\n\
### 2.2 Storage\nSQLite.\n\n\
## 3. Rollout\nTừng bước.\n\n\
## 4. Changelog\n- v1: khởi tạo\n";

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // s1 intro, s2 Goals, s3 Design, s4 API, s5 Storage, s6 Rollout, s7 Changelog
    fn plan() -> Vec<Section> {
        split(PLAN)
    }

    fn block(id: &str, base: u32, text: &str) -> PatchBlock {
        PatchBlock {
            id: id.into(),
            base_version: base,
            text: text.into(),
        }
    }

    #[test]
    fn required_set_includes_children_and_changelog() {
        let s = plan();
        assert_eq!(
            required_set(&s, &ids(&["2"])),
            ids(&["s3", "s4", "s5", "s7"])
        );
        assert_eq!(
            required_set(&s, &ids(&["2.2", "1", "2.2"])),
            ids(&["s2", "s5", "s7"])
        );
        // Không khớp số nào → vẫn có Changelog.
        assert_eq!(required_set(&s, &ids(&["9"])), ids(&["s7"]));
        // Changelog được tham chiếu trực tiếp → không trùng.
        assert_eq!(required_set(&s, &ids(&["4"])), ids(&["s7"]));
    }

    #[test]
    fn required_set_without_changelog() {
        let s = split("## 1. A\na\n## 2. B\nb\n");
        assert_eq!(required_set(&s, &ids(&["2"])), ids(&["s2"]));
        assert!(required_set(&s, &[]).is_empty());
    }

    #[test]
    fn section_mode_ok_ratio() {
        let s = split(&format!(
            "## 1. A\n{}\n## 2. B\n{}\n",
            "a".repeat(400),
            "b".repeat(40)
        ));
        assert!(section_mode_ok(&s, &ids(&["s2"]), 0.5));
        assert!(!section_mode_ok(&s, &ids(&["s1"]), 0.5));
        assert!(section_mode_ok(&s, &ids(&["s1", "s2"]), 1.0));
        assert!(!section_mode_ok(&s, &[], 0.5));
        assert!(!section_mode_ok(&[], &ids(&["s1"]), 0.5));
    }

    #[test]
    fn section_index_format() {
        let s = plan();
        let idx = section_index(&s);
        let lines: Vec<&str> = idx.lines().collect();
        assert_eq!(lines.len(), s.len());
        assert_eq!(
            lines[0],
            format!(
                "s1 | (intro) | ~{} tokens | Mô tả chung.",
                approx_tokens(&s[0].text)
            )
        );
        // "3.3" không bị coi là kết câu.
        assert!(lines[2].starts_with("s3 | ## 2. Design | ~"));
        assert!(lines[2].ends_with("| Thiết kế tổng 3.3 lần nhanh hơn."));
        // Bỏ nội dung trong fence.
        assert!(lines[3].ends_with("| REST."));
        assert!(lines[3].contains("### 2.1 API"));
    }

    #[test]
    fn section_index_truncates_long_sentence() {
        let s = split(&format!("## A\n{}\n", "ư".repeat(500)));
        let idx = section_index(&s);
        let sentence = idx.trim_end().rsplit(" | ").next().unwrap();
        assert_eq!(sentence.chars().count(), 160);
        assert!(sentence.ends_with('…'));
    }

    #[test]
    fn payload_round_trips_through_parse_and_merge() {
        let s = plan();
        let req = required_set(&s, &ids(&["2.1"]));
        let payload = required_payload(&s, &req, 3);
        assert!(payload.starts_with("<!-- section id=\"s4\" base_version=\"3\" -->\n### 2.1 API\n"));
        let blocks = parse_patch(&payload).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            vec!["s4", "s7"]
        );
        // Fence bên trong section không làm hỏng marker.
        assert!(blocks[0].text.contains("## not heading"));
        let merged = validate_and_merge(&s, &req, &blocks, 3).unwrap();
        assert_eq!(join(&merged), PLAN);
        assert_eq!(merged, s);
    }

    #[test]
    fn merge_keeps_other_sections_byte_identical() {
        let s = plan();
        let req = ids(&["s3", "s7"]);
        let out = "Đây là patch:\n\
<!-- section id=\"s3\" base_version=\"2\" -->\n\n## 2. Design (v2)\nThiết kế mới.\n<!-- /section -->\n\
chatter ở giữa\n\
<!-- section id=\"s7\" base_version=\"2\" -->\n## 4. Changelog\n- v1: khởi tạo\n- v2: đổi design\n<!-- /section -->\nHết.";
        let patch = parse_patch(out).unwrap();
        let merged = validate_and_merge(&s, &req, &patch, 2).unwrap();
        assert_eq!(merged.len(), s.len());
        for (a, b) in s.iter().zip(&merged) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.level, b.level);
            if !req.contains(&a.id) {
                assert_eq!(a.text.as_bytes(), b.text.as_bytes());
                assert_eq!(a.hash, b.hash);
            }
        }
        assert_eq!(merged[2].text, "## 2. Design (v2)\nThiết kế mới.\n");
        assert_eq!(merged[2].heading, "2. Design (v2)");
        assert_eq!(merged[2].number.as_deref(), Some("2"));
        assert_ne!(merged[2].hash, s[2].hash);
        // Changelog là section cuối: giữ đúng khoảng trắng cuối của bản gốc.
        assert_eq!(
            merged[6].text,
            "## 4. Changelog\n- v1: khởi tạo\n- v2: đổi design\n"
        );
        let joined = join(&merged);
        assert!(joined.starts_with(&join(&s[..2])));
        assert!(joined.contains("### 2.1 API\nREST.\n```\n## not heading\n```\n### 2.2 Storage\nSQLite.\n\n## 3. Rollout"));
    }

    #[test]
    fn merge_preserves_crlf_suffix_of_original() {
        let s = split("## A\r\na\r\n\r\n## B\r\nb\r\n");
        let patch = vec![block("s1", 1, "## A\nnew\n\n\n")];
        let merged = validate_and_merge(&s, &ids(&["s1"]), &patch, 1).unwrap();
        assert_eq!(merged[0].text, "## A\nnew\r\n\r\n");
        assert_eq!(merged[1].text, s[1].text);
    }

    // ── FallbackReason: parse_patch ──

    #[test]
    fn parse_no_markers_is_bad_structure() {
        assert_eq!(
            parse_patch("## 1. A\nviết lại toàn bộ\n"),
            Err(FallbackReason::BadStructure)
        );
        assert_eq!(parse_patch(""), Err(FallbackReason::BadStructure));
    }

    #[test]
    fn parse_markers_inside_fence_are_ignored() {
        let out =
            "```\n<!-- section id=\"s1\" base_version=\"1\" -->\n## A\n<!-- /section -->\n```\n";
        assert_eq!(parse_patch(out), Err(FallbackReason::BadStructure));
    }

    #[test]
    fn parse_structural_change() {
        let out = "<!-- section id=\"s1\" base_version=\"1\" -->\n## A\n<!-- /section -->\n<!-- structural-change -->\n";
        assert_eq!(parse_patch(out), Err(FallbackReason::StructuralChange));
        // Trong fence thì không tính.
        let fenced = "<!-- section id=\"s1\" base_version=\"1\" -->\n## A\n```\n<!-- structural-change -->\n```\n<!-- /section -->\n";
        assert_eq!(parse_patch(fenced).unwrap().len(), 1);
    }

    #[test]
    fn parse_unclosed_block_is_truncated() {
        let out = "<!-- section id=\"s1\" base_version=\"1\" -->\n## A\nđang viết dở";
        assert_eq!(parse_patch(out), Err(FallbackReason::Truncated));
    }

    #[test]
    fn parse_nested_or_stray_markers_are_bad_structure() {
        let nested = "<!-- section id=\"s1\" base_version=\"1\" -->\n## A\n<!-- section id=\"s2\" base_version=\"1\" -->\n## B\n<!-- /section -->\n";
        assert_eq!(parse_patch(nested), Err(FallbackReason::BadStructure));
        let stray = "<!-- /section -->\n";
        assert_eq!(parse_patch(stray), Err(FallbackReason::BadStructure));
        let bad_attr = "<!-- section id=\"s1\" base_version=\"x\" -->\n## A\n<!-- /section -->\n";
        assert_eq!(parse_patch(bad_attr), Err(FallbackReason::BadStructure));
        let no_id = "<!-- section base_version=\"1\" -->\n## A\n<!-- /section -->\n";
        assert_eq!(parse_patch(no_id), Err(FallbackReason::BadStructure));
    }

    #[test]
    fn parse_tolerates_spacing_and_attribute_order() {
        let out =
            "  <!--   section base_version='4'  id=\"s9\"-->\r\n## A\r\nx\r\n<!--/section -->\r\n";
        let b = parse_patch(out).unwrap();
        assert_eq!(b, vec![block("s9", 4, "## A\r\nx\r\n")]);
    }

    // ── FallbackReason: validate_and_merge ──

    #[test]
    fn validate_missing_ids() {
        let s = plan();
        let patch = vec![block("s3", 1, "## 2. Design\nx\n")];
        assert_eq!(
            validate_and_merge(&s, &ids(&["s3", "s7"]), &patch, 1),
            Err(FallbackReason::MissingIds)
        );
    }

    #[test]
    fn validate_extra_ids() {
        let s = plan();
        let patch = vec![
            block("s3", 1, "## 2. Design\nx\n"),
            block("s6", 1, "## 3. Rollout\ny\n"),
        ];
        assert_eq!(
            validate_and_merge(&s, &ids(&["s3"]), &patch, 1),
            Err(FallbackReason::ExtraIds)
        );
        // Id required không tồn tại trong plan.
        let ghost = vec![block("s99", 1, "## X\n")];
        assert_eq!(
            validate_and_merge(&s, &ids(&["s99"]), &ghost, 1),
            Err(FallbackReason::ExtraIds)
        );
    }

    #[test]
    fn validate_duplicate_ids() {
        let s = plan();
        let patch = vec![
            block("s3", 1, "## 2. Design\nx\n"),
            block("s3", 1, "## 2. Design\ny\n"),
        ];
        assert_eq!(
            validate_and_merge(&s, &ids(&["s3"]), &patch, 1),
            Err(FallbackReason::DuplicateIds)
        );
    }

    #[test]
    fn validate_base_mismatch() {
        let s = plan();
        let patch = vec![block("s3", 1, "## 2. Design\nx\n")];
        assert_eq!(
            validate_and_merge(&s, &ids(&["s3"]), &patch, 2),
            Err(FallbackReason::BaseMismatch)
        );
    }

    #[test]
    fn validate_bad_structure_cases() {
        let s = plan();
        let req = ids(&["s3"]);
        let bad = |text: &str| validate_and_merge(&s, &req, &[block("s3", 1, text)], 1);
        // Sai cấp heading.
        assert_eq!(bad("### 2. Design\nx\n"), Err(FallbackReason::BadStructure));
        // Không bắt đầu bằng heading.
        assert_eq!(
            bad("Lời dẫn\n## 2. Design\nx\n"),
            Err(FallbackReason::BadStructure)
        );
        assert_eq!(bad("   \n"), Err(FallbackReason::BadStructure));
        // Thêm heading cấp ≤3 khác (tạo section mới).
        assert_eq!(
            bad("## 2. Design\nx\n### 2.9 New\ny\n"),
            Err(FallbackReason::BadStructure)
        );
        assert_eq!(
            bad("## 2. Design\nx\n# Top\n"),
            Err(FallbackReason::BadStructure)
        );
        // Fence chưa đóng.
        assert_eq!(
            bad("## 2. Design\n```\ncode\n"),
            Err(FallbackReason::BadStructure)
        );
        // Hợp lệ: heading trong fence, `####` và khoảng trắng đầu được trim.
        let ok = bad("\n\n## 2. Design\n#### Sub\n~~~\n## in fence\n~~~\n").unwrap();
        assert_eq!(
            ok[2].text,
            "## 2. Design\n#### Sub\n~~~\n## in fence\n~~~\n"
        );
    }

    #[test]
    fn validate_intro_block() {
        let s = plan();
        let req = ids(&["s1"]);
        let ok =
            validate_and_merge(&s, &req, &[block("s1", 1, "# Kế hoạch v2\nMới.\n")], 1).unwrap();
        assert_eq!(ok[0].text, "# Kế hoạch v2\nMới.\n\n");
        assert_eq!(ok[0].level, 0);
        let bad = validate_and_merge(&s, &req, &[block("s1", 1, "Mới.\n## 9. Lạ\n")], 1);
        assert_eq!(bad, Err(FallbackReason::BadStructure));
    }

    #[test]
    fn ratio_reason_and_serde_names() {
        // `Ratio` do orchestrator trả khi section_mode_ok = false.
        let s = plan();
        let all: Vec<String> = s.iter().map(|x| x.id.clone()).collect();
        assert!(!section_mode_ok(&s, &all, 0.5));
        let names: Vec<String> = [
            FallbackReason::MissingIds,
            FallbackReason::ExtraIds,
            FallbackReason::DuplicateIds,
            FallbackReason::BaseMismatch,
            FallbackReason::BadStructure,
            FallbackReason::StructuralChange,
            FallbackReason::Ratio,
            FallbackReason::Truncated,
        ]
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
        assert_eq!(
            names,
            vec![
                "\"missing_ids\"",
                "\"extra_ids\"",
                "\"duplicate_ids\"",
                "\"base_mismatch\"",
                "\"bad_structure\"",
                "\"structural_change\"",
                "\"ratio\"",
                "\"truncated\"",
            ]
        );
    }
}
