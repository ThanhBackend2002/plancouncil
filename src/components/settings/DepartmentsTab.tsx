import { Network, Plus, Trash } from "lucide-react";
import type { AppConfig, Department } from "../../lib/contracts";
import { Button, Panel, cx } from "../ui";
import { Field, NumberInput, RoleChips, RoleSelect, SectionTitle, TextArea, TextInput, Toggle, newId, type Errors } from "./fields";

export function DepartmentsTab({
  draft,
  errors,
  update,
}: {
  draft: AppConfig;
  errors: Errors;
  update: (f: (c: AppConfig) => AppConfig) => void;
}) {
  const setDept = (id: string, patch: Partial<Department>) =>
    update((c) => ({ ...c, departments: c.departments.map((d) => (d.id === id ? { ...d, ...patch } : d)) }));

  const add = () => {
    const d: Department = {
      id: newId("dept"),
      name: "Phòng ban mới",
      mission: "",
      lead_id: "",
      member_ids: [],
      max_rounds: 2,
    };
    update((c) => ({ ...c, departments: [...c.departments, d] }));
  };

  const remove = (id: string) =>
    update((c) => ({
      ...c,
      departments: c.departments.filter((d) => d.id !== id),
      meeting: { ...c.meeting, department_ids: c.meeting.department_ids.filter((x) => x !== id) },
    }));

  const setDefault = (id: string, on: boolean) =>
    update((c) => ({
      ...c,
      meeting: {
        ...c.meeting,
        department_ids: on
          ? c.departments.map((d) => d.id).filter((x) => x === id || c.meeting.department_ids.includes(x))
          : c.meeting.department_ids.filter((x) => x !== id),
      },
    }));

  return (
    <Panel className="p-4">
      <SectionTitle
        title={`Phòng ban (${draft.departments.length})`}
        desc="Trưởng phòng viết plan của phòng, thành viên phản biện nội bộ cho tới khi thống nhất, rồi plan được gửi lên cuộc họp chính. Chủ trì sẽ tự giao việc cho từng phòng dựa trên sứ mệnh."
      >
        <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={add}>
          Thêm phòng ban
        </Button>
      </SectionTitle>
      <div className="space-y-3">
        {draft.departments.map((d) => {
          const isDefault = draft.meeting.department_ids.includes(d.id);
          const hasError = Object.keys(errors).some((k) => k.startsWith(`dept.${d.id}.`));
          return (
            <div key={d.id} className={cx("rounded-lg border bg-canvas/40 p-3", hasError ? "border-danger-solid/40" : "border-line")}>
              <div className="mb-3 flex items-center gap-2">
                <span className="grid size-7 place-items-center rounded-control bg-surface-2">
                  <Network className="size-4 text-fg-soft" />
                </span>
                <span className="text-sm font-medium text-fg">{d.name || "(chưa đặt tên)"}</span>
                <span className="text-[11px] text-fg-subtle">
                  {1 + d.member_ids.length} người · tối đa {d.max_rounds} vòng
                </span>
                <div className="ml-auto flex items-center gap-2">
                  <Toggle checked={isDefault} onChange={(v) => setDefault(d.id, v)} label="Mặc định tham gia" />
                  <Button size="sm" variant="ghost" icon={<Trash className="size-3.5" />} onClick={() => remove(d.id)} aria-label="Xoá phòng ban" />
                </div>
              </div>
              <div className="grid grid-cols-[1fr_1fr_100px] gap-3">
                <Field label="Tên phòng ban" error={errors[`dept.${d.id}.name`]}>
                  <TextInput value={d.name} onChange={(v) => setDept(d.id, { name: v })} placeholder="VD: QA & QC" />
                </Field>
                <Field label="Trưởng phòng (viết plan)" error={errors[`dept.${d.id}.lead`]}>
                  <RoleSelect
                    roles={draft.roles}
                    value={d.lead_id}
                    exclude={d.member_ids}
                    onChange={(id) => setDept(d.id, { lead_id: id })}
                  />
                </Field>
                <Field label="Số vòng" error={errors[`dept.${d.id}.max_rounds`]} hint="1–5">
                  <NumberInput value={d.max_rounds} onChange={(v) => setDept(d.id, { max_rounds: v ?? Number.NaN })} />
                </Field>
                <Field label="Sứ mệnh / phạm vi trách nhiệm" className="col-span-3" hint="Chủ trì dựa vào đây để giao việc; trưởng phòng dựa vào đây để giới hạn phạm vi plan.">
                  <TextArea value={d.mission} onChange={(v) => setDept(d.id, { mission: v })} rows={2} placeholder="VD: Test strategy, test scenarios, automation, quality gates…" />
                </Field>
                <Field
                  label={`Thành viên phản biện (${d.member_ids.length})`}
                  className="col-span-3"
                  error={errors[`dept.${d.id}.members`]}
                  hint="Không có thành viên → plan của trưởng phòng được gửi thẳng lên cuộc họp chính."
                >
                  <RoleChips roles={draft.roles} value={d.member_ids} exclude={[d.lead_id]} onChange={(ids) => setDept(d.id, { member_ids: ids })} />
                </Field>
              </div>
            </div>
          );
        })}
        {draft.departments.length === 0 && (
          <p className="py-6 text-center text-sm text-fg-subtle">Chưa có phòng ban nào. Cuộc họp sẽ chỉ gồm chủ trì và reviewer.</p>
        )}
      </div>
    </Panel>
  );
}
