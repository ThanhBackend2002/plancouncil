// Phần dùng chung của cảnh pixel (phòng họp + toà nhà): ngày/đêm, ăn mừng, pháo giấy, đi ra/vào.

import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Phase } from "../store/session";

// ───────────────────────────── Ngày / đêm ─────────────────────────────

export type DayPhase = "day" | "dusk" | "night";

export function dayPhaseAt(hour: number): DayPhase {
  if (hour >= 6 && hour < 17) return "day";
  if (hour >= 17 && hour < 19) return "dusk";
  return "night";
}

/** Theo giờ máy, cập nhật mỗi 5 phút. */
export function useDayPhase(): DayPhase {
  const [phase, setPhase] = useState(() => dayPhaseAt(new Date().getHours()));
  useEffect(() => {
    const t = setInterval(() => setPhase(dayPhaseAt(new Date().getHours())), 5 * 60_000);
    return () => clearInterval(t);
  }, []);
  return phase;
}

export const SKY: Record<DayPhase, { top: string; bottom: string; cloud: string | null }> = {
  day: { top: "#7dd3fc", bottom: "#bae6fd", cloud: "#f8fafc" },
  dusk: { top: "#f97316", bottom: "#fcd34d", cloud: "#fed7aa" },
  night: { top: "#0f172a", bottom: "#1e3a8a", cloud: null },
};

const STARS: readonly (readonly [number, number])[] = [
  [4, 4],
  [14, 9],
  [24, 3],
  [9, 14],
  [30, 12],
];

/** Ô cửa sổ (khung + bầu trời) tại (x, y), kích thước w×h. Đêm: trăng + sao. */
export const SkyWindow = memo(function SkyWindow({ x, y, w, h, phase }: { x: number; y: number; w: number; h: number; phase: DayPhase }) {
  const sky = SKY[phase];
  const ix = x + 2;
  const iy = y + 2;
  const iw = w - 4;
  const ih = h - 4;
  return (
    <g>
      <rect x={x} y={y} width={w} height={h} fill="#475569" />
      <rect x={ix} y={iy} width={iw} height={ih} fill={sky.top} />
      <rect x={ix} y={iy + Math.round(ih * 0.66)} width={iw} height={ih - Math.round(ih * 0.66)} fill={sky.bottom} />
      {/* Mây / mặt trời / trăng co theo cỡ cửa sổ (cửa sổ nhỏ trong toà nhà chỉ ~14px). */}
      {sky.cloud && iw >= 30 && (
        <>
          <rect x={ix + Math.round(iw * 0.2)} y={iy + Math.round(ih * 0.2)} width={Math.round(iw * 0.24)} height={4} fill={sky.cloud} />
          <rect x={ix + Math.round(iw * 0.12)} y={iy + Math.round(ih * 0.2) + 2} width={Math.round(iw * 0.4)} height={3} fill={sky.cloud} />
        </>
      )}
      {sky.cloud && iw < 30 && <rect x={ix + 2} y={iy + 2} width={Math.max(3, Math.round(iw * 0.35))} height={2} fill={sky.cloud} />}
      {phase === "dusk" && <rect x={ix + iw - Math.max(4, Math.round(iw * 0.3))} y={iy + ih - Math.max(3, Math.round(ih * 0.35))} width={Math.max(3, Math.round(iw * 0.16))} height={Math.max(2, Math.round(ih * 0.25))} fill="#fde68a" />}
      {phase === "night" && (
        <>
          <rect x={ix + iw - Math.max(4, Math.round(iw * 0.28))} y={iy + 2} width={Math.min(6, Math.max(3, Math.round(iw * 0.12)))} height={Math.min(6, Math.max(3, Math.round(iw * 0.12)))} fill="#fef9c3" />
          {STARS.map(([sx, sy]) =>
            sx < iw - 8 && sy < ih - 1 ? <rect key={`${sx}-${sy}`} x={ix + sx} y={iy + sy} width={1} height={1} fill="#f8fafc" className="px-dot" style={{ animationDelay: `${sx * 70}ms` }} /> : null,
          )}
        </>
      )}
      {/* song cửa */}
      <rect x={x + Math.round(w / 2) - 1} y={iy} width={2} height={ih} fill="#475569" />
      <rect x={ix} y={iy + Math.round(ih / 2) - 1} width={iw} height={2} fill="#475569" />
    </g>
  );
});

/** Lớp phủ tối khi đêm / chạng vạng (vẽ cuối cùng trong SVG). */
export function NightShade({ w, h, phase }: { w: number; h: number; phase: DayPhase }) {
  if (phase === "day") return null;
  return <rect x={0} y={0} width={w} height={h} fill="#0b1026" opacity={phase === "night" ? 0.22 : 0.1} pointerEvents="none" />;
}

// ───────────────────────────── Ăn mừng ─────────────────────────────

const DONE_OK: Phase[] = ["converged", "partially_approved"];

/**
 * true trong `ms` sau khi `status` chuyển từ đang họp sang hội tụ. Không ăn mừng khi mở phiên cũ /
 * đổi nơi họp (`resetKey` đổi → chỉ ghi nhận trạng thái, không tính là chuyển).
 */
export function useCelebration(status: Phase, resetKey: string, ms = 5_000): boolean {
  const prev = useRef<{ key: string; status: Phase } | null>(null);
  const [on, setOn] = useState(false);
  useEffect(() => {
    const p = prev.current;
    prev.current = { key: resetKey, status };
    if (!p || p.key !== resetKey) {
      setOn(false);
      return;
    }
    if (p.status === "running" && DONE_OK.includes(status)) {
      setOn(true);
      const t = setTimeout(() => setOn(false), ms);
      return () => clearTimeout(t);
    }
  }, [status, resetKey, ms]);
  return on;
}

