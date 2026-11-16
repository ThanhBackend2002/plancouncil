// Suy ra trạng thái từng section (heading ## / ###) của 1 version plan từ các round review.
// Hàm thuần, không phụ thuộc React/store để dễ test.

import type { Review } from "./contracts";
import type { SectionPair } from "./sections";

export type SectionStatusKind = "changes_requested" | "approved" | "pending";

export interface SectionBadgeInfo {
  status: SectionStatusKind;
  /** Số review REJECT nhắc tới section (chỉ có nghĩa với changes_requested). */
  count: number;
  /** Section đổi so với version trước (chỉ gán cho heading `##`). */
  changed: boolean;
}

/** Phần tối thiểu của RoundView mà hàm cần (RoundView của store thoả mãn cấu trúc này). */
export interface RoundLike {
  planVersion: number;
  converged: boolean;
  completed?: boolean;
  slots: Record<string, { review: Pick<Review, "verdict" | "sections"> | null }>;
}

const LEADING_NUMBER = /^\s*(?:§\s*)?(\d+(?:\.\d+)*)\.?(?=\s|$|[:\-–—)])/;

/** Số hiệu đầu heading: "3.3 API" → "3.3", "2. Kiến trúc" → "2"; không có số → null. */
export function headingNumber(heading: string): string | null {
  return LEADING_NUMBER.exec(heading)?.[1] ?? null;
}

/** Chuẩn hoá số hiệu do reviewer trả ("§3.3", " 3.3. ") về dạng "3.3". */
export function normalizeNumber(raw: string): string {
  return raw.trim().replace(/^§\s*/, "").replace(/\.+$/, "");
}

/**
 * 2 số hiệu khớp nếu bằng nhau hoặc 1 bên là tiền tố theo ranh giới dấu chấm
 * ("3" phủ "3.3" và ngược lại, nhưng "3" không phủ "30").
 */
export function numbersMatch(a: string, b: string): boolean {
  const x = normalizeNumber(a);
  const y = normalizeNumber(b);
  if (!x || !y) return false;
  return x === y || x.startsWith(`${y}.`) || y.startsWith(`${x}.`);
}

/** Review có nhắc tới section `number` không. */
export function reviewMentions(review: Pick<Review, "sections"> | null | undefined, number: string): boolean {
  return !!review?.sections?.some((s) => numbersMatch(s, number));
}

/** Round dùng để đánh giá 1 version: round CUỐI có `planVersion` đó (nếu có nhiều). */
export function roundForVersion<R extends RoundLike>(rounds: R[], version: number): R | null {
  let found: R | null = null;
  for (const r of rounds) if (r.planVersion === version) found = r;
  return found;
}

/**
 * - changes_requested: có ≥ 1 review REJECT nhắc tới section (count = số review đó).
 * - approved: CHỈ khi round hội tụ (mọi reviewer APPROVE) — không suy ra duyệt từng section từ kết quả một phần.
 * - pending: còn lại (REJECT nhưng không nêu section, round chưa xong, version chưa có round…).
 * `numbers` là các số hiệu heading hiện có trong plan; kết quả có key = từng số đó.
 */
export function deriveSectionStatus(
  rounds: RoundLike[],
  version: number,
  numbers: Iterable<string>,
): Record<string, SectionBadgeInfo> {
  const round = roundForVersion(rounds, version);
  const rejects = round
    ? Object.values(round.slots)
        .map((s) => s.review)
        .filter((r): r is NonNullable<typeof r> => !!r && r.verdict === "REJECT")
    : [];
  const out: Record<string, SectionBadgeInfo> = {};
  for (const n of numbers) {
    const count = rejects.filter((r) => reviewMentions(r, n)).length;
    const status: SectionStatusKind =
      count > 0 ? "changes_requested" : round?.converged && round.completed !== false ? "approved" : "pending";
    out[n] = { status, count, changed: false };
  }
  return out;
}

/** Số hiệu các section `##` có nội dung đổi (sửa / thêm) giữa 2 version, từ kết quả `pairSections`. */
export function changedNumbers(pairs: SectionPair[]): Set<string> {
  const out = new Set<string>();
  for (const p of pairs) {
    if (p.status !== "modified" && p.status !== "added") continue;
    const n = headingNumber(p.titleB ?? "");
    if (n) out.add(n);
  }
  return out;
}

/** Các số hiệu heading `##`/`###` (ngoài code fence) trong 1 đoạn markdown. */
export function headingNumbers(md: string): string[] {
  const out: string[] = [];
  let fence: string | null = null;
  for (const line of md.split("\n")) {
    const f = /^ {0,3}(`{3,}|~{3,})/.exec(line);
    if (f) {
      if (!fence) fence = f[1]![0]!;
      else if (f[1]![0] === fence) fence = null;
      continue;
    }
    if (fence) continue;
    const m = /^ {0,3}#{2,3}\s+(.+?)\s*#*\s*$/.exec(line);
    const n = m ? headingNumber(m[1]!) : null;
    if (n) out.push(n);
  }
  return out;
}
