import { useState } from "react";
import { Eye, FileText, LoaderCircle, RotateCcw, Trash } from "lucide-react";
import { formatDate, formatTokens } from "../lib/agents";
import type { SessionSummary } from "../lib/contracts";
import { ipc } from "../lib/ipc";
import { useCouncil } from "../store/council";
import { useHistory } from "../store/history";
import { toastError } from "../store/toast";
import { useUi } from "../store/ui";
import { Button, StatusBadge } from "./ui";

export function HistoryView() {
  const { sessions, loading, loaded, refresh } = useHistory();

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col p-6">
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-lg font-semibold">Lịch sử phiên họp</h1>
        <span className="text-sm text-neutral-500">{sessions.length} phiên</span>
        <Button
          size="sm"
          className="ml-auto"
          icon={<RotateCcw className="size-3.5" />}
          loading={loading}
          onClick={() => void refresh()}
        >
          Làm mới
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto rounded-xl border border-neutral-800">
        {!loaded && loading ? (
          <div className="flex items-center justify-center gap-2 p-10 text-sm text-neutral-500">
            <LoaderCircle className="size-4 animate-spin" /> Đang tải…
          </div>
        ) : sessions.length === 0 ? (
          <div className="flex flex-col items-center gap-2 p-12 text-center text-sm text-neutral-500">
            <FileText className="size-8 text-neutral-700" />
            Chưa có phiên nào. Phiên sẽ được lưu tự động khi kết thúc.
          </div>
        ) : (
          <ul className="divide-y divide-neutral-800">
            {sessions.map((s) => (
              <Row key={s.id} s={s} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function Row({ s }: { s: SessionSummary }) {
  const remove = useHistory((st) => st.remove);
  const openRecord = useCouncil((st) => st.openRecord);
  const liveId = useCouncil((st) => (st.live.status === "running" ? st.live.id : null));
  const setTab = useUi((st) => st.setTab);
  const [opening, setOpening] = useState(false);
  const [confirm, setConfirm] = useState(false);

  const open = async () => {
    setOpening(true);
    try {
      openRecord(await ipc.loadSession(s.id));
      setTab("council");
    } catch (e) {
      toastError(e, "Không mở được phiên");
    } finally {
      setOpening(false);
    }
  };

  return (
    <li className="flex items-center gap-4 px-4 py-3 hover:bg-neutral-900/50">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm text-neutral-200" title={s.prompt_preview}>
          {s.prompt_preview}
        </p>
        <p className="mt-0.5 text-xs text-neutral-500">
          {formatDate(s.created_at)} · {s.versions} version · {s.rounds} round
          {s.departments > 0 && ` · ${s.departments} phòng ban`}
          {s.total_tokens > 0 && ` · ${formatTokens(s.total_tokens)} token`}
        </p>
      </div>
      <StatusBadge status={s.status} />
      <div className="flex gap-1.5">
        <Button size="sm" icon={<Eye className="size-3.5" />} loading={opening} onClick={() => void open()}>
          Mở
        </Button>
        {confirm ? (
          <>
            <Button size="sm" variant="danger" onClick={() => void remove(s.id)}>
              Xác nhận xoá
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>
              Huỷ
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            icon={<Trash className="size-3.5" />}
            disabled={s.id === liveId}
            title={s.id === liveId ? "Không xoá được phiên đang chạy" : "Xoá phiên"}
            onClick={() => setConfirm(true)}
            aria-label="Xoá phiên"
          />
        )}
      </div>
    </li>
  );
}
