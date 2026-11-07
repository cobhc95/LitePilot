//! Pictures in replies. Each run of pictures becomes one gallery: a single row of uniform tiles drawn
//! over a reserved line in the chat, with arrows when there are more than fit. Clicking a tile opens a
//! viewer inside the window. Only the compressed bytes and tiny tile bitmaps are kept; a picture is
//! decoded just long enough to make a tile, or while it is open in the viewer. Everything is freed
//! when the popup hides.
use super::util::w;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE, VK_LEFT, VK_RIGHT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

type P = *mut c_void;
pub const FIRST_CHAR: u32 = 0xE000;
const MAX_ITEMS: usize = 6000;
const TWIPS_PER_PX: i32 = 15;
/// Every picture in the chat is the same small tile, cropped to fill.
const TILE_W: i32 = 84;
const TILE_H: i32 = 63;
const GAP: i32 = 6;
const ARROW: i32 = 22;

/// Posted to the main window when a web picture finishes downloading: wParam = id, lParam = Box<Option<Vec<u8>>>.
pub const WM_IMAGE: u32 = 0x8020;
/// Largest web picture LitePilot will download.
const MAX_FETCH: usize = 6 * 1024 * 1024;

/// Colors the chat passes in, so tiles and the viewer match the theme.
#[derive(Clone, Copy)]
pub struct Colors { pub bg: u32, pub tile: u32, pub text: u32 }

/// Pictures stay in memory as their compressed bytes. Old ones are shrunk once (`shrunk`), never removed.
struct Img { key: u64, label: String, bytes: Vec<u8>, shrunk: bool, px: (u32, u32), thumb: HBITMAP, thumb_px: (i32, i32), failed: bool, remote: Option<String>, loading: bool }

impl Img {
    fn have(&self) -> bool { !self.bytes.is_empty() }
    fn read(&self) -> Vec<u8> { self.bytes.clone() }
}

struct Gallery { key: u64, imgs: Vec<usize>, offset: usize, tiles: Vec<(usize, RECT)>, prev: Option<RECT>, next: Option<RECT> }

thread_local! {
    static IMGS: RefCell<Vec<Img>> = const { RefCell::new(Vec::new()) };
    static GALLERIES: RefCell<Vec<Gallery>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy)]
struct Api {
    from_stream: unsafe extern "system" fn(P, *mut P) -> i32,
    dispose: unsafe extern "system" fn(P) -> i32,
    graphics: unsafe extern "system" fn(HDC, *mut P) -> i32,
    delete_graphics: unsafe extern "system" fn(P) -> i32,
    draw_part: unsafe extern "system" fn(P, P, i32, i32, i32, i32, i32, i32, i32, i32, i32, P, P, P) -> i32,
    quality: unsafe extern "system" fn(P, i32) -> i32,
    smoothing: unsafe extern "system" fn(P, i32) -> i32,
    solid_fill: unsafe extern "system" fn(u32, *mut P) -> i32,
    delete_brush: unsafe extern "system" fn(P) -> i32,
    fill_ellipse: unsafe extern "system" fn(P, P, f32, f32, f32, f32) -> i32,
    new_pen: unsafe extern "system" fn(u32, f32, i32, *mut P) -> i32,
    pen_caps: unsafe extern "system" fn(P, i32, i32, i32) -> i32,
    pen_join: unsafe extern "system" fn(P, i32) -> i32,
    delete_pen: unsafe extern "system" fn(P) -> i32,
    draw_lines: unsafe extern "system" fn(P, P, *const [f32; 2], i32) -> i32,
    draw_arc: unsafe extern "system" fn(P, P, f32, f32, f32, f32, f32, f32) -> i32,
    path_new: unsafe extern "system" fn(i32, *mut P) -> i32,
    path_arc: unsafe extern "system" fn(P, f32, f32, f32, f32, f32, f32) -> i32,
    path_close: unsafe extern "system" fn(P) -> i32,
    path_delete: unsafe extern "system" fn(P) -> i32,
    fill_path: unsafe extern "system" fn(P, P, P) -> i32,
    draw_path: unsafe extern "system" fn(P, P, P) -> i32,
    mem_stream: unsafe extern "system" fn(*const u8, u32) -> P,
}
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

#[repr(C)]
struct StartupInput { version: u32, callback: P, no_thread: i32, no_codecs: i32 }

fn api() -> Option<Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    *API.get_or_init(|| unsafe {
        let gp: HMODULE = LoadLibraryW(w("gdiplus.dll").as_ptr());
        let sh: HMODULE = LoadLibraryW(w("shlwapi.dll").as_ptr());
        if gp.is_null() || sh.is_null() { return None; }
        macro_rules! f { ($m:expr, $n:literal) => { std::mem::transmute(GetProcAddress($m, concat!($n, "\0").as_ptr())?) } }
        let startup: unsafe extern "system" fn(*mut usize, *const StartupInput, P) -> i32 = f!(gp, "GdiplusStartup");
        let (mut token, input) = (0usize, StartupInput { version: 1, callback: std::ptr::null_mut(), no_thread: 0, no_codecs: 0 });
        if startup(&mut token, &input, std::ptr::null_mut()) != 0 { return None; }
        Some(Api {
            from_stream: f!(gp, "GdipCreateBitmapFromStream"),
            dispose: f!(gp, "GdipDisposeImage"),
            graphics: f!(gp, "GdipCreateFromHDC"),
            delete_graphics: f!(gp, "GdipDeleteGraphics"),
            draw_part: f!(gp, "GdipDrawImageRectRectI"),
            quality: f!(gp, "GdipSetInterpolationMode"),
            smoothing: f!(gp, "GdipSetSmoothingMode"),
            solid_fill: f!(gp, "GdipCreateSolidFill"),
            delete_brush: f!(gp, "GdipDeleteBrush"),
            fill_ellipse: f!(gp, "GdipFillEllipse"),
            new_pen: f!(gp, "GdipCreatePen1"),
            pen_caps: f!(gp, "GdipSetPenLineCap197819"),
            pen_join: f!(gp, "GdipSetPenLineJoin"),
            delete_pen: f!(gp, "GdipDeletePen"),
            draw_lines: f!(gp, "GdipDrawLines"),
            draw_arc: f!(gp, "GdipDrawArc"),
            path_new: f!(gp, "GdipCreatePath"),
            path_arc: f!(gp, "GdipAddPathArc"),
            path_close: f!(gp, "GdipClosePathFigure"),
            path_delete: f!(gp, "GdipDeletePath"),
            fill_path: f!(gp, "GdipFillPath"),
            draw_path: f!(gp, "GdipDrawPath"),
            mem_stream: f!(sh, "SHCreateMemStream"),
        })
    })
}

