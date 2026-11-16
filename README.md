# PlanCouncil

Phòng họp Multi-Agent dạng "công ty": bạn tự quản lý các vai trò (C-Level → Staff, mỗi role một model) và phòng ban. Chủ trì giao việc → các phòng ban họp nội bộ song song → chủ trì tổng hợp plan → reviewer phản biện cho tới khi hội tụ. Giao diện phòng họp pixel-art, nhân vật trao đổi bằng bong bóng thoại.

Tài liệu: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (kiến trúc, IPC contract, bảo mật) · [docs/ROADMAP.md](docs/ROADMAP.md) (các bước tiếp theo) · [docs/MCP.md](docs/MCP.md) (dùng từ Claude Code qua MCP: `plancouncil --mcp`).

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
npm run verify                 # cargo test + clippy + vitest + tsc
```

## Build bộ cài

Mỗi OS build trên chính OS đó (Tauri không cross-compile ổn định):

| OS | Lệnh | Kết quả trong `src-tauri/target/.../release/bundle/` |
|---|---|---|
| macOS (máy đang dùng) | `npm run build:mac` | `macos/PlanCouncil.app`, `dmg/*.dmg` |
| macOS (Intel + Apple Silicon) | `rustup target add x86_64-apple-darwin aarch64-apple-darwin` rồi `npm run build:mac:universal` | `universal-apple-darwin/release/bundle/dmg/*.dmg` |
| Windows 11 | `npm run build:win` (cần MSVC Build Tools) | `nsis/*-setup.exe`, `msi/*.msi` |
| Ubuntu | cài gói ở mục Chạy (+ `libgtk-3-dev libxdo-dev patchelf`) rồi `npm run build:linux` | `deb/*.deb`, `rpm/*.rpm`, `appimage/*.AppImage` |

**Build cả 3 OS cùng lúc:** push code lên GitHub → tab Actions → **Build apps** → Run workflow (hoặc push tag `v0.1.0`). Bộ cài nằm ở mục Artifacts của lần chạy.

Bộ cài **chưa ký số**:
- macOS: lần đầu mở bằng chuột phải → Open, hoặc `xattr -cr /Applications/PlanCouncil.app`. Bản chưa ký sẽ hỏi lại quyền Keychain sau mỗi lần cài bản mới.
- Windows: SmartScreen cảnh báo → More info → Run anyway.

**WSL (Ubuntu) với `node_modules` cài từ Windows:** chạy `npm run build` bên Windows, rồi trong WSL chạy `bash scripts/wsl-run.sh` (build Rust ở `~/.cache/plancouncil-target`, nhúng sẵn `dist/`, mở cửa sổ qua WSLg).

Dữ liệu: `~/.config/plancouncil/` (Linux) · `%APPDATA%\plancouncil\` (Windows) · `~/Library/Application Support/plancouncil/` (macOS). Log: thư mục log của app (tauri-plugin-log).
