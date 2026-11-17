// Toà nhà PlanCouncil: mỗi nơi họp là 1 phòng (họp chính trên cùng, phòng ban 2 phòng / tầng),
// sảnh ở tầng trệt có các role đang rảnh, thang máy chạy tới nơi đang họp và chở trưởng phòng
// mang plan lên họp chính khi phòng ban xong. Bấm 1 phòng để vào xem.

import { memo, useEffect, useRef, useState } from "react";
import { DoorOpen } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import type { RoleId } from "../lib/contracts";
import { MAIN_SCOPE } from "../lib/contracts";
import type { RoleResolver } from "../lib/agents";
import { SPRITE_W } from "../lib/pixel";
import { activeOf, useCouncil } from "../store/council";
import { useConfig } from "../store/config";
import type { Phase, ScopeState } from "../store/session";
import { PixelCharacter, type Marker } from "./PixelCharacter";
import { CelebrationBanner, Confetti, NightShade, SkyWindow, useCelebration, useDayPhase, type DayPhase } from "./scene";
import { cx } from "./ui";

const W = 240;
const ROOF = 16;
const FLOOR_H = 46;
const SHAFT_X = 214;
const SHAFT_W = 26;
const ROOMS_W = SHAFT_X - 2;
const CAR_W = 22;
const CAR_H = 32;
const RIDE_MS = 1_700;
const ARRIVED_MS = 3_200;
const MAX_LOBBY = 7;

interface Room {
  id: string;
  /** Tầng tính từ trên xuống (0 = họp chính). */
  floor: number;
  x: number;
  w: number;
  scope: ScopeState | null;
}

const pct = (v: number, total: number) => `${(v / total) * 100}%`;
const floorY = (floor: number) => ROOF + floor * FLOOR_H;

const LIGHT: Partial<Record<Phase, string>> = {
  running: "#38bdf8",
  converged: "#22c55e",
  partially_approved: "#22c55e",
  failed: "#ef4444",
  cancelled: "#94a3b8",
  interrupted: "#f59e0b",
  max_rounds_reached: "#f97316",
  truncated: "#f97316",
  budget_exhausted: "#f97316",
  unresolved: "#f97316",
};

function members(sc: ScopeState): RoleId[] {
  return [...(sc.authorId ? [sc.authorId] : []), ...sc.reviewerIds.filter((id) => id !== sc.authorId)];
}

function markerOf(sc: ScopeState, id: RoleId): Marker {
  const a = sc.agents[id];
  if (!a) return null;
  if (a.status === "error") return "error";
  if (id === sc.authorId) return null;
  if (a.status === "done" && a.review && a.review.verdict !== "ABSTAIN") return a.review.verdict === "APPROVE" ? "approve" : "reject";
  return null;
}

/** Xếp `n` người vào bề ngang `w` bắt đầu từ `x` (chồng nhẹ khi đông). */
function spread(n: number, x: number, w: number): number[] {
  const step = Math.min(SPRITE_W + 2, (w - 12) / Math.max(n, 1));
  const start = x + (w - step * n) / 2 + (step - SPRITE_W) / 2;
  return Array.from({ length: n }, (_, i) => Math.round(start + i * step));
}

interface Ride {
  lead: RoleId;
  dept: string;
  from: number;
  stage: "fetch" | "carry" | "arrived";
}