/// Saves a clipboard DIB (header, colors, pixels) as a PNG file. GDI+ is the one the pictures already use.
pub fn dib_to_png(dib: &[u8], path: &std::path::Path) -> bool {
    if dib.len() < 40 || api().is_none() { return false; }
    unsafe {
        let gp = LoadLibraryW(w("gdiplus.dll").as_ptr());
        macro_rules! f { ($n:literal) => { match GetProcAddress(gp, concat!($n, "\0").as_ptr()) { Some(p) => std::mem::transmute(p), None => return false } } }
        let from_dib: unsafe extern "system" fn(*const u8, *const u8, *mut P) -> i32 = f!("GdipCreateBitmapFromGdiDib");
        let save: unsafe extern "system" fn(P, *const u16, *const [u8; 16], P) -> i32 = f!("GdipSaveImageToFile");
        let dispose: unsafe extern "system" fn(P) -> i32 = f!("GdipDisposeImage");
        let u32at = |i: usize| u32::from_le_bytes([dib[i], dib[i + 1], dib[i + 2], dib[i + 3]]);
        let (size, bpp, comp, used) = (u32at(0) as usize, u16::from_le_bytes([dib[14], dib[15]]), u32at(16), u32at(32) as usize);
        let colors = if used != 0 { used } else if bpp <= 8 { 1usize << bpp } else { 0 };
        let masks = if comp == 3 && size == 40 { 12 } else { 0 };
        let off = size + masks + colors * 4;
        if off >= dib.len() { return false; }
        let mut bmp: P = std::ptr::null_mut();
        if from_dib(dib.as_ptr(), dib.as_ptr().add(off), &mut bmp) != 0 || bmp.is_null() { return false; }
        // The PNG encoder's id: {557CF406-1A04-11D3-9A73-0000F81EF32E}.
        let clsid: [u8; 16] = [0x06, 0xF4, 0x7C, 0x55, 0x04, 0x1A, 0xD3, 0x11, 0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E];
        let ok = save(bmp, w(&path.to_string_lossy()).as_ptr(), &clsid, std::ptr::null_mut()) == 0;
        dispose(bmp);
        ok
    }
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); }
    }
    Some(out)
}

/// Pixel size from a PNG, JPEG, or GIF header.
fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    if b.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some(((be16(16)? << 16) | be16(18)?, (be16(20)? << 16) | be16(22)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((u16::from_le_bytes([*b.get(6)?, *b.get(7)?]) as u32, u16::from_le_bytes([*b.get(8)?, *b.get(9)?]) as u32));
    }
    if !b.starts_with(&[0xFF, 0xD8]) { return None; }
    let mut i = 2;
    while *b.get(i)? == 0xFF {
        let m = *b.get(i + 1)?;
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return Some((be16(i + 7)?, be16(i + 5)?));
        }
        i += 2 + be16(i + 2)? as usize;
    }
    None
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// One picture. None if it can't be shown (unsupported, or a download already failed).
fn register(label: &str, url: &str) -> Option<usize> {
    let key = hash(url);
    IMGS.with_borrow_mut(|v| {
        if let Some(i) = v.iter().position(|m| m.key == key) {
            return (!v[i].failed).then_some(i);
        }
        if v.len() >= MAX_ITEMS { return None; }
        let (bytes, px, remote) = if url.starts_with("https://") || url.starts_with("wiki:") {
            (Vec::new(), (0, 0), Some(url.to_string()))
        } else {
            let data = ["png", "jpeg", "jpg", "gif"].iter().find_map(|k| url.strip_prefix(&format!("data:image/{k};base64,")))?;
            let bytes = b64(data)?;
            let px = image_size(&bytes).filter(|s| s.0 > 0 && s.1 > 0)?;
            (bytes, px, None)
        };
        let m = Img { key, label: label.to_string(), bytes, shrunk: false, px, thumb: std::ptr::null_mut(), thumb_px: (0, 0), failed: false, remote, loading: false };
        v.push(m);
        Some(v.len() - 1)
    })
}

/// A picture file the user attached (kept apart from `register` so a reply can never name a local file).
fn register_file(name: &str, path: &std::path::Path) -> Option<usize> {
    let key = hash(&format!("file:{}", path.display()));
    IMGS.with_borrow_mut(|v| {
        if let Some(i) = v.iter().position(|m| m.key == key) { return (!v[i].failed).then_some(i); }
        if v.len() >= MAX_ITEMS || std::fs::metadata(path).ok()?.len() > crate::core::attach::MAX_FILE { return None; }
        let bytes = std::fs::read(path).ok()?;
        let px = image_size(&bytes).filter(|s| s.0 > 0 && s.1 > 0)?;
        v.push(Img { key, label: name.to_string(), bytes, shrunk: false, px, thumb: std::ptr::null_mut(), thumb_px: (0, 0), failed: false, remote: None, loading: false });
        Some(v.len() - 1)
    })
}

