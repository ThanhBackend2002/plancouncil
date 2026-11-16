import { memo, useState } from "react";
import type { RoleId } from "../lib/contracts";
import type { RoleResolver } from "../lib/agents";
import { SPRITE_W } from "../lib/pixel";
import type { ScopeState, Speech } from "../store/session";
import { PixelCharacter, type Marker } from "./PixelCharacter";
import { cx } from "./ui";

// Cảnh văn phòng pixel: 240×150 pixel logic, scale theo khung.
const W = 240;
const H = 150;
const AUTHOR = { x: 62, y: 76 };
const TABLE = { x: 92, y: 74, w: 140 };
const TOP_Y = 56; // hàng sau bàn (bị bàn che chân)
const BOTTOM_Y = 98; // hàng trước bàn

interface Seat {
  id: RoleId;
  x: number;
  y: number;
  row: "author" | "top" | "bottom";
}

function layout(reviewers: RoleId[]): Seat[] {
  const top = reviewers.slice(0, Math.ceil(reviewers.length / 2));
  const bottom = reviewers.slice(top.length);
  const spread = (ids: RoleId[], row: "top" | "bottom", y: number, offset: number) => {
    const span = TABLE.w - 16;
    const step = span / Math.max(ids.length, 1);
    return ids.map((id, i) => ({
      id,
      row,
      y,
      x: Math.round(TABLE.x + 8 + step * (i + 0.5) + offset - SPRITE_W / 2),
    }));
  };
  return [...spread(top, "top", TOP_Y, 0), ...spread(bottom, "bottom", BOTTOM_Y, top.length > bottom.length ? 4 : 0)];
}

const pct = (v: number, total: number) => `${(v / total) * 100}%`;

const TONE_BORDER: Record<Speech["tone"], string> = {
  info: "#7c3aed",
  approve: "#16a34a",
  reject: "#dc2626",
  error: "#d97706",
};