export const Building = memo(function Building({
  resolve,
  mainOverride,
  activeId,
  sessionKey,
  onEnter,
  onRoomView,
}: {
  resolve: RoleResolver;
  /** Cuộc họp chính dùng để vẽ (vd. bản xem trước khi chưa có phiên). */
  mainOverride: ScopeState | null;
  /** Nơi họp đang được theo dõi (thang máy đỗ ở tầng đó khi rảnh). */
  activeId: string;
  sessionKey: string;
  onEnter: (scopeId: string) => void;
  onRoomView: () => void;
}) {
  const { scopeOrder, scopes, status } = useCouncil(
    useShallow((s) => {
      const a = activeOf(s);
      return { scopeOrder: a.scopeOrder, scopes: a.scopes, status: a.status };
    }),
  );
  const roles = useConfig((s) => s.config?.roles);
  const phase = useDayPhase();

  const main = mainOverride ?? scopes[MAIN_SCOPE] ?? null;
  const depts = scopeOrder.filter((id) => id !== MAIN_SCOPE).map((id) => scopes[id]).filter((s): s is ScopeState => !!s);
  const deptFloors = Math.ceil(depts.length / 2);
  const lobbyFloor = 1 + deptFloors;
  const H = floorY(lobbyFloor + 1) + 4;

  const rooms: Room[] = [
    { id: MAIN_SCOPE, floor: 0, x: 2, w: ROOMS_W - 2, scope: main },
    ...depts.map((sc, i) => {
      const half = (ROOMS_W - 4) / 2;
      const alone = i === depts.length - 1 && depts.length % 2 === 1;
      return { id: sc.id, floor: 1 + Math.floor(i / 2), x: alone ? 2 : 2 + (i % 2) * (half + 2), w: alone ? ROOMS_W - 2 : half, scope: sc };
    }),
  ];
  const floorOf = (id: string) => rooms.find((r) => r.id === id)?.floor ?? 0;

  // Role rảnh ở sảnh: có trong cấu hình nhưng không ngồi ở nơi họp nào.
  const busy = new Set(rooms.flatMap((r) => (r.scope ? members(r.scope) : [])));
  const idle = (roles ?? []).map((r) => r.id).filter((id) => !busy.has(id)).slice(0, MAX_LOBBY);

  // ── Thang máy: chở trưởng phòng lên họp chính khi phòng ban xong ──
  const [ride, setRide] = useState<Ride | null>(null);
  const queue = useRef<Ride[]>([]);
  // Lấy khỏi hàng đợi NGOÀI updater của setState (StrictMode gọi updater 2 lần → mất chuyến).
  const riding = useRef(false);
  riding.current = ride !== null;
  const prevStatus = useRef<{ key: string; map: Map<string, Phase> } | null>(null);
  const deptStatus = depts.map((d) => `${d.id}:${d.status}`).join("|");
  useEffect(() => {
    const cur = new Map(depts.map((d) => [d.id, d.status]));
    const prev = prevStatus.current;
    prevStatus.current = { key: sessionKey, map: cur };
    if (!prev || prev.key !== sessionKey || status !== "running") return;
    for (const d of depts) {
      if (prev.map.get(d.id) === "running" && d.status !== "running" && d.authorId) {
        queue.current.push({ lead: d.authorId, dept: d.name, from: floorOf(d.id), stage: "fetch" });
      }
    }
    if (!riding.current) {
      const next = queue.current.shift();
      if (next) setRide(next);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `deptStatus` tóm tắt trạng thái phòng ban
  }, [deptStatus, sessionKey, status]);
  useEffect(() => {
    if (!ride) return;
    const t = setTimeout(
      () => {
        if (ride.stage === "fetch") setRide({ ...ride, stage: "carry" });
        else if (ride.stage === "carry") setRide({ ...ride, stage: "arrived" });
        else setRide(queue.current.shift() ?? null);
      },
      ride.stage === "arrived" ? ARRIVED_MS : RIDE_MS,
    );
    return () => clearTimeout(t);
  }, [ride]);
  useEffect(() => {
    queue.current = [];
    setRide(null);
  }, [sessionKey]);

  const carFloor = ride ? (ride.stage === "fetch" ? ride.from : 0) : floorOf(activeId);
  const carY = floorY(carFloor) + FLOOR_H - 2 - CAR_H;
  const celebrating = useCelebration(main?.status ?? "idle", sessionKey);

  return (
    <section
      aria-label="Toà nhà PlanCouncil"
      data-scene
      className="relative shrink-0 overflow-hidden rounded-xl border border-neutral-800 bg-[#14121f]"
      style={{ aspectRatio: `${W} / ${H}` }}
    >
      <svg className="px-scene absolute inset-0 size-full" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="xMidYMid meet" shapeRendering="crispEdges" aria-hidden>
        <Facade h={H} />
        {rooms.map((r) => (
          <RoomBox key={r.id} room={r} phase={phase} resolve={resolve} viewing={r.id === activeId} celebrating={celebrating && r.id === MAIN_SCOPE} />
        ))}
        <Lobby floor={lobbyFloor} idle={idle} resolve={resolve} phase={phase} />
        {/* giếng thang + cabin (đi tới tầng bằng CSS transition) */}
        <rect x={SHAFT_X} y={ROOF} width={SHAFT_W} height={H - ROOF - 4} fill="#1f1b2e" />
        <rect x={SHAFT_X + SHAFT_W / 2 - 1} y={ROOF} width={1} height={H - ROOF - 4} fill="#3f3a56" />
        {Array.from({ length: lobbyFloor + 1 }, (_, f) => (
          <rect key={f} x={SHAFT_X} y={floorY(f) + FLOOR_H - 2} width={SHAFT_W} height={2} fill="#0b0b12" />
        ))}
        <g className="px-move-slow" style={{ transform: `translate(${SHAFT_X + (SHAFT_W - CAR_W) / 2}px, ${carY}px)` }}>
          <rect x={0} y={0} width={CAR_W} height={CAR_H} fill="#94a3b8" />
          <rect x={1} y={3} width={CAR_W - 2} height={CAR_H - 4} fill={ride?.stage === "carry" ? "#fef3c7" : "#475569"} />
          <rect x={CAR_W / 2 - 3} y={0} width={6} height={2} fill={ride ? "#22c55e" : "#334155"} />
          {ride?.stage === "carry" && (
            <PixelCharacter meta={resolve(ride.lead)} x={(CAR_W - SPRITE_W) / 2} y={CAR_H - 25} status="idle" speaking={false} marker={null} />
          )}
          {ride?.stage !== "carry" && <rect x={CAR_W / 2 - 0.5} y={3} width={1} height={CAR_H - 4} fill="#334155" />}
        </g>
        <NightShade w={W} h={H} phase={phase} />
      </svg>

      {/* Biển tên + vùng bấm của từng phòng (HTML cho chữ sắc nét + bàn phím truy cập được) */}
      {rooms.map((r) => {
        const sc = r.scope;
        const y = floorY(r.floor);
        const name = r.id === MAIN_SCOPE ? "Họp chính" : (sc?.name ?? r.id);
        const round = sc && sc.currentRound > 0 ? ` · R${sc.currentRound}/${sc.maxRounds}` : "";
        return (
          <button
            key={r.id}
            type="button"
            onClick={() => onEnter(r.id)}
            aria-label={`Vào phòng ${name}`}
            title={sc?.task ?? `Vào phòng ${name}`}
            className="px-room absolute cursor-pointer text-left focus-visible:outline-none"
            style={{ left: pct(r.x, W), top: pct(y + 2, H), width: pct(r.w, W), height: pct(FLOOR_H - 4, H) }}
          >
            <span className="px-room-hl pointer-events-none absolute inset-0 rounded-[2px] opacity-0 ring-2 ring-sky-300/80 transition-opacity" />
            <span
              className={cx(
                "px-font pointer-events-none absolute top-0.5 left-1 max-w-[92%] truncate rounded-[1px] px-1 text-[9px] leading-[13px] font-semibold",
                r.id === activeId ? "bg-amber-300 text-neutral-950" : "bg-black/60 text-neutral-200",
              )}
            >
              {name}
              {round}
            </span>
          </button>
        );
      })}
      <div
        className="px-font pointer-events-none absolute left-1 rounded-[1px] bg-black/60 px-1 text-[9px] leading-[13px] text-neutral-300"
        style={{ top: pct(floorY(lobbyFloor) + 3, H) }}
      >
        Sảnh{idle.length ? ` · ${idle.length} người rảnh` : ""}
      </div>

      {ride?.stage === "arrived" && (
        <div
          className="px-bubble px-box px-font pointer-events-none absolute z-10 max-w-[70%] -translate-y-full rounded-[1px] bg-[#fdfdf8] px-2 py-1 text-[11px] leading-snug text-neutral-900"
          style={{ right: pct(SHAFT_W + 6, W), top: pct(floorY(0) + 14, H), ["--px-border" as string]: "#16a34a" }}
        >
          🛗 <b>{resolve(ride.lead).name}</b> mang plan {ride.dept} lên Họp chính
        </div>
      )}

      <button
        type="button"
        onClick={onRoomView}
        className="px-font absolute right-2 bottom-1.5 z-10 inline-flex items-center gap-1 rounded-sm bg-black/55 px-2 py-0.5 text-[10px] text-neutral-200 hover:bg-black/75 hover:text-white"
        title="Quay lại phòng đang theo dõi"
      >
        <DoorOpen className="size-3" aria-hidden /> VÀO PHÒNG
      </button>
      {celebrating && (
        <>
          <Confetti pieces={48} />
          <CelebrationBanner text="HỘI TỤ!" />
        </>
      )}
    </section>
  );
});

/** Mái + biển hiệu + nền tường ngoài. */
const Facade = memo(function Facade({ h }: { h: number }) {
  return (
    <g>
      <rect x={0} y={0} width={W} height={h} fill="#14121f" />
      <rect x={0} y={ROOF - 4} width={W} height={4} fill="#3f3a56" />
      <rect x={70} y={1} width={100} height={10} fill="#4c1d95" />
      <rect x={71} y={2} width={98} height={8} fill="#6d28d9" />
      <text x={W / 2} y={8.5} textAnchor="middle" fontSize={6} fontFamily="monospace" fontWeight="bold" fill="#fde68a">
        PLANCOUNCIL HQ
      </text>
      <rect x={16} y={4} width={2} height={7} fill="#64748b" />
      <rect x={14} y={2} width={6} height={2} fill="#ef4444" className="px-dot" />
    </g>
  );
});

const RoomBox = memo(function RoomBox({
  room,
  phase,
  resolve,
  viewing,
  celebrating,
}: {
  room: Room;
  phase: DayPhase;
  resolve: RoleResolver;
  viewing: boolean;
  celebrating: boolean;
}) {
  const { x, w, scope } = room;
  const y = floorY(room.floor);
  const ids = scope ? members(scope) : [];
  const xs = spread(ids.length, x + 4, w - 8);
  const thinking = !!scope && Object.values(scope.agents).some((a) => a.status === "thinking");
  const light = scope ? (LIGHT[scope.status] ?? "#475569") : "#475569";
  const tableX = x + Math.round(w * 0.18);
  const tableW = Math.round(w * 0.64);
  return (
    <g>
      {/* tường + sàn */}
      <rect x={x} y={y + 2} width={w} height={FLOOR_H - 4} fill={viewing ? "#352f4d" : "#2b2740"} />
      <rect x={x} y={y + 30} width={w} height={14} fill="#3a3352" />
      {Array.from({ length: Math.floor(w / 8) }, (_, i) => (i % 2 === 0 ? <rect key={i} x={x + i * 8} y={y + 30} width={8} height={7} fill="#332d48" /> : null))}
      <rect x={x} y={y + FLOOR_H - 2} width={w} height={2} fill="#0b0b12" />
      <SkyWindow x={x + w - 24} y={y + 8} w={18} h={14} phase={phase} />
      {/* đèn trạng thái phòng */}
      <rect x={x + w - 31} y={y + 9} width={4} height={4} fill="#0b0b12" />
      <rect x={x + w - 30} y={y + 10} width={2} height={2} fill={thinking ? "#fbbf24" : light} className={thinking || scope?.status === "running" ? "px-dot" : undefined} />
      {ids.map((id, i) => {
        const a = scope?.agents[id];
        return (
          <PixelCharacter
            key={id}
            meta={resolve(id)}
            x={xs[i] ?? x}
            y={y + 17}
            status={a?.status ?? "idle"}
            speaking={false}
            marker={scope ? markerOf(scope, id) : null}
            celebrating={celebrating}
            typing={id === scope?.authorId && a?.status === "thinking"}
          />
        );
      })}
      {/* bàn họp che chân → như đang ngồi quanh bàn */}
      {ids.length > 0 && (
        <>
          <rect x={tableX} y={y + 33} width={tableW} height={4} fill="#a86b3c" />
          <rect x={tableX} y={y + 37} width={tableW} height={3} fill="#7a4a26" />
        </>
      )}
    </g>
  );
});

const Lobby = memo(function Lobby({ floor, idle, resolve, phase }: { floor: number; idle: RoleId[]; resolve: RoleResolver; phase: DayPhase }) {
  const y = floorY(floor);
  const xs = spread(idle.length, 60, ROOMS_W - 64);
  return (
    <g>
      <rect x={2} y={y + 2} width={ROOMS_W - 2} height={FLOOR_H - 4} fill="#262236" />
      <rect x={2} y={y + 30} width={ROOMS_W - 2} height={14} fill="#4b3f2a" />
      <rect x={2} y={y + FLOOR_H - 2} width={ROOMS_W - 2} height={2} fill="#0b0b12" />
      {/* máy cà phê */}
      <rect x={8} y={y + 18} width={12} height={20} fill="#475569" />
      <rect x={9} y={y + 20} width={10} height={6} fill="#1e293b" />
      <rect x={10} y={y + 21} width={3} height={2} fill="#ef4444" className="px-dot" />
      <rect x={12} y={y + 30} width={4} height={4} fill="#f8fafc" />
      <rect x={13} y={y + 27} width={1} height={2} fill="#e2e8f0" className="px-dot" />
      {/* sofa */}
      <rect x={26} y={y + 30} width={26} height={8} fill="#7c3aed" />
      <rect x={26} y={y + 26} width={26} height={5} fill="#6d28d9" />
      {/* cây */}
      <rect x={ROOMS_W - 14} y={y + 32} width={8} height={8} fill="#92400e" />
      <rect x={ROOMS_W - 15} y={y + 22} width={10} height={10} fill="#15803d" />
      <rect x={ROOMS_W - 13} y={y + 18} width={6} height={5} fill="#22c55e" />
      <SkyWindow x={ROOMS_W - 42} y={y + 8} w={22} h={14} phase={phase} />
      {idle.map((id, i) => (
        <PixelCharacter key={id} meta={resolve(id)} x={xs[i] ?? 60} y={y + 17} status="idle" speaking={false} marker={null} dimmed />
      ))}
    </g>
  );
});
