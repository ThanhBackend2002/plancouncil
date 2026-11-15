import { MAIN_SCOPE } from "../lib/contracts";
import { resolveScope, useActiveSession, useCouncil } from "../store/council";
import { useConfig, useResolver } from "../store/config";
import { mainOf, type ScopeState } from "../store/session";
import { MeetingRoom } from "./MeetingRoom";
import { MemberControls } from "./MemberControls";
import { PlanViewer } from "./PlanViewer";
import { PromptPanel } from "./PromptPanel";
import { ScopeBar } from "./ScopeBar";
import { Timeline } from "./Timeline";

/** Split View: trái 40% (prompt + phòng họp pixel + hội thoại), phải 60% (Plan Viewer). */
export function CouncilView() {
  const session = useActiveSession();
  const sel = useCouncil((s) => s.scope);
  const config = useConfig((s) => s.config);
  const resolve = useResolver(session.cast);
  const scopeId = resolveScope(session, sel);
  let scope: ScopeState = session.scopes[scopeId] ?? mainOf(session);

  // Chưa có phiên: xem trước cuộc họp mặc định trong cấu hình.
  if (session.status === "idle" && !session.id && config) {
    scope = {
      ...scope,
      authorId: config.meeting.chair_id,
      reviewerIds: config.meeting.reviewer_ids,
      maxRounds: config.max_rounds,
    };
  }
  const title = scope.id === MAIN_SCOPE ? "HỌP CHÍNH" : scope.name.toUpperCase();

  return (
    <div className="grid h-full grid-cols-[minmax(440px,2fr)_3fr] gap-3 p-3">
      <div className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto">
        <PromptPanel />
        <ScopeBar session={session} active={scope.id} />
        <MeetingRoom
          scope={scope}
          resolve={resolve}
          title={title}
          idleCaption={session.status === "idle" ? "PHÒNG HỌP TRỐNG" : undefined}
        />
        <MemberControls session={session} scope={scope} resolve={resolve} />
        <Timeline session={session} scope={scope} resolve={resolve} />
      </div>
      <div className="min-h-0 min-w-0">
        <PlanViewer session={session} scope={scope} />
      </div>
    </div>
  );
}
