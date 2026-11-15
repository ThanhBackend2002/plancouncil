import { create } from "zustand";
import type { HealthStatus, ModelHealth, RoleId } from "../lib/contracts";
import type { Tone } from "../lib/agents";
import { ipc } from "../lib/ipc";
import { toastError } from "./toast";

export const HEALTH_META: Record<HealthStatus, { label: string; tone: Tone; hint: string }> = {
  ok: { label: "Hoạt động", tone: "success", hint: "Key và model dùng được." },
  quota: { label: "Hết quota", tone: "danger", hint: "Hết credit / số dư / hạn mức. Nạp thêm hoặc đổi sang provider/model khác." },
  auth: { label: "Sai key", tone: "danger", hint: "API key sai, hết hạn hoặc không có quyền với model này." },
  not_found: { label: "Không có model", tone: "danger", hint: "Provider không có model này. Bấm Làm mới danh sách model ở tab Provider rồi chọn lại." },
  rate_limit: { label: "Bị giới hạn", tone: "warning", hint: "Đang bị rate limit. Thường dùng lại được sau ít phút; giảm số request song song ở tab Chung." },
  bad_request: { label: "Bị từ chối", tone: "warning", hint: "Provider từ chối request (tham số/model không tương thích)." },
  server: { label: "Lỗi server", tone: "warning", hint: "Provider đang lỗi/quá tải. Thử lại sau." },
  timeout: { label: "Quá thời gian", tone: "warning", hint: "Không phản hồi trong 30 giây." },
  network: { label: "Lỗi mạng", tone: "danger", hint: "Không kết nối được tới base URL của provider." },
  unassigned: { label: "Chưa gán", tone: "neutral", hint: "Role chưa chọn provider/model." },
  other: { label: "Lỗi", tone: "danger", hint: "Lỗi không xác định, xem chi tiết." },
};

interface HealthStore {
  /** Kết quả theo role id (cặp provider+model dùng chung → cùng 1 kết quả). */
  byRole: Record<RoleId, ModelHealth>;
  checking: Set<RoleId> | "all" | null;
  check: (roleIds?: RoleId[]) => Promise<ModelHealth[]>;
  clear: () => void;
}

export const useHealth = create<HealthStore>((set, get) => ({
  byRole: {},
  checking: null,
  check: async (roleIds) => {
    set({ checking: roleIds ? new Set(roleIds) : "all" });
    try {
      const res = await ipc.checkModels(roleIds);
      const next = roleIds ? { ...get().byRole } : {};
      for (const h of res) for (const id of h.role_ids) next[id] = h;
      set({ byRole: next });
      return res;
    } catch (e) {
      toastError(e, "Không kiểm tra được model");
      return [];
    } finally {
      set({ checking: null });
    }
  },
  clear: () => set({ byRole: {} }),
}));

export const isChecking = (checking: HealthStore["checking"], id: RoleId) =>
  checking === "all" || (checking instanceof Set && checking.has(id));