/// A row of thumbnails for the pictures attached to one message. `salt` (the message text) keeps two messages
/// with the same picture from sharing a row.
pub fn register_files(pics: &[(String, std::path::PathBuf)], salt: &str) -> Option<(u32, u32)> {
    let ids: Vec<usize> = pics.iter().filter_map(|(n, p)| register_file(n, p)).collect();
    if ids.is_empty() { return None; }
    make_gallery(hash(&format!("{}|{salt}", pics[0].1.display())), ids)
}

/// A row of pictures. Returns (gallery id, row height in twips), or None if none can be shown.
pub fn register_gallery(pics: &[(&str, &str)]) -> Option<(u32, u32)> {
    let ids: Vec<usize> = pics.iter().filter_map(|(l, u)| register(l, u)).collect();
    if ids.is_empty() { return None; }
    // Keyed by the first picture, so a row that grows while a reply streams keeps its place.
    make_gallery(hash(pics[0].1), ids)
}

fn make_gallery(key: u64, ids: Vec<usize>) -> Option<(u32, u32)> {
    GALLERIES.with_borrow_mut(|g| {
        let i = match g.iter().position(|x| x.key == key) {
            Some(i) => { g[i].imgs = ids; i }
            None => {
                if g.len() >= MAX_ITEMS { return None; }
                g.push(Gallery { key, imgs: ids, offset: 0, tiles: vec![], prev: None, next: None });
                g.len() - 1
            }
        };
        Some((i as u32, (TILE_H * TWIPS_PER_PX) as u32))
    })
}

/// A picture link to download. `wiki:Title` means the main photo of that English Wikipedia article,
/// found with one small request to Wikipedia's page summary API.
fn resolve(url: &str) -> Option<String> {
    let Some(title) = url.strip_prefix("wiki:") else { return Some(url.to_string()) };
    let t: String = title.trim().replace(' ', "_").chars().map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | '.' | '(' | ')' | ',' => c.to_string(),
        c => c.to_string().bytes().map(|b| format!("%{b:02X}")).collect(),
    }).collect();
    let json = super::http::fetch(&format!("https://en.wikipedia.org/api/rest_v1/page/summary/{t}"), 256 * 1024)?;
    let v: serde_json::Value = serde_json::from_slice(&json).ok()?;
    v["thumbnail"]["source"].as_str().or(v["originalimage"]["source"].as_str()).map(|s| {
        // A 500px rendition (one of the sizes Wikimedia serves): about 80 KB, enough for the viewer.
        match s.rsplit_once('/') { Some((head, file)) if s.contains("/thumb/") => match file.split_once("px-") { Some((_, rest)) => format!("{head}/500px-{rest}"), None => s.to_string() }, _ => s.to_string() }
    })
}

/// Starts downloading a web picture once. The result comes back to `main` as WM_IMAGE.
fn start_fetch(main: HWND, id: usize) {
    let url = IMGS.with_borrow_mut(|v| {
        let m = v.get_mut(id)?;
        if m.loading || m.failed || m.have() { return None; }
        m.loading = true;
        m.remote.clone()
    });
    let Some(url) = url else { return };
    let main = main as usize;
    std::thread::spawn(move || {
        let got = resolve(&url).and_then(|u| super::http::fetch(&u, MAX_FETCH));
        unsafe { PostMessageW(main as HWND, WM_IMAGE, id, Box::into_raw(Box::new(got)) as isize); }
    });
}

/// A download finished. Returns true when the chat should re-render (a picture failed, so its
/// row changes); otherwise a repaint is enough.
pub fn loaded(id: usize, bytes: Option<Vec<u8>>) -> bool {
    let failed = IMGS.with_borrow_mut(|v| {
        let Some(m) = v.get_mut(id) else { return false };
        m.loading = false;
        match bytes.and_then(|b| image_size(&b).filter(|s| s.0 > 0 && s.1 > 0).map(|s| (b, s))) {
            Some((b, px)) => { m.bytes = b; m.px = px; false }
            None => { m.failed = true; true }
        }
    });
    viewer_refresh(id);
    failed
}

unsafe fn release(stream: P) {
    // IUnknown::Release is the third vtable slot.
    let vt = *(stream as *const *const unsafe extern "system" fn(P) -> u32);
    (*vt.add(2))(stream);
}

