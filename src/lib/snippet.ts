// Tách đoạn trích của `search_history` (chỗ khớp bọc `[[` … `]]`) thành các phần text / hit
// để render bằng React node — KHÔNG dùng dangerouslySetInnerHTML.

export const HIT_OPEN = "[[";
export const HIT_CLOSE = "]]";

export interface SnippetPart {
  text: string;
  hit: boolean;
}

/** `[[` không có `]]` đi sau → giữ nguyên là text thường. Bỏ phần rỗng; gộp text liền kề. */
export function splitSnippet(snippet: string): SnippetPart[] {
  const out: SnippetPart[] = [];
  const push = (text: string, hit: boolean) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.hit === hit && !hit) last.text += text;
    else out.push({ text, hit });
  };
  let i = 0;
  while (i < snippet.length) {
    const open = snippet.indexOf(HIT_OPEN, i);
    if (open === -1) break;
    const close = snippet.indexOf(HIT_CLOSE, open + HIT_OPEN.length);
    if (close === -1) break;
    push(snippet.slice(i, open), false);
    push(snippet.slice(open + HIT_OPEN.length, close), true);
    i = close + HIT_CLOSE.length;
  }
  push(snippet.slice(i), false);
  return out;
}
