import { MotionConfig } from "motion/react";
import { Header } from "./components/Header";
import { CouncilView } from "./components/CouncilView";
import { HistoryView } from "./components/HistoryView";
import { SettingsView } from "./components/settings/SettingsView";
import { Toasts } from "./components/Toasts";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { useUi } from "./store/ui";
import { useAppHotkeys } from "./lib/useHotkeys";

export default function App() {
  const tab = useUi((s) => s.tab);
  useAppHotkeys(); // Ctrl/Cmd+K → Lịch sử + focus ô tìm
  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-full flex-col bg-neutral-950 text-neutral-100">
        <Header />
        {/* Giữ cả 3 view mounted để không mất scroll/state khi đổi tab. */}
        <main className="relative min-h-0 flex-1">
          <div className={tab === "council" ? "h-full" : "hidden"}>
            <ErrorBoundary label="Phòng họp">
              <CouncilView />
            </ErrorBoundary>
          </div>
          <div className={tab === "history" ? "h-full" : "hidden"}>
            <ErrorBoundary label="Lịch sử">
              <HistoryView />
            </ErrorBoundary>
          </div>
          <div className={tab === "settings" ? "h-full" : "hidden"}>
            <ErrorBoundary label="Cài đặt">
              <SettingsView />
            </ErrorBoundary>
          </div>
        </main>
        <Toasts />
      </div>
    </MotionConfig>
  );
}
