import { memo, useMemo } from "react";
import { makeSprite, resolveLook, SPRITE_W, type Rect } from "../lib/pixel";
import type { RoleMeta } from "../lib/agents";
import type { AgentStatus } from "../store/session";

function Rects({ rects }: { rects: Rect[] }) {
  return (
    <>
      {rects.map((r, i) => (
        <rect key={i} x={r.x} y={r.y} width={r.w} height={r.h} fill={r.fill} />
      ))}
    </>
  );
}

const delayOf = (id: string) => {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return `${-(h % 1200)}ms`;
};

export type Marker = "approve" | "reject" | "error" | null;

/**
 * Nhân vật trong cảnh SVG. (x, y) = góc trên-trái sprite, đơn vị pixel logic của cảnh.
 * Vị trí đặt bằng CSS transform + transition → đổi (x, y) là nhân vật "đi bộ" tới chỗ mới.
 */
export const PixelCharacter = memo(function PixelCharacter({
  meta,
  x,
  y,
  status,
  speaking,
  marker,
  dimmed,
  walking,
  celebrating,
  typing,
}: {
  meta: RoleMeta;
  x: number;
  y: number;
  status: AgentStatus;
  speaking: boolean;
  marker: Marker;
  dimmed?: boolean;
  /** Đang đi (bước nhanh, không hiện dấu trên đầu). */
  walking?: boolean;
  /** Ăn mừng: nhảy lên, giơ hai tay. */
  celebrating?: boolean;
  /** Đang viết plan: laptop trước mặt. */
  typing?: boolean;
}) {
  const sprite = useMemo(
    () => makeSprite({ id: meta.id, color: meta.color, level: meta.level, icon: meta.iconKey, appearance: meta.appearance }),
    [meta.id, meta.color, meta.level, meta.iconKey, meta.appearance],
  );
  const look = useMemo(
    () => resolveLook({ id: meta.id, color: meta.color, level: meta.level, icon: meta.iconKey, appearance: meta.appearance }),
    [meta.id, meta.color, meta.level, meta.iconKey, meta.appearance],
  );
  const thinking = status === "thinking";
  const delay = delayOf(meta.id);
  const bob = celebrating ? "px-jump" : thinking || walking ? "px-bob-fast" : "px-bob";
  // REJECT = giơ tay phát biểu; ăn mừng = giơ cả hai tay.
  const raise = celebrating ? "both" : !thinking && !walking && marker === "reject" ? "right" : null;

  return (
    <g className="px-move" style={{ transform: `translate(${x}px, ${y}px)` }} opacity={dimmed ? 0.55 : 1}>
      {/* bóng dưới chân */}
      <rect x={3} y={23} width={10} height={1} fill="#000" opacity={0.25} />
      <g className={bob} style={{ animationDelay: celebrating ? `${Math.abs(parseInt(delay)) % 300}ms` : delay }}>
        <Rects rects={sprite.body} />
        {raise && <RaisedArm skin={look.skin} sleeve={look.outfitColor} side="right" />}
        {raise === "both" && <RaisedArm skin={look.skin} sleeve={look.outfitColor} side="left" />}
        <g className="px-blink" style={{ animationDelay: delay }}>
          <Rects rects={sprite.blink} />
        </g>
        {speaking && (
          <g className="px-talk">
            <Rects rects={sprite.mouthOpen} />
          </g>
        )}
        {thinking && !typing && <ThinkingCloud />}
        {!thinking && !walking && !celebrating && marker && <MarkerIcon marker={marker} />}
      </g>
      {typing && <Laptop />}
    </g>
  );
});

/** Cánh tay giơ lên cạnh vai (sprite rộng 16: thân ~x 3..12). */
function RaisedArm({ skin, sleeve, side }: { skin: string; sleeve: string; side: "left" | "right" }) {
  const x = side === "right" ? 13 : 1;
  return (
    <g className="px-wave">
      <rect x={x} y={9} width={2} height={5} fill={sleeve} />
      <rect x={x} y={6} width={2} height={3} fill={skin} />
      <rect x={x} y={5} width={2} height={1} fill={skin} opacity={0.85} />
    </g>
  );
}

/** Laptop đặt trước người viết plan: màn hình nhấp nháy như đang gõ. */
function Laptop() {
  return (
    <g transform="translate(12 13)">
      <rect x={0} y={0} width={9} height={6} fill="#334155" />
      <rect x={1} y={1} width={7} height={4} fill="#38bdf8" />
      <rect x={2} y={2} width={4} height={1} fill="#e0f2fe" className="px-dot" />
      <rect x={-1} y={6} width={11} height={1} fill="#1e293b" />
    </g>
  );
}

/** Đám mây "..." phía trên đầu khi đang suy nghĩ. */
function ThinkingCloud() {
  return (
    <g transform={`translate(${SPRITE_W / 2 - 6} -9)`}>
      <rect x={1} y={0} width={10} height={6} fill="#f8fafc" />
      <rect x={0} y={1} width={12} height={4} fill="#f8fafc" />
      <rect x={4} y={6} width={2} height={1} fill="#f8fafc" />
      <rect x={3} y={8} width={1} height={1} fill="#f8fafc" />
      {[0, 1, 2].map((i) => (
        <rect key={i} className="px-dot" style={{ animationDelay: `${i * 180}ms` }} x={2 + i * 3} y={2} width={2} height={2} fill="#6d28d9" />
      ))}
    </g>
  );
}

const CHECK = ["....G", "...GG", "G.GG.", "GGG..", ".G..."];
const CROSS = ["R...R", ".R.R.", "..R..", ".R.R.", "R...R"];
const BANG = ["..A..", "..A..", "..A..", ".....", "..A.."];

function MarkerIcon({ marker }: { marker: Exclude<Marker, null> }) {
  const [tpl, color] =
    marker === "approve" ? [CHECK, "#22c55e"] : marker === "reject" ? [CROSS, "#ef4444"] : [BANG, "#f59e0b"];
  return (
    <g transform={`translate(${SPRITE_W / 2 - 3.5} -8)`}>
      <rect x={-1} y={-1} width={7} height={7} fill="#0b0b12" opacity={0.75} />
      {tpl.flatMap((row, yy) =>
        [...row].map((ch, xx) => (ch === "." ? null : <rect key={`${xx}-${yy}`} x={xx} y={yy} width={1} height={1} fill={color} />)),
      )}
    </g>
  );
}

/** Chân dung pixel (đầu + vai) dùng trong khung hội thoại. */
export const PixelPortrait = memo(function PixelPortrait({
  meta,
  size = 28,
  full,
}: {
  meta: RoleMeta;
  size?: number;
  /** Toàn thân (16×24) thay vì chỉ đầu + vai. `size` = chiều rộng. */
  full?: boolean;
}) {
  const sprite = useMemo(
    () => makeSprite({ id: meta.id, color: meta.color, level: meta.level, icon: meta.iconKey, appearance: meta.appearance }),
    [meta.id, meta.color, meta.level, meta.iconKey, meta.appearance],
  );
  return (
    <svg
      width={size}
      height={full ? size * 1.5 : size}
      viewBox={full ? "0 0 16 24" : "0 0 16 16"}
      shapeRendering="crispEdges"
      className="shrink-0 rounded-sm"
      style={{ background: full ? undefined : `${meta.color}26`, imageRendering: "pixelated" }}
      aria-hidden
    >
      <Rects rects={full ? sprite.body : sprite.body.filter((r) => r.y < 16)} />
    </svg>
  );
});
