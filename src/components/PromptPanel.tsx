import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import {
  Activity,
  ArrowLeft,
  Building,
  CircleCheck,
  CornerDownLeft,
  Eye,
  FolderOpen,
  Globe,
  KeyRound,
  Layers,
  Network,
  Play,
  Settings,
  Square,
  WandSparkles,
} from "lucide-react";
import { formatDate, formatTokens } from "../lib/agents";
import { ipc } from "../lib/ipc";
import type { AppConfig, ModelHealth, PlanDepth, RoleId, SessionOptions, StaffingProposal } from "../lib/contracts";
import { HEALTH_META, useHealth } from "../store/health";
import { useCouncil } from "../store/council";
import { hasUsableKeys, providerOf, rolesInMeeting, useConfig, useResolver } from "../store/config";
import { toastError } from "../store/toast";
import { useUi } from "../store/ui";
import { registerStartHandler } from "../lib/useHotkeys";
import { Dialog } from "./Dialog";
import { PixelPortrait } from "./PixelCharacter";
import { RoleSelect } from "./settings/fields";
import { Button, Panel, StatusBadge, cx } from "./ui";

const MAX_PROMPT_CHARS = 50_000;

/** Tuỳ chọn phiên mặc định theo cấu hình (khi người dùng chưa chỉnh gì ở màn Phòng họp). */
function defaultOptions(config: AppConfig): SessionOptions {
  return {
    workspace_id: null,
    web_search: config.web_search.enabled_by_default,
    company_context: config.company_context.enabled_by_default,
  };
}

/** Xác nhận dừng họp khi bấm Esc (thay cho window.confirm). */
function StopConfirmDialog() {
  const open = useUi((u) => u.stopConfirmOpen);
  const setOpen = useUi((u) => u.setStopConfirmOpen);
  const running = useCouncil((c) => c.live.status === "running");
  // Phiên đã kết thúc trong lúc hộp thoại đang mở → đóng.
  useEffect(() => {
    if (open && !running) setOpen(false);
  }, [open, running, setOpen]);
  return (
    <Dialog
      open={open && running}
      onClose={() => setOpen(false)}
      tone="warning"
      title="Dừng cuộc họp đang chạy?"
      description="Các vòng đang dở sẽ bị huỷ."
      footer={
        <>
          <Button onClick={() => setOpen(false)}>Tiếp tục họp</Button>
          <Button
            variant="danger"
            onClick={() => {
              setOpen(false);
              void useCouncil.getState().stop();
            }}
          >
            Dừng họp
          </Button>
        </>
      }
    />
  );
}

