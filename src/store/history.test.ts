import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HistorySearchHit } from "../lib/contracts";

const searchHistory = vi.fn<(q: string, limit?: number) => Promise<HistorySearchHit[]>>();

vi.mock("../lib/ipc", () => ({
  ipc: {
    searchHistory: (q: string, limit?: number) => searchHistory(q, limit),
    listSessions: vi.fn(async () => []),
    deleteSession: vi.fn(async () => true),
  },
  toCommandError: (e: unknown) => e,
}));
vi.mock("./toast", () => ({ toastError: vi.fn() }));

const { useHistory, SEARCH_DEBOUNCE_MS } = await import("./history");

const hit = (session_id: string): HistorySearchHit => ({
  session_id,
  session_title: `Phiên ${session_id}`,
  scope: "main",
  version_no: 1,
  kind: "section",
  section_key: "a",
  heading: "A",
  snippet: "[[x]]",
  rank: -1,
  created_at: "2026-01-01T00:00:00Z",
});

/** Promise điều khiển được từ test → mô phỏng response về không theo thứ tự. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const st = () => useHistory.getState();

beforeEach(() => {
  vi.useFakeTimers();
  searchHistory.mockReset();
  st().clearSearch();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("history search", () => {
  it("debounce: gõ liên tục chỉ gọi 1 lần với câu cuối (đã trim)", async () => {
    searchHistory.mockResolvedValue([hit("s1")]);
    st().setQuery("r");
    st().setQuery("re");
    st().setQuery("redis ");
    expect(st().searchLoading).toBe(true);
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS - 1);
    expect(searchHistory).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(searchHistory).toHaveBeenCalledTimes(1);
    expect(searchHistory.mock.calls[0]![0]).toBe("redis");
    expect(st()).toMatchObject({ hitsFor: "redis", searchLoading: false, searchError: null });
    expect(st().hits.map((h) => h.session_id)).toEqual(["s1"]);
  });

  it("bỏ qua response cũ về sau response mới", async () => {
    const first = deferred<HistorySearchHit[]>();
    const second = deferred<HistorySearchHit[]>();
    searchHistory.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    st().setQuery("cu");
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    st().setQuery("moi");
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    expect(searchHistory).toHaveBeenCalledTimes(2);

    second.resolve([hit("new")]);
    await vi.advanceTimersByTimeAsync(0);
    first.resolve([hit("old")]);
    await vi.advanceTimersByTimeAsync(0);
    expect(st().hitsFor).toBe("moi");
    expect(st().hits.map((h) => h.session_id)).toEqual(["new"]);
  });

  it("lỗi của request cũ cũng bị bỏ qua; lỗi mới nhất được giữ", async () => {
    const first = deferred<HistorySearchHit[]>();
    searchHistory
      .mockReturnValueOnce(first.promise)
      .mockRejectedValueOnce({ kind: "index_unavailable", message: "không có index" });
    st().setQuery("a");
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    st().setQuery("b");
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    first.reject({ kind: "internal", message: "cũ" });
    await vi.advanceTimersByTimeAsync(0);
    expect(st().searchError).toEqual({ kind: "index_unavailable", message: "không có index" });
    expect(st()).toMatchObject({ hits: [], searchLoading: false, hitsFor: "b" });
  });

  it("xoá câu tìm huỷ timer và vô hiệu response đang bay", async () => {
    const pending = deferred<HistorySearchHit[]>();
    searchHistory.mockReturnValueOnce(pending.promise);
    st().setQuery("x");
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    st().setQuery("   ");
    pending.resolve([hit("late")]);
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS * 2);
    expect(searchHistory).toHaveBeenCalledTimes(1);
    expect(st()).toMatchObject({ hits: [], hitsFor: null, searchLoading: false });

    st().setQuery("y");
    st().clearSearch();
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS * 2);
    expect(searchHistory).toHaveBeenCalledTimes(1);
    expect(st().query).toBe("");
  });
});