/// Decodes `bytes` into a new w×h bitmap, either filling it (cropping the middle) or fitting
/// inside it, then frees the decoded picture. Only the small bitmap is kept.
unsafe fn render(a: &Api, bytes: &[u8], px: (u32, u32), w: i32, h: i32, crop: bool, like: HDC, bg: u32) -> HBITMAP {
    let stream = (a.mem_stream)(bytes.as_ptr(), bytes.len() as u32);
    if stream.is_null() { return std::ptr::null_mut(); }
    let mut gp = std::ptr::null_mut();
    let mut out: HBITMAP = std::ptr::null_mut();
    if (a.from_stream)(stream, &mut gp) == 0 {
        let dc = CreateCompatibleDC(like);
        out = CreateCompatibleBitmap(like, w, h);
        let old = SelectObject(dc, out as _);
        let brush = CreateSolidBrush(bg);
        FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, brush);
        DeleteObject(brush as _);
        let (iw, ih) = (px.0 as i32, px.1 as i32);
        let (sx, sy, sw, sh, dx, dy, dw, dh) = if crop {
            // Take the largest middle part with the tile's shape.
            let (sw, sh) = if iw * h > ih * w { (ih * w / h, ih) } else { (iw, iw * h / w) };
            ((iw - sw) / 2, (ih - sh) / 2, sw, sh, 0, 0, w, h)
        } else {
            let (dw, dh) = if iw * h > ih * w { (w, ih * w / iw) } else { (iw * h / ih, h) };
            (0, 0, iw, ih, (w - dw) / 2, (h - dh) / 2, dw, dh)
        };
        let mut g = std::ptr::null_mut();
        if (a.graphics)(dc, &mut g) == 0 {
            (a.quality)(g, 7); // high-quality bicubic, so small tiles stay crisp
            (a.draw_part)(g, gp, dx, dy, dw, dh, sx, sy, sw, sh, 2, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            (a.delete_graphics)(g);
        }
        SelectObject(dc, old);
        DeleteDC(dc);
        (a.dispose)(gp);
    }
    release(stream);
    out
}

/// Called when the popup hides. Every picture is kept; only old ones (all but the newest `KEEP_FULL`) are shrunk
/// once to a smaller JPEG, so memory stays low without any picture disappearing.
pub fn settle() {
    close_viewer();
    let n = IMGS.with_borrow(|v| v.len());
    for i in 0..n.saturating_sub(KEEP_FULL) {
        let todo = IMGS.with_borrow(|v| { let m = &v[i]; (!m.shrunk && m.have()).then(|| (m.bytes.clone(), m.px)) });
        let Some((bytes, px)) = todo else { continue };
        let small = if px.0.max(px.1) > SHRINK_TO || bytes.len() > 200 * 1024 { unsafe { shrink(&bytes, px) } } else { None };
        IMGS.with_borrow_mut(|v| {
            let m = &mut v[i];
            m.shrunk = true;
            if let Some((b, p)) = small { if b.len() < m.bytes.len() { m.bytes = b; m.px = p; } }
        });
    }
}

const KEEP_FULL: usize = 24;
const SHRINK_TO: u32 = 800;

/// Re-encodes a picture as a JPEG no larger than SHRINK_TO pixels on its long side. None if anything fails.
unsafe fn shrink(bytes: &[u8], px: (u32, u32)) -> Option<(Vec<u8>, (u32, u32))> {
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    let a = api()?;
    let gp = LoadLibraryW(w("gdiplus.dll").as_ptr());
    let ole = LoadLibraryW(w("ole32.dll").as_ptr());
    if gp.is_null() || ole.is_null() { return None; }
    let thumb_fn: unsafe extern "system" fn(P, u32, u32, *mut P, P, P) -> i32 = std::mem::transmute(GetProcAddress(gp, b"GdipGetImageThumbnail ".as_ptr())?);
    let save: unsafe extern "system" fn(P, P, *const [u8; 16], P) -> i32 = std::mem::transmute(GetProcAddress(gp, b"GdipSaveImageToStream ".as_ptr())?);
    let new_stream: unsafe extern "system" fn(P, i32, *mut P) -> i32 = std::mem::transmute(GetProcAddress(ole, b"CreateStreamOnHGlobal ".as_ptr())?);
    let hglobal_of: unsafe extern "system" fn(P, *mut P) -> i32 = std::mem::transmute(GetProcAddress(ole, b"GetHGlobalFromStream ".as_ptr())?);
    let src = (a.mem_stream)(bytes.as_ptr(), bytes.len() as u32);
    if src.is_null() { return None; }
    let mut img = std::ptr::null_mut();
    let mut out = None;
    if (a.from_stream)(src, &mut img) == 0 && !img.is_null() {
        let long = px.0.max(px.1).max(1);
        let scale = (SHRINK_TO as f32 / long as f32).min(1.0);
        let (tw, th) = (((px.0 as f32 * scale) as u32).max(1), ((px.1 as f32 * scale) as u32).max(1));
        let mut thumb = std::ptr::null_mut();
        if thumb_fn(img, tw, th, &mut thumb, std::ptr::null_mut(), std::ptr::null_mut()) == 0 && !thumb.is_null() {
            let mut st = std::ptr::null_mut();
            if new_stream(std::ptr::null_mut(), 1, &mut st) == 0 && !st.is_null() {
                // The JPEG encoder's id: {557CF401-1A04-11D3-9A73-0000F81EF32E}.
                let clsid: [u8; 16] = [0x01, 0xF4, 0x7C, 0x55, 0x04, 0x1A, 0xD3, 0x11, 0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E];
                let mut hg = std::ptr::null_mut();
                if save(thumb, st, &clsid, std::ptr::null_mut()) == 0 && hglobal_of(st, &mut hg) == 0 && !hg.is_null() {
                    let len = GlobalSize(hg);
                    let p = GlobalLock(hg) as *const u8;
                    if !p.is_null() && len > 0 { out = Some((std::slice::from_raw_parts(p, len).to_vec(), (tw, th))); }
                    GlobalUnlock(hg);
                }
                release(st);
            }
            (a.dispose)(thumb);
        }
        (a.dispose)(img);
    }
    release(src);
    out
}

#[repr(C, packed(4))]
struct FindText { min: i32, max: i32, text: *const u16, found_min: i32, found_max: i32 }

unsafe fn round_fill(dc: HDC, r: &RECT, c: u32, rad: i32) {
    if smooth_round(dc, r, Some((c, 255)), None, rad) { return; }
    let (b, pen) = (CreateSolidBrush(c), CreatePen(PS_SOLID, 1, c));
    let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, pen as _));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(pen as _);
}