export function PromptPanel() {
  // Chỉ lấy field cần dùng (không lấy cả `live`) → không re-render theo từng event / plan_delta.
  const { liveStatus, livePrompt, liveError, viewing, starting, stopping, start, stop, closeView, nextOptions } = useCouncil(
    useShallow((s) => ({
      liveStatus: s.live.status,
      livePrompt: s.live.prompt,
      liveError: s.live.error,
      viewing: s.viewing,
      starting: s.starting,
      stopping: s.stopping,
      start: s.start,
      stop: s.stop,
      closeView: s.closeView,
      nextOptions: s.nextOptions,
    })),
  );
  const config = useConfig((s) => s.config);
  const [text, setText] = useState("");
  const [proposal, setProposal] = useState<StaffingProposal | null>(null);
  const [proposing, setProposing] = useState(false);
  const running = liveStatus === "running";
  const mode = config?.meeting.staffing ?? "manual";
  /** Hàm bắt đầu mới nhất cho phím tắt toàn cục (gán lại mỗi lần render bên dưới). */
  const startRef = useRef<() => void>(() => {});

  // Phím tắt Ctrl/⌘+Enter (bắt đầu họp) và Esc (dừng họp) nằm ở lib/useHotkeys.ts;
  // ở đây chỉ đăng ký hàm bắt đầu mới nhất cho phím tắt đó.
  useEffect(() => registerStartHandler(() => startRef.current()), []);

  if (viewing) {
    return (
      <Panel className="shrink-0 p-3">
        <div className="mb-2 flex items-center gap-2 text-xs text-warn">
          <Eye className="size-4" />
          <span>Đang xem phiên cũ ({formatDate(viewing.createdAt)}), chỉ xem</span>
          <span className="ml-auto">
            <StatusBadge status={viewing.status} />
          </span>
        </div>
        <p className="max-h-24 overflow-y-auto rounded-control bg-canvas/60 p-2.5 text-sm whitespace-pre-wrap text-fg-soft">
          {viewing.prompt}
        </p>
        <div className="mt-2 flex gap-2">
          <Button size="sm" icon={<ArrowLeft className="size-3.5" />} onClick={closeView}>
            Về phiên hiện tại
          </Button>
          <Button
            size="sm"
            variant="ghost"
            disabled={running}
            onClick={() => {
              setText(viewing.prompt);
              closeView();
            }}
          >
            Dùng lại prompt này
          </Button>
        </div>
      </Panel>
    );
  }

  const value = running ? livePrompt : text;
  const tooLong = text.length > MAX_PROMPT_CHARS;
  const canStart = !running && !starting && !proposing && text.trim() !== "" && !tooLong && !!config;

  const propose = async () => {
    if (!config) return;
    setProposing(true);
    try {
      setProposal(await ipc.suggestStaffing(text.trim(), (nextOptions ?? defaultOptions(config)).company_context));
    } catch (e) {
      toastError(e, "Không đề xuất được thành phần họp");
    } finally {
      setProposing(false);
    }
  };

  /** Bắt đầu họp; `staff` = thành phần đã duyệt (ghi đè cho riêng phiên này). */
  const launch = async (staff?: { chair_id: RoleId; reviewer_ids: RoleId[]; department_ids: string[] }) => {
    if (!config) return;
    const st = useCouncil.getState();
    const prevOptions = st.nextOptions;
    const prevDepts = st.nextDepartments;
    const base = prevOptions ?? defaultOptions(config);
    if (staff) {
      st.setNextDepartments(staff.department_ids);
      st.setNextOptions({ ...base, chair_id: staff.chair_id, reviewer_ids: staff.reviewer_ids, auto_staff: false });
    } else {
      st.setNextOptions({ ...base, auto_staff: mode === "auto" });
    }
    const ok = await start(text.trim());
    // Thành phần đã duyệt chỉ dùng cho phiên này; giữ nguyên lựa chọn workspace/web/bối cảnh.
    st.setNextOptions(prevOptions);
    if (staff) st.setNextDepartments(prevDepts);
    if (ok) setProposal(null);
  };

  const onStart = () => {
    if (!canStart) return;
    if (mode === "suggest") {
      if (!proposal) void propose();
    } else {
      void launch();
    }
  };
  startRef.current = onStart;

  return (
    <Panel className="shrink-0 p-3">
      <StopConfirmDialog />
      <label htmlFor="prompt" className="mb-1.5 block text-xs font-medium text-fg-muted">
        Yêu cầu
      </label>
      <textarea
        id="prompt"
        value={value}
        readOnly={running}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            onStart();
          }
        }}
        rows={3}
        placeholder="Ví dụ: Thiết kế hệ thống leaderboard realtime cho game 1 triệu người chơi đồng thời, có quảng cáo thưởng và IAP…"
        className="w-full resize-none rounded-control border border-line bg-canvas/70 px-3 py-2 text-sm text-fg placeholder:text-fg-subtle focus:border-accent/60 focus:outline-none read-only:text-fg-muted"
      />
      <div className="mt-2 flex items-center gap-2">
        {running ? (
          <Button variant="danger" icon={<Square className="size-3.5" />} loading={stopping} onClick={() => void stop()}>
            {stopping ? "Đang dừng…" : "Stop"}
          </Button>
        ) : (
          <Button
            variant="primary"
            icon={mode === "suggest" ? <WandSparkles className="size-4" /> : <Play className="size-4" />}
            loading={starting || proposing}
            disabled={!canStart || (mode === "suggest" && !!proposal)}
            onClick={onStart}
            title={
              mode === "suggest"
                ? "AI đề xuất thành phần họp phù hợp, bạn duyệt rồi mới họp"
                : mode === "auto"
                  ? "AI tự chọn thành phần họp rồi họp luôn"
                  : "Họp với thành phần đã cấu hình"
            }
          >
            {mode === "suggest" ? (proposing ? "Đang đề xuất…" : "Đề xuất thành phần") : "Start Council"}
          </Button>
        )}
        {!running && (
          <span className="hidden items-center gap-1 text-[11px] text-fg-subtle xl:inline-flex">
            <CornerDownLeft className="size-3" /> Ctrl+Enter
          </span>
        )}
        <span className={`ml-auto text-[11px] tabular-nums ${tooLong ? "text-danger" : "text-fg-subtle"}`}>
          {value.length.toLocaleString("vi-VN")}/{MAX_PROMPT_CHARS.toLocaleString("vi-VN")}
        </span>
      </div>
      {proposal && !running && (
        <StaffingPanel
          key={JSON.stringify(proposal.staffing)}
          proposal={proposal}
          busy={starting}
          onConfirm={(s) => void launch(s)}
          onRetry={() => void propose()}
          onCancel={() => setProposal(null)}
        />
      )}
      <QuickConfig disabled={running} />
      {liveStatus === "failed" && liveError && (
        <p className="mt-2 rounded-control border border-danger-line bg-danger-subtle p-2 text-xs break-words text-danger">
          {liveError}
        </p>
      )}
    </Panel>
  );
}

