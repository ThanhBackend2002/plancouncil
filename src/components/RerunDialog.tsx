import { useEffect, useId, useMemo, useRef, useState } from "react";
import { LoaderCircle, Play, RotateCcw } from "lucide-react";
import type { RerunOverrides, RerunResult } from "../lib/contracts";
import { ipc, toCommandError } from "../lib/ipc";
import { activeOf, useCouncil } from "../store/council";
import { toast, toastError } from "../store/toast";
import { useUi } from "../store/ui";
import { Dialog } from "./Dialog";
import { Button } from "./ui";

const FIELD =
  "h-8 w-28 rounded-md border border-line-strong bg-canvas px-2 text-sm text-fg placeholder:text-fg-faint focus:border-accent/60 focus:outline-none";

/** Chuỗi → số nguyên dương; "" → null (không ghi đè); không hợp lệ → undefined. */
function parseInt0(v: string, min: number, max: number): number | null | undefined {
  const t = v.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isInteger(n) && n >= min && n <= max ? n : undefined;
}

/**
 * Hộp thoại "Chạy tiếp": gọi rerun_from_version(dry_run) để biết phần nào không phủ được, rồi
 * (khi xác nhận) tạo phiên MỚI từ version đã chọn.
 */
export function RerunDialog({
  open,
  onClose,
  sessionId,
  scope,
  version,
  reviewerIds,
  defaultMaxRounds,
  focusBudget,
  resolveName,
}: {
  open: boolean;
  onClose: () => void;
  sessionId: string;
  scope: string;
  /** Version dự kiến chạy tiếp (version hợp lệ mới nhất). */
  version: number;
  /** Reviewer hiện có của scope (để dựng `overrides.reviewers` khi bỏ chọn role vắng mặt). */
  reviewerIds: string[];
  defaultMaxRounds: number;
  /** Đặt focus vào ô ngân sách token (từ banner "Hết ngân sách"). */
  focusBudget?: boolean;
  resolveName: (roleId: string) => string;
}) {
  const [dry, setDry] = useState<RerunResult | null>(null);
  const [dryError, setDryError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirmUncovered, setConfirmUncovered] = useState(false);
  const [dropped, setDropped] = useState<Set<string>>(new Set());
  const [rounds, setRounds] = useState("");
  const [tokens, setTokens] = useState("");
  const budgetRef = useRef<HTMLInputElement>(null);
  const noteId = useId();

  // Mở hộp thoại → dry-run. Đóng → reset.
  useEffect(() => {
    if (!open) return;
    let alive = true;
    setDry(null);
    setDryError(null);
    setConfirmUncovered(false);
    setDropped(new Set());
    setRounds("");
    setTokens("");
    setLoading(true);
    ipc
      .rerunFromVersion({ session_id: sessionId, scope, version_no: version, dry_run: true })
      .then((r) => alive && setDry(r))
      .catch((e) => alive && setDryError(toCommandError(e).message))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [open, sessionId, scope, version]);

  // Ô ngân sách chỉ có sau khi dry-run xong → focus lúc đó.
  useEffect(() => {
    if (open && focusBudget && dry) budgetRef.current?.focus();
  }, [open, focusBudget, dry]);

  const parsedRounds = parseInt0(rounds, 1, 10);
  const parsedTokens = parseInt0(tokens, 1000, 1_000_000_000);
  const uncovered = dry?.uncovered_sections ?? [];
  const noVersion = !!dry && dry.available_versions.length === 0;
  const valid = parsedRounds !== undefined && parsedTokens !== undefined && (uncovered.length === 0 || confirmUncovered);

  const overrides = useMemo<RerunOverrides | undefined>(() => {
    const o: RerunOverrides = {};
    if (dropped.size > 0) o.reviewers = reviewerIds.filter((id) => !dropped.has(id));
    if (typeof parsedRounds === "number") o.max_rounds = parsedRounds;
    if (typeof parsedTokens === "number") o.max_total_tokens = parsedTokens;
    return Object.keys(o).length > 0 ? o : undefined;
  }, [dropped, reviewerIds, parsedRounds, parsedTokens]);

  const adopt = (id: string) => {
    useCouncil.getState().adoptSession(id, activeOf(useCouncil.getState()).prompt);
    useUi.getState().setTab("council");
    toast("success", "Đã tạo phiên mới", "Phiên cũ được giữ nguyên trong lịch sử.");
    onClose();
  };

  const confirm = async () => {
    setBusy(true);
    try {
      const r = await ipc.rerunFromVersion({ session_id: sessionId, scope, version_no: version, overrides });
      if (r.new_session_id) adopt(r.new_session_id);
      else toast("warning", "Backend không tạo phiên mới");
    } catch (e) {
      toastError(e, "Không chạy tiếp được");
    } finally {
      setBusy(false);
    }
  };

  const restart = async () => {
    setBusy(true);
    try {
      const r = await ipc.restartFromRequest(sessionId);
      adopt(r.new_session_id);
    } catch (e) {
      toastError(e, "Không bắt đầu lại được");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      tone="warning"
      title={noVersion ? "Bắt đầu lại từ yêu cầu" : `Chạy tiếp từ version ${version}`}
      footer={
        <>
          <Button onClick={onClose}>Huỷ</Button>
          {noVersion ? (
            <Button variant="primary" loading={busy} icon={<RotateCcw className="size-3.5" />} onClick={() => void restart()}>
              Bắt đầu lại từ yêu cầu
            </Button>
          ) : (
            <Button variant="primary" loading={busy} disabled={!dry || !valid} icon={<Play className="size-3.5" />} onClick={() => void confirm()}>
              Tạo phiên mới và chạy
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-3 text-xs text-fg-soft">
        <p className="rounded-control border border-warn-line bg-warn-subtle px-3 py-2 text-warn">
          Thao tác này bắt đầu một phiên <b>MỚI</b> từ version đã chọn, <b>không</b> tiếp tục phiên cũ tại chỗ. Phiên cũ giữ nguyên trong lịch sử.
        </p>

        {loading && (
          <p role="status" className="flex items-center gap-2 text-fg-muted">
            <LoaderCircle className="size-3.5 animate-spin" aria-hidden /> Đang kiểm tra khả năng chạy tiếp…
          </p>
        )}
        {dryError && (
          <p role="alert" className="rounded-control border border-danger-line bg-danger-subtle px-3 py-2 text-danger">
            Không kiểm tra được: {dryError}
          </p>
        )}

        {noVersion && (
          <p role="status" className="text-fg-muted">
            Chưa có version plan hợp lệ nào để chạy tiếp. Bạn có thể bắt đầu lại từ yêu cầu ban đầu (phiên mới).
          </p>
        )}

        {dry && !noVersion && (
          <>
            {uncovered.length > 0 && (
              <div role="alert" className="rounded-control border border-warn-line bg-warn-subtle px-3 py-2 text-warn">
                <p className="font-medium">Section không còn reviewer phụ trách ({uncovered.length})</p>
                <p className="mt-0.5 text-fg-muted">
                  Tag reviewer của các section này trỏ tới role không còn trong cuộc họp, nên sẽ được coi như <code>reviewers: all</code> (mọi reviewer duyệt).
                </p>
                <ul className="mt-1 list-disc pl-4">
                  {uncovered.map((s) => (
                    <li key={s}>{s}</li>
                  ))}
                </ul>
                <label className="mt-2 flex cursor-pointer items-center gap-2 text-fg-soft">
                  <input type="checkbox" className="accent-accent" checked={confirmUncovered} onChange={(e) => setConfirmUncovered(e.target.checked)} />
                  Tôi hiểu và đồng ý chạy tiếp
                </label>
              </div>
            )}

            {dry.missing_roles.length > 0 && (
              <fieldset className="rounded-control border border-line px-3 py-2">
                <legend className="px-1 font-medium text-fg-soft">Role không còn tồn tại</legend>
                <p className="mb-1 text-fg-muted">Bỏ chọn để loại role khỏi danh sách reviewer của phiên mới.</p>
                {dry.missing_roles.map((r) => (
                  <label key={r} className="flex cursor-pointer items-center gap-2 py-0.5">
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={!dropped.has(r)}
                      onChange={(e) =>
                        setDropped((d) => {
                          const n = new Set(d);
                          if (e.target.checked) n.delete(r);
                          else n.add(r);
                          return n;
                        })
                      }
                    />
                    {resolveName(r)}
                  </label>
                ))}
              </fieldset>
            )}

            <div className="flex flex-wrap items-end gap-4">
              <label className="block">
                <span className="mb-1 block font-medium text-fg-muted">Số vòng tối đa</span>
                <input
                  className={FIELD}
                  inputMode="numeric"
                  value={rounds}
                  placeholder={String(defaultMaxRounds || "")}
                  onChange={(e) => setRounds(e.target.value)}
                  aria-invalid={parsedRounds === undefined}
                />
                {parsedRounds === undefined && <span className="mt-1 block text-[11px] text-danger">Số nguyên 1–10</span>}
              </label>
              <label className="block">
                <span className="mb-1 block font-medium text-fg-muted">Ngân sách token (mềm)</span>
                <input
                  ref={budgetRef}
                  className={FIELD}
                  inputMode="numeric"
                  value={tokens}
                  placeholder="giữ nguyên"
                  onChange={(e) => setTokens(e.target.value)}
                  aria-invalid={parsedTokens === undefined}
                  aria-describedby={noteId}
                />
                {parsedTokens === undefined && <span className="mt-1 block text-[11px] text-danger">Số nguyên ≥ 1000</span>}
              </label>
            </div>
            <p id={noteId} className="text-fg-subtle">
              Để trống = dùng cấu hình hiện tại. Ngân sách là giới hạn mềm, tính theo token ước tính.
            </p>
          </>
        )}
      </div>
    </Dialog>
  );
}
