//! Sổ ngân sách token của 1 phiên (giới hạn MỀM).
//!
//! Trước mỗi lần gọi LLM, orchestrator `reserve` một lượng ước lượng; xong việc thì `settle`
//! bằng usage thật (thiếu usage → cộng số ước lượng và đánh dấu `estimated`).
//! Giới hạn là mềm: chỉ chặn ở bước xin giữ chỗ, usage thật vượt ước lượng vẫn được cộng đủ.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::llm_client::Usage;

pub type ReservationId = u64;

#[derive(Debug, Default)]
struct Inner {
    used: u64,
    reserved: HashMap<ReservationId, u64>,
    estimated: bool,
    next_id: ReservationId,
}

impl Inner {
    fn reserved_total(&self) -> u64 {
        self.reserved.values().fold(0u64, |a, b| a.saturating_add(*b))
    }

    fn committed(&self) -> u64 {
        self.used.saturating_add(self.reserved_total())
    }

    fn insert(&mut self, est: u64) -> ReservationId {
        self.next_id += 1;
        self.reserved.insert(self.next_id, est);
        self.next_id
    }
}

/// Ảnh chụp sổ ngân sách tại 1 thời điểm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BudgetSnapshot {
    pub used: u64,
    pub reserved: u64,
    pub limit: Option<u64>,
    /// true nếu ≥ 1 lần gọi không có `usage` → `used` chứa số ước lượng.
    pub estimated: bool,
}

impl BudgetSnapshot {
    /// "ok" | "warn" (đã dùng ≥ 80%) | "exhausted" (đã dùng ≥ 100%). Không giới hạn → "ok".
    pub fn state(&self) -> &'static str {
        match self.limit {
            Some(l) if self.used >= l => "exhausted",
            Some(l) if self.used.saturating_mul(10) >= l.saturating_mul(8) => "warn",
            _ => "ok",
        }
    }
}

#[derive(Debug, Default)]
pub struct BudgetLedger {
    limit: Option<u64>,
    inner: Mutex<Inner>,
}

impl BudgetLedger {
    pub fn new(limit: Option<u64>) -> Self {
        Self { limit, inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Giữ chỗ `est` token. Nguyên tử: chỉ thành công khi `used + Σreserved + est <= limit`.
    /// Không có giới hạn → luôn thành công.
    pub fn reserve(&self, est: u64) -> Option<ReservationId> {
        self.reserve_group(&[est]).map(|ids| ids[0])
    }

    /// Giữ chỗ cho cả nhóm (vd. mọi reviewer của 1 round): hoặc tất cả, hoặc không cái nào.
    /// Nhóm rỗng (không cần gọi ai) → luôn thành công, kể cả khi đã vượt giới hạn.
    pub fn reserve_group(&self, ests: &[u64]) -> Option<Vec<ReservationId>> {
        if ests.is_empty() {
            return Some(vec![]);
        }
        let mut g = self.lock();
        if let Some(limit) = self.limit {
            let need = ests.iter().fold(g.committed(), |acc, e| acc.saturating_add(*e));
            if need > limit {
                return None;
            }
        }
        Some(ests.iter().map(|&e| g.insert(e)).collect())
    }

    /// Trả chỗ đã giữ và cộng usage thật. Thiếu usage → cộng `fallback_est`, đánh dấu `estimated`.
    /// Id không tồn tại (đã settle rồi) → bỏ qua, tránh cộng 2 lần.
    pub fn settle(&self, id: ReservationId, actual: Option<Usage>, fallback_est: u64) {
        let mut g = self.lock();
        if g.reserved.remove(&id).is_none() {
            log::warn!("budget: reservation {id} không tồn tại hoặc đã settle");
            return;
        }
        match actual {
            Some(u) => g.used = g.used.saturating_add(u.prompt_tokens + u.completion_tokens),
            None => {
                g.used = g.used.saturating_add(fallback_est);
                g.estimated = true;
            }
        }
    }

    pub fn snapshot(&self) -> BudgetSnapshot {
        let g = self.lock();
        BudgetSnapshot {
            used: g.used,
            reserved: g.reserved_total(),
            limit: self.limit,
            estimated: g.estimated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(p: u64, c: u64) -> Option<Usage> {
        Some(Usage { prompt_tokens: p, completion_tokens: c, cached_tokens: 0 })
    }

    #[test]
    fn unlimited_always_reserves() {
        let b = BudgetLedger::new(None);
        let ids = b.reserve_group(&[u64::MAX, u64::MAX]).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(b.reserve(u64::MAX).is_some());
        assert_eq!(b.snapshot().state(), "ok");
    }

    #[test]
    fn reserve_counts_used_and_outstanding_reservations() {
        let b = BudgetLedger::new(Some(1000));
        let a = b.reserve(600).unwrap();
        assert!(b.reserve(401).is_none(), "600 đang giữ + 401 > 1000");
        let c = b.reserve(400).unwrap();
        assert_eq!(b.snapshot().reserved, 1000);
        b.settle(a, usage(100, 50), 0);
        b.settle(c, usage(10, 0), 0);
        let s = b.snapshot();
        assert_eq!((s.used, s.reserved, s.estimated), (160, 0, false));
        assert!(b.reserve(840).is_some() && b.reserve(1).is_none());
    }

    #[test]
    fn group_is_all_or_nothing() {
        let b = BudgetLedger::new(Some(1000));
        let first = b.reserve(100).unwrap();
        assert!(b.reserve_group(&[300, 300, 301]).is_none());
        assert_eq!(b.snapshot().reserved, 100, "nhóm bị từ chối không được giữ chỗ phần nào");
        let ids = b.reserve_group(&[300, 300, 300]).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(b.snapshot().reserved, 1000);
        b.settle(first, usage(0, 0), 0);
        for id in ids {
            b.settle(id, usage(1, 1), 0);
        }
        assert_eq!(b.snapshot().used, 6);
    }

    #[test]
    fn missing_usage_uses_fallback_and_marks_estimated() {
        let b = BudgetLedger::new(Some(10_000));
        let id = b.reserve(5_000).unwrap();
        b.settle(id, None, 1_234);
        let s = b.snapshot();
        assert_eq!((s.used, s.reserved, s.estimated), (1_234, 0, true));
        // settle lần 2 cùng id → không cộng thêm
        b.settle(id, None, 1_234);
        assert_eq!(b.snapshot().used, 1_234);
    }

    #[test]
    fn soft_limit_accepts_overrun_and_reports_state() {
        let b = BudgetLedger::new(Some(1_000));
        let id = b.reserve(100).unwrap();
        assert_eq!(b.snapshot().state(), "ok");
        b.settle(id, usage(700, 100), 0);
        assert_eq!(b.snapshot().state(), "warn");
        let id = b.reserve(200).unwrap();
        b.settle(id, usage(900, 0), 0);
        let s = b.snapshot();
        assert_eq!(s.used, 1_700, "giới hạn mềm: usage thật vượt ước lượng vẫn được cộng");
        assert_eq!(s.state(), "exhausted");
        assert!(b.reserve(0).is_none());
    }
}