/** Bảng duyệt thành phần họp do AI đề xuất: bật/tắt reviewer, phòng ban, đổi chủ trì. */
function StaffingPanel({
  proposal,
  busy,
  onConfirm,
  onRetry,
  onCancel,
}: {
  proposal: StaffingProposal;
  busy: boolean;
  onConfirm: (s: { chair_id: RoleId; reviewer_ids: RoleId[]; department_ids: string[] }) => void;
  onRetry: () => void;
  onCancel: () => void;
}) {
  const config = useConfig((s) => s.config);
  const resolve = useResolver();
  const s = proposal.staffing;
  const [chair, setChair] = useState(s.chair_id);
  const [reviewers, setReviewers] = useState<RoleId[]>(s.reviewer_ids);
  const [extra, setExtra] = useState<RoleId[]>([]);
  const [depts, setDepts] = useState<string[]>(s.department_ids);
  if (!config) return null;

  const all = [...s.reviewer_ids, ...extra].filter((id) => id !== chair);
  const toggle = (id: RoleId) => setReviewers((r) => (r.includes(id) ? r.filter((x) => x !== id) : [...r, id]));
  const usable = config.roles.filter((r) => providerOf(config, r.id) && r.model.trim());
  const chosen = reviewers.filter((id) => id !== chair);

  return (
    <div className="mt-3 rounded-lg border border-accent/30 bg-accent/5 p-3 text-xs">
      <div className="mb-2 flex items-center gap-2">
        <WandSparkles className="size-4 text-accent-fg" />
        <span className="font-medium text-fg">Thành phần họp đề xuất</span>
        <span className="text-fg-subtle">· tốn ~{formatTokens(proposal.tokens)} token</span>
      </div>
      {(s.fallback || s.note) && <p className="mb-2 text-warn">{s.note || "Dùng thành phần mặc định."}</p>}

      <div className="mb-2 flex items-center gap-2">
        <span className="w-20 shrink-0 text-fg-subtle">Chủ trì</span>
        <PixelPortrait meta={resolve(chair)} size={20} />
        <span className="w-48">
          <RoleSelect roles={usable} value={chair} onChange={setChair} />
        </span>
        {s.reasons[chair] && <span className="truncate text-fg-subtle">{s.reasons[chair]}</span>}
      </div>

      <div className="mb-2 flex items-start gap-2">
        <span className="w-20 shrink-0 pt-1 text-fg-subtle">Reviewer</span>
        <div className="min-w-0 flex-1 space-y-1">
          {all.map((id) => {
            const on = reviewers.includes(id);
            const m = resolve(id);
            return (
              <label key={id} className={cx("flex cursor-pointer items-center gap-2", !on && "opacity-50")}>
                <input type="checkbox" checked={on} onChange={() => toggle(id)} className="accent-accent" />
                <PixelPortrait meta={m} size={18} />
                <b style={{ color: m.color }}>{m.name}</b>
                <span className="truncate text-fg-subtle">{s.reasons[id] ?? (extra.includes(id) ? "bạn thêm" : "")}</span>
              </label>
            );
          })}
          <span className="block w-56">
            <RoleSelect
              roles={usable}
              value=""
              exclude={[chair, ...all]}
              placeholder="+ thêm reviewer"
              onChange={(id) => {
                setExtra((e) => [...e, id]);
                setReviewers((r) => [...r, id]);
              }}
            />
          </span>
        </div>
      </div>

      {config.departments.length > 0 && (
        <div className="mb-3 flex items-start gap-2">
          <span className="w-20 shrink-0 pt-0.5 text-fg-subtle">Phòng ban</span>
          <div className="flex flex-wrap gap-1">
            {config.departments.map((d) => {
              const on = depts.includes(d.id);
              return (
                <button
                  key={d.id}
                  type="button"
                  aria-pressed={on}
                  title={s.reasons[d.id] ?? d.mission}
                  onClick={() => setDepts((x) => (on ? x.filter((y) => y !== d.id) : config.departments.map((z) => z.id).filter((z) => z === d.id || x.includes(z))))}
                  className={cx(
                    "rounded-full border px-2 py-0.5",
                    on ? "border-accent/60 bg-accent-subtle text-accent-fg" : "border-line-strong text-fg-subtle hover:text-fg-soft",
                  )}
                >
                  {d.name}
                </button>
              );
            })}
            {depts.length === 0 && <span className="text-fg-subtle">không cần phòng ban nào họp trước</span>}
          </div>
        </div>
      )}

      <div className="flex gap-2">
        <Button
          size="sm"
          variant="primary"
          icon={<Play className="size-3.5" />}
          loading={busy}
          disabled={chosen.length === 0}
          onClick={() => onConfirm({ chair_id: chair, reviewer_ids: chosen, department_ids: depts })}
        >
          Họp với thành phần này ({chosen.length} reviewer{depts.length ? `, ${depts.length} phòng ban` : ""})
        </Button>
        <Button size="sm" variant="ghost" onClick={onRetry} disabled={busy}>
          Đề xuất lại
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel} disabled={busy}>
          Huỷ
        </Button>
      </div>
    </div>
  );
}

