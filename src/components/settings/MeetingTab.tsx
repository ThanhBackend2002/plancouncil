import type { AppConfig, PlanDepth, RoleId, StaffingMode } from "../../lib/contracts";
import { Button, Panel, cx } from "../ui";
import { Field, INPUT, NumberInput, RoleChips, RoleSelect, SectionTitle, Toggle, type Errors } from "./fields";

const PRESETS: { name: string; chair: RoleId; reviewers: RoleId[]; desc: string }[] = [
  {
    name: "Hội đồng kỹ thuật",
    chair: "architect",
    reviewers: ["scale_reviewer", "chaos_reviewer", "pragmatic_reviewer"],
    desc: "Architect + Performance, Security, Tech Lead",
  },
  {
    name: "Ban giám đốc",
    chair: "ceo",
    reviewers: ["cto", "cfo", "coo", "cpo"],
    desc: "CEO chủ trì, C-Level phản biện",
  },
  {
    name: "Kỹ thuật + Chất lượng",
    chair: "cto",
    reviewers: ["pragmatic_reviewer", "qa_lead", "devops_sre", "ciso"],
    desc: "CTO + Tech Lead, QA Lead, SRE, CISO",
  },
  {
    name: "Game (UA & Monet)",
    chair: "cpo",
    reviewers: ["ua_manager", "monetization_manager", "liveops_manager", "data_analyst", "cfo"],
    desc: "CPO + UA, Monetization, LiveOps, Data, CFO",
  },
];