// memo: props (scope, resolve, chuỗi) giữ tham chiếu khi plan_delta → không vẽ lại cảnh theo từng delta.
export const MeetingRoom = memo(function MeetingRoom({
  scope,
  resolve,
  title,
  idleCaption,
}: {
  scope: ScopeState;
  resolve: RoleResolver;
  title: string;
  idleCaption?: string;
}) {
  const [hover, setHover] = useState<RoleId | null>(null);
  const seats: Seat[] = [
    ...(scope.authorId ? [{ id: scope.authorId, x: AUTHOR.x, y: AUTHOR.y, row: "author" as const }] : []),
    ...layout(scope.reviewerIds),
  ];
  const round = scope.rounds.find((r) => r.round === scope.currentRound);
  const latestPlan = scope.plans.at(-1);

  // Ai đang nói: hover > câu thoại gần nhất.
  const hoverSpeech = (() => {
    if (!hover) return null;
    const a = scope.agents[hover];
    if (a?.review) return { agentId: hover, text: a.review.summary_bubble, tone: a.review.verdict === "APPROVE" ? "approve" : "reject", seq: -1 } as Speech;
    if (a?.error) return { agentId: hover, text: `Lỗi: ${a.error}`, tone: "error", seq: -1 } as Speech;
    const m = resolve(hover);
    return { agentId: hover, text: `${m.name}${m.title && m.title !== m.name ? ` · ${m.title}` : ""}${m.model ? ` · ${m.model}` : ""}`, tone: "info", seq: -1 } as Speech;
  })();
  const speech = hoverSpeech ?? scope.speech;
  const speaker = speech ? seats.find((s) => s.id === speech.agentId) : undefined;
  const crowded = seats.length > 9;

  const markerOf = (id: RoleId): Marker => {
    const a = scope.agents[id];
    if (!a || id === scope.authorId) return a?.status === "error" ? "error" : null;
    if (a.status === "error") return "error";
    if (a.status === "done" && a.review && a.review.verdict !== "ABSTAIN") return a.review.verdict === "APPROVE" ? "approve" : "reject";
    return null;
  };

  return (
    <section
      aria-label="Phòng họp"
      className="relative shrink-0 overflow-hidden rounded-xl border border-neutral-800 bg-[#1d1a2c]"
      style={{ aspectRatio: `${W} / ${H}` }}
    >
      <svg
        className="px-scene absolute inset-0 size-full"
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="xMidYMid meet"
        shapeRendering="crispEdges"
        aria-hidden
      >
        <Office title={title} version={latestPlan?.version ?? 0} writing={!!latestPlan && !latestPlan.complete} />
        {/* Hàng sau bàn → bàn → hàng trước bàn → người viết plan (đứng trước bảng) */}
        {seats
          .filter((s) => s.row === "top")
          .map((s) => (
            <PixelCharacter
              key={s.id}
              meta={resolve(s.id)}
              x={s.x}
              y={s.y}
              status={scope.agents[s.id]?.status ?? "idle"}
              speaking={speaker?.id === s.id}
              marker={markerOf(s.id)}
            />
          ))}
        <Table />
        {seats
          .filter((s) => s.row !== "top")
          .map((s) => (
            <PixelCharacter
              key={s.id}
              meta={resolve(s.id)}
              x={s.x}
              y={s.y}
              status={scope.agents[s.id]?.status ?? "idle"}
              speaking={speaker?.id === s.id}
              marker={markerOf(s.id)}
            />
          ))}
      </svg>

      {/* Nhãn tên + vùng hover (HTML để chữ sắc nét) */}
      {seats.map((s) => {
        const m = resolve(s.id);
        // hàng sau: nhãn trên đầu (phía trên dấu ✓/✗); còn lại: dưới chân
        const labelY = s.row === "top" ? s.y - 10 : s.y + 25;
        return (
          <div key={s.id}>
            <div
              className="absolute cursor-default"
              style={{ left: pct(s.x, W), top: pct(s.y, H), width: pct(SPRITE_W, W), height: pct(24, H) }}
              onMouseEnter={() => setHover(s.id)}
              onMouseLeave={() => setHover((h) => (h === s.id ? null : h))}
            />
            {(!crowded || s.row === "author" || hover === s.id) && (
              <div
                className={cx(
                  "px-font pointer-events-none absolute max-w-[78px] -translate-x-1/2 truncate rounded-[1px] px-1 text-[9px] leading-[13px] font-semibold text-neutral-950",
                  s.row === "top" && "-translate-y-full",
                )}
                style={{ left: pct(s.x + SPRITE_W / 2, W), top: pct(labelY, H), background: m.color }}
                title={`${m.name}${m.title ? ` — ${m.title}` : ""}`}
              >
                {m.name}
              </div>
            )}
          </div>
        );
      })}

      {/* Bong bóng thoại của người đang nói */}
      {speech && speaker && (
        <SpeechBubble
          key={`${speech.agentId}-${speech.seq}-${hover ?? ""}`}
          speech={speech}
          name={resolve(speech.agentId).name}
          x={speaker.x + SPRITE_W / 2}
          y={speaker.row === "top" ? speaker.y - 16 : speaker.y - 10}
        />
      )}

      {/* Trạng thái góc phải */}
      <div className="px-font pointer-events-none absolute top-2 right-2 flex items-center gap-1.5 rounded-sm bg-black/55 px-2 py-1 text-[10px] text-neutral-200">
        {scope.currentRound > 0 ? (
          <>
            ROUND {scope.currentRound}/{scope.maxRounds || "?"}
            {round && (
              <span className="flex gap-0.5">
                {scope.reviewerIds.map((id) => {
                  const slot = round.slots[id];
                  const c = slot?.review
                    ? slot.review.verdict === "APPROVE"
                      ? "bg-emerald-400"
                      : "bg-red-400"
                    : slot?.error
                      ? "bg-amber-400"
                      : "bg-neutral-600";
                  return <span key={id} className={cx("size-1.5", c)} />;
                })}
              </span>
            )}
          </>
        ) : (
          (idleCaption ?? (scope.status === "running" ? "CHUẨN BỊ…" : "PHÒNG HỌP"))
        )}
      </div>
      {scope.task && (
        <div className="pointer-events-none absolute bottom-1.5 left-2 max-w-[60%] truncate rounded-sm bg-black/55 px-2 py-0.5 text-[10px] text-neutral-300" title={scope.task}>
          Nhiệm vụ: {scope.task}
        </div>
      )}
    </section>
  );
});

const SpeechBubble = memo(function SpeechBubble({ speech, name, x, y }: { speech: Speech; name: string; x: number; y: number }) {
  // Giữ bong bóng trong khung: neo theo % và kẹp 18%–82%.
  const left = Math.min(82, Math.max(18, (x / W) * 100));
  return (
    <div
      className="px-bubble pointer-events-none absolute z-10 w-max max-w-[210px] -translate-x-1/2 -translate-y-full"
      style={{ left: `${left}%`, top: pct(y, H) }}
    >
      <div
        className="px-box px-font rounded-[1px] bg-[#fdfdf8] px-2 py-1 text-[11px] leading-snug text-neutral-900"
        style={{ ["--px-border" as string]: TONE_BORDER[speech.tone] }}
      >
        <span className="mr-1 font-bold" style={{ color: TONE_BORDER[speech.tone] }}>
          {name}:
        </span>
        <span className="line-clamp-3">{speech.text}</span>
      </div>
      <div className="mx-auto mt-[2px] h-[6px] w-[6px] bg-[#fdfdf8]" style={{ boxShadow: `0 2px 0 0 ${TONE_BORDER[speech.tone]}` }} />
    </div>
  );
});

