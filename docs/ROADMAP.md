# PlanCouncil — Roadmap

> Cập nhật: 2026-11-16

## Tổng quan tiến độ

| Bước | Nội dung | Trạng thái |
|---|---|---|
| 1 | Cấu trúc dự án, `Cargo.toml`, `package.json`, cấu hình Tauri | ✅ Xong |
| 2 | Backend Rust: models, config, storage, llm_client, prompts, orchestrator, commands | ✅ Xong — 63 test, chạy được trên Ubuntu (WSL) và Windows 11 |
| 3 | Frontend React: phòng họp pixel-art, Plan Viewer, Diff, Settings, History | ✅ Xong, chạy trong Tauri trên Windows 11 (`npm run tauri dev`) |
| 4 | Hardening: keyring, CI, E2E, installer Windows | ⬜ (build/chạy Windows dev đã xong) |
| 5 | Đóng gói và phát hành: installer, code signing, auto-update | ⬜ |
| 6 | Mở rộng | 🟡 Đã có: provider + kéo model, role/phòng ban tuỳ biến, đếm token, đọc workspace, tìm web, bối cảnh công ty, kiểm tra model, AI chọn thành phần, thành viên động, review theo section. Còn: chi phí theo bảng giá, json_schema, SQLite, nhiều phiên đồng thời |

Các bước 3a → 3h có thể demo ngay sau mỗi bước con.

---

## Bước 3 — Frontend

**Mục tiêu:** chạy được `npm run tauri dev` và hoàn thành một phiên họp thật từ đầu đến cuối trên UI.

**Quyết định đã chốt:**
- Avatar: SVG thuần + `motion` (không cần asset ngoài).
- Diff View: unified theo từng section, highlight cấp từ cho cặp dòng xoá→thêm.

**File tương ứng:**

| Bước | File |
|---|---|
| 3a | `main.tsx`, `App.tsx`, `index.css`, `lib/ipc.ts`, `lib/bridge.ts`, `store/session.ts` (reducer thuần), `store/council.ts` |
| 3b | `components/CouncilView.tsx`, `PromptPanel.tsx` (kèm Quick Config), `Header.tsx`, `Toasts.tsx` |
| 3c | `components/MeetingRoom.tsx` |
| 3d | `components/Timeline.tsx` |
| 3e | `components/PlanViewer.tsx`, `Markdown.tsx`, `Mermaid.tsx` |
| 3f | `components/DiffView.tsx`, `lib/sections.ts` |
| 3g | `components/SettingsView.tsx`, `store/config.ts` |
| 3h | `components/HistoryView.tsx`, `store/history.ts` |

### 3a. Nền tảng app và IPC
- `src/main.tsx`, `App.tsx`, `index.css` (Tailwind v4, dark theme kiểu Linear/Raycast).
- `src/lib/ipc.ts`: wrapper có kiểu cho 10 command, chuẩn hoá `CommandError`.
- `src/store/council.ts` (zustand): reducer xử lý `EventEnvelope`
  - bỏ qua event của session khác, phát hiện `seq` bị nhảy;
  - `plan_stream_reset` xoá buffer, `plan_delta` nối text, `plan_ready` thay toàn bộ.
- Khi mở app: gọi `active_session()` để khôi phục trạng thái nếu webview bị reload.

**Tiêu chí xong:** log được toàn bộ event của một phiên thật ra console theo đúng thứ tự.

### 3b. Layout Split View
- Cột trái 40% (Meeting Room + Timeline + Quick Config), cột phải 60% (Plan Viewer).
- Ô nhập prompt, nút **Start Council** / **Stop**, hiển thị round hiện tại và trạng thái phiên.
- Thông báo lỗi dạng toast theo `kind` (`already_running`, `invalid`, `llm`…).

**Tiêu chí xong:** chạy và dừng được phiên từ UI; resize cửa sổ đến `minWidth 1100` không vỡ layout.

### 3c. Phòng họp hoạt hình
- SVG bàn họp, Architect ở giữa, 3 Reviewer xung quanh, mỗi người một màu nhận diện.
- Trạng thái avatar: `idle` → `thinking` (phát sáng + hiệu ứng gõ phím) → `done` (speech bubble `summary_bubble` + badge xanh APPROVE / đỏ REJECT) hoặc `error`.
- Hiện cảnh báo nhỏ khi review có `policy_override`.
- Tôn trọng `prefers-reduced-motion`.

**Tiêu chí xong:** 3 reviewer sáng đồng thời khi review song song; bubble đổi đúng theo từng round.

### 3d. Timeline và log góp ý
- Danh sách theo round: verdict, `critical_issues`, `suggestions` của từng reviewer, thu gọn/mở rộng được.
- Hiển thị `review_failed` và `round_completed` (`approvals/total`).

### 3e. Plan Viewer
- Tabs `v1`, `v2`, `v3`, `Final` sinh động theo số version thực tế.
- `react-markdown` + `remark-gfm`; khối `mermaid` render bằng `mermaid.render()` với `securityLevel: "strict"`, theme dark.
- Mermaid lỗi cú pháp → hiện code gốc kèm thông báo, không làm hỏng cả trang.
- Trong lúc stream: chỉ render lại Mermaid khi khối đó đã đóng (tránh render hàng trăm lần).
- Tự cuộn theo nội dung khi đang stream, dừng tự cuộn nếu người dùng cuộn lên.
- Nút **Copy Plan** (`navigator.clipboard`) và **Export .md** (`export_markdown`).

**Tiêu chí xong:** plan 5 section + 1 sơ đồ hiển thị đúng; Final có phụ lục Open Issues khi cần.

