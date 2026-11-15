//! Tự chọn thành phần họp: 1 lần gọi model (rẻ, ~2–3k token) đọc yêu cầu + danh sách role
//! (1 dòng/role) + phòng ban, trả về chủ trì, reviewer, phòng ban kèm lý do.
//! Kết quả luôn được kiểm tra lại phía server (id có thật, giới hạn số lượng, role bắt buộc).

use std::collections::BTreeMap;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::llm_client::{LlmClient, LlmError, Usage};
use crate::models::StaffingInfo;
use crate::prompts::{self, CompanyMode};

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStaffing {
    chair_id: String,
    #[serde(deserialize_with = "crate::models::flexible_string_list")]
    reviewer_ids: Vec<String>,
    #[serde(deserialize_with = "crate::models::flexible_string_list")]
    department_ids: Vec<String>,
    reasons: BTreeMap<String, String>,
}

/// Áp ràng buộc lên đề xuất của model; thiếu/sai → bù bằng cấu hình mặc định.
pub fn sanitize(cfg: &AppConfig, chair: String, reviewers: Vec<String>, depts: Vec<String>, reasons: BTreeMap<String, String>) -> StaffingInfo {
    let m = &cfg.meeting;
    let usable = |id: &str| cfg.role(id).is_some_and(|r| cfg.provider(&r.provider_id).is_some() && !r.model.trim().is_empty());
    let chair_id = if usable(&chair) { chair } else { m.chair_id.clone() };

    let mut out: Vec<String> = Vec::new();
    let push = |id: &str, out: &mut Vec<String>| {
        if id != chair_id && usable(id) && !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    };
    for id in &m.required_role_ids {
        push(id, &mut out);
    }
    let cap = (m.max_auto_reviewers as usize).max(out.len());
    for id in &reviewers {
        if out.len() >= cap {
            break;
        }
        push(id, &mut out);
    }
    let mut note = String::new();
    if out.is_empty() {
        for id in &m.reviewer_ids {
            push(id, &mut out);
        }
        note = "Model không chọn được reviewer hợp lệ → dùng reviewer mặc định.".into();
    }

    let mut dept_ids: Vec<String> = Vec::new();
    for d in depts {
        if dept_ids.len() >= m.max_auto_departments as usize {
            break;
        }
        // Chỉ nhận phòng ban mà trưởng phòng đã gán model (nếu không phòng ban sẽ lỗi ngay).
        if cfg.department(&d).is_some_and(|dep| usable(&dep.lead_id)) && !dept_ids.contains(&d) {
            dept_ids.push(d);
        }
    }
    let keep: Vec<&String> = std::iter::once(&chair_id).chain(out.iter()).chain(dept_ids.iter()).collect();
    let reasons = reasons
        .into_iter()
        .filter(|(k, _)| keep.contains(&k))
        .map(|(k, v)| (k, crate::models::truncate_chars(v.trim(), 160)))
        .collect();
    StaffingInfo { chair_id, reviewer_ids: out, department_ids: dept_ids, reasons, fallback: false, note }
}

/// Đề xuất thành phần họp cho `request`. Lỗi model → trả về cấu hình mặc định (`fallback = true`).
pub async fn propose(
    llm: &LlmClient,
    cfg: &AppConfig,
    request: &str,
    company: &CompanyMode,
    cancel: &CancellationToken,
) -> Result<(StaffingInfo, Option<Usage>, usize, usize), LlmError> {
    let m = &cfg.meeting;
    let fallback = |note: String| StaffingInfo {
        chair_id: m.chair_id.clone(),
        reviewer_ids: m.reviewer_ids.clone(),
        department_ids: m.department_ids.clone(),
        reasons: BTreeMap::new(),
        fallback: true,
        note,
    };
    // Điều phối viên = chủ trì mặc định (model đã được người dùng tin dùng cho việc tổng hợp).
    let Some(coordinator) = cfg.role(&m.chair_id) else {
        return Ok((fallback("Chưa có chủ trì hợp lệ.".into()), None, 0, 0));
    };
    let Some(ep) = cfg.resolve(&coordinator.id) else {
        return Ok((fallback("Chủ trì chưa có provider/model.".into()), None, 0, 0));
    };
    let msgs = prompts::staffing_messages(coordinator, cfg, request, m.max_auto_reviewers, m.max_auto_departments, company);
    let prompt_chars = msgs.iter().map(|x| x.content.len()).sum();
    match llm.chat_json::<RawStaffing>(&ep, &msgs, cancel).await {
        Ok((raw, text, usage)) => Ok((sanitize(cfg, raw.chair_id, raw.reviewer_ids, raw.department_ids, raw.reasons), usage, prompt_chars, text.len())),
        Err(LlmError::Cancelled) => Err(LlmError::Cancelled),
        Err(e) => {
            log::warn!("chọn thành phần họp lỗi ({e}); dùng cấu hình mặc định");
            Ok((fallback(format!("Không đề xuất được ({e}); dùng thành phần mặc định.")), None, prompt_chars, 0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::with_default_provider;

    #[test]
    fn sanitize_enforces_required_caps_and_valid_ids() {
        let mut cfg = with_default_provider("k", "m");
        cfg.meeting.required_role_ids = vec!["ciso".into()];
        cfg.meeting.max_auto_reviewers = 3;
        cfg.meeting.max_auto_departments = 1;
        let s = sanitize(
            &cfg,
            "cto".into(),
            vec!["cto".into(), "ghost".into(), "backend_dev".into(), "dba".into(), "tester".into(), "qa_lead".into()],
            vec!["qa".into(), "nope".into(), "growth".into()],
            BTreeMap::from([("backend_dev".into(), "API".into()), ("ghost".into(), "x".into())]),
        );
        assert_eq!(s.chair_id, "cto");
        assert_eq!(s.reviewer_ids, ["ciso", "backend_dev", "dba"], "bắt buộc trước, bỏ chủ trì/role ma, tối đa 3");
        assert_eq!(s.department_ids, ["qa"]);
        assert!(s.reasons.contains_key("backend_dev") && !s.reasons.contains_key("ghost"));
    }

    #[test]
    fn sanitize_falls_back_when_model_picks_nothing_usable() {
        let mut cfg = with_default_provider("k", "m");
        cfg.roles.iter_mut().find(|r| r.id == "tester").unwrap().model.clear();
        let s = sanitize(&cfg, "nobody".into(), vec!["tester".into()], vec![], BTreeMap::new());
        assert_eq!(s.chair_id, cfg.meeting.chair_id);
        assert_eq!(s.reviewer_ids, cfg.meeting.reviewer_ids, "tester chưa có model → không dùng được");
        assert!(!s.note.is_empty());
    }
}