const CONFETTI_COLORS = ["#f43f5e", "#f59e0b", "#22c55e", "#38bdf8", "#a78bfa", "#facc15"];

/** Pháo giấy pixel phủ cả khung (HTML, đặt trong phần tử `relative`). */
export const Confetti = memo(function Confetti({ pieces = 36 }: { pieces?: number }) {
  return (
    <div className="pointer-events-none absolute inset-0 z-20 overflow-hidden" aria-hidden>
      {Array.from({ length: pieces }, (_, i) => {
        // Giả ngẫu nhiên ổn định theo chỉ số (không nhảy vị trí khi re-render).
        const left = (i * 37) % 100;
        const delay = (i * 113) % 1200;
        return (
          <span key={i} className="px-confetti" style={{ left: `${left}%` }}>
            <i style={{ background: CONFETTI_COLORS[i % CONFETTI_COLORS.length], animationDelay: `${delay}ms` }} />
          </span>
        );
      })}
    </div>
  );
});

export function CelebrationBanner({ text }: { text: string }) {
  return (
    <div className="pointer-events-none absolute inset-x-0 top-[38%] z-20 flex justify-center" aria-hidden>
      <div className="px-banner px-font px-box rounded-[1px] bg-[#fef08a] px-3 py-1 text-sm font-bold tracking-widest text-[#713f12]" style={{ ["--px-border" as string]: "#a16207" }}>
        {text}
      </div>
    </div>
  );
}

// ───────────────────────────── Đi vào / đi ra ─────────────────────────────

export interface Placed {
  id: string;
  x: number;
  y: number;
}

export interface Walker<T extends Placed> {
  seat: T;
  /** Vị trí đang vẽ (cửa khi vừa vào / sắp ra). */
  x: number;
  y: number;
  walking: boolean;
  leaving: boolean;
}

const WALK_MS = 1_500;

/**
 * Người mới xuất hiện đi từ `door` vào chỗ; người biến mất đi từ chỗ ra `door` rồi mới biến khỏi cảnh.
 * Chỉ diễn khi `live` (đang họp) và cùng `resetKey`; lần đầu / đổi khung cảnh thì đặt thẳng vào chỗ.
 */
export function useWalkers<T extends Placed>(seats: T[], door: { x: number; y: number }, live: boolean, resetKey: string): Walker<T>[] {
  const known = useRef<{ key: string; seats: Map<string, T> } | null>(null);
  // id → trạng thái đi: "in-door" (đặt ở cửa, frame kế tiếp đi vào), "in" (đang đi vào), "out-seat", "out".
  const [moves, setMoves] = useState<Record<string, "in-door" | "in" | "out-seat" | "out">>({});
  const [ghosts, setGhosts] = useState<T[]>([]);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const ids = seats.map((s) => s.id).join("|");

  useLayoutEffect(() => {
    const cur = new Map(seats.map((s) => [s.id, s]));
    const prev = known.current;
    known.current = { key: resetKey, seats: cur };
    if (!prev || prev.key !== resetKey || !live) {
      setMoves({});
      setGhosts([]);
      return;
    }
    const added = seats.filter((s) => !prev.seats.has(s.id)).map((s) => s.id);
    const removed = [...prev.seats.values()].filter((s) => !cur.has(s.id));
    if (added.length === 0 && removed.length === 0) return;
    setMoves((m) => {
      const next = { ...m };
      for (const id of added) next[id] = "in-door";
      for (const s of removed) next[s.id] = "out-seat";
      return next;
    });
    if (removed.length) setGhosts((g) => [...g.filter((x) => !removed.some((r) => r.id === x.id)), ...removed]);
    // 2 frame: để trình duyệt vẽ vị trí xuất phát trước khi đổi đích (transition mới chạy).
    requestAnimationFrame(() =>
      requestAnimationFrame(() =>
        setMoves((m) => {
          const next = { ...m };
          for (const id of added) if (next[id] === "in-door") next[id] = "in";
          for (const s of removed) if (next[s.id] === "out-seat") next[s.id] = "out";
          return next;
        }),
      ),
    );
    const t = setTimeout(() => {
      setMoves((m) => {
        const next = { ...m };
        for (const id of added) delete next[id];
        for (const s of removed) delete next[s.id];
        return next;
      });
      setGhosts((g) => g.filter((x) => !removed.some((r) => r.id === x.id)));
    }, WALK_MS + 100);
    timers.current.push(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `ids` tóm tắt `seats`
  }, [ids, resetKey, live]);

  useEffect(() => () => timers.current.forEach(clearTimeout), []);

  const out: Walker<T>[] = seats.map((s) => {
    const m = moves[s.id];
    const atDoor = m === "in-door";
    return { seat: s, x: atDoor ? door.x : s.x, y: atDoor ? door.y : s.y, walking: m === "in-door" || m === "in", leaving: false };
  });
  for (const g of ghosts) {
    if (seats.some((s) => s.id === g.id)) continue;
    const m = moves[g.id];
    const atSeat = m === "out-seat";
    out.push({ seat: g, x: atSeat ? g.x : door.x, y: atSeat ? g.y : door.y, walking: true, leaving: true });
  }
  return out;
}