/** Kiểm tra nhanh model của các role sẽ tham gia phiên (còn quota / đúng key / model tồn tại). */
function MeetingHealth({ roleIds, disabled }: { roleIds: RoleId[]; disabled: boolean }) {
  const { byRole, checking, check } = useHealth();
  const config = useConfig((s) => s.config);
  const resolve = useResolver();
  const setTab = useUi((s) => s.setTab);
  if (!config) return null;
  // Chỉ tính kết quả còn khớp provider/model hiện tại của role.
  const results = roleIds
    .map((id) => {
      const r = config.roles.find((x) => x.id === id);
      const h = byRole[id];
      return r && h && h.provider_id === r.provider_id && h.model === r.model.trim() ? { id, h } : null;
    })
    .filter((x): x is { id: RoleId; h: ModelHealth } => x !== null);
  const bad = results.filter((x) => x.h.status !== "ok");
  const busy = checking !== null;

  return (
    <div className="flex flex-wrap items-center gap-2 text-xs">
      <Button size="sm" icon={<Activity className="size-3.5" />} loading={busy} disabled={disabled || roleIds.length === 0} onClick={() => void check(roleIds)}>
        Kiểm tra model ({roleIds.length} role)
      </Button>
      {!busy && results.length > 0 && bad.length === 0 && (
        <span className="inline-flex items-center gap-1 text-success">
          <CircleCheck className="size-3.5" /> {results.length}/{roleIds.length} role sẵn sàng
        </span>
      )}
      {!busy &&
        bad.map(({ id, h }) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab("settings")}
            className="inline-flex items-center gap-1 rounded-full border border-danger-line bg-danger-subtle py-0.5 pr-2 pl-0.5 text-danger hover:bg-danger-subtle"
            title={`${HEALTH_META[h.status].hint}\n${h.message}`}
          >
            <PixelPortrait meta={resolve(id)} size={16} />
            {resolve(id).name}: {HEALTH_META[h.status].label}
          </button>
        ))}
    </div>
  );
}

