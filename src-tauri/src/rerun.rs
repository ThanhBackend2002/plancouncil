//! Chạy lại phiên: `rerun_from_version` (bắt đầu từ 1 phiên bản plan đã lưu) và
//! `restart_from_request` (chạy lại yêu cầu gốc với thành phần đã chụp).
//!
//! Module này chỉ **lập kế hoạch** (thuần, không I/O): kiểm tra phiên bản, đối chiếu thành phần
//! của phiên gốc với cấu hình HIỆN TẠI theo id role, áp override rồi trả về config + seed.
//! Việc khởi chạy dùng chung đường `start_council` ở `lib.rs`.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::config::{AppConfig, SessionOptions};
use crate::models::{ParentRef, SessionRecord, Verdict, MAIN_SCOPE};
use crate::orchestrator::Seed;
use crate::routing;

/// Override cho phiên chạy lại. Thiếu field = giữ như phiên gốc / cấu hình hiện tại.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RerunOverrides {
    /// Tập con thành phần reviewer của phiên gốc.
    pub reviewers: Option<Vec<String>>,
    /// 1..=10.
    pub max_rounds: Option<u32>,
    /// Thiếu → giữ cấu hình hiện tại; `null` → không giới hạn; số → giới hạn mới (≥ 1000).
    #[serde(deserialize_with = "present_option")]
    pub max_total_tokens: Option<Option<u64>>,
}

/// Phân biệt "không gửi" (None) với `null` (Some(None)).
fn present_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<u64>>, D::Error> {
    Option::<u64>::deserialize(d).map(Some)
}

/// Kết quả trả về UI (cả khi `dry_run`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RerunResult {
    /// None khi `dry_run`.
    pub new_session_id: Option<Uuid>,
    /// Section chỉ gắn tag cho reviewer không còn trong nhóm mới (sẽ bị coi như `all`).
    pub uncovered_sections: Vec<String>,
    /// Role của phiên gốc không còn trong cấu hình hoặc chưa có provider/model dùng được.
    pub missing_roles: Vec<String>,
    /// Các phiên bản plan đã lưu của scope (rỗng → UI đề nghị "chạy lại từ yêu cầu").
    pub available_versions: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RerunError {
    #[error("Không có phiên bản plan v{version} hợp lệ trong phiên gốc")]
    NoValidVersion { version: u32, available: Vec<u32> },
    #[error("Thành phần của phiên gốc có role không còn dùng được: {} — bỏ họ khỏi danh sách reviewer để tiếp tục", roles.join(", "))]
    MissingRoles { roles: Vec<String> },
    #[error("Chưa hỗ trợ chạy lại từ phiên bản của phòng ban '{0}' — hãy chọn phiên bản của cuộc họp chính")]
    UnsupportedScope(String),
    #[error("{0}")]
    Invalid(String),
}

impl RerunError {
    pub fn code(&self) -> &'static str {
        match self {
            RerunError::NoValidVersion { .. } => "no_valid_version",
            RerunError::MissingRoles { .. } => "missing_roles",
            RerunError::UnsupportedScope(_) => "unsupported_scope",
            RerunError::Invalid(_) => "invalid",
        }
    }
}

/// Mọi thứ cần để khởi chạy phiên mới (qua đường `start_council`).
#[derive(Debug, Clone)]
pub struct RerunPlan {
    pub prompt: String,
    /// Config hiện tại + thành phần / số vòng / ngân sách của phiên chạy lại.
    pub cfg: AppConfig,
    pub options: SessionOptions,
    pub seed: Option<Seed>,
    pub parent: ParentRef,
    /// Role thiếu nằm trong nhóm sẽ họp → không được khởi chạy (chỉ báo khi `dry_run`).
    pub blocking: Vec<String>,
    pub preview: RerunResult,
}

/// Các phiên bản plan đã lưu của scope. None = scope không có trong bản ghi.
pub fn available_versions(rec: &SessionRecord, scope: &str) -> Option<Vec<u32>> {
    if scope == MAIN_SCOPE {
        return Some(rec.plans.iter().map(|p| p.version).collect());
    }
    rec.departments.iter().find(|d| d.id == scope).map(|d| d.plans.iter().map(|p| p.version).collect())
}

