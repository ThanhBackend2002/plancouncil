# PlanCouncil

Phòng họp Multi-Agent dạng "công ty": bạn tự quản lý các vai trò (C-Level → Staff, mỗi role một model) và phòng ban. Chủ trì giao việc → các phòng ban họp nội bộ song song → chủ trì tổng hợp plan → reviewer phản biện cho tới khi hội tụ. Giao diện phòng họp pixel-art, nhân vật trao đổi bằng bong bóng thoại.

Tài liệu: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (kiến trúc, IPC contract, bảo mật) · [docs/ROADMAP.md](docs/ROADMAP.md) (các bước tiếp theo).

## Cấu trúc

```
plancouncil/
├── package.json · vite.config.ts · tsconfig.json · index.html
├── src/                         # React 19 + TS + Tailwind v4
│   ├── main.tsx · App.tsx · index.css
│   ├── lib/
│   │   ├── contracts.ts         # IPC contract mirror của models.rs
│   │   ├── ipc.ts               # wrapper có kiểu cho 10 command
│   │   ├── bridge.ts            # listen council-event → store
│   │   ├── agents.ts            # màu, icon, nhãn của agent/trạng thái
│   │   └── sections.ts          # tách plan theo ## cho Diff View
│   │   └── pixel.ts             # sinh nhân vật pixel-art 16×24 từ role (không dùng asset ngoài)
│   ├── store/                   # zustand: session (reducer nhiều scope), council, config, history, toast, ui
│   └── components/
│       ├── MeetingRoom.tsx      # văn phòng pixel: người viết plan + reviewer quanh bàn, bong bóng thoại
│       ├── PixelCharacter.tsx   # nhân vật + chân dung pixel (chớp mắt, nói, suy nghĩ, ✓/✗)
│       ├── Timeline.tsx         # khung hội thoại kiểu chat
│       ├── ScopeBar.tsx         # chuyển giữa họp chính và các phòng ban
│       ├── PlanViewer · Markdown · Mermaid · DiffView · HistoryView
│       └── settings/            # tab Provider · Vai trò · Phòng ban · Cuộc họp · Chung
└── src-tauri/
    ├── Cargo.toml · build.rs · tauri.conf.json
    ├── capabilities/default.json
    ├── icons/                   # placeholder — thay bằng `npm run tauri icon <logo.png>`
    └── src/
        ├── main.rs              # entry (ẩn console trên Windows release)
        ├── lib.rs               # Tauri commands, AppState, TauriSink
        ├── models.rs            # domain + event schema + validation
        ├── config.rs            # endpoint per-agent, redact/merge API key
        ├── storage.rs           # config.json + sessions/*.json (atomic write)
        ├── llm_client.rs        # OpenAI-compatible client: retry, SSE, lenient JSON
        ├── prompts.rs           # prompt dựng từ persona của role + anti-injection framing
        ├── roster.rs            # danh mục role (C-Level → Staff) + phòng ban mặc định
        └── orchestrator.rs      # convergence loop (không phụ thuộc Tauri) + integration tests
```

## Chạy

```bash
# Ubuntu: sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev libayatana-appindicator3-dev build-essential
# Windows 11: WebView2 có sẵn; cần MSVC Build Tools
npm install
npm run tauri dev
cd src-tauri && cargo test     # 63 test, gồm mock LLM server cho toàn bộ vòng lặp
```

**WSL (Ubuntu) với `node_modules` cài từ Windows:** chạy `npm run build` bên Windows, rồi trong WSL chạy `bash scripts/wsl-run.sh` (build Rust ở `~/.cache/plancouncil-target`, nhúng sẵn `dist/`, mở cửa sổ qua WSLg).

Dữ liệu: `~/.config/plancouncil/` (Linux) · `%APPDATA%\plancouncil\` (Windows). Log: thư mục log của app (tauri-plugin-log).