/** Workspace (đọc code dự án) · Web search · Bối cảnh công ty — riêng cho phiên kế tiếp. */
function SessionOptionsRow({ disabled }: { disabled: boolean }) {
  const config = useConfig((s) => s.config);
  const { nextOptions, setNextOptions } = useCouncil(
    useShallow((s) => ({ nextOptions: s.nextOptions, setNextOptions: s.setNextOptions })),
  );
  const setTab = useUi((s) => s.setTab);
  if (!config) return null;
  const opts: SessionOptions = nextOptions ?? defaultOptions(config);
  const set = (patch: Partial<SessionOptions>) => setNextOptions({ ...opts, ...patch });
  const hasContext = config.company_context.text.trim() !== "";
  const toolsOff = config.tool_policy === "off";

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-fg-muted">
      <label className="inline-flex items-center gap-1.5" title="Cho các role đọc code của dự án (chỉ đọc) để plan bám sát dự án thật">
        <FolderOpen className="size-3.5" />
        <select
          value={opts.workspace_id ?? ""}
          disabled={disabled}
          onChange={(e) => set({ workspace_id: e.target.value || null })}
          className="max-w-[180px] rounded border border-line-strong bg-surface px-1.5 py-0.5 text-fg-soft"
        >
          <option value="">Không dùng workspace</option>
          {config.workspaces.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
        </select>
        {config.workspaces.length === 0 && (
          <button type="button" onClick={() => setTab("settings")} className="text-info hover:text-info">
            + thêm
          </button>
        )}
      </label>
      <label className="inline-flex cursor-pointer items-center gap-1.5" title="Cho role tìm kiếm và đọc trang web công khai">
        <input
          type="checkbox"
          checked={opts.web_search}
          disabled={disabled}
          onChange={(e) => set({ web_search: e.target.checked })}
          className="accent-accent"
        />
        <Globe className="size-3.5" /> Tìm web
      </label>
      <label
        className={cx("inline-flex items-center gap-1.5", hasContext ? "cursor-pointer" : "opacity-50")}
        title={
          hasContext
            ? "Bật: mọi role phải tuân thủ tech stack & nội quy công ty. Tắt: được tự do đề xuất vượt giới hạn."
            : "Chưa cấu hình bối cảnh công ty (Cài đặt → Bối cảnh & Công cụ)"
        }
      >
        <input
          type="checkbox"
          checked={hasContext && opts.company_context}
          disabled={disabled || !hasContext}
          onChange={(e) => set({ company_context: e.target.checked })}
          className="accent-accent"
        />
        <Building className="size-3.5" /> Bối cảnh công ty
      </label>
      <label className="inline-flex items-center gap-1.5" title="Mức chi tiết của plan. MVP: ngắn gọn, không vẽ thêm hạ tầng. Enterprise: production-grade đầy đủ.">
        <Layers className="size-3.5" />
        <select
          value={opts.plan_depth ?? config.meeting.plan_depth}
          disabled={disabled}
          onChange={(e) => set({ plan_depth: e.target.value as PlanDepth })}
          className="rounded border border-line-strong bg-surface px-1.5 py-0.5 text-fg-soft"
        >
          <option value="mvp">MVP</option>
          <option value="standard">Vừa đủ</option>
          <option value="enterprise">Enterprise</option>
        </select>
      </label>
      {(opts.workspace_id || opts.web_search) && toolsOff && (
        <span className="text-warn">Công cụ đang tắt (Cài đặt → Bối cảnh & Công cụ), role chỉ thấy tổng quan workspace.</span>
      )}
      {nextOptions && (
        <button type="button" onClick={() => setNextOptions(null)} className="text-info hover:text-info">
          về mặc định
        </button>
      )}
    </div>
  );
}

