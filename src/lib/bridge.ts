// Cầu nối event Rust → store. Gọi đúng 1 lần trong main.tsx (ngoài React để tránh
// StrictMode đăng ký listener 2 lần).

import { listen } from "@tauri-apps/api/event";
import { COUNCIL_EVENT, SESSION_SAVED_EVENT, type EventEnvelope } from "./contracts";
import { inTauri, ipc } from "./ipc";
import { useCouncil } from "../store/council";
import { useConfig } from "../store/config";
import { useHistory } from "../store/history";
import { toast } from "../store/toast";

let started = false;

export async function initBridge(): Promise<void> {
  if (started) return;
  started = true;

  if (!inTauri()) {
    toast("warning", "Đang chạy ngoài Tauri", "Dùng `npm run tauri dev` để kết nối backend Rust.");
    return;
  }

  // Đăng ký listener TRƯỚC khi khôi phục để không lỡ event nào.
  await listen<EventEnvelope>(COUNCIL_EVENT, (e) => {
    const env = e.payload;
    console.debug(`[council] #${env.seq} ${env.type}`, env);
    useCouncil.getState().ingest(env);
  });

  await listen<{ session_id: string; ok: boolean }>(SESSION_SAVED_EVENT, async (e) => {
    const { session_id, ok } = e.payload;
    if (!ok) {
      toast("error", "Không lưu được lịch sử phiên", "Xem log của app để biết chi tiết.");
      return;
    }
    void useHistory.getState().refresh();

    // Phiên live bị thiếu dữ liệu (reload giữa chừng / mất event) → nạp bản đầy đủ từ đĩa.
    const live = useCouncil.getState().live;
    if (live.id === session_id && (live.resumed || live.seqGap)) {
      try {
        useCouncil.getState().hydrateLive(await ipc.loadSession(session_id));
      } catch (err) {
        console.warn("[council] hydrate thất bại", err);
      }
    }
  });

  await useCouncil.getState().restore();
  void useConfig.getState().load();
  void useHistory.getState().refresh();
}
