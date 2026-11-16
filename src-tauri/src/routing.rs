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
    /// Số đầu tiêu đề: "## 3. Data Flow" → "3", "### 3.3 Foo" → "3.3"
    number: Option<String>,
    /// Số của `##` cha (chỉ với `###`)
    parent_number: Option<String>,
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
    let mut cur = Block {
        level: 0,
        heading: String::new(),
        tags: None,
        text: String::new(),
        section_index: 0,
        number: None,
        parent_number: None,
    };
    let mut in_fence = false;
    let mut parent_tags: Option<Vec<String>> = None;
    let mut parent_number: Option<String> = None;
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
                Block {
                    level,
                    heading: line.to_owned(),
                    tags: None,
                    text: String::new(),
                    section_index: 0,
                    number: heading_number(line),
                    parent_number: None,
                },
            ));
            if level == 2 {
                section_no += 1;
                parent_tags = None;
                parent_number = cur.number.clone();
            } else {
                cur.tags = parent_tags.clone(); // ### kế thừa tag của ## cha
                cur.parent_number = parent_number.clone();
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

/// Số đầu tiêu đề: "## 3. Data Flow" → "3", "### 3.3 Foo" → "3.3". Không có số → None.
fn heading_number(heading: &str) -> Option<String> {
    let rest = heading.trim_start().trim_start_matches('#').trim_start();
    let chars: Vec<char> = rest.chars().collect();
    let end = scan_number(&chars, 0);
    if end == 0 {
        return None;
    }
    // Sau số phải là hết dòng / khoảng trắng / dấu câu ("3D Engine" không phải mục 3).
    match chars.get(end) {
        None => {}
        Some(c) if c.is_whitespace() || matches!(c, '.' | ')' | ':' | '-' | '–' | '—') => {}
        Some(_) => return None,
    }
    Some(chars[..end].iter().collect())
}

/// Đọc `\d+(\.\d+)*` bắt đầu tại `i`; trả về vị trí kết thúc (= `i` nếu không có số).
fn scan_number(chars: &[char], mut i: usize) -> usize {
    let start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return start;
    }
    while i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
        i += 1;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    i
}

/// Trước vị trí số (bỏ khoảng trắng / ':') là "section", "mục" hoặc "§".
fn after_keyword(before: &[char]) -> bool {
    let mut j = before.len();
    while j > 0 && (before[j - 1].is_whitespace() || before[j - 1] == ':') {
        j -= 1;
    }
    let tail: String = before[j.saturating_sub(8)..j].iter().collect::<String>().to_lowercase();
    ["section", "sections", "mục", "§"].iter().any(|k| tail.ends_with(k))
}

/// Các số mục được nhắc trong văn bản: nằm trong ngoặc tròn, đứng sau "section"/"mục"/"§",
/// hoặc có dấu chấm (3.3). "2.1-5" → "2.1" (phần sau gạch nối là số thứ tự item).
fn mentioned_numbers(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth = depth.saturating_sub(1);
        }
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let prev = start.checked_sub(1).map(|j| chars[j]);
        i = scan_number(&chars, start);
        let token: String = chars[start..i].iter().collect();
        // Bỏ số thứ tự item sau gạch nối: "2.1-5", "3.3–2".
        if i + 1 < chars.len() && matches!(chars[i], '-' | '–') && chars[i + 1].is_ascii_digit() {
            i = scan_number(&chars, i + 1);
        }
        let next = chars.get(i).copied();
        // "v1", "#3", "x2", "1.5x", "3D" → không phải số mục.
        if prev.is_some_and(|p| p.is_alphanumeric() || matches!(p, '#' | '_' | '.'))
            || next.is_some_and(|n| n.is_alphanumeric() || n == '_')
        {
            continue;
        }
        if depth > 0 || token.contains('.') || after_keyword(&chars[..start]) {
            out.push(token);
        }
    }
    out
}

