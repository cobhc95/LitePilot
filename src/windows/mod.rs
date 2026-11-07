mod app;
mod bubbles;
mod chip_menu;
mod tile_menu;
mod cli;
mod code;
mod actions_view;
mod attach_win;
mod backup_crypt;
mod history_view;
mod resident;
mod http;
mod images;
mod key_badges;
mod install;
mod memory_store;
mod rtf;
mod secrets;
mod sysinfo;
mod tidy;
mod settings;
pub mod util;

pub fn run() {
    util::silence_beeps();
    let a: Vec<String> = std::env::args().collect();
    if a.iter().any(|x| x == "--uninstall") {
        return install::uninstall();
    }
    // The read-only memory lookup an assistant calls as a tool: LitePilot.exe --memory-mcp <folder with memory.bin>.
    if let Some(i) = a.iter().position(|x| x == "--memory-mcp") {
        let dir = std::path::PathBuf::from(a.get(i + 1).cloned().unwrap_or_default());
        return memory_store::serve_mcp(&dir);
    }
    // The read-only look-at-this-computer tool for the assistant: LitePilot.exe --system-mcp.
    if a.iter().any(|x| x == "--system-mcp") {
        return sysinfo::serve();
    }
    if install::is_setup_exe() {
        return install::install();
    }
    app::run(a.iter().any(|x| x == "--show"));
}