unsafe fn glyph(dc: HDC, r: &RECT, text: &str, color: u32, size: i32) {
    let f = CreateFontW(-size, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, w("Segoe UI").as_ptr());
    let of = SelectObject(dc, f as _);
    SetBkMode(dc, 1);
    SetTextColor(dc, color);
    let t = w(text);
    let mut rc = *r;
    DrawTextW(dc, t.as_ptr(), t.len() as i32 - 1, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, of);
    DeleteObject(f as _);
}

/// An anti-aliased rounded rectangle (`rad` is the corner diameter, as with GDI's RoundRect).
/// `fill` and `border` are (color, alpha) and (color, alpha, width). Returns false without GDI+.
pub unsafe fn smooth_round(dc: HDC, r: &RECT, fill: Option<(u32, u32)>, border: Option<(u32, u32, f32)>, rad: i32) -> bool {
    let Some(a) = api() else { return false };
    let argb = |c: u32, alpha: u32| (alpha << 24) | ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255);
    let mut g = std::ptr::null_mut();
    if (a.graphics)(dc, &mut g) != 0 { return false; }
    (a.smoothing)(g, 4);
    // GDI+ edges sit on pixel boundaries; keep a border fully inside the rectangle.
    let inset = border.map_or(0.0, |b| b.2 / 2.0);
    let (l, t, rt, bt) = (r.left as f32 + inset, r.top as f32 + inset, r.right as f32 - inset, r.bottom as f32 - inset);
    let d = (rad as f32).min(rt - l).min(bt - t).max(1.0);
    let mut path = std::ptr::null_mut();
    (a.path_new)(0, &mut path);
    (a.path_arc)(path, l, t, d, d, 180.0, 90.0);
    (a.path_arc)(path, rt - d, t, d, d, 270.0, 90.0);
    (a.path_arc)(path, rt - d, bt - d, d, d, 0.0, 90.0);
    (a.path_arc)(path, l, bt - d, d, d, 90.0, 90.0);
    (a.path_close)(path);
    if let Some((c, al)) = fill {
        let mut b = std::ptr::null_mut();
        (a.solid_fill)(argb(c, al), &mut b);
        (a.fill_path)(g, b, path);
        (a.delete_brush)(b);
    }
    if let Some((c, al, wd)) = border {
        let mut pen = std::ptr::null_mut();
        (a.new_pen)(argb(c, al), wd, 0, &mut pen);
        (a.draw_path)(g, pen, path);
        (a.delete_pen)(pen);
    }
    (a.path_delete)(path);
    (a.delete_graphics)(g);
    true
}

/// A smooth, anti-aliased loading ring with a soft glow: a faint track and a comet whose tail fades,
/// its head at `head_deg` (clockwise from 3 o'clock). Draws straight onto `dc` (no extra memory).
/// Returns false if GDI+ isn't available, so the caller can draw its plain fallback.
pub unsafe fn spinner(dc: HDC, cx: f32, cy: f32, r: f32, width: f32, head_deg: f32, track: u32, color: u32) -> bool {
    let Some(a) = api() else { return false };
    let argb = |c: u32, alpha: u32| (alpha << 24) | ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255);
    let mut g = std::ptr::null_mut();
    if (a.graphics)(dc, &mut g) != 0 { return false; }
    (a.smoothing)(g, 4);
    let arc = |w: f32, alpha: u32, col: u32, from: f32, sweep: f32, round: bool| {
        let mut pen = std::ptr::null_mut();
        (a.new_pen)(argb(col, alpha), w, 0, &mut pen);
        if round { (a.pen_caps)(pen, 2, 2, 0); }
        (a.draw_arc)(g, pen, cx - r, cy - r, 2.0 * r, 2.0 * r, from, sweep);
        (a.delete_pen)(pen);
    };
    arc(width, 0x70, track, 0.0, 359.9, false);
    const SEGS: i32 = 16;
    let seg = 150.0 / SEGS as f32;
    for glow in [true, false] {
        for i in 0..SEGS {
            let k = (i + 1) as f32 / SEGS as f32;
            let from = head_deg - 150.0 + i as f32 * seg;
            let last = i + 1 == SEGS;
            if glow {
                // Two wider, very transparent passes under the comet make it glow.
                arc(width * 3.2, (k * k * 26.0) as u32, color, from, seg + 0.6, last);
                arc(width * 2.0, (k * k * 40.0) as u32, color, from, seg + 0.6, last);
            } else {
                arc(width, (40.0 + k * k * 215.0) as u32, color, from, seg + 0.6, last);
            }
        }
    }
    (a.delete_graphics)(g);
    true
}

/// A round, anti-aliased arrow button: a softly translucent disc with a thin rounded chevron.
unsafe fn arrow_button(dc: HDC, r: &RECT, left: bool, disc: u32, fg: u32) {
    let Some(a) = api() else { return };
    let argb = |c: u32, alpha: u32| (alpha << 24) | ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255);
    let mut g = std::ptr::null_mut();
    if (a.graphics)(dc, &mut g) != 0 { return; }
    (a.smoothing)(g, 4); // anti-alias
    let (x, y, d) = (r.left as f32, r.top as f32, (r.right - r.left) as f32);
    let mut b = std::ptr::null_mut();
    (a.solid_fill)(argb(disc, 0xE6), &mut b);
    (a.fill_ellipse)(g, b, x, y, d, d);
    (a.delete_brush)(b);
    let mut pen = std::ptr::null_mut();
    (a.new_pen)(argb(fg, 0xFF), (d * 0.09).max(1.5), 0, &mut pen);
    (a.pen_caps)(pen, 2, 2, 0); // round ends
    (a.pen_join)(pen, 2); // round corner
    let (cx, cy, k) = (x + d / 2.0, y + d / 2.0, d * 0.16);
    let dir = if left { 1.0 } else { -1.0 };
    let pts = [[cx + dir * k * 0.6, cy - k * 1.2], [cx - dir * k * 0.6, cy], [cx + dir * k * 0.6, cy + k * 1.2]];
    (a.draw_lines)(g, pen, pts.as_ptr(), 3);
    (a.delete_pen)(pen);
    (a.delete_graphics)(g);
}

