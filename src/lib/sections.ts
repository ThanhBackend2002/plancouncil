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

const H2 = /^##\s+(.+?)\s*#*\s*$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;

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
  let inFence = false;
  const flush = () => {
    const body = cur.lines.join("\n").replace(/^\n+|\s+$/g, "");
    if (cur.title || body) out.push({ key: cur.title ? sectionKey(cur.title) : "__intro", title: cur.title || "(Mở đầu)", body });
  };
  for (const line of md.split("\n")) {
    if (FENCE.test(line)) inFence = !inFence;
    const m = !inFence ? H2.exec(line) : null;
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