/// Các section (theo số tiêu đề, vd "3.3", "4") reviewer phải được đọc đầy đủ dù không được gắn tag:
/// mục được nhắc trong các dòng Changelog gửi cho reviewer này (cột 2 chứa id/tên) và trong
/// critical issue vòng trước — để reviewer kiểm chứng được issue của mình đã được sửa.
///
/// - `Some(vec![])` = không có gì cần giữ thêm (kể cả khi issue / dòng Changelog không nhắc số mục nào
///   — issue kiểu "Thiếu idempotency key" thường nằm ngay trong phần reviewer phụ trách).
/// - `None` = có nhắc số mục nhưng không số nào khớp tiêu đề → không biết phần sửa nằm đâu,
///   gửi nguyên plan cho chắc.
pub fn referenced_sections(md: &str, role_id: &str, role_name: &str, prev_issues: &[String]) -> Option<Vec<String>> {
    let blocks = parse(md);
    let id = role_id.trim().to_lowercase();
    let name = role_name.trim().to_lowercase();
    let addressed = |cell: &str| {
        let cell = cell.to_lowercase();
        (!id.is_empty() && cell.contains(&id)) || (!name.is_empty() && cell.contains(&name))
    };

    let mut refs: Vec<String> = prev_issues.iter().flat_map(|s| mentioned_numbers(s)).collect();
    let mut parent_changelog = false;
    for b in &blocks {
        if b.level == 2 {
            parent_changelog = is_changelog(&b.heading);
        }
        let changelog = is_changelog(&b.heading) || (b.level == 3 && parent_changelog);
        if !changelog {
            continue;
        }
        for line in b.text.lines() {
            let row = line.trim();
            if !row.starts_with('|') {
                continue;
            }
            let cells: Vec<&str> = row.trim_start_matches('|').split('|').collect();
            if cells.get(1).is_some_and(|c| addressed(c)) {
                refs.extend(mentioned_numbers(row));
            }
        }
    }

    let numbers: Vec<&str> = blocks.iter().filter_map(|b| b.number.as_deref()).collect();
    // Số không khớp tiêu đề nào → lùi về mục cha gần nhất ("3.3.1" → "3.3") nếu có (vd mục `####`).
    let mut resolved: Vec<&str> = Vec::new();
    for r in &refs {
        let mut cand = r.as_str();
        loop {
            if numbers.contains(&cand) {
                resolved.push(cand);
                break;
            }
            match cand.rsplit_once('.') {
                Some((parent, _)) => cand = parent,
                None => break,
            }
        }
    }
    if !refs.is_empty() && resolved.is_empty() {
        return None;
    }
    let mut keep: Vec<String> = Vec::new();
    for n in numbers {
        if resolved.contains(&n) && !keep.iter().any(|k| k == n) {
            keep.push(n.to_owned());
        }
    }
    Some(keep)
}