export function MeetingTab({
  draft,
  errors,
  update,
}: {
  draft: AppConfig;
  errors: Errors;
  update: (f: (c: AppConfig) => AppConfig) => void;
}) {
  const m = draft.meeting;
  const setMeeting = (patch: Partial<AppConfig["meeting"]>) => update((c) => ({ ...c, meeting: { ...c.meeting, ...patch } }));
  const has = (id: RoleId) => draft.roles.some((r) => r.id === id);

  return (
    <div className="space-y-4">
      <Panel className="p-4">
        <SectionTitle title="Mẫu cuộc họp" desc="Áp nhanh chủ trì + reviewer. Role không còn tồn tại sẽ bị bỏ qua." />
        <div className="grid grid-cols-2 gap-2">
          {PRESETS.map((p) => {
            const reviewers = p.reviewers.filter(has);
            const ok = has(p.chair) && reviewers.length > 0;
            const active = m.chair_id === p.chair && reviewers.join() === m.reviewer_ids.join();
            return (
              <button
                key={p.name}
                type="button"
                disabled={!ok}
                onClick={() => setMeeting({ chair_id: p.chair, reviewer_ids: reviewers })}
                className={cx(
                  "rounded-lg border px-3 py-2 text-left transition-colors disabled:opacity-40",
                  active ? "border-violet-500/60 bg-violet-500/10" : "border-neutral-800 hover:border-neutral-600",
                )}
              >
                <div className="text-sm font-medium text-neutral-100">{p.name}</div>
                <div className="text-[11px] text-neutral-500">{p.desc}</div>
              </button>
            );
          })}
        </div>
      </Panel>

      <Panel className="p-4">
        <SectionTitle
          title="Cuộc họp chính"
          desc="Chủ trì giao việc cho phòng ban, tổng hợp báo cáo thành plan tổng và chỉnh sửa theo góp ý của reviewer."
        />
        <div className="grid grid-cols-[1.4fr_1fr_1fr] gap-4">
          <Field label="Chủ trì (viết plan tổng)" error={errors["meeting.chair"]}>
            <RoleSelect
              roles={draft.roles}
              value={m.chair_id}
              exclude={m.reviewer_ids}
              onChange={(id) => setMeeting({ chair_id: id })}
            />
          </Field>
          <Field label="Số vòng tối đa" error={errors["general.max_rounds"]} hint="1–10">
            <NumberInput value={draft.max_rounds} onChange={(v) => update((c) => ({ ...c, max_rounds: v ?? Number.NaN }))} />
          </Field>
          <Field label="Vòng 1 nghiêm" hint="Bắt buộc REJECT + ≥ 1 vấn đề (dễ làm plan phình to, mặc định tắt)">
            <Toggle checked={m.strict_first_round} onChange={(v) => setMeeting({ strict_first_round: v })} label="Bật" />
          </Field>
          <Field label={`Reviewer (${m.reviewer_ids.length}/12)`} className="col-span-3" error={errors["meeting.reviewers"]}>
            <RoleChips roles={draft.roles} value={m.reviewer_ids} exclude={[m.chair_id]} onChange={(ids) => setMeeting({ reviewer_ids: ids })} />
          </Field>
          <Field label="Phòng ban tham gia mặc định" className="col-span-3" hint="Có thể bật/tắt riêng cho từng phiên ở màn Phòng họp.">
            <div className="flex flex-wrap gap-1.5">
              {draft.departments.map((d) => {
                const on = m.department_ids.includes(d.id);
                return (
                  <button
                    key={d.id}
                    type="button"
                    aria-pressed={on}
                    onClick={() =>
                      setMeeting({
                        department_ids: on
                          ? m.department_ids.filter((x) => x !== d.id)
                          : draft.departments.map((x) => x.id).filter((x) => x === d.id || m.department_ids.includes(x)),
                      })
                    }
                    className={cx(
                      "rounded-full border px-2.5 py-1 text-xs",
                      on ? "border-violet-500/60 bg-violet-500/15 text-violet-200" : "border-neutral-700 text-neutral-400 hover:text-neutral-200",
                    )}
                  >
                    {d.name}
                  </button>
                );
              })}
              {draft.departments.length === 0 && <span className="text-xs text-neutral-500">Chưa có phòng ban.</span>}
            </div>
          </Field>
        </div>
        {m.reviewer_ids.length > 6 && (
          <p className="mt-3 text-[11px] text-amber-400">
            Nhiều reviewer = nhiều request mỗi vòng. Cân nhắc giới hạn song song ở tab Chung để tránh lỗi 429.
          </p>
        )}
        <div className="mt-3">
          <Button size="sm" variant="ghost" onClick={() => setMeeting({ reviewer_ids: [] })} disabled={m.reviewer_ids.length === 0}>
            Bỏ hết reviewer
          </Button>
        </div>
      </Panel>

      <Panel className="p-4">
        <SectionTitle
          title="Chọn thành phần họp"
          desc="AI đọc yêu cầu và chọn chủ trì, reviewer, phòng ban phù hợp (1 lần gọi ~2–3k token). Ít người hơn = ít token hơn và góp ý đúng chuyên môn hơn."
        />
        <div className="grid grid-cols-3 gap-2">
          {STAFFING.map((s) => (
            <button
              key={s.id}
              type="button"
              aria-pressed={m.staffing === s.id}
              onClick={() => setMeeting({ staffing: s.id })}
              className={cx(
                "rounded-lg border px-3 py-2 text-left",
                m.staffing === s.id ? "border-violet-500/60 bg-violet-500/10" : "border-neutral-800 hover:border-neutral-600",
              )}
            >
              <div className="text-sm font-medium text-neutral-100">{s.label}</div>
              <div className="mt-0.5 text-[11px] text-neutral-500">{s.desc}</div>
            </button>
          ))}
        </div>
        <div className="mt-4 grid grid-cols-3 gap-4">
          <Field label={`Role bắt buộc luôn có mặt (${m.required_role_ids.length})`} className="col-span-3" error={errors["meeting.required"]} hint="VD: Tech Lead, Security — AI không được bỏ qua.">
            <RoleChips roles={draft.roles} value={m.required_role_ids} onChange={(ids) => setMeeting({ required_role_ids: ids })} />
          </Field>
          <Field label="Số reviewer tối đa AI chọn" error={errors["meeting.max_auto_reviewers"]} hint="1–12">
            <NumberInput value={m.max_auto_reviewers} onChange={(v) => setMeeting({ max_auto_reviewers: v ?? Number.NaN })} />
          </Field>
          <Field label="Số phòng ban tối đa AI chọn" error={errors["meeting.max_auto_departments"]} hint="0–8">
            <NumberInput value={m.max_auto_departments} onChange={(v) => setMeeting({ max_auto_departments: v ?? Number.NaN })} />
          </Field>
          <Field label="Mức độ plan mặc định" hint="Đổi được cho từng phiên ở màn Phòng họp">
            <select value={m.plan_depth} onChange={(e) => setMeeting({ plan_depth: e.target.value as PlanDepth })} className={INPUT}>
              <option value="mvp">MVP — ngắn gọn, bản đầu dùng được</option>
              <option value="standard">Vừa đủ — đủ để bắt tay làm</option>
              <option value="enterprise">Enterprise — production-grade</option>
            </select>
          </Field>
        </div>
      </Panel>

      <Panel className="p-4">
        <SectionTitle title="Trong lúc họp" desc="Thành viên thay đổi được giữa các vòng; plan chia section để mỗi người chỉ đọc phần của mình." />
        <div className="grid grid-cols-3 gap-4">
          <Field label="Thành viên động" hint="Reviewer đề nghị mời chuyên gia còn thiếu, hoặc tự rút nếu không thuộc chuyên môn">
            <Toggle checked={m.dynamic_membership} onChange={(v) => setMeeting({ dynamic_membership: v })} label="Bật" />
          </Field>
          <Field label="Số người reviewer được mời thêm" error={errors["meeting.max_invites"]} hint="0–6 mỗi nơi họp · bạn tự mời thì không giới hạn">
            <NumberInput value={m.max_invites} onChange={(v) => setMeeting({ max_invites: v ?? Number.NaN })} />
          </Field>
          <Field label="Review theo section" hint="Người viết gắn tag reviewer cho từng phần; phần không đổi → giữ nguyên APPROVE, không gọi lại model">
            <Toggle checked={m.section_routing} onChange={(v) => setMeeting({ section_routing: v })} label="Bật (tiết kiệm token)" />
          </Field>
        </div>
      </Panel>
    </div>
  );
}

const STAFFING: { id: StaffingMode; label: string; desc: string }[] = [
  { id: "manual", label: "Thủ công", desc: "Luôn dùng chủ trì / reviewer / phòng ban cấu hình ở trên." },
  { id: "suggest", label: "Đề xuất rồi duyệt", desc: "AI đề xuất kèm lý do, bạn bật/tắt rồi mới họp. Khuyên dùng." },
  { id: "auto", label: "Tự động", desc: "AI chọn rồi họp luôn, lý do hiện trong phiên." },
];
