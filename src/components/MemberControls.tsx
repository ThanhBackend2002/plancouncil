import { memo, useState } from "react";
import { UserPlus, X } from "lucide-react";
import type { RoleResolver } from "../lib/agents";
import { ipc } from "../lib/ipc";
import { useActiveShallow } from "../store/council";
import { providerOf, useConfig } from "../store/config";
import type { ScopeState } from "../store/session";
import { toast, toastError } from "../store/toast";
import { PixelPortrait } from "./PixelCharacter";
import { RoleSelect } from "./settings/fields";
import { Button, Panel } from "./ui";

/** Mời thêm / mời ra trong lúc họp — áp dụng từ vòng review kế tiếp của nơi họp đang xem. */
export const MemberControls = memo(function MemberControls({ scope, resolve }: { scope: ScopeState; resolve: RoleResolver }) {
  const config = useConfig((s) => s.config);
  const session = useActiveShallow((s) => ({ readOnly: s.readOnly, status: s.status, id: s.id }));
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState(false);
  if (!config || session.readOnly || session.status !== "running" || !session.id || scope.status !== "running") return null;

  const present = new Set([scope.authorId, ...scope.reviewerIds]);
  const candidates = config.roles.filter((r) => !present.has(r.id) && providerOf(config, r.id) && r.model.trim());
  const where = scope.kind === "main" ? "họp chính" : scope.name;

  const act = async (kind: "invite" | "dismiss", roleId: string) => {
    setBusy(true);
    try {
      const ok =
        kind === "invite"
          ? await ipc.inviteMember(session.id!, scope.id, roleId)
          : await ipc.dismissMember(session.id!, scope.id, roleId);
      if (ok) {
        toast("info", `${kind === "invite" ? "Sẽ mời" : "Sẽ mời ra"} ${resolve(roleId).name}`, `Áp dụng từ vòng review kế tiếp của ${where}.`);
        if (kind === "invite") setPick("");
      } else {
        toast("warning", "Phiên đã kết thúc");
      }
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel className="flex shrink-0 flex-wrap items-center gap-1.5 px-2.5 py-2 text-xs">
      <span className="mr-1 text-neutral-500">Thành viên {where}:</span>
      {scope.reviewerIds.map((id) => {
        const m = resolve(id);
        return (
          <span key={id} className="inline-flex items-center gap-1 rounded-full border border-neutral-700 py-0.5 pr-1 pl-0.5 text-neutral-200">
            <PixelPortrait meta={m} size={16} />
            {m.name}
            <button
              type="button"
              disabled={busy}
              onClick={() => void act("dismiss", id)}
              title="Mời ra khỏi cuộc họp (từ vòng sau)"
              className="rounded-full p-0.5 text-neutral-500 hover:bg-neutral-800 hover:text-red-300"
            >
              <X className="size-3" />
            </button>
          </span>
        );
      })}
      <span className="ml-auto flex items-center gap-1.5">
        <span className="w-52">
          <RoleSelect roles={candidates} value={pick} onChange={setPick} placeholder="-- mời thêm chuyên gia --" />
        </span>
        <Button size="sm" icon={<UserPlus className="size-3.5" />} disabled={!pick || busy} onClick={() => void act("invite", pick)}>
          Mời
        </Button>
      </span>
    </Panel>
  );
});
