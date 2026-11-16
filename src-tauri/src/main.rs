// Ẩn console window trên Windows ở bản release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `plancouncil --mcp`: MCP server qua stdio cho Claude Code (không mở cửa sổ).
    if std::env::args().skip(1).any(|a| a == "--mcp") {
        std::process::exit(plancouncil_lib::run_mcp());
    }
    plancouncil_lib::run()
}
