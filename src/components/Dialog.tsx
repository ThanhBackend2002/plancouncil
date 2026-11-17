// Hộp thoại modal dùng chung: role=dialog + aria-modal, tiêu đề gắn aria-labelledby, bẫy focus
// (Tab/Shift+Tab xoay vòng trong hộp thoại), Esc để đóng, trả focus về phần tử cũ khi đóng.

import { useEffect, useId, useRef, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { cx } from "./ui";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  initialFocusRef,
  tone = "neutral",
  className,
}: {
  open: boolean;
  /** Gọi khi bấm Esc, nút đóng hoặc nền mờ. */
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  /** Hàng nút hành động (căn phải). */
  footer?: ReactNode;
  /** Phần tử nhận focus khi mở (mặc định: phần tử focus được đầu tiên). */
  initialFocusRef?: RefObject<HTMLElement | null>;
  tone?: "neutral" | "warning" | "danger";
  className?: string;
}) {
  const titleId = useId();
  const descId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusables = () => Array.from(panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    (initialFocusRef?.current ?? focusables()[0] ?? panel.current)?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (e.key !== "Tab") return;
      const list = focusables();
      const first = list[0];
      const last = list[list.length - 1];
      if (!first || !last) {
        e.preventDefault();
        panel.current?.focus();
        return;
      }
      const inside = panel.current?.contains(document.activeElement);
      if (e.shiftKey && (document.activeElement === first || !inside)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (document.activeElement === last || !inside)) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      if (previous && previous.isConnected) previous.focus();
    };
  }, [open, initialFocusRef]);

  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/60" aria-hidden="true" onMouseDown={() => onCloseRef.current()} />
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descId : undefined}
        tabIndex={-1}
        className={cx(
          "relative w-full rounded-xl border bg-surface p-5 shadow-2xl shadow-black/60 focus:outline-none",
          // `className` tự đặt max-w-* → bỏ mặc định (2 class max-w cùng lúc thì class nào thắng tuỳ thứ tự CSS).
          !/(^|\s)max-w-/.test(className ?? "") && "max-w-lg",
          tone === "warning" ? "border-warn-line" : tone === "danger" ? "border-danger-line" : "border-line-strong",
          className,
        )}
      >
        <div className="mb-3 flex items-start gap-3">
          <h2 id={titleId} className="min-w-0 flex-1 text-sm font-semibold text-fg">
            {title}
          </h2>
          <button
            type="button"
            onClick={() => onCloseRef.current()}
            className="rounded p-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
            aria-label="Đóng"
          >
            <X className="size-4" />
          </button>
        </div>
        {description && (
          <div id={descId} className="mb-3 text-xs text-fg-muted">
            {description}
          </div>
        )}
        {children}
        {footer && <div className="mt-4 flex flex-wrap justify-end gap-2">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}
