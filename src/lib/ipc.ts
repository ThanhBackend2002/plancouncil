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
  RerunArgs,
  RerunResult,
  RoleDef,
  SearchHit,
  SessionOptions,
  SessionRecord,
  StaffingProposal,
  SessionSummary,
  UsageSummary,
} from "./contracts";
import type { KeyStorage, SecretCommandError, SecretStatusReport, SecretTarget, SetSecretResult } from "./contracts";
import type { RerunCommandError, RerunOverrides, RestartResult, UsageBreakdown } from "./contracts";

const KINDS: ReadonlySet<CommandError["kind"]> = new Set([
  "invalid",
  "already_running",
  "not_found",
  "storage",
  "llm",
  "internal",
  "index_unavailable",
  "index",
  "no_valid_version",
  "missing_roles",
  "unsupported_scope",
  "secret_missing",
]);

export function toCommandError(e: unknown): CommandError {
  if (e && typeof e === "object" && "kind" in e && "message" in e) {
    const kind = String((e as { kind: unknown }).kind);
    const message = String((e as { message: unknown }).message);
    const target = (e as { target?: CommandError["target"] }).target;
    return {
      kind: KINDS.has(kind as CommandError["kind"]) ? (kind as CommandError["kind"]) : "internal",
      message,
      ...(target ? { target } : {}),
    };
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
  /**
   * Chạy tiếp từ 1 version: tạo phiên MỚI (không resume tại chỗ). `dry_run` chỉ kiểm tra.
   * Backend dùng `rename_all = "snake_case"` → đi qua `rerunIpc` (giữ nguyên kind + payload lỗi).
   */
  rerunFromVersion: (args: RerunArgs): Promise<RerunResult> =>
    rerunIpc.rerunFromVersion(args.session_id, args.scope, args.version_no, args.dry_run, args.overrides),
  restartFromRequest: (sessionId: string): Promise<RestartResult> => rerunIpc.restartFromRequest(sessionId),
  getUsage: (sessionId: string): Promise<UsageSummary> => rerunIpc.getUsage(sessionId),
  exportMarkdown: (content: string, suggestedName: string) =>
    call<string | null>("export_markdown", { content, suggestedName }),
};

// ── API key (keyring) — giữ nguyên `kind` của backend để UI phân biệt keyring_unavailable / keyring_locked ──

async function callSecret<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri()) {
    throw { kind: "internal", message: "Không chạy trong Tauri — hãy mở app bằng `npm run tauri dev`." } satisfies SecretCommandError;
  }
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (e && typeof e === "object" && "kind" in e && "message" in e) {
      const o = e as { kind: unknown; message: unknown; target?: SecretTarget };
      throw { kind: String(o.kind), message: String(o.message), target: o.target } satisfies SecretCommandError;
    }
    throw { kind: "internal", message: e instanceof Error ? e.message : String(e) } satisfies SecretCommandError;
  }
}

export const secretIpc = {
  /** `apiKey` là giá trị mask = dùng key hiện tại (đổi nơi lưu / thử lại / mở khoá). "file" chỉ gửi khi người dùng đã đồng ý. */
  setSecret: (target: SecretTarget, apiKey: string, storage: KeyStorage) =>
    callSecret<SetSecretResult>("set_secret", { target, api_key: apiKey, storage }),
  deleteSecret: (target: SecretTarget) => callSecret<void>("delete_secret", { target }),
  status: () => callSecret<SecretStatusReport>("secret_status"),
  deleteLeftoverFiles: (files: string[]) => callSecret<string[]>("delete_leftover_files", { files }),
};

// ── Chạy lại phiên + chi phí — giữ nguyên `kind` và payload lỗi (`available_versions`, `missing_roles`) ──

async function callRerun<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri()) {
    throw { kind: "internal", message: "Không chạy trong Tauri — hãy mở app bằng `npm run tauri dev`." } satisfies RerunCommandError;
  }
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (e && typeof e === "object" && "kind" in e && "message" in e) {
      const o = e as RerunCommandError;
      throw {
        kind: String(o.kind),
        message: String(o.message),
        available_versions: o.available_versions,
        missing_roles: o.missing_roles,
        target: o.target,
      } satisfies RerunCommandError;
    }
    throw { kind: "internal", message: e instanceof Error ? e.message : String(e) } satisfies RerunCommandError;
  }
}

export const rerunIpc = {
  /** `dryRun` = chỉ kiểm tra (không bị chặn khi đang có phiên chạy). `scope` = "main". */
  rerunFromVersion: (sessionId: string, scope: string, versionNo: number, dryRun?: boolean, overrides?: RerunOverrides) =>
    callRerun<RerunResult>("rerun_from_version", {
      session_id: sessionId,
      scope,
      version_no: versionNo,
      dry_run: dryRun,
      overrides,
    }),
  restartFromRequest: (sessionId: string) => callRerun<RestartResult>("restart_from_request", { session_id: sessionId }),
  getUsage: (sessionId: string) => callRerun<UsageBreakdown>("get_usage", { session_id: sessionId }),
};