/// Bản plan rút gọn cho 1 reviewer. None = gửi nguyên văn (không có tag / role không được gắn ở đâu / rút gọn không đáng kể).
///
/// `keep` = số tiêu đề luôn gửi đầy đủ (xem `referenced_sections`); số của `##` kéo theo mọi `###` con.
pub fn view_for(md: &str, role_id: &str, keep: &[String]) -> Option<String> {
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
    let kept = |n: &Option<String>| n.as_ref().is_some_and(|n| keep.iter().any(|k| k == n));
    for b in &blocks {
        if b.level == 0
            || (b.level == 2 && b.section_index == 0)
            || wants(b, role_id)
            || kept(&b.number)
            || kept(&b.parent_number)
        {
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
        let v = view_for(PLAN, "frontend_dev", &[]).expect("phải rút gọn");
        assert!(v.contains("Nội dung A") && v.contains("Hợp đồng API") && v.contains("Token refresh"));
        assert!(v.contains("Tổng quan") && v.contains("Kiến trúc tổng"), "overview + 'all' luôn có");
        assert!(!v.contains("Nội dung B"), "không gửi phần backend");
        assert!(v.contains("## 4. Backend (B)\n_(lược bớt"), "giữ tiêu đề phần bị lược");
        let b = view_for(PLAN, "backend_dev", &[]).unwrap();
        assert!(b.contains("Nội dung B") && !b.contains("Nội dung A") && b.contains("Token refresh"));
    }

    #[test]
    fn untagged_or_unmentioned_falls_back_to_full_plan() {
        assert!(view_for("## 1. Overview\nx\n## 2. Y\ny\n", "dba", &[]).is_none());
        assert!(view_for(PLAN, "dba", &[]).is_none(),"role chưa được gắn ở đâu → đọc toàn bộ");
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
        let v = view_for(md, "x", &[]).unwrap();
        assert!(v.contains("## not a heading") && v.contains("nội dung A") && !v.contains("nội dung B"));
    }

    /// Plan có Changelog trỏ tới mục bị lược với reviewer.
    fn plan_with_changelog() -> String {
        let long = "nội dung dài dài dài dài dài dài dài dài dài dài dài dài dài dài. ".repeat(6);
        format!(
            "## 1. Overview\nTổng quan.\n\
## 2. Frontend\n<!-- reviewers: ui_ux_designer, qa -->\nNội dung frontend {long}\n\
## 3. Data Flow\n<!-- reviewers: backend_dev -->\nMở đầu data flow.\n\
### 3.1 Queue\nChi tiết queue {long}\n\
### 3.3 Cache\nChi tiết cache {long}\n\
## 4. Scale\n<!-- reviewers: scale_reviewer -->\nNội dung scale {long}{long}{long}\n\
## 5. Changelog (v1 → v2)\n\
| # | Reviewer | Issue | Resolution | Status |\n\
|---|---|---|---|---|\n\
| 1 | ui_ux_designer #3 | Cache cũ | Thêm TTL (3.3) | Fixed |\n\
| 2 | Scale Reviewer #2 | Queue | xem mục 3.1-2 | Fixed |\n"
        )
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn number_extraction() {
        assert_eq!(heading_number("## 3. Data Flow").as_deref(), Some("3"));
        assert_eq!(heading_number("### 3.3 Foo").as_deref(), Some("3.3"));
        assert_eq!(heading_number("## Overview"), None);
        assert_eq!(heading_number("## 3D Engine"), None);
        assert_eq!(mentioned_numbers("resolution ... (2.1-2, 5.4)"), s(&["2.1", "5.4"]));
        assert_eq!(mentioned_numbers("2.1-5 và section 4, mục 6, §7"), s(&["2.1", "4", "6", "7"]));
        assert!(mentioned_numbers("ui_ux_designer #3, v1 → v2, bước 3, 1.5x").is_empty());
    }

    #[test]
    fn changelog_row_unomits_section_only_for_addressed_reviewer() {
        let p = plan_with_changelog();
        let keep_x = referenced_sections(&p, "ui_ux_designer", "UI/UX Designer", &[]).unwrap();
        assert_eq!(keep_x, s(&["3.3"]));
        let x = view_for(&p, "ui_ux_designer", &keep_x).expect("vẫn phải rút gọn");
        assert!(x.contains("Chi tiết cache"), "3.3 được Changelog nhắc → gửi đầy đủ");
        assert!(!x.contains("Chi tiết queue") && !x.contains("Nội dung scale"));

        // Khớp theo tên (không phân biệt hoa thường) + "3.1-2" → "3.1".
        let keep_y = referenced_sections(&p, "scale_reviewer", "Scale Reviewer", &[]).unwrap();
        assert_eq!(keep_y, s(&["3.1"]));
        let y = view_for(&p, "scale_reviewer", &keep_y).unwrap();
        assert!(y.contains("Chi tiết queue") && !y.contains("Chi tiết cache"), "dòng của X không mở 3.3 cho Y");
    }

    #[test]
    fn previous_issue_unomits_section() {
        let p = plan_with_changelog();
        assert_eq!(referenced_sections(&p, "qa", "QA", &[]), Some(vec![]));
        assert!(!view_for(&p, "qa", &[]).unwrap().contains("Chi tiết cache"));
        let issues = s(&["Cache ở 3.3 không có TTL"]);
        let keep = referenced_sections(&p, "qa", "QA", &issues).unwrap();
        assert_eq!(keep, s(&["3.3"]));
        assert!(view_for(&p, "qa", &keep).unwrap().contains("Chi tiết cache"));
    }

    #[test]
    fn unresolvable_reference_means_full_plan() {
        let p = plan_with_changelog();
        assert_eq!(referenced_sections(&p, "qa", "QA", &s(&["Thiếu mục 9.9"])), None);
        assert_eq!(referenced_sections(&p, "qa", "QA", &s(&["Thiếu mục 9.9", "Cache (3.3)"])), Some(s(&["3.3"])));
        // Issue không nhắc số mục nào → không có gì cần giữ thêm, vẫn rút gọn.
        assert_eq!(referenced_sections(&p, "qa", "QA", &s(&["Không có idempotency key"])), Some(vec![]));
        // Mục con không có tiêu đề riêng → lùi về mục cha.
        assert_eq!(referenced_sections(&p, "qa", "QA", &s(&["(3.3.1) sai"])), Some(s(&["3.3"])));
    }

    #[test]
    fn keeping_h2_pulls_in_h3_children() {
        let p = plan_with_changelog();
        let v = view_for(&p, "qa", &s(&["3"])).expect("phần scale vẫn bị lược");
        assert!(v.contains("Mở đầu data flow") && v.contains("Chi tiết queue") && v.contains("Chi tiết cache"));
        assert!(!v.contains("Nội dung scale"));
    }

    #[test]
    fn fingerprint_unaffected_by_referenced_sections() {
        let p = plan_with_changelog();
        let changed = p.replace("Chi tiết cache", "Chi tiết cache + TTL");
        assert_eq!(
            fingerprint(&p, "ui_ux_designer"),
            fingerprint(&changed, "ui_ux_designer"),
            "3.3 chỉ là ngữ cảnh kiểm chứng"
        );
        assert_ne!(fingerprint(&p, "backend_dev"), fingerprint(&changed, "backend_dev"));
    }
}
