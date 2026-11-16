//! Báo cáo token + chi phí của 1 phiên (`get_usage`). Chi phí tính lúc ĐỌC từ `calls` × bảng giá
//! HIỆN TẠI → người dùng sửa giá là thấy ngay, không cần chạy lại. Model chưa có giá → `null`.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::config::Pricing;
use crate::models::{CallUsage, SessionRecord};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RoleUsage {
    pub agent: String,
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
    pub calls: u32,
    pub estimated: bool,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RoundUsage {
    pub scope: String,
    /// 0 = trước vòng review đầu (chọn thành phần / giao việc / bản nháp).
    pub round: u32,
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
    pub calls: u32,
    pub estimated: bool,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageTotals {
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
    pub estimated: bool,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageBreakdown {
    pub by_role: Vec<RoleUsage>,
    pub by_round: Vec<RoundUsage>,
    pub totals: UsageTotals,
    /// Model đã dùng nhưng chưa có trong bảng giá (sắp xếp, không trùng).
    pub unpriced_models: Vec<String>,
}

/// Cộng dồn 1 nhóm lần gọi; chi phí None ngay khi có 1 lần gọi không có giá.
#[derive(Default)]
struct Acc {
    prompt: u64,
    cached: u64,
    completion: u64,
    calls: u32,
    estimated: bool,
    cost: Option<f64>,
}

impl Acc {
    fn new() -> Self {
        Self { cost: Some(0.0), ..Self::default() }
    }

    fn add(&mut self, c: &CallUsage, price: Option<f64>) {
        self.prompt += c.prompt;
        self.cached += c.cached;
        self.completion += c.completion;
        self.calls += 1;
        self.estimated |= c.estimated;
        self.cost = self.cost.zip(price).map(|(a, b)| a + b);
    }
}

pub fn usage_breakdown(rec: &SessionRecord, pricing: &Pricing) -> UsageBreakdown {
    let total = &rec.usage.total;
    if rec.calls.is_empty() {
        // Bản ghi cũ (chưa lưu từng lần gọi): chỉ có tổng theo role, không tính được chi phí.
        return UsageBreakdown {
            by_role: rec
                .usage
                .by_role
                .iter()
                .map(|(agent, u)| RoleUsage {
                    agent: agent.clone(),
                    prompt: u.prompt_tokens,
                    cached: u.cached_tokens,
                    completion: u.completion_tokens,
                    calls: u.calls,
                    estimated: u.estimated_calls > 0,
                    cost_usd: None,
                })
                .collect(),
            by_round: vec![],
            totals: UsageTotals {
                prompt: total.prompt_tokens,
                cached: total.cached_tokens,
                completion: total.completion_tokens,
                estimated: total.estimated_calls > 0,
                cost_usd: None,
            },
            unpriced_models: vec![],
        };
    }

    let mut unpriced = BTreeSet::new();
    let mut all = Acc::new();
    // Giữ thứ tự xuất hiện (lần gọi được ghi theo thời gian).
    let mut roles: Vec<(String, Acc)> = Vec::new();
    let mut role_ix: HashMap<String, usize> = HashMap::new();
    let mut rounds: Vec<((String, u32), Acc)> = Vec::new();
    let mut round_ix: HashMap<(String, u32), usize> = HashMap::new();
    for c in &rec.calls {
        let price = pricing.get(&c.model).map(|p| p.cost(c.prompt, c.cached, c.completion));
        if price.is_none() {
            unpriced.insert(c.model.clone());
        }
        all.add(c, price);
        let i = *role_ix.entry(c.agent.clone()).or_insert_with(|| {
            roles.push((c.agent.clone(), Acc::new()));
            roles.len() - 1
        });
        roles[i].1.add(c, price);
        let key = (c.scope.clone(), c.round);
        let j = *round_ix.entry(key.clone()).or_insert_with(|| {
            rounds.push((key, Acc::new()));
            rounds.len() - 1
        });
        rounds[j].1.add(c, price);
    }
    // Vượt `MAX_CALL_RECORDS` → `calls` không đủ, tổng token lấy theo `usage.total` và không tính chi phí.
    let complete = total.calls == 0 || rec.calls.len() as u64 >= u64::from(total.calls);
    let totals = if complete && total.calls > 0 {
        UsageTotals {
            prompt: total.prompt_tokens,
            cached: total.cached_tokens,
            completion: total.completion_tokens,
            estimated: total.estimated_calls > 0,
            cost_usd: all.cost,
        }
    } else if complete {
        UsageTotals { prompt: all.prompt, cached: all.cached, completion: all.completion, estimated: all.estimated, cost_usd: all.cost }
    } else {
        UsageTotals {
            prompt: total.prompt_tokens,
            cached: total.cached_tokens,
            completion: total.completion_tokens,
            estimated: total.estimated_calls > 0,
            cost_usd: None,
        }
    };
    UsageBreakdown {
        by_role: roles
            .into_iter()
            .map(|(agent, a)| RoleUsage {
                agent,
                prompt: a.prompt,
                cached: a.cached,
                completion: a.completion,
                calls: a.calls,
                estimated: a.estimated,
                cost_usd: a.cost,
            })
            .collect(),
        by_round: rounds
            .into_iter()
            .map(|((scope, round), a)| RoundUsage {
                scope,
                round,
                prompt: a.prompt,
                cached: a.cached,
                completion: a.completion,
                calls: a.calls,
                estimated: a.estimated,
                cost_usd: a.cost,
            })
            .collect(),
        totals,
        unpriced_models: unpriced.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelPrice;
    use uuid::Uuid;

    fn call(id: u32, agent: &str, round: u32, model: &str, p: u64, cached: u64, c: u64) -> CallUsage {
        CallUsage {
            call_id: id,
            scope: "main".into(),
            agent: agent.into(),
            round,
            kind: "review".into(),
            model: model.into(),
            prompt: p,
            cached,
            completion: c,
            estimated: false,
            finish: "stop".into(),
            latency_ms: 5,
        }
    }

    fn rec_with(calls: Vec<CallUsage>) -> SessionRecord {
        let mut r = SessionRecord::new(Uuid::new_v4(), "p".into(), 3);
        for c in &calls {
            r.usage.total.add(c.prompt, c.cached, c.completion, c.estimated);
            r.usage.by_role.entry(c.agent.clone()).or_default().add(c.prompt, c.cached, c.completion, c.estimated);
        }
        r.calls = calls;
        r
    }

    fn pricing() -> Pricing {
        let mut p = Pricing::new();
        p.insert("cheap".into(), ModelPrice { input_per_m: 1.0, cached_per_m: 0.5, output_per_m: 2.0 });
        p
    }

    #[test]
    fn cost_is_computed_per_role_and_round_with_current_pricing() {
        let rec = rec_with(vec![
            call(0, "architect", 0, "cheap", 1_000_000, 0, 500_000),
            call(1, "dba", 1, "cheap", 1_000_000, 1_000_000, 0),
            call(2, "architect", 1, "cheap", 0, 0, 1_000_000),
        ]);
        let u = usage_breakdown(&rec, &pricing());
        assert_eq!(u.totals.cost_usd, Some(2.0 + 0.5 + 2.0));
        assert_eq!((u.totals.prompt, u.totals.cached, u.totals.completion), (2_000_000, 1_000_000, 1_500_000));
        assert_eq!(u.by_role.iter().map(|r| (r.agent.as_str(), r.cost_usd)).collect::<Vec<_>>(), vec![("architect", Some(4.0)), ("dba", Some(0.5))]);
        assert_eq!(u.by_round.iter().map(|r| (r.round, r.calls, r.cost_usd)).collect::<Vec<_>>(), vec![(0, 1, Some(2.0)), (1, 2, Some(2.5))]);
        assert!(u.unpriced_models.is_empty());
    }

    #[test]
    fn missing_price_makes_cost_null() {
        let rec = rec_with(vec![call(0, "architect", 0, "cheap", 10, 0, 10), call(1, "dba", 1, "mystery", 10, 0, 10)]);
        let u = usage_breakdown(&rec, &pricing());
        assert_eq!(u.totals.cost_usd, None);
        assert_eq!(u.unpriced_models, vec!["mystery"]);
        assert!(u.by_role[0].cost_usd.is_some() && u.by_role[1].cost_usd.is_none());
        // Bảng giá rỗng → mọi thứ null.
        assert_eq!(usage_breakdown(&rec, &Pricing::new()).totals.cost_usd, None);
    }

    #[test]
    fn old_records_have_role_totals_only() {
        let old: SessionRecord = serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000000","prompt":"p","created_at":"2026-01-01T00:00:00Z",
                "finished_at":null,"status":"converged","max_rounds":3,"plans":[],"rounds":[],
                "usage":{"total":{"prompt_tokens":30,"completion_tokens":5,"calls":2,"estimated_calls":1},
                         "by_role":{"dba":{"prompt_tokens":30,"completion_tokens":5,"calls":2,"estimated_calls":1}}},
                "final_markdown":null,"error":null}"#,
        )
        .unwrap();
        assert!(old.calls.is_empty());
        let u = usage_breakdown(&old, &pricing());
        assert!(u.by_round.is_empty());
        assert_eq!(u.totals, UsageTotals { prompt: 30, cached: 0, completion: 5, estimated: true, cost_usd: None });
        assert_eq!(u.by_role.len(), 1);
        assert_eq!(u.by_role[0].cost_usd, None);
    }

    #[test]
    fn capped_call_log_gives_null_cost() {
        let mut rec = rec_with(vec![call(0, "a", 0, "cheap", 10, 0, 10)]);
        rec.usage.total.add(10, 0, 10, false); // 1 lần gọi không còn trong `calls`
        let u = usage_breakdown(&rec, &pricing());
        assert_eq!(u.totals.cost_usd, None);
        assert_eq!(u.totals.prompt, 20);
    }
}
