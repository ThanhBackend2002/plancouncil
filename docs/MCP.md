# PlanCouncil MCP server (Claude Code)

Cho phép Claude Code đưa một vấn đề vào PlanCouncil, theo dõi cuộc họp và nhận lại kết quả có cấu trúc.

## Cài đặt

PlanCouncil chạy như MCP server qua stdio khi được gọi với cờ `--mcp`. Bản desktop và MCP server là
**cùng một file chạy**, không cần cài gì thêm.

```bash
# Bản dev (sau `cargo build` hoặc `npm run tauri dev`)
claude mcp add plancouncil -- /đường/dẫn/plancouncil/src-tauri/target/debug/plancouncil --mcp

# Bản đã cài: trỏ tới file chạy của app (vd. trong thư mục cài đặt / PlanCouncil.app/Contents/MacOS/)
claude mcp add plancouncil -- "/đường/dẫn/tới/PlanCouncil" --mcp
```

Kiểm tra: trong Claude Code gõ `/mcp`, server `plancouncil` phải ở trạng thái connected với 7 tool.

Trước khi dùng, mở app desktop ít nhất 1 lần để cấu hình provider, API key, vai trò và workspace. MCP server
đọc đúng cấu hình đó.

## Tool

| Tool | Việc |
|---|---|
| `start_council` | Bắt đầu họp. Trả `session_id` ngay, cuộc họp chạy nền (thường vài phút). |
| `get_council_updates` | Cập nhật mới kể từ lần gọi trước: plan vN, review + verdict, round, thành viên. Hỗ trợ long-poll `wait_seconds` (≤ 55). |
| `get_council_status` | Phase, round, plan version, phiếu round gần nhất, token. Dùng được cho phiên cũ. |
| `get_council_result` | Kết luận, `confidence`, phiếu bầu, `dissent`, `open_issues`, `suggestions`, usage theo model + plan markdown (khối text thứ 2). Có `wait_seconds` để chờ phiên xong. |
| `cancel_council` | Dừng phiên đang chạy, giữ plan hợp lệ gần nhất. |
| `send_council_message` | Bổ sung thông tin giữa cuộc họp. Áp cho mọi lần gọi model kế tiếp (người viết + reviewer). Tối đa 20 lần × 4000 ký tự / phiên. |
| `list_councils` | Phiên gần đây trong lịch sử. |

Luồng điển hình: `start_council` → lặp `get_council_updates(wait_seconds=30)` (hoặc `get_council_result(wait_seconds=50)`)
→ đọc `get_council_result`.

Tham số chính của `start_council`: `prompt` (bắt buộc), `context` (đoạn code / ràng buộc liên quan, ≤ 30.000 ký tự,
tổng với prompt ≤ 50.000), `workspace`, `web_search`, `company_context`, `max_rounds` (1–10), `departments`,
`chair_id`, `reviewer_ids`, `auto_staff`, `plan_depth` (`mvp` | `standard` | `enterprise`). Thiếu tham số nào thì
dùng mặc định trong app.

## Quyền truy cập workspace

- MCP server **không** tự quét thư mục. Claude Code chỉ gửi những gì nằm trong `prompt` / `context`.
- `workspace` chỉ nhận workspace **đã khai báo trong app** (Cài đặt → Bối cảnh & Công cụ), khớp theo id, tên hoặc
  đường dẫn thật. Đường dẫn khác bị từ chối kèm danh sách workspace hợp lệ.
- Việc đọc file dùng chung `WorkspaceFs` với app desktop: chặn thoát khỏi thư mục gốc (traversal / symlink), bỏ qua
  file bí mật, giới hạn kích thước. Agent chỉ đọc, không bao giờ sửa file.

## Hành vi vận hành

- **1 phiên / tiến trình MCP**, như app desktop. Gọi `start_council` khi đang có phiên → lỗi `already_running`.
- **Chạy cùng lúc với app desktop:** được. Hai bên dùng chung thư mục dữ liệu; phiên MCP hiện trong tab Lịch sử
  (timeline ghi "Phiên được gửi từ Claude Code (MCP)" và các thông tin bổ sung). App không xem trực tiếp được phiên
  MCP đang chạy, chỉ thấy các checkpoint đã lưu (sau mỗi vòng / mỗi phiên bản plan).
