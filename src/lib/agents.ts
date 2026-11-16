import {
  BadgeCheck,
  Brain,
  Bug,
  Calendar,
  ChartColumn,
  ClipboardList,
  Cloud,
  Coins,
  Compass,
  Cpu,
  Crown,
  Database,
  FlaskConical,
  Gauge,
  ListChecks,
  Megaphone,
  Monitor,
  Palette,
  Radio,
  Scale,
  Search,
  Server,
  Settings,
  Shield,
  ShieldExclamationPoint,
  Smartphone,
  Target,
  User,
  Users,
  Wallet,
  Workflow,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import type { Appearance, CastMember, RoleDef, RoleId, RoleLevel, SessionStatus } from "./contracts";

/** Icon key (lưu trong RoleDef.icon) → component lucide. */
export const ROLE_ICONS: Record<string, LucideIcon> = {
  crown: Crown,
  cpu: Cpu,
  target: Target,
  wallet: Wallet,
  settings: Settings,
  shield: Shield,
  users: Users,
  database: Database,
  clipboard: ClipboardList,
  calendar: Calendar,
  search: Search,
  megaphone: Megaphone,
  coins: Coins,
  radio: Radio,
  compass: Compass,
  wrench: Wrench,
  "badge-check": BadgeCheck,
  gauge: Gauge,
  "shield-alert": ShieldExclamationPoint,
  server: Server,
  monitor: Monitor,
  smartphone: Smartphone,
  cloud: Cloud,
  workflow: Workflow,
  brain: Brain,
  flask: FlaskConical,
  "list-checks": ListChecks,
  bug: Bug,
  palette: Palette,
  chart: ChartColumn,
  scale: Scale,
  user: User,
};

export const LEVELS: { id: RoleLevel; label: string }[] = [
  { id: "c_level", label: "C-Level" },
  { id: "head", label: "Head / Director" },
  { id: "manager", label: "Manager" },
  { id: "lead", label: "Lead / Architect" },
  { id: "engineer", label: "Engineer" },
  { id: "specialist", label: "Specialist / Staff" },
];
export const LEVEL_LABEL: Record<RoleLevel, string> = Object.fromEntries(LEVELS.map((l) => [l.id, l.label])) as Record<
  RoleLevel,
  string
>;

export interface RoleMeta {
  id: RoleId;
  name: string;
  title: string;
  color: string;
  icon: LucideIcon;
  iconKey: string;
  model: string;
  level: RoleLevel;
  appearance?: Appearance;
}

/** Phiên cũ (trước khi có role động) không có cast → dùng bảng này. */
const LEGACY: Record<string, Omit<RoleMeta, "id" | "model">> = {
  architect: { name: "Architect", title: "Solution Architect", color: "#a78bfa", icon: Compass, iconKey: "compass", level: "lead" },
  scale_reviewer: { name: "Scale", title: "Scale & Performance Reviewer", color: "#38bdf8", icon: Gauge, iconKey: "gauge", level: "engineer" },
  chaos_reviewer: {
    name: "Chaos & Security",
    title: "Chaos & Security Reviewer",
    color: "#f472b6",
    icon: ShieldExclamationPoint,
    iconKey: "shield-alert",
    level: "engineer",
  },
  pragmatic_reviewer: { name: "Pragmatic", title: "Pragmatic Tech Lead", color: "#fbbf24", icon: Wrench, iconKey: "wrench", level: "lead" },
};

export type RoleResolver = (id: RoleId) => RoleMeta;

/** Ưu tiên ảnh chụp trong phiên (cast) → role hiện tại trong config → bảng legacy → mặc định. */
export function makeResolver(cast: CastMember[], roles: RoleDef[]): RoleResolver {
  const byCast = new Map(cast.map((c) => [c.id, c]));
  const byRole = new Map(roles.map((r) => [r.id, r]));
  const cache = new Map<RoleId, RoleMeta>();
  return (id) => {
    const hit = cache.get(id);
    if (hit) return hit;
    const c = byCast.get(id) ?? byRole.get(id);
    const legacy = LEGACY[id];
    const meta: RoleMeta = c
      ? {
          id,
          name: c.name || id,
          title: c.title,
          color: c.color || "#a1a1aa",
          icon: ROLE_ICONS[c.icon] ?? User,
          iconKey: c.icon,
          model: c.model,
          level: c.level ?? "engineer",
          appearance: c.appearance,
        }
      : legacy
        ? { id, model: "", ...legacy }
        : { id, name: id, title: "", color: "#a1a1aa", icon: User, iconKey: "user", model: "", level: "engineer" };
    cache.set(id, meta);
    return meta;
  };
}

export type Tone = "neutral" | "info" | "success" | "warning" | "danger";

export const STATUS_META: Record<SessionStatus | "idle", { label: string; tone: Tone }> = {
  idle: { label: "Sẵn sàng", tone: "neutral" },
  running: { label: "Đang họp", tone: "info" },
  converged: { label: "Hội tụ", tone: "success" },
  partially_approved: { label: "Duyệt một phần", tone: "warning" },
  max_rounds_reached: { label: "Hết số vòng", tone: "warning" },
  truncated: { label: "Bị cắt", tone: "warning" },
  budget_exhausted: { label: "Hết ngân sách", tone: "warning" },
  unresolved: { label: "Chưa giải quyết", tone: "warning" },
  interrupted: { label: "Bị gián đoạn", tone: "neutral" },
  cancelled: { label: "Đã dừng", tone: "neutral" },
  failed: { label: "Lỗi", tone: "danger" },
};

export const TONE_CLASS: Record<Tone, string> = {
  neutral: "bg-surface-2 text-fg-soft ring-line-strong",
  info: "bg-info-subtle text-info ring-info-line",
  success: "bg-success-subtle text-success ring-success-line",
  warning: "bg-warn-subtle text-warn ring-warn-line",
  danger: "bg-danger-subtle text-danger ring-danger-line",
};

export const ERROR_TITLE: Record<string, string> = {
  invalid: "Dữ liệu không hợp lệ",
  already_running: "Đang có phiên chạy",
  not_found: "Không tìm thấy",
  storage: "Lỗi lưu trữ",
  llm: "Lỗi LLM",
  internal: "Lỗi nội bộ",
};

/** 950 → "950", 12_345 → "12.3k", 1_234_567 → "1.23M" */
export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 2 : 1).replace(/\.?0+$/, "")}k`;
  return `${(n / 1_000_000).toFixed(2).replace(/\.?0+$/, "")}M`;
}

export const USAGE_KIND_LABEL: Record<string, string> = {
  assign: "Giao việc",
  research: "Nghiên cứu (công cụ)",
  plan: "Viết plan",
  review: "Phản biện",
  staffing: "Chọn thành phần họp",
};

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("vi-VN", { dateStyle: "short", timeStyle: "short" });
}
