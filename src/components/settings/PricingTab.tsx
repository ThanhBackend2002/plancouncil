import { useEffect, useMemo, useRef, useState } from "react";
import { Plus, Trash } from "lucide-react";
import type { AppConfig, ModelPrice } from "../../lib/contracts";
import { Button, Panel } from "../ui";
import { INPUT, NumberInput, SectionTitle, type Errors } from "./fields";

interface Row {
  rid: number;
  model: string;
  /** NaN = ô trống. */
  input: number;
  cached: number;
  output: number;
}

const blank = Number.NaN;
const isEmpty = (r: Row) => [r.input, r.cached, r.output].every(Number.isNaN);

let nextRid = 1;

function buildRows(pricing: Record<string, ModelPrice> | undefined, roleModels: string[]): Row[] {
  const rows: Row[] = Object.entries(pricing ?? {}).map(([model, p]) => ({
    rid: nextRid++,
    model,
    input: p.input_per_m ?? blank,
    cached: p.cached_per_m ?? blank,
    output: p.output_per_m ?? blank,
  }));
  // Gợi ý sẵn các model của vai trò chưa có giá (dòng trống, chưa lưu cho tới khi nhập giá).
  for (const m of roleModels) {
    if (!rows.some((r) => r.model === m)) rows.push({ rid: nextRid++, model: m, input: blank, cached: blank, output: blank });
  }
  return rows;
}

/** Dòng → bản đồ giá để lưu. Dòng trống tên / trống giá / trùng tên không được lưu. Giá sai giữ nguyên NaN để validate chặn lưu. */
function toPricing(rows: Row[]): Record<string, ModelPrice> {
  const out: Record<string, ModelPrice> = {};
  for (const r of rows) {
    const m = r.model.trim();
    if (!m || isEmpty(r) || m in out) continue;
    out[m] = { input_per_m: r.input, cached_per_m: r.cached, output_per_m: r.output };
  }
  return out;
}

const badPrice = (v: number) => !Number.isFinite(v) || v < 0;

/** Giá model do người dùng tự nhập (USD / 1M token) để tính chi phí. */
export function PricingTab({
  draft,
  errors,
  update,
}: {
  draft: AppConfig;
  errors: Errors;
  update: (f: (c: AppConfig) => AppConfig) => void;
}) {
  const roleModels = useMemo(() => [...new Set(draft.roles.map((r) => r.model.trim()).filter(Boolean))], [draft.roles]);
  const [rows, setRows] = useState<Row[]>(() => buildRows(draft.pricing, roleModels));
  const emitted = useRef(JSON.stringify(draft.pricing ?? {}));

  // Bản nháp đổi từ bên ngoài (Hoàn tác / lưu xong) → dựng lại bảng.
  const external = JSON.stringify(draft.pricing ?? {});
  useEffect(() => {
    if (external !== emitted.current) {
      emitted.current = external;
      setRows(buildRows(draft.pricing, roleModels));
    }
  }, [external, draft.pricing, roleModels]);

  const commit = (next: Row[]) => {
    setRows(next);
    const pricing = toPricing(next);
    emitted.current = JSON.stringify(pricing);
    update((c) => ({ ...c, pricing }));
  };
  const patch = (rid: number, p: Partial<Row>) => commit(rows.map((r) => (r.rid === rid ? { ...r, ...p } : r)));

  const seen = new Set<string>();
  const rowError = (r: Row): string | null => {
    const m = r.model.trim();
    if (!m && !isEmpty(r)) return "Cần tên model";
    if (!m) return null;
    if (seen.has(m)) return "Trùng tên model — dòng này không được lưu";
    seen.add(m);
    if (!isEmpty(r) && [r.input, r.cached, r.output].some(badPrice)) return "Điền đủ 3 giá, số ≥ 0";
    return null;
  };
  const listId = "pricing-models";

  return (
    <Panel className="p-4">
      <SectionTitle title="Giá model" desc="Giá do bạn tự nhập; không có giá thì chi phí hiển thị —. Đơn vị: USD trên 1 triệu token.">
        <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => commit([...rows, { rid: nextRid++, model: "", input: blank, cached: blank, output: blank }])}>
          Thêm model
        </Button>
      </SectionTitle>
      {errors["pricing"] && (
        <p role="alert" className="mb-2 text-xs text-danger">
          {errors["pricing"]}
        </p>
      )}
      <datalist id={listId}>
        {roleModels.map((m) => (
          <option key={m} value={m} />
        ))}
      </datalist>
      <div role="table" aria-label="Bảng giá model" className="space-y-2">
        <div role="row" className="grid grid-cols-[minmax(0,2fr)_repeat(3,minmax(0,1fr))_2.25rem] gap-2 text-xs font-medium text-fg-muted">
          <span role="columnheader">Model</span>
          <span role="columnheader">Input / 1M</span>
          <span role="columnheader">Cached / 1M</span>
          <span role="columnheader">Output / 1M</span>
          <span role="columnheader" className="sr-only">
            Xoá
          </span>
        </div>
        {rows.length === 0 && <p className="py-3 text-xs text-fg-subtle">Chưa có dòng nào. Bấm “Thêm model”.</p>}
        {rows.map((r) => {
          const err = rowError(r);
          return (
            <div key={r.rid} role="row" className="space-y-1">
              <div className="grid grid-cols-[minmax(0,2fr)_repeat(3,minmax(0,1fr))_2.25rem] items-center gap-2">
                <input
                  role="cell"
                  list={listId}
                  value={r.model}
                  onChange={(e) => patch(r.rid, { model: e.target.value })}
                  placeholder="tên model"
                  aria-label="Tên model"
                  aria-invalid={!!err}
                  spellCheck={false}
                  autoComplete="off"
                  className={INPUT}
                />
                <div role="cell" aria-label={`Giá input ${r.model}`}>
                  <NumberInput value={r.input} step={0.01} nullable onChange={(v) => patch(r.rid, { input: v ?? blank })} />
                </div>
                <div role="cell" aria-label={`Giá cached ${r.model}`}>
                  <NumberInput value={r.cached} step={0.01} nullable onChange={(v) => patch(r.rid, { cached: v ?? blank })} />
                </div>
                <div role="cell" aria-label={`Giá output ${r.model}`}>
                  <NumberInput value={r.output} step={0.01} nullable onChange={(v) => patch(r.rid, { output: v ?? blank })} />
                </div>
                <Button size="sm" variant="ghost" title={`Xoá dòng ${r.model || "trống"}`} icon={<Trash className="size-3.5" />} onClick={() => commit(rows.filter((x) => x.rid !== r.rid))} />
              </div>
              {err && <p className="text-[11px] text-danger">{err}</p>}
            </div>
          );
        })}
      </div>
    </Panel>
  );
}
