import { AnimatePresence, motion } from "motion/react";
import { CircleCheck, CircleX, Info, TriangleExclamationPoint, X } from "lucide-react";
import { useToast, type ToastTone } from "../store/toast";
import { cx } from "./ui";

const TONE: Record<ToastTone, { icon: typeof Info; cls: string }> = {
  info: { icon: Info, cls: "text-info" },
  success: { icon: CircleCheck, cls: "text-success" },
  warning: { icon: TriangleExclamationPoint, cls: "text-warn" },
  error: { icon: CircleX, cls: "text-danger" },
};

export function Toasts() {
  const { toasts, dismiss } = useToast();
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[380px] flex-col gap-2" aria-live="polite">
      <AnimatePresence initial={false}>
        {toasts.map((t) => {
          const { icon: Icon, cls } = TONE[t.tone];
          return (
            <motion.div
              key={t.id}
              layout
              initial={{ opacity: 0, y: 12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, x: 24 }}
              transition={{ duration: 0.18 }}
              className="pointer-events-auto flex gap-3 rounded-lg border border-line bg-surface/95 p-3 shadow-xl shadow-black/15 backdrop-blur dark:shadow-black/40"
              role={t.tone === "error" ? "alert" : "status"}
            >
              <Icon className={cx("mt-0.5 size-4 shrink-0", cls)} />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-fg">{t.title}</div>
                {t.message && (
                  <div className="mt-0.5 line-clamp-4 text-xs break-words text-fg-muted">{t.message}</div>
                )}
              </div>
              <button
                type="button"
                onClick={() => dismiss(t.id)}
                className="shrink-0 self-start rounded p-0.5 text-fg-subtle hover:text-fg-soft"
                aria-label="Đóng thông báo"
              >
                <X className="size-3.5" />
              </button>
            </motion.div>
          );
        })}
      </AnimatePresence>
    </div>
  );
}
