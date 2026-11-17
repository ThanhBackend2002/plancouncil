import { useMemo } from "react";
import { MAIN_SCOPE } from "../lib/contracts";
import { activeOf, resolveScope, useActive, useActiveShallow, useCouncil } from "../store/council";
import { useConfig, useResolver } from "../store/config";
import { initialSession, mainOf, type ScopeState } from "../store/session";
import { useUi } from "../store/ui";
import { Building } from "./Building";
import { MeetingRoom } from "./MeetingRoom";
import { MemberControls } from "./MemberControls";
import { PlanViewer } from "./PlanViewer";
import { ProgressBar } from "./ProgressBar";
import { PromptPanel } from "./PromptPanel";
import { ScopeBar } from "./ScopeBar";
import { StateBanner } from "./StateBanner";
import { Timeline } from "./Timeline";

/** Split View: trái 40% (prompt + phòng họp pixel + hội thoại), phải 60% (Plan Viewer). */
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

  return (
    <div className="grid h-full grid-cols-[minmax(440px,2fr)_3fr] gap-3 p-3">
      <div className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto">
        <StateBanner />
        <PromptPanel />
        <ScopeBar active={scope.id} />
        {/* data-scene: đánh dấu cảnh pixel (luôn tối, dùng palette Tailwind gốc, không dùng token). */}
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