function QuickConfig({ disabled }: { disabled: boolean }) {
  const { config, save, saving } = useConfig();
  const { nextDepartments, setNextDepartments } = useCouncil(
    useShallow((s) => ({ nextDepartments: s.nextDepartments, setNextDepartments: s.setNextDepartments })),
  );
  const setTab = useUi((s) => s.setTab);
  const resolve = useResolver();
  if (!config) return null;

  const selected = nextDepartments ?? config.meeting.department_ids;
  const toggle = (id: string) => {
    const next = selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id];
    // Giữ thứ tự như trong cấu hình.
    setNextDepartments(config.departments.map((d) => d.id).filter((x) => next.includes(x)));
  };
  const update = async (patch: Partial<AppConfig>) => {
    try {
      await save({ ...config, ...patch });
    } catch (e) {
      toastError(e);
    }
  };
  const chair = resolve(config.meeting.chair_id);

  return (
    <div className="mt-3 space-y-2.5 border-t border-line pt-2.5">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-fg-muted">
        <span className="inline-flex min-w-0 items-center gap-1.5" title={`Chủ trì: ${chair.name} · ${chair.model}`}>
          <PixelPortrait meta={chair} size={18} />
          Chủ trì <b className="truncate text-fg-soft">{chair.name}</b>
          <span className="flex -space-x-1">
            {config.meeting.reviewer_ids.slice(0, 6).map((id) => (
              <span key={id} title={resolve(id).name}>
                <PixelPortrait meta={resolve(id)} size={16} />
              </span>
            ))}
          </span>
          <span className="text-fg-subtle">{config.meeting.reviewer_ids.length} reviewer</span>
        </span>
        <label className="inline-flex items-center gap-1.5">
          Số vòng
          <select
            value={config.max_rounds}
            disabled={saving || disabled}
            onChange={(e) => void update({ max_rounds: Number(e.target.value) })}
            className="rounded border border-line-strong bg-surface px-1.5 py-0.5 text-fg-soft"
          >
            {Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>
        <label className="inline-flex cursor-pointer items-center gap-1.5">
          <input
            type="checkbox"
            checked={config.stream_plans}
            disabled={saving || disabled}
            onChange={(e) => void update({ stream_plans: e.target.checked })}
            className="accent-accent"
          />
          Stream plan
        </label>
        <button
          type="button"
          onClick={() => setTab("settings")}
          className="ml-auto inline-flex items-center gap-1 text-fg-muted hover:text-fg"
        >
          <Settings className="size-3.5" /> Cài đặt
        </button>
      </div>

      <SessionOptionsRow disabled={disabled} />

      {config.departments.length > 0 && (
        <div>
          <div className="mb-1 flex items-center gap-1.5 text-[11px] text-fg-subtle">
            <Network className="size-3.5" />
            Phòng ban họp trước ({selected.length}/{config.departments.length})
            {nextDepartments && (
              <button type="button" onClick={() => setNextDepartments(null)} className="ml-1 text-info hover:text-info">
                về mặc định
              </button>
            )}
          </div>
          <div className="flex flex-wrap gap-1">
            {config.departments.map((d) => {
              const on = selected.includes(d.id);
              return (
                <button
                  key={d.id}
                  type="button"
                  disabled={disabled}
                  onClick={() => toggle(d.id)}
                  aria-pressed={on}
                  title={`${d.mission}\nTrưởng phòng: ${resolve(d.lead_id).name} · ${d.member_ids.length} thành viên`}
                  className={cx(
                    "rounded-full border px-2 py-0.5 text-[11px] transition-colors disabled:opacity-50",
                    on
                      ? "border-accent/60 bg-accent-subtle text-accent-fg"
                      : "border-line-strong text-fg-muted hover:border-line-heavy hover:text-fg-soft",
                  )}
                >
                  {d.name}
                </button>
              );
            })}
          </div>
        </div>
      )}

      <MeetingHealth roleIds={rolesInMeeting(config, selected)} disabled={disabled} />

      {!hasUsableKeys(config, selected) && (
        <button
          type="button"
          onClick={() => setTab("settings")}
          className="flex w-full items-center gap-2 rounded-control border border-warn-line bg-warn-subtle px-2.5 py-1.5 text-left text-xs text-warn hover:bg-warn-subtle"
        >
          <KeyRound className="size-3.5 shrink-0" />
          Có role tham gia chưa có provider, model hoặc API key. Bấm để mở Cài đặt.
        </button>
      )}
    </div>
  );
}
