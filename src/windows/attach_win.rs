//! Pasting or dropping files and pictures into the typing box. Each one is copied into the data folder and
//! shown as a tile above the typing box (see thumbs.rs); its × un-attaches it. Nothing stays in memory.
use super::app::{self, app};
use super::util::*;
use crate::core::attach;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::DragQueryFileW;

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

fn dir() -> PathBuf { app().data.join("attachments") }

/// A file name that is safe inside a `[📎 …]` mark and the attachments folder.
fn clean(name: &str) -> String {
    let s: String = name.chars().map(|c| if matches!(c, '[' | ']' | '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
}

/// `name` inside the attachments folder, numbered if something else already has that name.
fn unique(name: &str, same_len: Option<u64>) -> (String, PathBuf, bool) {
    let d = dir();
    let (stem, ext) = name.rsplit_once('.').map_or((name, String::new()), |(s, e)| (s, format!(".{e}")));
    for n in 0.. {
        let cand = if n == 0 { name.to_string() } else { format!("{stem}-{n}{ext}") };
        let p = d.join(&cand);
        match std::fs::metadata(&p) {
            Err(_) => return (cand, p, false),
            // The same file attached twice reuses its copy.
            Ok(m) if Some(m.len()) == same_len => return (cand, p, true),
            Ok(_) => {}
        }
    }
    unreachable!()
}

fn insert_token(name: &str) {
    super::thumbs::add(name);
    unsafe { SetFocus(app().input_hwnd()); }
}

fn add_file(src: &Path) {
    let Ok(meta) = std::fs::metadata(src) else { return };
    let shown = src.file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string();
    if meta.is_dir() { return app::show_note(&format!("{shown} is a folder. Attach files, not folders.")); }
    if meta.len() > attach::MAX_FILE {
        return app::show_note(&format!("{shown} is over {} MB, so it wasn't attached.", attach::MAX_FILE / 1024 / 1024));
    }
    let _ = std::fs::create_dir_all(dir());
    let (name, dest, reuse) = unique(&clean(&shown), Some(meta.len()));
    if !reuse && std::fs::copy(src, &dest).is_err() { return app::show_note(&format!("Couldn't read {shown}.")); }
    insert_token(&name);
    if matches!(attach::kind(&name), attach::Kind::Other) {
        // Anything that isn't a picture, PDF, or text is only useful to the model if it reads as text.
        let mut head = [0u8; 2048];
        let n = std::fs::File::open(&dest).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
        if head[..n].contains(&0) { app::show_note(&format!("{name} is a type models can't read (video, audio, program, and so on). Only its name will be sent.")); }
    }
}

fn add_png(bytes: &[u8]) {
    let _ = std::fs::create_dir_all(dir());
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let (name, dest, _) = unique(&format!("image-{secs}.png"), None);
    if std::fs::write(&dest, bytes).is_ok() { insert_token(&name); }
}

/// Handles the clipboard when it holds files or a picture (and no text). True when it did.
pub fn paste() -> bool {
    let a = app();
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP) == 0 && IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 { return false; }
        let png_fmt = RegisterClipboardFormatW(w("PNG").as_ptr());
        let has = |f: u32| IsClipboardFormatAvailable(f) != 0;
        if !(has(CF_HDROP) || has(png_fmt) || has(CF_DIB)) { return false; }
        if OpenClipboard(a.hwnd) == 0 { return false; }
        let mut files: Vec<PathBuf> = Vec::new();
        let mut image: Option<(Vec<u8>, bool)> = None; // bytes, is already a PNG
        let drop = GetClipboardData(CF_HDROP);
        if !drop.is_null() {
            files = paths(drop);
        } else {
            for (f, is_png) in [(png_fmt, true), (CF_DIB, false)] {
                let h = GetClipboardData(f);
                if h.is_null() { continue; }
                let size = GlobalSize(h);
                let p = GlobalLock(h) as *const u8;
                if !p.is_null() && size > 0 {
                    image = Some((std::slice::from_raw_parts(p, size).to_vec(), is_png));
                    GlobalUnlock(h);
                    break;
                }
            }
        }
        CloseClipboard();
        if !files.is_empty() {
            files.iter().for_each(|f| add_file(f));
            true
        } else if let Some((bytes, is_png)) = image {
            if is_png {
                add_png(&bytes);
            } else {
                let _ = std::fs::create_dir_all(dir());
                let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                let (name, dest, _) = unique(&format!("image-{secs}.png"), None);
                if super::images::dib_to_png(&bytes, &dest) {
                    if std::fs::metadata(&dest).is_ok_and(|m| m.len() > attach::MAX_FILE) {
                        let _ = std::fs::remove_file(&dest);
                        app::show_note("That picture is over 8 MB, so it wasn't attached.");
                    } else {
                        insert_token(&name);
                    }
                } else {
                    app::show_note("Couldn't read the picture on the clipboard.");
                }
            }
            true
        } else {
            false
        }
    }
}

unsafe fn paths(h: HANDLE) -> Vec<PathBuf> {
    let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
    (0..n).filter_map(|i| {
        let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
        let mut b = vec![0u16; len + 1];
        DragQueryFileW(h, i, b.as_mut_ptr(), len as u32 + 1);
        Some(PathBuf::from(String::from_utf16_lossy(&b[..len])))
    }).collect()
}

/// Files dropped from Explorer (a drop handle from WM_DROPFILES or EN_DROPFILES).
pub fn dropped(h: HANDLE) {
    unsafe { paths(h) }.iter().for_each(|f| add_file(f));
}

/// The "Attach a file" action: a standard Open dialog, then the file is marked in the box.
pub fn pick() {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OFN_NOCHANGEDIR};
    let a = app();
    unsafe {
        let mut buf = vec![0u16; 1024];
        let mut o: OPENFILENAMEW = std::mem::zeroed();
        o.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        o.hwndOwner = a.hwnd;
        o.lpstrFile = buf.as_mut_ptr();
        o.nMaxFile = buf.len() as u32;
        o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut o) != 0 {
            let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
            add_file(Path::new(&String::from_utf16_lossy(&buf[..n])));
        }
    }
}
