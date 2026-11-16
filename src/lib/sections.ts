// Tách plan theo heading `##` để Diff View so sánh từng section.

export interface Section {
  key: string;
  title: string;
  body: string;
}

export type SectionStatus = "added" | "removed" | "modified" | "unchanged";

export interface SectionPair {
  key: string;
  titleA: string | null;
  titleB: string | null;
  a: string;
  b: string;
  status: SectionStatus;
}

const H2 = /^ {0,3}##\s+(.+?)\s*#*\s*$/;
/** Fence mở (CommonMark): thụt ≤ 3 dấu cách, ≥ 3 dấu ` hoặc ~; info của fence ` không được chứa `. */
// Cờ `s`: `.` khớp cả `\r` (file CRLF tách theo `\n` vẫn còn `\r` cuối dòng).
const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})(.*)$/s;

interface Fence {
  ch: string;
  len: number;
}

/** Dòng mở fence → ký tự + độ dài; null nếu không phải fence mở. */
export function fenceOpen(line: string): Fence | null {
  const m = FENCE_OPEN.exec(line);
  if (!m) return null;
  const marker = m[1]!;
  const ch = marker[0]!;
  if (ch === "`" && m[2]!.includes("`")) return null;
  return { ch, len: marker.length };
}

/** Dòng đóng fence: cùng ký tự, dài ≥ fence mở, thụt ≤ 3 dấu cách, sau đó chỉ có khoảng trắng. */
export function closesFence(line: string, open: Fence): boolean {
  const m = /^ {0,3}(`+|~+)\s*$/.exec(line);
  return !!m && m[1]![0] === open.ch && m[1]!.length >= open.len;
}

/** Bỏ phần trong ngoặc (vd "Changelog (v1 → v2)") để heading khớp giữa các version. */
export function sectionKey(title: string): string {
  return title
    .replace(/\(.*?\)/g, "")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim()
    .toLowerCase();
}

export function splitSections(md: string): Section[] {
  const out: Section[] = [];
  let cur: { title: string; lines: string[] } = { title: "", lines: [] };
  let fence: Fence | null = null;
  const flush = () => {
    const body = cur.lines.join("\n").replace(/^\n+|\s+$/g, "");
    if (cur.title || body) out.push({ key: cur.title ? sectionKey(cur.title) : "__intro", title: cur.title || "(Mở đầu)", body });
  };
  for (const line of md.split("\n")) {
    let m: RegExpExecArray | null = null;
    if (fence) {
      if (closesFence(line, fence)) fence = null;
    } else {
      fence = fenceOpen(line);
      if (!fence) m = H2.exec(line);
    }
    if (m) {
      flush();
      cur = { title: m[1]!, lines: [] };
    } else {
      cur.lines.push(line);
    }
  }
  flush();

  // Heading trùng → thêm hậu tố để key duy nhất.
  const seen = new Map<string, number>();
  for (const s of out) {
    const n = seen.get(s.key) ?? 0;
    seen.set(s.key, n + 1);
    if (n > 0) s.key = `${s.key}#${n + 1}`;
  }
  return out;
}

/** Ghép section của A và B theo key, giữ thứ tự của B, chèn section chỉ có ở A vào đúng chỗ. */
export function pairSections(mdA: string, mdB: string): SectionPair[] {
  const A = splitSections(mdA);
  const B = splitSections(mdB);
  const mapA = new Map(A.map((s) => [s.key, s]));
  const mapB = new Map(B.map((s) => [s.key, s]));

  const order: string[] = B.map((s) => s.key);
  A.forEach((s, i) => {
    if (mapB.has(s.key)) return;
    let at = 0;
    for (let j = i - 1; j >= 0; j--) {
      const idx = order.indexOf(A[j]!.key);
      if (idx !== -1) {
        at = idx + 1;
        break;
      }
    }
    order.splice(at, 0, s.key);
  });

  return order.map((key) => {
    const a = mapA.get(key);
    const b = mapB.get(key);
    const status: SectionStatus = !a ? "added" : !b ? "removed" : a.body === b.body && a.title === b.title ? "unchanged" : "modified";
    return { key, titleA: a?.title ?? null, titleB: b?.title ?? null, a: a?.body ?? "", b: b?.body ?? "", status };
  });
}