/// Draws every gallery row in view: cached tiles copied with BitBlt, so repaints and scrolling cost
/// almost nothing. Costs nothing when a chat has no pictures.
pub fn paint(view: HWND, c: Colors) {
    let count = GALLERIES.with_borrow(|g| g.len());
    if count == 0 { return; }
    let Some(a) = api() else { return };
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(view, &mut rc);
        let dpi = GetDpiForWindow(view).max(96) as i32;
        // Tiles grow with the chat's zoom, as the text line they sit on does.
        let (mut zn, mut zd) = (0i32, 0i32);
        SendMessageW(view, 0x4E0, &mut zn as *mut _ as usize, &mut zd as *mut _ as isize); // EM_GETZOOM
        let (zn, zd) = if zn > 0 && zd > 0 { (zn, zd) } else { (1, 1) };
        let s = |v: i32| v * dpi / 96 * zn / zd;
        let (tw, th, gap) = (s(TILE_W), s(TILE_H), s(GAP));
        let hdc = super::bubbles::dc_of(view);
        // The chat view reaches under the typing box and header; never draw tiles over sibling windows.
        let mut sib = GetWindow(view, GW_HWNDFIRST);
        while !sib.is_null() {
            if sib != view && IsWindowVisible(sib) != 0 {
                let mut sr: RECT = std::mem::zeroed();
                GetWindowRect(sib, &mut sr);
                let mut p = [windows_sys::Win32::Foundation::POINT { x: sr.left, y: sr.top }, windows_sys::Win32::Foundation::POINT { x: sr.right, y: sr.bottom }];
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(view, &mut p[0]);
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(view, &mut p[1]);
                ExcludeClipRect(hdc, p[0].x, p[0].y, p[1].x, p[1].y);
            }
            sib = GetWindow(sib, GW_HWNDNEXT);
        }
        let mem = CreateCompatibleDC(hdc);
        let main = GetParent(view);
        for gid in 0..count {
            let needle = [(FIRST_CHAR + gid as u32) as u16, 0];
            // RichEdit finds its own positions, which stay correct around tables.
            let mut f = FindText { min: 0, max: -1, text: needle.as_ptr(), found_min: -1, found_max: -1 };
            let cp = SendMessageW(view, 0x47C, 1, &mut f as *mut _ as isize); // EM_FINDTEXTEXW, FR_DOWN
            if cp < 0 { continue; }
            let mut pt = [0i32; 2];
            SendMessageW(view, 0xD6, pt.as_mut_ptr() as usize, cp); // EM_POSFROMCHAR
            let (x0, y0) = (pt[0], pt[1]);
            if y0 > rc.bottom || y0 + th < 0 { continue; }
            let ids = GALLERIES.with_borrow(|g| g[gid].imgs.clone());
            // Download the whole row now, so pictures off to the right are ready when paged to.
            for &id in &ids { start_fetch(main, id); }
            // As many tiles as fit; arrows page through the rest. Always a single row.
            let fit = ((rc.right - x0 - s(8) + gap) / (tw + gap)).max(1) as usize;
            let shown = fit.min(ids.len());
            let offset = GALLERIES.with_borrow_mut(|g| { let o = g[gid].offset.min(ids.len() - shown); g[gid].offset = o; o });
            let mut tiles = Vec::new();
            for (k, &id) in ids.iter().enumerate().skip(offset).take(shown) {
                let x = x0 + (k - offset) as i32 * (tw + gap);
                let r = RECT { left: x, top: y0, right: x + tw, bottom: y0 + th };
                tiles.push((k, r));
                let ready = IMGS.with_borrow(|v| v[id].have());
                if !ready {
                    round_fill(hdc, &r, c.tile, s(8));
                    start_fetch(main, id);
                    continue;
                }
                let thumb = IMGS.with_borrow_mut(|v| {
                    let m = &mut v[id];
                    if m.thumb.is_null() || m.thumb_px != (tw, th) {
                        if !m.thumb.is_null() { DeleteObject(m.thumb as _); }
                        m.thumb = render(&a, &m.read(), m.px, tw, th, true, hdc, c.tile);
                        m.thumb_px = (tw, th);
                    }
                    m.thumb
                });
                if thumb.is_null() { round_fill(hdc, &r, c.tile, s(8)); continue; }
                let old = SelectObject(mem, thumb as _);
                BitBlt(hdc, r.left, r.top, tw, th, mem, 0, 0, SRCCOPY);
                SelectObject(mem, old);
            }
            let arrow = |left: bool| -> RECT {
                let edge = if left { x0 } else { x0 + shown as i32 * (tw + gap) - gap };
                let cx = if left { edge + s(4) } else { edge - s(4) - s(ARROW) };
                RECT { left: cx, top: y0 + (th - s(ARROW)) / 2, right: cx + s(ARROW), bottom: y0 + (th + s(ARROW)) / 2 }
            };
            let prev = (offset > 0).then(|| arrow(true));
            let next = (offset + shown < ids.len()).then(|| arrow(false));
            for (r, left) in [(prev, true), (next, false)] {
                if let Some(r) = r { arrow_button(hdc, &r, left, c.bg, c.text); }
            }
            GALLERIES.with_borrow_mut(|g| { g[gid].tiles = tiles; g[gid].prev = prev; g[gid].next = next; });
        }
        DeleteDC(mem);
        super::bubbles::release(view, hdc);
    }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool { x >= r.left && x < r.right && y >= r.top && y < r.bottom }