/// Role dùng được với cấu hình hiện tại: còn tồn tại, provider còn, đã chọn model.
fn usable(cfg: &AppConfig, id: &str) -> bool {
    cfg.role(id)
        .is_some_and(|r| cfg.provider(&r.provider_id).is_some() && !r.model.trim().is_empty())
}

fn dedupe(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter().filter(|id| seen.insert(id.clone())).collect()
}

/// Thành phần reviewer cuộc họp chính của phiên gốc: reviewer ban đầu + người được mời vào giữa chừng.
fn main_roster(rec: &SessionRecord) -> Vec<String> {
    let invited = rec
        .membership
        .iter()
        .filter(|m| m.scope == MAIN_SCOPE && m.action == "invited")
        .map(|m| m.agent.clone());
    dedupe(rec.reviewer_ids.iter().cloned().chain(invited))
        .into_iter()
        .filter(|id| *id != rec.chair_id)
        .collect()
}

/// Tuỳ chọn phiên (workspace / web / bối cảnh công ty) theo ảnh chụp của phiên gốc.
fn options_from(cfg: &AppConfig, rec: &SessionRecord) -> Result<SessionOptions, RerunError> {
    let workspace_id = match rec.options.workspace.as_deref() {
        None => None,
        Some(name) => Some(
            cfg.workspaces
                .iter()
                .find(|w| w.name == name)
                .map(|w| w.id.clone())
                .ok_or_else(|| RerunError::Invalid(format!("Workspace \"{name}\" của phiên gốc không còn trong cấu hình")))?,
        ),
    };
    Ok(SessionOptions {
        workspace_id,
        web_search: rec.options.web_search,
        company_context: rec.options.company_context,
        ..SessionOptions::default()
    })
}

fn apply_limits(cfg: &mut AppConfig, base_rounds: u32, ov: &RerunOverrides) -> Result<(), RerunError> {
    cfg.max_rounds = match ov.max_rounds {
        Some(n) if !(1..=10).contains(&n) => return Err(RerunError::Invalid("Số vòng phải trong khoảng 1..=10".into())),
        Some(n) => n,
        None => base_rounds.clamp(1, 10),
    };
    if let Some(limit) = ov.max_total_tokens {
        cfg.meeting.max_total_tokens = limit;
    }
    cfg.validate().map_err(RerunError::Invalid)
}

/// Lập kế hoạch chạy lại từ phiên bản `version_no` của `scope`.
pub fn plan_rerun(
    cur: &AppConfig,
    rec: &SessionRecord,
    scope: &str,
    version_no: u32,
    ov: &RerunOverrides,
) -> Result<RerunPlan, RerunError> {
    let available = available_versions(rec, scope)
        .ok_or_else(|| RerunError::Invalid(format!("Phiên gốc không có nơi họp '{scope}'")))?;
    if scope != MAIN_SCOPE {
        return Err(RerunError::UnsupportedScope(scope.to_owned()));
    }
    let plan = rec
        .plans
        .iter()
        .find(|p| p.version == version_no && !p.markdown.trim().is_empty())
        .ok_or_else(|| RerunError::NoValidVersion { version: version_no, available: available.clone() })?;

    let roster = main_roster(rec);
    let missing: Vec<String> = std::iter::once(&rec.chair_id)
        .chain(&roster)
        .filter(|id| !usable(cur, id))
        .cloned()
        .collect();
    let reviewers = match &ov.reviewers {
        Some(ids) => {
            let ids = dedupe(ids.iter().map(|s| s.trim().to_owned()));
            if let Some(bad) = ids.iter().find(|id| !roster.contains(id)) {
                return Err(RerunError::Invalid(format!("Reviewer '{bad}' không thuộc thành phần của phiên gốc")));
            }
            ids
        }
        None => roster.clone(),
    };
    if reviewers.is_empty() {
        return Err(RerunError::Invalid("Cần ít nhất 1 reviewer".into()));
    }
    let blocking: Vec<String> = missing
        .iter()
        .filter(|id| **id == rec.chair_id || reviewers.contains(id))
        .cloned()
        .collect();

    let mut cfg = cur.clone();
    cfg.meeting.chair_id = rec.chair_id.clone();
    cfg.meeting.reviewer_ids = reviewers.clone();
    // Plan xuất phát đã tổng hợp báo cáo phòng ban → phiên chạy lại chỉ họp cuộc họp chính.
    cfg.meeting.department_ids = vec![];
    if blocking.is_empty() {
        apply_limits(&mut cfg, rec.max_rounds, ov)?;
    }

    // Issue còn mở: review REJECT của vòng gần nhất đã bác đúng phiên bản này.
    let open: Vec<_> = rec
        .rounds
        .iter()
        .rev()
        .find(|r| {
            r.plan_version == version_no
                && r.slots.iter().any(|s| s.review.as_ref().is_some_and(|v| v.verdict == Verdict::Reject))
        })
        .map(|r| {
            r.slots
                .iter()
                .filter_map(|s| s.review.clone())
                .filter(|v| v.verdict == Verdict::Reject && reviewers.contains(&v.agent))
                .collect()
        })
        .unwrap_or_default();

    let preview = RerunResult {
        new_session_id: None,
        uncovered_sections: routing::uncovered_sections(&plan.markdown, &reviewers),
        missing_roles: missing,
        available_versions: available,
    };
    Ok(RerunPlan {
        prompt: rec.prompt.clone(),
        options: options_from(cur, rec)?,
        cfg,
        seed: Some(Seed { version: plan.version, markdown: plan.markdown.clone(), reviews: open }),
        parent: ParentRef { session_id: rec.id, scope: scope.to_owned(), version_no },
        blocking,
        preview,
    })
}

