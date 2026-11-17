import { useEffect, useMemo } from "react";
import { MAIN_SCOPE } from "../lib/contracts";
import { activeOf, resolveScope, useActive, useActiveShallow, useCouncil } from "../store/council";
import { useConfig, useResolver } from "../store/config";
import { initialSession, mainOf, type ScopeState } from "../store/session";
import { useUi } from "../store/ui";
import { Building } from "./Building";
import { Dialog } from "./Dialog";
import { PlanTracker } from "./PlanTracker";
import { MeetingRoom } from "./MeetingRoom";
import { MemberControls } from "./MemberControls";
import { PlanViewer } from "./PlanViewer";
import { ProgressBar } from "./ProgressBar";
import { PromptPanel } from "./PromptPanel";
import { ScopeBar } from "./ScopeBar";
import { StateBanner } from "./StateBanner";
import { Timeline } from "./Timeline";

/**
 * Bố cục "stage" (mặc định): cột trái hẹp = yêu cầu + điều khiển + hội thoại; vùng chính = phòng họp lớn,
 * tiến độ, thẻ các phiên bản plan — bấm vN mở plan trong popup giữa màn hình.
 * Bố cục "split" (cũ): trái 40% (prompt + phòng họp + hội thoại), phải 60% (Plan Viewer luôn hiện).
 */
export function CouncilView() {
  // Selector hẹp: plan_delta chỉ đổi `drafts` → scopeId / scope / cast giữ nguyên tham chiếu,
  // CouncilView và các con không re-render theo từng delta.
  const scopeId = useCouncil((s) => resolveScope(activeOf(s), s.scope));
  const found = useActive((s) => s.scopes[scopeId] ?? null);
  const { status, id, cast } = useActiveShallow((s) => ({ status: s.status, id: s.id, cast: s.cast }));
  const config = useConfig((s) => s.config);
  const resolve = useResolver(cast);

  const scope: ScopeState = useMemo(() => {
    // Giống `mainOf(session)` khi thiếu scope (không nên xảy ra vì luôn có "main").
    const base = found ?? mainOf({ ...initialSession(), scopes: {}, status });
    // Chưa có phiên: xem trước cuộc họp mặc định trong cấu hình.
    if (status === "idle" && !id && config) {
      return {
        ...base,
        authorId: config.meeting.chair_id,
        reviewerIds: config.meeting.reviewer_ids,
        maxRounds: config.max_rounds,
      };
    }
    return base;
  }, [found, status, id, config]);
  const title = scope.id === MAIN_SCOPE ? "HỌP CHÍNH" : scope.name.toUpperCase();
  const roomView = useUi((s) => s.roomView);
  const setRoomView = useUi((s) => s.setRoomView);
  const setScope = useCouncil((s) => s.setScope);
  // Chưa có phiên: toà nhà vẽ cuộc họp chính theo bản xem trước của cấu hình.
  const mainPreview = status === "idle" && !id && scope.id === MAIN_SCOPE ? scope : null;
  const layout = useUi((s) => s.layout);
  const tab = useUi((s) => s.tab);
  const planOpen = useUi((s) => s.planOpen);
  const closePlan = useUi((s) => s.closePlan);
  // Đổi phiên → đóng popup plan (đang trỏ vào plan của phiên cũ).
  useEffect(() => closePlan(), [id, closePlan]);

  /* data-scene: đánh dấu cảnh pixel (luôn tối, dùng palette Tailwind gốc, không dùng token). */
  const scene = (
    <div data-scene className="contents">
      {roomView === "building" ? (
        <Building
          resolve={resolve}
          mainOverride={mainPreview}
          activeId={scope.id}
          sessionKey={id ?? "preview"}
          onEnter={(sid) => {
            setScope(sid);
            setRoomView("room");
          }}
          onRoomView={() => setRoomView("room")}
        />
      ) : (
        <MeetingRoom
          scope={scope}
          resolve={resolve}
          title={title}
          idleCaption={status === "idle" ? "PHÒNG HỌP TRỐNG" : undefined}
          onBuilding={() => setRoomView("building")}
        />
      )}
    </div>
  );

  if (layout === "stage") {
    return (
      <>
        <div className="grid h-full grid-cols-[minmax(320px,400px)_minmax(0,1fr)] gap-3 p-3">
          <aside className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto">
            <StateBanner />
            <PromptPanel />
            <MemberControls scope={scope} resolve={resolve} />
            <Timeline scope={scope} resolve={resolve} />
          </aside>
          <section aria-label="Cuộc họp" className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto">
            <ScopeBar active={scope.id} />
            {/* Phòng họp co theo chiều cao màn hình để tiến độ + thẻ plan luôn nằm trong tầm nhìn. */}
            <div className="mx-auto w-full" style={{ maxWidth: "max(520px, calc((100vh - 330px) * 1.6))" }}>
              {scene}
            </div>
            <ProgressBar />
            <PlanTracker scope={scope} resolve={resolve} />
          </section>
        </div>
        <Dialog
          open={planOpen && tab === "council"}
          onClose={closePlan}
          title={`Plan · ${scope.id === MAIN_SCOPE ? "Họp chính" : scope.name}`}
          className="flex h-[88vh] max-w-6xl flex-col"
        >
          <div className="min-h-0 flex-1">
            <PlanViewer scope={scope} resolve={resolve} />
          </div>
        </Dialog>
      </>
    );
  }

  return (
    <div className="grid h-full grid-cols-[minmax(440px,2fr)_3fr] gap-3 p-3">
      <div className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto">
        <StateBanner />
        <PromptPanel />
        <ScopeBar active={scope.id} />
        {scene}
        <MemberControls scope={scope} resolve={resolve} />
        <ProgressBar />
        <Timeline scope={scope} resolve={resolve} />
      </div>
      <div className="min-h-0 min-w-0">
        <PlanViewer scope={scope} resolve={resolve} />
      </div>
    </div>
  );
}