- **Lease:** trong lúc chạy, phiên MCP làm mới `live/<id>.lease` mỗi 30 giây. App khởi động không đánh dấu phiên có lease
  còn hạn (< 2 phút) là "bị gián đoạn", và không cho xoá phiên đó.
- **API key:** MCP server chỉ *đọc* keyring / `secrets.json`, không ghi `config.json`. Key lưu chế độ "chỉ trong RAM"
  không có ở tiến trình MCP → lỗi `secret_missing`. Khi đó chuyển key sang keyring trong app.
- **Claude Code thoát:** MCP server dừng phiên đang chạy và chờ tối đa 15 giây để lưu bản ghi cuối (trạng thái `cancelled`).
- **Server khởi động lại:** cập nhật (`get_council_updates`) của phiên cũ không còn. `get_council_status` / `get_council_result`
  vẫn đọc được từ lịch sử.
- **Log:** ghi ra stderr (Claude Code hiển thị trong log MCP). Log không chứa API key; prompt không được ghi đầy đủ.

## Lỗi thường gặp

Lỗi của tool trả về `isError: true` với tiền tố mã lỗi:

| Mã | Ý nghĩa / cách xử lý |
|---|---|
| `[invalid]` | Tham số sai, cấu hình chưa sẵn sàng (role chưa có model…), workspace chưa được cấp quyền. |
| `[secret_missing]` | Thiếu API key ở keyring (key chỉ lưu trong RAM của app). |
| `[already_running]` | Server đang chạy phiên khác. |
| `[not_found]` | Không có phiên với `session_id` này. |
| `[keyring_unavailable]` / `[keyring_locked]` | Keyring hệ điều hành không dùng được / đang khoá. |

## ADR: kiến trúc

**Bối cảnh.** Claude Code cần gọi PlanCouncil như một công cụ. Orchestrator (`orchestrator.rs`) không phụ thuộc Tauri:
chỉ cần `EventSink`, `Checkpoint`, `LlmClient` và `AppConfig`.

**Quyết định.**
1. *Transport:* MCP qua stdio (JSON-RPC 2.0, mỗi message 1 dòng), tự cài trong `mcp.rs`, không thêm dependency. Không mở
   cổng mạng, không cần xác thực.
2. *Vòng đời:* Claude Code khởi chạy `plancouncil --mcp`. Tiến trình này tự chạy cuộc họp, không cần app desktop đang mở.
   Không tách binary riêng để bộ cài vẫn chỉ có 1 file chạy.
3. *Giao thức nội bộ:* gọi thẳng các API trong crate: `Council::with_extras` + `run`, `build_extras`,
   `check_secrets_present`, `SessionWriter`. Đường chạy giống hệt app desktop, chỉ khác `EventSink`: `FeedSink` gom event
   thành cập nhật ngắn gọn (bỏ delta stream) cho long-poll.
4. *Trao đổi giữa chừng:* `MeetingControl::add_note`. Thông tin bổ sung được chèn vào mọi prompt sau khi nhận (khối
   `<requester_updates>`, qua `Council::decorate`) và lưu vào `SessionRecord.user_notes`. Không ngắt vòng đang chạy.
5. *Chia sẻ dữ liệu với app:* dùng chung thư mục, SQLite index (WAL + busy timeout), lease file để tránh nhận nhầm phiên
   "bị gián đoạn".

**Hệ quả.** Ưu: không phải duy trì kênh IPC giữa hai tiến trình; phiên MCP có đủ tính năng của app (phòng ban, công
cụ, ngân sách token, checkpoint). Nhược: app desktop không xem trực tiếp được phiên MCP (chỉ thấy checkpoint); hai tiến
trình có thể chạy song song 2 phiên và cùng tiêu quota provider.

**Mở rộng sau.** Đẩy thông báo (MCP notifications/progress), tìm kiếm lịch sử (`search_history`), báo cáo theo loại
nhiệm vụ, HTTP có xác thực khi cần nhiều máy.