/// True when the point is over a picture tile or a row's arrow button.
pub fn over_control(x: i32, y: i32) -> bool {
    GALLERIES.with_borrow(|g| g.iter().any(|gal| {
        gal.prev.is_some_and(|r| inside(&r, x, y)) || gal.next.is_some_and(|r| inside(&r, x, y)) || gal.tiles.iter().any(|(_, r)| inside(r, x, y))
    }))
}

/// Keyboard: opens the first visible picture of the newest row.
pub fn key_open(view: HWND, c: Colors) -> bool {
    let hit = GALLERIES.with_borrow(|g| g.iter().enumerate().rev().find_map(|(gid, gal)| gal.tiles.first().map(|(k, _)| (gid, *k))));
    match hit { Some((gid, k)) => { open_viewer(view, gid, k, c); true } None => false }
}

/// Keyboard: pages the newest row back (`forward` false) or on.
pub fn key_page(view: HWND, forward: bool) -> bool {
    let done = GALLERIES.with_borrow_mut(|g| {
        let Some(gal) = g.iter_mut().rev().find(|x| !x.tiles.is_empty()) else { return false };
        let per = gal.tiles.len().max(1);
        if forward && gal.next.is_some() { gal.offset += per; true }
        else if !forward && gal.prev.is_some() { gal.offset = gal.offset.saturating_sub(per); true }
        else { false }
    });
    if done { unsafe { InvalidateRect(view, std::ptr::null(), 0); } }
    done
}

/// A click in the chat: arrows page a row, a tile opens the viewer. True when it was handled.
pub fn click(view: HWND, x: i32, y: i32, c: Colors) -> bool {
    let hit = GALLERIES.with_borrow_mut(|g| {
        for (gid, gal) in g.iter_mut().enumerate() {
            let per = gal.tiles.len().max(1);
            if gal.prev.is_some_and(|r| inside(&r, x, y)) { gal.offset = gal.offset.saturating_sub(per); return Some((gid, None)); }
            if gal.next.is_some_and(|r| inside(&r, x, y)) { gal.offset += per; return Some((gid, None)); }
            if let Some((k, _)) = gal.tiles.iter().find(|(_, r)| inside(r, x, y)) { return Some((gid, Some(*k))); }
        }
        None
    });
    match hit {
        Some((_, None)) => unsafe { InvalidateRect(view, std::ptr::null(), 0); true },
        Some((gid, Some(k))) => { open_viewer(view, gid, k, c); true }
        None => false,
    }
}

// ---- The viewer: one picture at a time, as large as the window allows. ----

struct Viewer { hwnd: HWND, gid: usize, index: usize, bmp: HBITMAP, colors: Colors }
thread_local! { static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) }; static BACK: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) }; }

pub fn viewer_open() -> bool { VIEWER.with_borrow(|v| v.is_some()) }

pub fn close_viewer() {
    if let Some(v) = VIEWER.with_borrow_mut(|v| v.take()) {
        unsafe {
            if !v.bmp.is_null() { DeleteObject(v.bmp as _); }
            DestroyWindow(v.hwnd);
            super::app::viewer_mode(false);
            let back = BACK.get();
            if !back.is_null() { SetFocus(back); }
        }
    }
}

/// Fits the viewer to the window after a resize.
pub fn fit_viewer(main: HWND) {
    let h = VIEWER.with_borrow(|v| v.as_ref().map(|v| v.hwnd));
    if let Some(h) = h {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(main, &mut r);
            MoveWindow(h, 0, 0, r.right, r.bottom, 1);
        }
        show_index(0);
    }
}

fn viewer_refresh(id: usize) {
    let current = VIEWER.with_borrow(|v| v.as_ref().map(|v| GALLERIES.with_borrow(|g| g.get(v.gid).and_then(|x| x.imgs.get(v.index).copied()))).flatten());
    if current == Some(id) { show_index(0); }
}

fn open_viewer(view: HWND, gid: usize, index: usize, colors: Colors) {
    close_viewer();
    unsafe {
        let main = GetParent(view);
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("LitePilotViewer");
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(viewer_proc);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_HAND);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
        });
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(main, &mut r);
        let h = CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, r.right, r.bottom, main, std::ptr::null_mut(), inst, std::ptr::null());
        SetWindowPos(h, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        BACK.set(GetFocus());
        super::app::viewer_mode(true);
        VIEWER.set(Some(Viewer { hwnd: h, gid, index, bmp: std::ptr::null_mut(), colors }));
        SetFocus(h);
        show_index(0);
    }
}

