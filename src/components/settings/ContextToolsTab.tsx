import { useState } from "react";
import { CircleX, ExternalLink, FolderOpen, FolderPlus, Globe, LoaderCircle, Search, ShieldCheck, Trash } from "lucide-react";
import type { AppConfig, SearchHit, ToolPolicy, WebSearchProvider, Workspace } from "../../lib/contracts";
import { ipc, isCommandError } from "../../lib/ipc";
import { toastError } from "../../store/toast";
import { Button, Panel, cx } from "../ui";
import {
  Field,
  INPUT,
  KeyInput,
  NumberInput,
  SecretControls,
  SectionTitle,
  TextArea,
  TextInput,
  Toggle,
  findStatus,
  newId,
  useSecretStatus,
  type Errors,
} from "./fields";

const CONTEXT_TEMPLATE = `## Tech stack bắt buộc
- Backend: (vd. Go 1.22, gRPC, PostgreSQL 16, Redis)
- Frontend: (vd. React 19 + TypeScript, Tailwind)
- Mobile/Game client: (vd. Unity 2022 LTS, C#)
- Hạ tầng: (vd. AWS, Kubernetes EKS, Terraform, GitHub Actions)
- Quan sát: (vd. Prometheus, Grafana, Loki, Sentry)

## Quy định / nội quy
- Mọi API phải có versioning và tài liệu OpenAPI.
- Dữ liệu cá nhân lưu ở region Singapore, mã hoá at-rest.
- Không dùng dịch vụ trả phí mới nếu chưa qua CFO duyệt.
- Mỗi tính năng có feature flag + kế hoạch rollback.

## Quy ước đội ngũ
- Đội 6 backend, 4 client, 1 SRE; release 2 tuần/lần.
`;

const PROVIDERS: { id: WebSearchProvider; label: string; hint: string }[] = [
  { id: "duckduckgo", label: "DuckDuckGo", hint: "Không cần key; có thể bị chặn nếu truy vấn dồn dập" },
  { id: "tavily", label: "Tavily", hint: "Tối ưu cho AI agent, cần API key (tavily.com)" },
  { id: "brave", label: "Brave Search", hint: "Cần API key (api.search.brave.com)" },
  { id: "searxng", label: "SearXNG (tự host)", hint: "Cần base URL và bật format=json" },
];

const POLICIES: { id: ToolPolicy; label: string; desc: string }[] = [
  { id: "authors", label: "Chỉ người viết plan", desc: "Chủ trì + trưởng phòng nghiên cứu 1 lần trước bản nháp. Cân bằng token." },
  { id: "all", label: "Tất cả role", desc: "Reviewer cũng tự kiểm chứng ở round 1. Chính xác hơn nhưng tốn token hơn nhiều." },
  { id: "off", label: "Tắt", desc: "Không dùng công cụ. Nếu chọn workspace, role chỉ thấy tổng quan (cây thư mục + README)." },
];