/// Lập kế hoạch chạy lại từ yêu cầu gốc với thành phần đã chụp (chủ trì, reviewer, phòng ban,
/// số vòng, tuỳ chọn phiên). Role thiếu → `MissingRoles`.
pub fn plan_restart(cur: &AppConfig, rec: &SessionRecord) -> Result<RerunPlan, RerunError> {
    let mut cfg = cur.clone();
    let reviewers = dedupe(rec.reviewer_ids.iter().cloned());
    let depts: Vec<String> = dedupe(rec.departments.iter().map(|d| d.id.clone()));
    if let Some(d) = depts.iter().find(|d| cur.department(d).is_none()) {
        return Err(RerunError::Invalid(format!("Phòng ban '{d}' của phiên gốc không còn trong cấu hình")));
    }
    let mut ids: Vec<String> = vec![rec.chair_id.clone()];
    ids.extend(reviewers.iter().cloned());
    for d in depts.iter().filter_map(|d| cur.department(d)) {
        ids.push(d.lead_id.clone());
        ids.extend(d.member_ids.iter().cloned());
    }
    let missing: Vec<String> = dedupe(ids).into_iter().filter(|id| !usable(cur, id)).collect();
    if !missing.is_empty() {
        return Err(RerunError::MissingRoles { roles: missing });
    }
    cfg.meeting.chair_id = rec.chair_id.clone();
    cfg.meeting.reviewer_ids = reviewers;
    cfg.meeting.department_ids = depts;
    apply_limits(&mut cfg, rec.max_rounds, &RerunOverrides::default())?;
    Ok(RerunPlan {
        prompt: rec.prompt.clone(),
        options: options_from(cur, rec)?,
        cfg,
        seed: None,
        parent: ParentRef { session_id: rec.id, scope: MAIN_SCOPE.to_owned(), version_no: 0 },
        blocking: vec![],
        preview: RerunResult::default(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::models::{PlanVersion, Review, ReviewSlot, RoundRecord, SessionStatus};

    pub(crate) const TAGGED: &str = "## 1. Overview\nTổng quan.\n\
## 2. Scale\n<!-- reviewers: scale_reviewer -->\nShard theo user.\n\
## 3. Security\n<!-- reviewers: chaos_reviewer -->\nmTLS.\n\
## 4. Chung\n<!-- reviewers: all -->\nGhi chú.\n";

    fn review(agent: &str, verdict: Verdict, issue: &str) -> Review {
        Review {
            agent: agent.into(),
            role: agent.into(),
            verdict,
            summary_bubble: "s".into(),
            critical_issues: if issue.is_empty() { vec![] } else { vec![issue.into()] },
            suggestions: vec![],
            out_of_scope: vec![],
            invites: vec![],
            sections: vec![],
            carried_over: false,
            policy_override: None,
        }
    }

    /// Phiên gốc: architect chủ trì; v1 bị REJECT ở round 1, v2 REJECT ở round 2 (hết vòng).
    pub(crate) fn parent_record(cfg: &AppConfig) -> SessionRecord {
        let mut r = SessionRecord::new(Uuid::new_v4(), "Thiết kế hệ thống leaderboard realtime".into(), 2);
        r.status = SessionStatus::MaxRoundsReached;
        r.chair_id = cfg.meeting.chair_id.clone();
        r.reviewer_ids = cfg.meeting.reviewer_ids.clone();
        r.plans = vec![
            PlanVersion::new(1, "## 1. Overview\nv1\n".into(), Default::default()),
            PlanVersion::new(2, TAGGED.into(), Default::default()),
        ];
        let slots = |v: u32| {
            r.reviewer_ids
                .iter()
                .map(|id| ReviewSlot {
                    agent: id.clone(),
                    review: Some(review(id, Verdict::Reject, &format!("Issue mở của {id} ở v{v}"))),
                    error: None,
                })
                .collect::<Vec<_>>()
        };
        r.rounds = vec![
            RoundRecord { round: 1, plan_version: 1, slots: slots(1), approvals: 0, converged: false },
            RoundRecord { round: 2, plan_version: 2, slots: slots(2), approvals: 0, converged: false },
        ];
        r
    }

    fn cfg() -> AppConfig {
        crate::config::tests::with_default_provider("sk-test", "gpt-4.1-mini")
    }

    #[test]
    fn rerun_seeds_version_and_open_issues() {
        let c = cfg();
        let rec = parent_record(&c);
        let p = plan_rerun(&c, &rec, MAIN_SCOPE, 2, &RerunOverrides::default()).unwrap();
        let seed = p.seed.unwrap();
        assert_eq!((seed.version, seed.markdown.as_str()), (2, TAGGED));
        assert_eq!(seed.reviews.len(), rec.reviewer_ids.len());
        assert!(seed.reviews.iter().all(|r| r.critical_issues[0].ends_with("v2")));
        assert_eq!(p.parent, ParentRef { session_id: rec.id, scope: "main".into(), version_no: 2 });
        assert_eq!(p.preview.available_versions, vec![1, 2]);
        assert!(p.preview.missing_roles.is_empty() && p.blocking.is_empty());
        assert!(p.preview.uncovered_sections.is_empty());
        assert!(p.cfg.meeting.department_ids.is_empty());
        assert_eq!(p.cfg.max_rounds, 2);
    }

    #[test]
    fn dry_run_lists_uncovered_sections_for_dropped_reviewers() {
        let c = cfg();
        let rec = parent_record(&c);
        let ov = RerunOverrides { reviewers: Some(vec!["scale_reviewer".into()]), ..Default::default() };
        let p = plan_rerun(&c, &rec, MAIN_SCOPE, 2, &ov).unwrap();
        assert_eq!(p.preview.uncovered_sections, vec!["3"]);
        assert_eq!(p.cfg.meeting.reviewer_ids, vec!["scale_reviewer"]);
        assert!(p.seed.unwrap().reviews.iter().all(|r| r.agent == "scale_reviewer"));
    }

    #[test]
    fn missing_roles_block_unless_excluded() {
        let mut c = cfg();
        let rec = parent_record(&c);
        // chaos_reviewer không còn provider, scale_reviewer mất model.
        c.roles.iter_mut().find(|r| r.id == "chaos_reviewer").unwrap().provider_id.clear();
        c.roles.iter_mut().find(|r| r.id == "scale_reviewer").unwrap().model.clear();
        let p = plan_rerun(&c, &rec, MAIN_SCOPE, 2, &RerunOverrides::default()).unwrap();
        let mut missing = p.preview.missing_roles.clone();
        missing.sort();
        assert_eq!(missing, vec!["chaos_reviewer", "scale_reviewer"]);
        assert_eq!(p.blocking.len(), 2);
        let keep: Vec<String> = rec.reviewer_ids.iter().filter(|r| !missing.contains(r)).cloned().collect();
        assert!(!keep.is_empty());
        let ov = RerunOverrides { reviewers: Some(keep), ..Default::default() };
        let p = plan_rerun(&c, &rec, MAIN_SCOPE, 2, &ov).unwrap();
        assert!(p.blocking.is_empty());
        assert_eq!(p.preview.missing_roles.len(), 2, "vẫn báo để UI hiển thị");
        // Reviewer ngoài thành phần gốc → invalid.
        let ov = RerunOverrides { reviewers: Some(vec!["cto".into()]), ..Default::default() };
        assert!(matches!(plan_rerun(&c, &rec, MAIN_SCOPE, 2, &ov), Err(RerunError::Invalid(_))));
        // restart_from_request: role thiếu → lỗi có kiểu.
        assert_eq!(plan_restart(&c, &rec).unwrap_err().code(), "missing_roles");
        // Role bị xoá hẳn khỏi cấu hình cũng là "missing".
        let mut gone = rec.clone();
        gone.reviewer_ids.push("deleted_role".into());
        let p = plan_rerun(&cfg(), &gone, MAIN_SCOPE, 2, &RerunOverrides::default()).unwrap();
        assert_eq!(p.preview.missing_roles, vec!["deleted_role"]);
        assert_eq!(p.blocking, vec!["deleted_role"]);
    }

    #[test]
    fn invalid_versions_scopes_and_overrides() {
        let c = cfg();
        let rec = parent_record(&c);
        let e = plan_rerun(&c, &rec, MAIN_SCOPE, 7, &RerunOverrides::default()).unwrap_err();
        assert_eq!(e, RerunError::NoValidVersion { version: 7, available: vec![1, 2] });
        assert_eq!(e.code(), "no_valid_version");
        let mut empty = rec.clone();
        empty.plans.clear();
        assert_eq!(
            plan_rerun(&c, &empty, MAIN_SCOPE, 1, &RerunOverrides::default()).unwrap_err(),
            RerunError::NoValidVersion { version: 1, available: vec![] }
        );
        assert!(matches!(plan_rerun(&c, &rec, "qa", 1, &RerunOverrides::default()), Err(RerunError::Invalid(_))));
        let mut with_dept = rec.clone();
        with_dept.departments.push(crate::models::DepartmentRecord {
            id: "qa".into(),
            name: "QA".into(),
            task: "t".into(),
            lead_id: "cto".into(),
            member_ids: vec![],
            max_rounds: 1,
            status: SessionStatus::Converged,
            plans: vec![PlanVersion::new(1, "## 1. A\n".into(), Default::default())],
            rounds: vec![],
            final_markdown: None,
            error: None,
        });
        assert_eq!(plan_rerun(&c, &with_dept, "qa", 1, &RerunOverrides::default()).unwrap_err().code(), "unsupported_scope");
        for bad in [0, 11] {
            let ov = RerunOverrides { max_rounds: Some(bad), ..Default::default() };
            assert!(matches!(plan_rerun(&c, &rec, MAIN_SCOPE, 2, &ov), Err(RerunError::Invalid(_))));
        }
        let ov: RerunOverrides = serde_json::from_str(r#"{"max_rounds":4,"max_total_tokens":null}"#).unwrap();
        assert_eq!(ov.max_total_tokens, Some(None));
        let mut limited = c.clone();
        limited.meeting.max_total_tokens = Some(50_000);
        let p = plan_rerun(&limited, &rec, MAIN_SCOPE, 2, &ov).unwrap();
        assert_eq!((p.cfg.max_rounds, p.cfg.meeting.max_total_tokens), (4, None));
        let ov: RerunOverrides = serde_json::from_str(r#"{"max_total_tokens":5}"#).unwrap();
        assert!(matches!(plan_rerun(&c, &rec, MAIN_SCOPE, 2, &ov), Err(RerunError::Invalid(_))));
        let ov: RerunOverrides = serde_json::from_str("{}").unwrap();
        assert_eq!(ov.max_total_tokens, None);
    }

    #[test]
    fn restart_uses_snapshot_roster() {
        let c = cfg();
        let mut rec = parent_record(&c);
        rec.max_rounds = 5;
        rec.reviewer_ids.truncate(1);
        let p = plan_restart(&c, &rec).unwrap();
        assert!(p.seed.is_none());
        assert_eq!(p.cfg.meeting.reviewer_ids, rec.reviewer_ids);
        assert_eq!(p.cfg.max_rounds, 5);
        assert_eq!(p.parent.version_no, 0);
        rec.options.workspace = Some("đã xoá".into());
        assert!(matches!(plan_restart(&c, &rec), Err(RerunError::Invalid(_))));
    }
}
