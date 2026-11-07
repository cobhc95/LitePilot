#![cfg_attr(windows, windows_subsystem = "windows")]
//! `core` is platform-neutral (protocols, config, history).
//! `windows` holds everything OS-specific; other platforms add a sibling module.
mod core;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    // The release build aborts on a bug without a message; leave one line behind so a crash can be traced.
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(std::env::temp_dir().join("litepilot-crash.txt"), format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    windows::run();
}
#[cfg(not(windows))]
fn main() {
    eprintln!("LitePilot currently has a Windows front end only.");
}
