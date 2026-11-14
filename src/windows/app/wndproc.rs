//! The tray icon and the main window procedure.
use super::*;

pub(super) fn tray(hwnd: HWND, op: u32) {
    unsafe {
        let mut n: NOTIFYICONDATAW = std::mem::zeroed();
        n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = hwnd;
        n.uID = 1;
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        n.uCallbackMessage = WM_TRAY;
        n.hIcon = app_icon(true);
        for (i, c) in "LitePilot".encode_utf16().enumerate() { n.szTip[i] = c; }
        Shell_NotifyIconW(op, &n);
    }
}

pub(super) fn tray_menu(hwnd: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, CMD_OPEN, w("Open").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_SETTINGS, w("Settings").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_QUIT, w("Quit").as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        // The version, greyed out: for reference when reporting a problem.
        AppendMenuW(m, MF_STRING | MF_GRAYED, 0, w(&format!("LitePilot {}", crate::core::VERSION)).as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(m, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null());
        DestroyMenu(m);
    }
}

pub(super) unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SYSCHAR => 0,
        WM_MENUCHAR => (1 << 16) as isize,
        // Shortcuts also work when the window itself has focus (after clicking empty space, for example).
        WM_KEYDOWN | WM_SYSKEYDOWN if app().visible && on_key(wp as u16) => 0,
        WM_HOTKEY if wp == 3 => { show(); 0 }
        WM_HOTKEY | WM_TOGGLE => {
            // The shortcut hides the window only while you are in it. If it is open but behind another
            // program (keep on top off), the first press brings it forward instead of hiding it.
            let a = app();
            if a.visible && GetForegroundWindow() == a.hwnd && IsIconic(a.hwnd) == 0 { hide(); } else { show(); }
            0
        }
        WM_EVENT => {
            let e = Box::from_raw(lp as *mut Event);
            on_event(wp as u64, *e);
            0
        }
        images::WM_IMAGE => {
            let got = *Box::from_raw(lp as *mut Option<Vec<u8>>);
            let a = app();
            if images::loaded(wp, got) && a.visible && !a.streaming { render_keep(); } else { InvalidateRect(a.view, std::ptr::null(), 0); }
            0
        }
        WM_TIMER => {
            if wp == ID_RENDER {
                let a = app();
                if a.dirty { render_tail(); }
                a.spin = a.spin.wrapping_add(1);
                // The ring on the Stop button turns while a reply loads, about 30 frames a second.
                if a.visible { InvalidateRect(a.send_btn, std::ptr::null(), 0); status_paint(a.view); }
            }
            if wp == ID_BLINK {
                // Make the typing caret flash: the built-in blink isn't visible here, so toggle it.
                let a = app();
                if GetFocus() == a.input {
                    if CARET_HIDDEN.get() { ShowCaret(a.input); } else { HideCaret(a.input); }
                    CARET_HIDDEN.set(!CARET_HIDDEN.get());
                }
            }
            if wp == ID_UPDATE && !app().streaming {
                let a = app();
                if crate::windows::updater::on_tick(a.visible, &mut a.cfg) { save_config(); }
            }
            if wp == key_badges::TIMER { key_badges::show(); }
            if wp == ID_ZOOMED { KillTimer(h, ID_ZOOMED); apply_zoom(); }
            if wp == ID_RESIZED {
                KillTimer(h, ID_RESIZED);
                let a = app();
                if a.visible && !a.streaming { render_keep(); }
            }
            if wp == ID_COPIED {
                KillTimer(h, ID_COPIED);
                update_header();
                if COPY_DONE.get() { COPY_DONE.set(false); InvalidateRect(app().view, std::ptr::null(), 0); }
            }
            0
        }
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => { if app().visible { hide(); } else { show(); } }
                WM_RBUTTONUP => tray_menu(h),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            // EN_VSCROLL: the view scrolled itself (keys, caret, selection drag).
            if (wp >> 16) & 0xFFFF == 0x602 && lp as HWND == app().view {
                RedrawWindow(lp as HWND, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                return 0;
            }
            match wp & 0xFFFF {
                CMD_OPEN => show(),
                CMD_SETTINGS => settings::open(),
                ID_SETTINGS => { tile_menu::toggle(); SetFocus(app().input); }
                CMD_QUIT => { DestroyWindow(h); }
                ID_PROFILE => {
                    if view_open() { close_views(); } else { chip_menu::toggle(); SetFocus(app().input); }
                }
                ID_HISTORY => { if history_view::is_open() { close_views(); } else { history_view::open(); } }
                ID_NEW => {
                    close_views();
                    let p = history::new_chat_path(&app().data.join("chats"));
                    open_chat(p);
                    SetFocus(app().input);
                }
                history_view::ID_SEARCH if (wp >> 16) as u32 == 0x300 => history_view::filter(), // EN_CHANGE
                history_view::ID_LIST if (wp >> 16) as u32 == 2 => history_view::open_selected(), // LBN_DBLCLK
                actions_view::ID_SEARCH if (wp >> 16) as u32 == 0x300 => actions_view::filter(),
                actions_view::ID_LIST if (wp >> 16) as u32 == 2 => actions_view::run_selected(),
                ID_PIN => { toggle_on_top(); SetFocus(app().input); }
                ID_MIC => dictate(),
                ID_SEND => { if app().streaming { stop(); } else { send(); } SetFocus(app().input); }
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let nm = lp as *const NMHDR;
            if (*nm).code == EN_LINK_ { open_link(lp as *const ENLINK); }
            // Files dropped on the typing box or the chat: attach them rather than embed them.
            if (*nm).code == 0x703 { attach_win::dropped(*(lp as *const usize).add(3) as HANDLE); return 1; } // EN_DROPFILES: hDrop follows the NMHDR
            0
        }
        WM_SETFOCUS => {
            // Alt+Tab or a click on the title bar leaves focus on the window itself; send it to the typing box.
            let a = app();
            if a.visible && !view_open() { SetFocus(a.input); }
            0
        }
        // Keep-on-top guard. Whatever asks to restack the window (our own code, the shell, another program),
        // it stays in the top-most band while the setting is on, and is put back if it ever slips out.
        WM_WINDOWPOSCHANGING => {
            let a = app();
            let p = &mut *(lp as *mut WINDOWPOS);
            // Going right under one of LitePilot's own windows (the Alt labels, a menu) is left alone: Windows
            // does that to keep owned popups in front, and forcing the very top here buried them.
            let under_own = {
                let ins = p.hwndInsertAfter;
                let mut pid = 0u32;
                (ins as isize) > 1 && { GetWindowThreadProcessId(ins, &mut pid); pid == GetCurrentProcessId() }
            };
            if a.cfg.on_top && a.visible && !under_own && p.flags & (SWP_NOZORDER | SWP_HIDEWINDOW) == 0 && p.hwndInsertAfter as isize != -1 {
                p.hwndInsertAfter = HWND_TOPMOST;
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_WINDOWPOSCHANGED => {
            let a = app();
            if a.cfg.on_top && a.visible && IsIconic(h) == 0 && GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST as isize == 0 {
                SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            lift_settings(a);
            key_badges::lift();
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_ACTIVATE => {
            let a = app();
            set_title(wp & 0xFFFF != 0);
            lift_settings(a);
            if wp & 0xFFFF != 0 && a.visible && !view_open() && !chip_menu::is_open() && !tile_menu::has_keys() { SetFocus(a.input); }
            if wp & 0xFFFF == 0 {
                chip_menu::close();
                tile_menu::close();
                // Switching apps must not drop "keep on top".
                if a.cfg.on_top && a.visible && IsIconic(a.hwnd) == 0 { SetWindowPos(a.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            }
            if wp & 0xFFFF == 0 && a.cfg.hide_on_blur && a.visible { hide(); }
            0
        }
        WM_CTLCOLOREDIT => {
            let a = app();
            SetTextColor(wp as HDC, a.theme.text);
            SetBkColor(wp as HDC, a.theme.code);
            a.field_brush as isize
        }
        WM_CTLCOLORLISTBOX => {
            SetBkColor(wp as HDC, app().theme.bg);
            app().bg_brush as isize
        }
        WM_MEASUREITEM => {
            let m = &mut *(lp as *mut windows_sys::Win32::UI::Controls::MEASUREITEMSTRUCT);
            if m.CtlID as usize == history_view::ID_LIST { history_view::measure(m); }
            if m.CtlID as usize == actions_view::ID_LIST { actions_view::measure(m); }
            1
        }
        // Alt+F4 hides, like Esc. Quit is in the tray menu.
        WM_CLOSE => { hide(); 0 }
        WM_QUIT_APP => { DestroyWindow(h); 0 }
        // The title bar and frame are Windows' own (minimise, maximise, close, Snap). Empty space
        // in the window still drags it, as before.
        WM_NCHITTEST => {
            let r = DefWindowProcW(h, msg, wp, lp);
            if r == HTCLIENT as isize { HTCAPTION as isize } else { r }
        }
        WM_GETMINMAXINFO => {
            let mm = &mut *(lp as *mut MINMAXINFO);
            let d = GetDpiForWindow(h).max(96) as i32;
            mm.ptMinTrackSize = POINT { x: 380 * d / 96, y: 300 * d / 96 };
            0
        }
        WM_SIZE => {
            if !app().header.is_null() {
                place_children();
                images::fit_viewer(h);
                // Maximise, restore, Snap and dragging all end here: once the size settles, lay the chat out again so
                // text, tables and code cards fit the new width.
                SetTimer(h, ID_RESIZED, 120, None);
            }
            0
        }
        WM_EXITSIZEMOVE => {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            let a = app();
            let d = GetDpiForWindow(h).max(96) as i32;
            a.cfg.window_pos = Some((r.left, r.top));
            a.cfg.window_size = Some(((r.right - r.left) * 96 / d, (r.bottom - r.top) * 96 / d));
            save_config();
            a.card_tw = 0;
            set_card_width();
            if a.visible && !a.streaming { render_keep(); }
            0
        }
        WM_ERASEBKGND => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            let a = app();
            FillRect(wp as HDC, &r, a.bg_brush);
            if images::viewer_open() { return 1; }
            let p = a.pill;
            let old_b = SelectObject(wp as HDC, a.field_brush as _);
            let pen = CreatePen(PS_SOLID, 1, a.theme.code);
            let old_p = SelectObject(wp as HDC, pen as _);
            let rad = 36 * GetDpiForWindow(h).max(96) as i32 / 96; // fully round at one row, softer when it grows
            if !images::smooth_round(wp as HDC, &p, Some((a.theme.code, 255)), None, rad) { RoundRect(wp as HDC, p.left, p.top, p.right, p.bottom, rad, rad); }
            SelectObject(wp as HDC, old_b);
            SelectObject(wp as HDC, old_p);
            DeleteObject(pen as _);
            if a.input_lines >= 2 {
                // A hairline between the text and the controls row.
                let dpi = GetDpiForWindow(h).max(96) as i32;
                let y = p.bottom - 40 * dpi / 96;
                let pen = CreatePen(PS_SOLID, 1, a.theme.bubble);
                let old_p = SelectObject(wp as HDC, pen as _);
                MoveToEx(wp as HDC, p.left + 14 * dpi / 96, y, std::ptr::null_mut());
                LineTo(wp as HDC, p.right - 14 * dpi / 96, y);
                SelectObject(wp as HDC, old_p);
                DeleteObject(pen as _);
            }
            1
        }
        WM_DRAWITEM => {
            let a = app();
            let d = &*(lp as *const DRAWITEMSTRUCT);
            let id = d.CtlID as usize;
            if id == history_view::ID_LIST { history_view::draw(d); return 1; }
            if id == actions_view::ID_LIST { actions_view::draw(d); return 1; }
            let is_send = id == ID_SEND;
            if is_send && a.streaming { draw_busy_button(d); return 1; }
            let (hot, pressed) = (a.hot == d.hwndItem, d.itemState & ODS_SELECTED != 0);
            let on = id == ID_PIN && a.cfg.on_top;
            // Everything now sits in the input pill, so it shares the pill's color.
            FillRect(d.hDC, &d.rcItem, a.field_brush);
            if hot || pressed || id == ID_PROFILE {
                // Pressed sinks into the surface behind it; hover lifts. The model chip always has a soft shade,
                // halfway between the pill and the hover color. Brushes exist only while drawing.
                let mix = |x: u32, y: u32| (0..3).fold(0u32, |acc, i| acc | ((((x >> (8 * i)) & 255) + ((y >> (8 * i)) & 255)) / 2) << (8 * i));
                let c = if pressed { if is_send { a.theme.bg } else { a.theme.code } } else if hot { a.theme.bubble } else { mix(a.theme.code, a.theme.bubble) };
                let (b, pen) = (CreateSolidBrush(c), CreatePen(PS_SOLID, 1, c));
                let (ob, op) = (SelectObject(d.hDC, b as _), SelectObject(d.hDC, pen as _));
                let r = d.rcItem;
                if !images::smooth_round(d.hDC, &r, Some((c, 255)), None, 12) { RoundRect(d.hDC, r.left, r.top, r.right, r.bottom, 12, 12); }
                SelectObject(d.hDC, ob);
                SelectObject(d.hDC, op);
                DeleteObject(b as _);
                DeleteObject(pen as _);
            }
            SetBkMode(d.hDC, 1);
            SetTextColor(d.hDC, if is_send || on { a.theme.accent } else if hot { a.theme.text } else { a.theme.muted });
            let mut rc = d.rcItem;
            if id == ID_PROFILE {
                if a.streaming { SetTextColor(d.hDC, a.theme.accent); }
                let old = SelectObject(d.hDC, a.small_font as _);
                let t = w(&get_text(d.hwndItem));
                let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
                rc.left += dpi * 9 / 96;
                if let Some(kind) = chip_logo(a) {
                    let color = match kind { "claude" => 0x5777D9, "gemini" => 0xF48542, _ => a.theme.muted };
                    draw_logo(d.hDC, kind, rc.left, (rc.top + rc.bottom) / 2, 14 * dpi / 96, color);
                    rc.left += 20 * dpi / 96;
                }
                DrawTextW(d.hDC, t.as_ptr(), t.len() as i32 - 1, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                SelectObject(d.hDC, old);
            } else {
                let old = SelectObject(d.hDC, a.icon_font as _);
                let busy = is_send && a.streaming;
                let glyph = w(match id { ID_SEND if busy => "\u{E71A}", ID_SEND => "\u{E74A}", ID_HISTORY => "\u{E81C}", ID_MIC => "\u{E720}", ID_NEW => "\u{E710}", ID_PIN if on => "\u{E718}", ID_PIN => "\u{E77A}", _ => "\u{E700}" });
                DrawTextW(d.hDC, glyph.as_ptr(), 1, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(d.hDC, old);
            }
            1
        }
        WM_DESTROY => {
            stop();
            tray(h, NIM_DELETE);
            PostQuitMessage(0);
            0
        }
        _ => {
            if msg == TASKBAR_CREATED.load(Ordering::Relaxed) as u32 { tray(h, NIM_ADD); }
            DefWindowProcW(h, msg, wp, lp)
        }
    }
}
pub(super) static TASKBAR_CREATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
