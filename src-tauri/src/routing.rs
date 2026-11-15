//! Review theo section: người viết gắn tag reviewer ngay dưới mỗi tiêu đề `##` / `###`:
//!
//! ```markdown
//! ## 3. Data Flow
//! <!-- reviewers: backend_dev, dba -->
//! ### 3.1 Client cache
//! <!-- reviewers: frontend_dev -->
//! ```
//!
//! - `###` không có tag → kế thừa tag của `##` cha. Block không có tag / tag `all` → mọi reviewer đọc.
//! - Section đầu tiên (Overview / Scope) luôn được gửi làm ngữ cảnh chung.
//! - Section không thuộc reviewer → chỉ giữ tiêu đề + ghi chú "lược bớt".
//! - Plan không có tag nào → gửi nguyên văn (an toàn, như cũ).
//!
//! `fingerprint` = hash phần reviewer phụ trách → vòng sau nếu không đổi & lần trước đã APPROVE
//! thì giữ nguyên APPROVE, không gọi model.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const FENCE_PREFIXES: [&str; 2] = ["```", "~~~"];

#[derive(Debug, Clone)]
struct Block {
    /// 0 = phần mở đầu trước tiêu đề đầu tiên, 2 = `##`, 3 = `###`
    level: u8,
    heading: String,
    /// None = không có tag (→ mọi reviewer)
    tags: Option<Vec<String>>,
    text: String,
    /// Thứ tự của `##` (0 = section đầu tiên)
    section_index: usize,
}

