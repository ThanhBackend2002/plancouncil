import { useMemo } from "react";
import { Dices, RotateCcw } from "lucide-react";
import { makeResolver } from "../../lib/agents";
import type { Appearance, RoleDef } from "../../lib/contracts";
import { ACCESSORIES, EYEWEAR, HAIRS, HAIR_COLORS, OUTFITS, PRESETS, SKINS, resolveLook } from "../../lib/pixel";
import { PixelPortrait } from "../PixelCharacter";
import { Button, cx } from "../ui";

export const EMPTY_LOOK: Appearance = {
  gender: "",
  skin: "",
  hair_style: "",
  hair_color: "",
  outfit: "",
  outfit_color: "",
  eyewear: "",
  accessory: "",
};

const rand = <T,>(arr: readonly T[]): T => arr[Math.floor(Math.random() * arr.length)]!;

/** Chỉnh ngoại hình nhân vật pixel của 1 role; xem trước trực tiếp. */
export function AppearanceEditor({ role, onChange }: { role: RoleDef; onChange: (a: Appearance) => void }) {
  const a = { ...EMPTY_LOOK, ...role.appearance };
  const set = (patch: Partial<Appearance>) => onChange({ ...a, ...patch });
  const meta = useMemo(() => makeResolver([], [{ ...role, appearance: a }])(role.id), [role, a]);
  const resolved = resolveLook({ id: role.id, color: role.color, level: role.level, icon: role.icon, appearance: a });
  const genderNow = resolved.gender;
  const hairs = HAIRS.filter((h) => h.gender === "any" || h.gender === genderNow);

  const randomize = () => {
    const gender = rand(["male", "female"] as const);
    onChange({
      gender,
      skin: rand(SKINS).id,
      hair_style: rand(HAIRS.filter((h) => h.gender === "any" || h.gender === gender)).id,
      hair_color: rand(HAIR_COLORS).id,
      outfit: rand(OUTFITS.filter((o) => gender === "female" || o.id !== "dress")).id,
      outfit_color: "",
      eyewear: rand(["none", "none", "glasses", "sunglasses"]),
      accessory: rand(["none", "none", ...ACCESSORIES.map((x) => x.id)]),
    });
  };

  return (
    <div className="grid grid-cols-[132px_1fr] gap-4">
      <div className="flex flex-col items-center gap-2">
        <div className="grid h-[180px] w-[132px] place-items-center rounded-lg border border-neutral-800 bg-[linear-gradient(#2b2740_0_45%,#3a3352_45%)]">
          <PixelPortrait meta={meta} size={96} full />
        </div>
        <div className="flex gap-1">
          <Button size="sm" icon={<Dices className="size-3.5" />} onClick={randomize} title="Ngoại hình ngẫu nhiên">
            Ngẫu nhiên
          </Button>
          <Button size="sm" variant="ghost" icon={<RotateCcw className="size-3.5" />} onClick={() => onChange(EMPTY_LOOK)} title="Tự sinh theo id + cấp bậc" />
        </div>
      </div>

      <div className="min-w-0 space-y-2.5">
        <Row label="Mẫu có sẵn">
          <div className="flex flex-wrap gap-1.5">
            {PRESETS.map((p) => {
              const pm = makeResolver([], [{ ...role, id: `${role.id}~${p.id}`, appearance: p.look }])(`${role.id}~${p.id}`);
              const active = JSON.stringify(p.look) === JSON.stringify(a);
              return (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => onChange(p.look)}
                  title={p.label}
                  className={cx(
                    "flex flex-col items-center gap-0.5 rounded-md border p-1 text-[9px] leading-tight",
                    active ? "border-violet-500 bg-violet-500/15 text-violet-200" : "border-neutral-800 text-neutral-500 hover:border-neutral-600",
                  )}
                >
                  <PixelPortrait meta={pm} size={32} />
                  <span className="max-w-[58px] truncate">{p.label}</span>
                </button>
              );
            })}
          </div>
        </Row>

        <Row label="Giới tính">
          <Chips
            value={a.gender}
            onChange={(v) => {
              // Đổi giới tính → bỏ kiểu tóc không hợp để tự chọn lại.
              const hair = HAIRS.find((h) => h.id === a.hair_style);
              const keepHair = !hair || hair.gender === "any" || hair.gender === v || v === "";
              set({ gender: v, hair_style: keepHair ? a.hair_style : "", outfit: v === "male" && a.outfit === "dress" ? "" : a.outfit });
            }}
            options={[
              { id: "male", label: "Nam" },
              { id: "female", label: "Nữ" },
            ]}
          />
        </Row>

        <Row label="Màu da">
          <Swatches value={a.skin} onChange={(v) => set({ skin: v })} options={SKINS.map((s) => ({ id: s.id, label: s.label, color: s.color }))} />
        </Row>

        <Row label="Kiểu tóc">
          <Chips value={a.hair_style} onChange={(v) => set({ hair_style: v })} options={hairs.map((h) => ({ id: h.id, label: h.label }))} />
        </Row>

        <Row label="Màu tóc">
          <Swatches value={a.hair_color} onChange={(v) => set({ hair_color: v })} options={HAIR_COLORS.map((c) => ({ id: c.id, label: c.label, color: c.color }))} />
        </Row>

        <Row label="Trang phục">
          <Chips
            value={a.outfit}
            onChange={(v) => set({ outfit: v })}
            options={OUTFITS.filter((o) => genderNow === "female" || o.id !== "dress").map((o) => ({ id: o.id, label: o.label }))}
          />
        </Row>

        <Row label="Màu áo">
          <div className="flex items-center gap-2 text-xs text-neutral-400">
            <button
              type="button"
              onClick={() => set({ outfit_color: "" })}
              className={cx("rounded-full border px-2 py-0.5", !a.outfit_color ? "border-violet-500 text-violet-200" : "border-neutral-700 hover:text-neutral-200")}
            >
              Theo màu role
            </button>
            <input
              type="color"
              value={/^#[0-9a-f]{6}$/i.test(a.outfit_color) ? a.outfit_color : role.color}
              onChange={(e) => set({ outfit_color: e.target.value })}
              className="h-7 w-9 cursor-pointer rounded border border-neutral-700 bg-neutral-950"
              aria-label="Màu trang phục"
            />
          </div>
        </Row>

        <Row label="Kính">
          <Chips value={a.eyewear} onChange={(v) => set({ eyewear: v })} options={EYEWEAR.map((e) => ({ id: e.id, label: e.label }))} />
        </Row>

        <Row label="Phụ kiện">
          <Chips value={a.accessory} onChange={(v) => set({ accessory: v })} options={ACCESSORIES.map((x) => ({ id: x.id, label: x.label }))} />
        </Row>
      </div>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[84px_1fr] items-start gap-2">
      <span className="pt-1 text-[11px] font-medium text-neutral-500">{label}</span>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/** "" = tự động. */
function Chips({ value, onChange, options }: { value: string; onChange: (v: string) => void; options: { id: string; label: string }[] }) {
  return (
    <div className="flex flex-wrap gap-1">
      {[{ id: "", label: "Tự động" }, ...options].map((o) => (
        <button
          key={o.id || "auto"}
          type="button"
          aria-pressed={value === o.id}
          onClick={() => onChange(o.id)}
          className={cx(
            "rounded-full border px-2 py-0.5 text-[11px]",
            value === o.id ? "border-violet-500 bg-violet-500/15 text-violet-200" : "border-neutral-700 text-neutral-400 hover:text-neutral-200",
            o.id === "" && value !== "" && "border-dashed",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function Swatches({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: { id: string; label: string; color: string }[];
}) {
  return (
    <div className="flex flex-wrap items-center gap-1">
      <button
        type="button"
        onClick={() => onChange("")}
        className={cx(
          "rounded-full border px-2 py-0.5 text-[11px]",
          value === "" ? "border-violet-500 bg-violet-500/15 text-violet-200" : "border-dashed border-neutral-700 text-neutral-400 hover:text-neutral-200",
        )}
      >
        Tự động
      </button>
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          title={o.label}
          aria-label={o.label}
          aria-pressed={value === o.id}
          onClick={() => onChange(o.id)}
          className={cx("size-6 rounded-[3px] border-2", value === o.id ? "border-violet-400" : "border-neutral-800 hover:border-neutral-500")}
          style={{ background: o.color }}
        />
      ))}
    </div>
  );
}