/** Tường, sàn, bảng trắng, cửa sổ, cây — vẽ bằng rect. memo: props nguyên thuỷ, khỏi dựng lại ~100 ô sàn. */
const Office = memo(function Office({ title, version, writing }: { title: string; version: number; writing: boolean }) {
  const tiles = [];
  for (let y = 48; y < H; y += 12) {
    for (let x = 0; x < W; x += 12) {
      if (((x + y) / 12) % 2 === 0) tiles.push(<rect key={`${x}-${y}`} x={x} y={y} width={12} height={12} fill="#332d48" />);
    }
  }
  const lines = Math.min(6, Math.max(version, 0) + (writing ? 1 : 0) + 1);
  return (
    <g>
      {/* sàn + tường */}
      <rect x={0} y={48} width={W} height={H - 48} fill="#3a3352" />
      {tiles}
      <rect x={0} y={0} width={W} height={46} fill="#2b2740" />
      <rect x={0} y={44} width={W} height={4} fill="#1d1a2c" />
      {[20, 60, 100, 140, 180, 220].map((x) => (
        <rect key={x} x={x} y={0} width={1} height={44} fill="#26223a" />
      ))}

      {/* bảng trắng */}
      <rect x={6} y={6} width={52} height={36} fill="#64748b" />
      <rect x={8} y={8} width={48} height={32} fill="#eef2f7" />
      <text x={11} y={15} fontSize={5} fontFamily="monospace" fill="#4c1d95" fontWeight="bold">
        {title.length > 14 ? `${title.slice(0, 13)}…` : title}
      </text>
      {version > 0 && (
        <text x={45} y={15} fontSize={5} fontFamily="monospace" fill="#be123c" fontWeight="bold">
          v{version}
        </text>
      )}
      {Array.from({ length: lines }, (_, i) => (
        <rect
          key={i}
          x={11}
          y={19 + i * 3.5}
          width={i === lines - 1 && writing ? 14 : 22 + ((i * 7) % 18)}
          height={1.5}
          fill={i % 3 === 0 ? "#7c3aed" : "#94a3b8"}
          className={i === lines - 1 && writing ? "px-dot" : undefined}
        />
      ))}
      <rect x={20} y={42} width={24} height={2} fill="#475569" />

      {/* cửa sổ */}
      <rect x={176} y={6} width={54} height={28} fill="#475569" />
      <rect x={178} y={8} width={50} height={24} fill="#7dd3fc" />
      <rect x={178} y={24} width={50} height={8} fill="#bae6fd" />
      <rect x={190} y={13} width={12} height={4} fill="#f8fafc" />
      <rect x={186} y={15} width={20} height={3} fill="#f8fafc" />
      <rect x={202} y={8} width={2} height={24} fill="#475569" />
      <rect x={178} y={19} width={50} height={2} fill="#475569" />

      {/* đồng hồ */}
      <rect x={128} y={10} width={10} height={10} fill="#e2e8f0" />
      <rect x={129} y={11} width={8} height={8} fill="#f8fafc" />
      <rect x={132} y={12} width={1} height={4} fill="#111827" />
      <rect x={133} y={15} width={3} height={1} fill="#111827" />

      {/* cây */}
      <rect x={6} y={124} width={10} height={10} fill="#92400e" />
      <rect x={5} y={124} width={12} height={2} fill="#b45309" />
      <rect x={7} y={112} width={8} height={12} fill="#15803d" />
      <rect x={4} y={115} width={4} height={6} fill="#16a34a" />
      <rect x={14} y={114} width={4} height={6} fill="#16a34a" />
      <rect x={9} y={108} width={4} height={5} fill="#22c55e" />
    </g>
  );
});

const Table = memo(function Table() {
  const { x, y, w } = TABLE;
  return (
    <g>
      <rect x={x + 2} y={y + 18} width={w - 4} height={2} fill="#000" opacity={0.25} />
      <rect x={x + 4} y={y + 16} width={3} height={8} fill="#5b3a1f" />
      <rect x={x + w - 7} y={y + 16} width={3} height={8} fill="#5b3a1f" />
      <rect x={x} y={y} width={w} height={6} fill="#a86b3c" />
      <rect x={x} y={y} width={w} height={1} fill="#c98a55" />
      <rect x={x} y={y + 6} width={w} height={10} fill="#7a4a26" />
      <rect x={x} y={y + 15} width={w} height={1} fill="#5b3a1f" />
      {/* giấy tờ + laptop trên bàn */}
      <rect x={x + 18} y={y + 1} width={8} height={4} fill="#f8fafc" />
      <rect x={x + 60} y={y + 1} width={10} height={4} fill="#334155" />
      <rect x={x + 61} y={y + 2} width={8} height={2} fill="#38bdf8" />
      <rect x={x + 104} y={y + 1} width={7} height={4} fill="#fef3c7" />
      <rect x={x + 40} y={y + 2} width={3} height={3} fill="#e2e8f0" />
      <rect x={x + 40} y={y + 1} width={3} height={1} fill="#7c2d12" />
    </g>
  );
});