fn parse_tags(line: &str) -> Option<Vec<String>> {
    let l = line.trim();
    let inner = l.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let (key, rest) = inner.split_once(':')?;
    if !matches!(key.trim().to_ascii_lowercase().as_str(), "reviewers" | "reviewer" | "review") {
        return None;
    }
    let tags: Vec<String> = rest
        .split([',', ' ', ';'])
        .map(|s| s.trim().trim_start_matches('@').to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    (!tags.is_empty()).then_some(tags)
}

fn parse(md: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut cur = Block { level: 0, heading: String::new(), tags: None, text: String::new(), section_index: 0 };
    let mut in_fence = false;
    let mut parent_tags: Option<Vec<String>> = None;
    let mut section_no = 0usize;
    let mut expect_tag = false;

    for line in md.lines() {
        let t = line.trim_start();
        if FENCE_PREFIXES.iter().any(|f| t.starts_with(f)) {
            in_fence = !in_fence;
        }
        let level = if in_fence {
            0
        } else if t.starts_with("### ") {
            3
        } else if t.starts_with("## ") {
            2
        } else {
            0
        };
        if level > 0 {
            blocks.push(std::mem::replace(
                &mut cur,
                Block { level, heading: line.to_owned(), tags: None, text: String::new(), section_index: 0 },
            ));
            if level == 2 {
                section_no += 1;
                parent_tags = None;
            } else {
                cur.tags = parent_tags.clone(); // ### kế thừa tag của ## cha
            }
            cur.section_index = section_no.saturating_sub(1);
            cur.text.push_str(line);
            cur.text.push('\n');
            expect_tag = true;
            continue;
        }
        if expect_tag && !t.is_empty() {
            expect_tag = false;
            if let Some(tags) = parse_tags(t) {
                if cur.level == 2 {
                    parent_tags = Some(tags.clone());
                }
                cur.tags = Some(tags);
            }
        }
        cur.text.push_str(line);
        cur.text.push('\n');
    }
    blocks.push(cur);
    blocks.retain(|b| b.level > 0 || !b.text.trim().is_empty());
    blocks
}

fn wants(block: &Block, role_id: &str) -> bool {
    match &block.tags {
        None => true,
        Some(tags) => tags.iter().any(|t| t == role_id || t.eq_ignore_ascii_case("all")),
    }
}

/// Plan có dùng tag reviewer không.
pub fn is_tagged(md: &str) -> bool {
    parse(md).iter().any(|b| b.tags.is_some())
}

/// Bản plan rút gọn cho 1 reviewer. None = gửi nguyên văn (không có tag / role không được gắn ở đâu / rút gọn không đáng kể).
pub fn view_for(md: &str, role_id: &str) -> Option<String> {
    let blocks = parse(md);
    if !blocks.iter().any(|b| b.tags.is_some()) {
        return None;
    }
    let mentioned = blocks
        .iter()
        .any(|b| b.tags.as_ref().is_some_and(|t| t.iter().any(|x| x == role_id)));
    if !mentioned {
        return None; // reviewer mới được mời / người viết quên gắn → đọc toàn bộ cho chắc
    }
    let mut out = String::new();
    let mut omitted = 0usize;
    for b in &blocks {
        if b.level == 0 || (b.level == 2 && b.section_index == 0) || wants(b, role_id) {
            out.push_str(&b.text);
        } else {
            out.push_str(&b.heading);
            out.push_str("\n_(lược bớt: phần này do reviewer khác phụ trách)_\n\n");
            omitted += b.text.len();
        }
    }
    // Chỉ dùng bản rút gọn khi tiết kiệm được ≥ 15%.
    (omitted * 100 >= md.len() * 15).then_some(out)
}

/// Hash các block reviewer phụ trách (không tính phần lược bớt). Bỏ khoảng trắng thừa để
/// thay đổi định dạng vặt không làm mất "giữ nguyên APPROVE".
///
/// Plan có tag: Overview (section 1) chỉ là ngữ cảnh chung và gần như luôn đổi giữa các version →
/// không tính; chỉ tính phần được gắn cho reviewer + phần `all`. Plan không tag: tính toàn bộ.
pub fn fingerprint(md: &str, role_id: &str) -> u64 {
    let blocks = parse(md);
    let tagged = blocks.iter().any(|b| b.tags.is_some());
    let mut h = DefaultHasher::new();
    for b in &blocks {
        let context_only = b.level == 0 || (b.section_index == 0 && b.tags.is_none());
        let include = if tagged { !context_only && wants(b, role_id) } else { true };
        if include && !is_changelog(&b.heading) {
            for w in b.text.split_whitespace() {
                w.hash(&mut h);
            }
        }
    }
    h.finish()
}

/// Section Changelog luôn đổi giữa các version → không tính vào fingerprint.
fn is_changelog(heading: &str) -> bool {
    heading.to_lowercase().contains("changelog")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "## 1. Overview\nTổng quan dùng chung cho mọi người, mô tả mục tiêu.\n\
## 2. Architecture\n<!-- reviewers: all -->\nKiến trúc tổng.\n\
## 3. Frontend (A)\n<!-- reviewers: frontend_dev -->\nNội dung A rất dài về UI, state, cache phía client và trải nghiệm người dùng.\n\
## 4. Backend (B)\n<!-- reviewers: backend_dev -->\nNội dung B rất dài về API, database, transaction, queue và worker xử lý nền.\n\
## 5. Integration (C)\n<!-- reviewers: frontend_dev, backend_dev -->\nHợp đồng API giữa A và B.\n\
### 5.1 Auth\nToken refresh.\n\
## 6. Changelog (v1 → v2)\n| # | x |\n";

    #[test]
    fn frontend_reads_a_and_c_only() {
        let v = view_for(PLAN, "frontend_dev").expect("phải rút gọn");
        assert!(v.contains("Nội dung A") && v.contains("Hợp đồng API") && v.contains("Token refresh"));
        assert!(v.contains("Tổng quan") && v.contains("Kiến trúc tổng"), "overview + 'all' luôn có");
        assert!(!v.contains("Nội dung B"), "không gửi phần backend");
        assert!(v.contains("## 4. Backend (B)\n_(lược bớt"), "giữ tiêu đề phần bị lược");
        let b = view_for(PLAN, "backend_dev").unwrap();
        assert!(b.contains("Nội dung B") && !b.contains("Nội dung A") && b.contains("Token refresh"));
    }

    #[test]
    fn untagged_or_unmentioned_falls_back_to_full_plan() {
        assert!(view_for("## 1. Overview\nx\n## 2. Y\ny\n", "dba").is_none());
        assert!(view_for(PLAN, "dba").is_none(), "role chưa được gắn ở đâu → đọc toàn bộ");
        assert!(is_tagged(PLAN));
    }

    #[test]
    fn fingerprint_ignores_other_sections_and_changelog() {
        let changed_b = PLAN.replace("worker xử lý nền", "worker + retry");
        assert_eq!(fingerprint(PLAN, "frontend_dev"), fingerprint(&changed_b, "frontend_dev"), "B đổi không ảnh hưởng frontend");
        assert_ne!(fingerprint(PLAN, "backend_dev"), fingerprint(&changed_b, "backend_dev"));
        let new_changelog = PLAN.replace("| # | x |", "| 1 | sửa B |");
        assert_eq!(fingerprint(PLAN, "frontend_dev"), fingerprint(&new_changelog, "frontend_dev"));
        let shared = PLAN.replace("Token refresh", "Token refresh mỗi 15 phút");
        assert_ne!(fingerprint(PLAN, "frontend_dev"), fingerprint(&shared, "frontend_dev"), "C đổi → cả 2 phải đọc lại");
    }

    #[test]
    fn headings_inside_code_fences_are_ignored() {
        let md = "## 1. O\no\n## 2. A\n<!-- reviewers: x -->\n```md\n## not a heading\n```\nnội dung A dài dài dài dài dài dài dài dài dài\n## 3. B\n<!-- reviewers: y -->\nnội dung B dài dài dài dài dài dài dài dài dài dài\n";
        let v = view_for(md, "x").unwrap();
        assert!(v.contains("## not a heading") && v.contains("nội dung A") && !v.contains("nội dung B"));
    }
}