### 3f. Diff View
- Chọn 2 version bất kỳ (mặc định version trước và version sau cùng).
- Dùng thư viện `diff` (`diffLines`/`diffWords`), tách theo heading `##` để so sánh từng section.
- Đánh dấu section thêm/xoá/sửa; có nút chỉ xem section thay đổi.

### 3g. Settings
- Endpoint mặc định + override theo từng agent (base URL, API key, model, temperature).
- Ô API key hiển thị giá trị mask; sửa ô = nhập key mới, xoá trắng = xoá key.
- Nút **Test connection** cho từng agent (`test_connection`): hiện latency và phản hồi.
- `max_rounds`, timeout, retries, JSON mode, bật/tắt stream Architect.
- Hiển thị lỗi validate từ backend ngay tại form.

### 3h. History
- Danh sách phiên cũ (`list_sessions`), mở lại (`load_session`) ở chế độ chỉ xem, xoá (`delete_session`).
- Tự refresh khi nhận `council-session-saved`.

**Tiêu chí xong Bước 3:** người dùng mới cài app → nhập key → chạy phiên → xem diff → export file mà không cần đọc tài liệu.

---

## Bước 4 — Hardening

| Hạng mục | Chi tiết | Tiêu chí xong |
|---|---|---|
| Build Windows 11 | `cargo test` + `npm run tauri build` trên Windows; kiểm tra đường dẫn `%APPDATA%`, hộp thoại lưu, WebView2 | Bộ cài `.msi`/`.exe` chạy được trên máy sạch |
| OS keyring | Chuyển API key sang crate `keyring` (Windows Credential Manager / Secret Service); migrate key cũ từ `config.json` rồi xoá | `config.json` không còn chứa key |
| CI | GitHub Actions matrix `ubuntu-24.04` + `windows-latest`: `cargo fmt --check`, `clippy -D warnings`, `cargo test`, `tsc --noEmit`, `vite build` | Mọi PR phải xanh cả 2 OS |
| Audit dependency | `cargo audit`, `cargo deny` (chặn `openssl-sys`/`native-tls`), `npm audit` | Chạy trong CI |
| Test frontend | Vitest cho reducer của store (thứ tự event, stream reset, seq bị nhảy) | Coverage reducer ≥ 90% |
| E2E | Chạy app với mock LLM server (tái sử dụng mock trong `orchestrator.rs` thành binary dev) + WebDriver | 1 kịch bản hội tụ + 1 kịch bản Stop |
| Kiểm thử với provider thật | OpenAI, OpenRouter, 1 local proxy (LiteLLM/Ollama) — đặc biệt JSON mode và SSE | Ghi lại bảng tương thích trong docs |

## Bước 5 — Đóng gói và phát hành

- Icon và branding thật (`npm run tauri icon logo.png`).
- Code signing Windows (tránh SmartScreen); `.deb` + AppImage cho Linux.
- `tauri-plugin-updater` với chữ ký updater, kênh release trên GitHub Releases.
- Ghi phiên bản theo SemVer, CHANGELOG, migrate `schema_version` của config/session khi đổi format.

## Bước 6 — Mở rộng (ưu tiên theo giá trị)

| # | Tính năng | Giá trị | Ghi chú |
|---|---|---|---|
| 1 | ~~Thống kê token~~ ✅ (theo role / nơi họp / loại việc). Còn lại: quy đổi chi phí theo bảng giá model, latency | Kiểm soát chi phí khi dùng model đắt | Cần bảng giá per-model do người dùng nhập |
| 2 | JSON mode `json_schema` strict | Giảm lỗi parse, bớt repair call | Bật theo provider, giữ fallback hiện tại |
| 3 | Persona reviewer tuỳ biến (thêm/sửa/bỏ reviewer, chỉnh system prompt) | Dùng cho domain khác (game backend, data pipeline…) | Đổi `REVIEWERS` cố định thành danh sách trong config |
| 4 | Lịch sử bằng SQLite + tìm kiếm full-text | Khi số phiên lớn | `rusqlite` + FTS5 |
| 5 | Tiếp tục phiên (thêm round sau khi đã kết thúc) hoặc chạy lại từ một version | Tiết kiệm token | Cần lưu thêm trạng thái `previous` reviews |
| 6 | Nhiều phiên đồng thời | So sánh nhiều model | Kèm giới hạn concurrency theo provider |
| 7 | Đính kèm file ngữ cảnh (schema DB, README hiện có) vào prompt | Plan sát thực tế hơn | Giới hạn kích thước, cảnh báo token |
| 8 | i18n giao diện (Việt/Anh) | | |

## Rủi ro chính

| Rủi ro | Ảnh hưởng | Giảm thiểu |
|---|---|---|
| Model nhỏ/local không tuân thủ JSON hoặc Mermaid | Phiên lỗi, sơ đồ không render | Parser khoan dung + repair prompt (đã có); fallback hiện code Mermaid gốc (3e) |
| Reviewer "dời mục tiêu" khiến không bao giờ hội tụ | Tốn token, luôn `max_rounds_reached` | Prompt Round ≥ 2 + review cũ (đã có); theo dõi tỉ lệ hội tụ khi có thống kê (6.1) |
| Khác biệt WebKitGTK (Ubuntu) và WebView2 (Windows) | Lỗi hiển thị/animation chỉ trên 1 OS | Build target riêng trong `vite.config.ts` (đã có); CI 2 OS (Bước 4) |
| Chi phí khi dùng model đắt cho cả 4 agent | | Override model rẻ hơn cho reviewer; thống kê chi phí (6.1) |
