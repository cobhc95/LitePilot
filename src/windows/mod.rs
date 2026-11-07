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
mod updater;
pub mod util;

pub fn run() {
    util::silence_beeps();
    let a: Vec<String> = std::env::args().collect();
    if a.iter().any(|x| x == "--uninstall") {
        return install::uninstall();
    }
    if let Some(i) = a.iter().position(|x| x == "--finish-uninstall") {
        let pid = a.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or(0);
        return install::finish_uninstall(pid, std::path::PathBuf::from(a.get(i + 2).cloned().unwrap_or_default()));
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
    if let Some(i) = a.iter().position(|x| x == "--after-update") {
        updater::wait_for_old(a.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or(0));
    }
    updater::start();
    app::run(!a.iter().any(|x| x == "--background"));
}
