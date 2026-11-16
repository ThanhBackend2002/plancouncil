// Wrapper có kiểu cho 10 Tauri command. Mọi lỗi được chuẩn hoá thành `CommandError`.

import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  AppConfig,
  RoleId,
  CommandError,
  ConnectionTestResult,
  HistorySearchHit,
  ModelHealth,
  ModelList,
  RoleDef,
  SearchHit,
  SessionOptions,
  SessionRecord,
  StaffingProposal,
  SessionSummary,
} from "./contracts";

const KINDS: ReadonlySet<CommandError["kind"]> = new Set([
  "invalid",
  "already_running",
  "not_found",
  "storage",
  "llm",
  "internal",
  "index_unavailable",
  "index",
]);

export function toCommandError(e: unknown): CommandError {
  if (e && typeof e === "object" && "kind" in e && "message" in e) {
    const kind = String((e as { kind: unknown }).kind);
    const message = String((e as { message: unknown }).message);
    return { kind: KINDS.has(kind as CommandError["kind"]) ? (kind as CommandError["kind"]) : "internal", message };
  }
  if (e instanceof Error) return { kind: "internal", message: e.message };
  return { kind: "internal", message: String(e) };
}

export function isCommandError(e: unknown): e is CommandError {
  return !!e && typeof e === "object" && "kind" in e && "message" in e;
}

export const inTauri = (): boolean => {
  try {
    return isTauri();
  } catch {
    return false;
  }
};

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri()) {
    throw { kind: "internal", message: "Không chạy trong Tauri — hãy mở app bằng `npm run tauri dev`." } satisfies CommandError;
  }
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toCommandError(e);
  }
}

export const ipc = {
  getConfig: () => call<AppConfig>("get_config"),
  saveConfig: (config: AppConfig) => call<AppConfig>("save_config", { config }),
  /** `departments` undefined = dùng phòng ban mặc định trong cấu hình cuộc họp. */
  startCouncil: (prompt: string, departments?: string[], options?: SessionOptions) =>
    call<string>("start_council", { prompt, departments, options }),
  pickFolder: () => call<string | null>("pick_folder"),
  suggestStaffing: (prompt: string, companyContext?: boolean) =>
    call<StaffingProposal>("suggest_staffing", { prompt, companyContext }),
  /** Áp dụng từ vòng review kế tiếp. `scope` = "main" hoặc id phòng ban. */
  inviteMember: (sessionId: string, scope: string, roleId: RoleId) => call<boolean>("invite_member", { sessionId, scope, roleId }),
  dismissMember: (sessionId: string, scope: string, roleId: RoleId) => call<boolean>("dismiss_member", { sessionId, scope, roleId }),
  testWebSearch: (query: string) => call<SearchHit[]>("test_web_search", { query }),
  /** `roleIds` undefined = kiểm tra mọi role. */
  checkModels: (roleIds?: RoleId[]) => call<ModelHealth[]>("check_models", { roleIds }),
  stopCouncil: (sessionId: string) => call<boolean>("stop_council", { sessionId }),
  activeSession: () => call<string | null>("active_session"),
  listSessions: (limit?: number) => call<SessionSummary[]>("list_sessions", { limit }),
  loadSession: (sessionId: string) => call<SessionRecord>("load_session", { sessionId }),
  deleteSession: (sessionId: string) => call<boolean>("delete_session", { sessionId }),
  /** Tìm trong lịch sử, không phân biệt dấu. Câu rỗng → []. */
  searchHistory: (query: string, limit?: number, offset?: number) =>
    call<HistorySearchHit[]>("search_history", { query, limit, offset }),
  testConnection: (agent: RoleId) => call<ConnectionTestResult>("test_connection", { agent }),
  /** `baseUrl`/`apiKey` cho phép thử provider đang sửa dở; bỏ trống = dùng bản đã lưu. */
  listModels: (providerId: string, baseUrl?: string, apiKey?: string | null) =>
    call<ModelList>("list_models", { providerId, baseUrl, apiKey: apiKey ?? undefined }),
  defaultRoles: () => call<RoleDef[]>("default_roles"),
  exportMarkdown: (content: string, suggestedName: string) =>
    call<string | null>("export_markdown", { content, suggestedName }),
};