/// Moves by `step` (0 = re-render the current one), decodes that picture to fit, and repaints.
fn show_index(step: isize) {
    let Some((h, gid, mut index, colors)) = VIEWER.with_borrow(|v| v.as_ref().map(|v| (v.hwnd, v.gid, v.index, v.colors))) else { return };
    let Some(ids) = GALLERIES.with_borrow(|g| g.get(gid).map(|x| x.imgs.clone())) else { return close_viewer() };
    if ids.is_empty() { return close_viewer(); }
    index = (index as isize + step).clamp(0, ids.len() as isize - 1) as usize;
    let id = ids[index];
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let s = |v: i32| v * GetDpiForWindow(h).max(96) as i32 / 96;
        let (bw, bh) = ((r.right - 2 * s(56)).max(s(40)), (r.bottom - s(70)).max(s(40)));
        let (bytes, px) = IMGS.with_borrow(|v| (v[id].read(), v[id].px));
        if bytes.is_empty() { start_fetch(GetParent(h), id); }
        let bmp = match (api(), bytes.is_empty()) {
            (Some(a), false) => { let dc = GetDC(h); let b = render(&a, &bytes, px, bw, bh, false, dc, colors.bg); ReleaseDC(h, dc); b }
            _ => std::ptr::null_mut(),
        };
        VIEWER.with_borrow_mut(|v| if let Some(v) = v.as_mut() {
            if !v.bmp.is_null() { DeleteObject(v.bmp as _); }
            v.bmp = bmp;
            v.index = index;
        });
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

unsafe extern "system" fn viewer_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_SYSCHAR { return 0; }
    if msg == WM_MENUCHAR { return (1 << 16) as isize; }
    let s = |v: i32| v * GetDpiForWindow(h).max(96) as i32 / 96;
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            if let Some((gid, index, bmp, c)) = VIEWER.with_borrow(|v| v.as_ref().map(|v| (v.gid, v.index, v.bmp, v.colors))) {
                let brush = CreateSolidBrush(c.bg);
                FillRect(dc, &r, brush);
                DeleteObject(brush as _);
                let (bw, bh) = ((r.right - 2 * s(56)).max(s(40)), (r.bottom - s(70)).max(s(40)));
                let (x, y) = (s(56), s(16));
                if bmp.is_null() {
                    glyph(dc, &RECT { left: x, top: y, right: x + bw, bottom: y + bh }, "Loading\u{2026}", c.text, s(14));
                } else {
                    let mem = CreateCompatibleDC(dc);
                    let old = SelectObject(mem, bmp as _);
                    BitBlt(dc, x, y, bw, bh, mem, 0, 0, SRCCOPY);
                    SelectObject(mem, old);
                    DeleteDC(mem);
                }
                let ids = GALLERIES.with_borrow(|g| g.get(gid).map(|x| x.imgs.clone())).unwrap_or_default();
                let label = ids.get(index).map(|&id| IMGS.with_borrow(|v| v[id].label.clone())).unwrap_or_default();
                let caption = if label.is_empty() { format!("{} of {}", index + 1, ids.len()) } else { format!("{label} \u{00B7} {} of {}", index + 1, ids.len()) };
                glyph(dc, &RECT { left: 0, top: r.bottom - s(52), right: r.right, bottom: r.bottom - s(30) }, &caption, c.text, s(13));
                glyph(dc, &RECT { left: 0, top: r.bottom - s(30), right: r.right, bottom: r.bottom - s(8) }, "\u{2190} \u{2192} to browse  \u{00B7}  Esc to close", c.text, s(11));
                let mid = y + bh / 2;
                if index > 0 { arrow_button(dc, &RECT { left: s(12), top: mid - s(18), right: s(48), bottom: mid + s(18) }, true, c.tile, c.text); }
                if index + 1 < ids.len() { arrow_button(dc, &RECT { left: r.right - s(48), top: mid - s(18), right: r.right - s(12), bottom: mid + s(18) }, false, c.tile, c.text); }
                let x_btn = RECT { left: r.right - s(40), top: s(8), right: r.right - s(8), bottom: s(40) };
                glyph(dc, &x_btn, "\u{2715}", c.text, s(15));
            }
            EndPaint(h, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            let x = (lp & 0xFFFF) as i16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            if y > s(48) && x < s(56) { show_index(-1); } else if y > s(48) && x > r.right - s(56) { show_index(1); } else { close_viewer(); }
            0
        }
        WM_KEYDOWN => {
            match wp as u16 {
                VK_LEFT => show_index(-1),
                VK_RIGHT => show_index(1),
                VK_ESCAPE => close_viewer(),
                _ => {}
            }
            0
        }
        WM_GETDLGCODE => DLGC_WANTALLKEYS as isize,
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registers_and_dedupes_png() {
        let u = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
        let a = register_gallery(&[("p", u)]).unwrap();
        assert_eq!(a, register_gallery(&[("p", u)]).unwrap());
        assert_eq!(a.1, (TILE_H * TWIPS_PER_PX) as u32);
        assert!(register_gallery(&[("p", "data:image/png;base64,iVBORw0KGgo=")]).is_none());
        assert!(register_gallery(&[("p", "data:image/webp;base64,AAAA")]).is_none());
    }
    #[test]
    fn a_row_holds_every_picture() {
        let (g, _) = register_gallery(&[("a", "https://x.y/1.jpg"), ("b", "wiki:Colchester"), ("c", "https://x.y/3.jpg")]).unwrap();
        assert_eq!(GALLERIES.with_borrow(|v| v[g as usize].imgs.len()), 3);
    }
    #[test]
    fn sizes_from_headers() {
        let b: Vec<u8> = [&[0x89, 0x50, 0x4E, 0x47][..], &[0; 12], &[0, 0, 0x07, 0xD0], &[0, 0, 0x03, 0xE8]].concat();
        assert_eq!(image_size(&b), Some((2000, 1000)));
        let j = [0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 20, 0, 30];
        assert_eq!(image_size(&j), Some((30, 20)));
        assert_eq!(image_size(b"GIF89a\x05\x00\x07\x00"), Some((5, 7)));
    }
}