export function ContextToolsTab({
  draft,
  saved,
  errors,
  dirty,
  update,
}: {
  draft: AppConfig;
  saved: AppConfig;
  errors: Errors;
  dirty: boolean;
  update: (f: (c: AppConfig) => AppConfig) => void;
}) {
  const secrets = useSecretStatus(saved);
  const set = (patch: Partial<AppConfig>) => update((c) => ({ ...c, ...patch }));
  const setWeb = (patch: Partial<AppConfig["web_search"]>) => update((c) => ({ ...c, web_search: { ...c.web_search, ...patch } }));
  const setCtx = (patch: Partial<AppConfig["company_context"]>) =>
    update((c) => ({ ...c, company_context: { ...c.company_context, ...patch } }));
  const setWs = (id: string, patch: Partial<Workspace>) =>
    update((c) => ({ ...c, workspaces: c.workspaces.map((w) => (w.id === id ? { ...w, ...patch } : w)) }));

  const pick = async (id?: string) => {
    try {
      const path = await ipc.pickFolder();
      if (!path) return;
      if (id) {
        setWs(id, { path });
        return;
      }
      const name = path.split(/[\\/]/).filter(Boolean).pop() ?? "workspace";
      update((c) => ({ ...c, workspaces: [...c.workspaces, { id: newId("ws"), name, path }] }));
    } catch (e) {
      toastError(e, "Không mở được hộp thoại chọn thư mục");
    }
  };

  const ctxLen = draft.company_context.text.length;
  const web = draft.web_search;
  const provider = PROVIDERS.find((p) => p.id === web.provider);

  return (
    <div className="space-y-4">
      <Panel className="p-4">
        <SectionTitle
          title="Bối cảnh công ty"
          desc="Tech stack, nội quy, quy ước mà mọi plan phải tuân thủ. Được đưa vào prompt của MỌI role. Khi tắt cho 1 phiên, các role được khuyến khích nghĩ vượt giới hạn."
        >
          {!draft.company_context.text.trim() && (
            <Button size="sm" onClick={() => setCtx({ text: CONTEXT_TEMPLATE })}>
              Chèn mẫu
            </Button>
          )}
        </SectionTitle>
        <TextArea
          value={draft.company_context.text}
          onChange={(v) => setCtx({ text: v })}
          rows={12}
          placeholder="VD: Backend dùng Go + PostgreSQL; không dùng MongoDB; mọi service chạy trên Kubernetes…"
        />
        <div className="mt-2 flex items-center gap-4 text-xs">
          <Toggle
            checked={draft.company_context.enabled_by_default}
            onChange={(v) => setCtx({ enabled_by_default: v })}
            label="Mặc định áp dụng cho phiên mới"
          />
          <span className={cx("ml-auto tabular-nums", ctxLen > 20_000 ? "text-red-400" : "text-neutral-600")}>
            {ctxLen.toLocaleString("vi-VN")}/20.000 · ~{Math.ceil(ctxLen / 3).toLocaleString("vi-VN")} token mỗi lần gọi
          </span>
        </div>
        {errors["context.text"] && <p className="mt-1 text-xs text-red-400">{errors["context.text"]}</p>}
      </Panel>

      <Panel className="p-4">
        <SectionTitle
          title="Workspace (dự án được cấp quyền đọc)"
          desc="Chọn thư mục dự án để các role đọc code/cấu hình và đưa ra hướng đi phù hợp. Chỉ ĐỌC; không đọc được ngoài thư mục, bỏ qua node_modules/.git/build, chặn file bí mật (.env, *.pem, id_rsa…)."
        >
          <Button size="sm" variant="primary" icon={<FolderPlus className="size-3.5" />} onClick={() => void pick()}>
            Thêm thư mục
          </Button>
        </SectionTitle>
        <div className="space-y-2">
          {draft.workspaces.map((w) => (
            <div key={w.id} className="grid grid-cols-[1fr_2fr_auto_auto] items-end gap-2">
              <Field label="Tên" error={errors[`context.ws.${w.id}`]}>
                <TextInput value={w.name} onChange={(v) => setWs(w.id, { name: v })} />
              </Field>
              <Field label="Đường dẫn">
                <input readOnly value={w.path} title={w.path} className={cx(INPUT, "font-mono text-[12px] text-neutral-400")} />
              </Field>
              <Button icon={<FolderOpen className="size-3.5" />} onClick={() => void pick(w.id)}>
                Đổi
              </Button>
              <Button
                variant="ghost"
                icon={<Trash className="size-3.5" />}
                onClick={() => update((c) => ({ ...c, workspaces: c.workspaces.filter((x) => x.id !== w.id) }))}
                aria-label="Bỏ workspace"
              />
            </div>
          ))}
          {draft.workspaces.length === 0 && (
            <p className="py-3 text-center text-xs text-neutral-500">Chưa có workspace. Chọn workspace cho từng phiên ở màn Phòng họp.</p>
          )}
        </div>
        <p className="mt-3 flex items-start gap-1.5 text-[11px] text-neutral-500">
          <ShieldCheck className="mt-px size-3.5 shrink-0 text-emerald-500" />
          Nội dung file được đọc sẽ được gửi tới provider model của role đó. Đừng cấp quyền thư mục chứa dữ liệu bạn không muốn chia sẻ với provider.
        </p>
      </Panel>

      <Panel className="p-4">
        <SectionTitle title="Tìm kiếm web" desc="Cho role tìm thông tin mới (phiên bản thư viện, giá dịch vụ, tiêu chuẩn…) và đọc trang web công khai. Không truy cập được IP nội bộ/LAN." />
        <div className="grid grid-cols-3 gap-4">
          <Field label="Dịch vụ" hint={provider?.hint}>
            <select value={web.provider} onChange={(e) => setWeb({ provider: e.target.value as WebSearchProvider })} className={INPUT}>
              {PROVIDERS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </Field>
          {(web.provider === "tavily" || web.provider === "brave") && (
            <Field label="API key" className="col-span-2">
              <KeyInput
                value={web.api_key}
                saved={saved.web_search.provider === web.provider ? saved.web_search.api_key : null}
                onChange={(v) => setWeb({ api_key: v })}
              />
              <SecretControls
                target={{ kind: "websearch", id: web.provider }}
                status={findStatus(secrets.report, { kind: "websearch", id: web.provider })}
                secretsFile={secrets.report?.secrets_file}
                value={web.api_key}
                canStore={saved.web_search.provider === web.provider}
                update={update}
                refresh={secrets.refresh}
              />
            </Field>
          )}
          {web.provider === "searxng" && (
            <Field label="Base URL" className="col-span-2" error={errors["context.searxng"]}>
              <TextInput value={web.base_url} onChange={(v) => setWeb({ base_url: v })} placeholder="https://searx.example.com" mono />
            </Field>
          )}
          <Field label="Số kết quả mỗi lần tìm" error={errors["context.max_results"]} hint="1–10">
            <NumberInput value={web.max_results} onChange={(v) => setWeb({ max_results: v ?? Number.NaN })} />
          </Field>
          <Field label="Mặc định" hint="Bật tìm web cho phiên mới">
            <Toggle checked={web.enabled_by_default} onChange={(v) => setWeb({ enabled_by_default: v })} label="Bật" />
          </Field>
        </div>
        <WebTest dirty={dirty} />
      </Panel>

      <Panel className="p-4">
        <SectionTitle title="Ai được dùng công cụ" desc="Công cụ = đọc workspace + tìm web. Mỗi lần nghiên cứu tốn thêm vài lần gọi model." />
        <div className="grid grid-cols-3 gap-2">
          {POLICIES.map((p) => (
            <button
              key={p.id}
              type="button"
              aria-pressed={draft.tool_policy === p.id}
              onClick={() => set({ tool_policy: p.id })}
              className={cx(
                "rounded-lg border px-3 py-2 text-left",
                draft.tool_policy === p.id ? "border-violet-500/60 bg-violet-500/10" : "border-neutral-800 hover:border-neutral-600",
              )}
            >
              <div className="text-sm font-medium text-neutral-100">{p.label}</div>
              <div className="mt-0.5 text-[11px] text-neutral-500">{p.desc}</div>
            </button>
          ))}
        </div>
        <div className="mt-3 w-60">
          <Field label="Số lần gọi công cụ tối đa / lần nghiên cứu" error={errors["context.max_tool_steps"]} hint="1–12">
            <NumberInput value={draft.max_tool_steps} onChange={(v) => set({ max_tool_steps: v ?? Number.NaN })} />
          </Field>
        </div>
        <p className="mt-2 text-[11px] text-neutral-500">
          Model cần hỗ trợ tool calling (OpenAI function calling). Model không hỗ trợ sẽ tự bỏ qua bước nghiên cứu, vẫn họp bình thường.
        </p>
      </Panel>
    </div>
  );
}

function WebTest({ dirty }: { dirty: boolean }) {
  const [query, setQuery] = useState("latest stable PostgreSQL version");
  const [loading, setLoading] = useState(false);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setLoading(true);
    setError(null);
    setHits(null);
    try {
      setHits(await ipc.testWebSearch(query));
    } catch (e) {
      setError(isCommandError(e) ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="mt-4 rounded-md border border-neutral-800 bg-neutral-950/50 p-3">
      <div className="flex gap-2">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-neutral-500" />
          <input value={query} onChange={(e) => setQuery(e.target.value)} className={cx(INPUT, "pl-8")} />
        </div>
        <Button
          icon={loading ? <LoaderCircle className="size-3.5 animate-spin" /> : <Globe className="size-3.5" />}
          disabled={dirty || loading || !query.trim()}
          title={dirty ? "Lưu cấu hình trước khi thử" : "Thử tìm kiếm với cấu hình đã lưu"}
          onClick={() => void run()}
        >
          Thử tìm
        </Button>
      </div>
      {error && (
        <p className="mt-2 flex items-start gap-1.5 text-xs text-red-300">
          <CircleX className="mt-px size-3.5 shrink-0" />
          {error}
        </p>
      )}
      {hits && (
        <ul className="mt-2 space-y-1.5 text-xs">
          {hits.length === 0 && <li className="text-neutral-500">Không có kết quả.</li>}
          {hits.map((h, i) => (
            <li key={i}>
              <div className="flex items-center gap-1 text-neutral-200">
                <ExternalLink className="size-3 shrink-0 text-neutral-500" />
                <span className="truncate font-medium">{h.title}</span>
              </div>
              <div className="truncate font-mono text-[10px] text-sky-400/80">{h.url}</div>
              <div className="line-clamp-2 text-neutral-500">{h.snippet}</div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
