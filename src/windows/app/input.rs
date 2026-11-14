//! Subclassed child controls: the typing box and chat (keys, paste, wheel, right-click menu) and toolbar buttons.
use super::*;

thread_local! {
    /// Wheel movement not yet a full notch, so small touchpad steps still add up to a zoom step.
    pub(super) static WHEEL_REST: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Where the middle-button drag last was (client y), while one is in progress.
    pub(super) static DRAG_Y: Cell<Option<i32>> = const { Cell::new(None) };
}

/// The right-click menu for the typing box (undo, cut, copy, paste, delete, select all) and the chat (copy,
/// select all). A right-click outside the selection first selects the word under the pointer. `at` is a screen
/// point, or None when opened from the keyboard (Shift+F10 or the menu key).
pub(super) unsafe fn context_menu(h: HWND, at: Option<(i32, i32)>) {
    let a = app();
    let is_input = h == a.input;
    let pick = |m: u32, wp: usize, lp: isize| SendMessageW(h, m, wp, lp);
    let sel = || { let (mut s, mut e) = (0i32, 0i32); pick(0xB0, &mut s as *mut _ as usize, &mut e as *mut _ as isize); (s, e) }; // EM_GETSEL
    SetFocus(h);
    let (x, y) = at.unwrap_or_else(|| { let mut r: RECT = std::mem::zeroed(); GetWindowRect(h, &mut r); (r.left + 24, r.top + 24) });
    if at.is_some() {
        let mut c = POINT { x, y };
        ScreenToClient(h, &mut c);
        let idx = pick(0xD7, 0, &c as *const _ as isize) as i32; // EM_CHARFROMPOS
        let (s, e) = sel();
        if idx >= 0 && (idx < s || idx >= e) {
            let (ws, we) = (pick(0x44C, 6, idx as isize) as i32, pick(0x44C, 7, idx as isize) as i32); // EM_FINDWORDBREAK: word start, word end
            if we > ws { pick(0xB1, ws as usize, we as isize); } else { pick(0xB1, idx as usize, idx as isize); } // EM_SETSEL
        }
    }
    let has_sel = { let (s, e) = sel(); e > s };
    let has_text = text_len(h) > 0;
    let m = CreatePopupMenu();
    let item = |id: usize, label: &str, on: bool| { AppendMenuW(m, MF_STRING | if on { 0 } else { MF_GRAYED }, id, w(label).as_ptr()); };
    if is_input {
        item(1, "Undo\tCtrl+Z", pick(0xC6, 0, 0) != 0); // EM_CANUNDO
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        item(2, "Cut\tCtrl+X", has_sel);
    }
    item(3, "Copy\tCtrl+C", has_sel);
    if is_input {
        item(4, "Paste\tCtrl+V", pick(0x432, 0, 0) != 0); // EM_CANPASTE (any format)
        item(5, "Delete", has_sel);
    }
    AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
    item(6, "Select all\tCtrl+A", has_text);
    let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, 0, a.hwnd, std::ptr::null());
    DestroyMenu(m);
    match cmd {
        1 => { pick(WM_UNDO, 0, 0); }
        2 => { pick(WM_CUT, 0, 0); }
        3 => { pick(WM_COPY, 0, 0); }
        4 => { pick(WM_PASTE, 0, 0); } // goes through child_proc, so pictures and files paste as attachments
        5 => { pick(0xC2, 0, w("").as_ptr() as isize); } // EM_REPLACESEL with nothing
        6 => { pick(0xB1, 0, -1); }
        _ => {}
    }
}

/// Sees every key LitePilot's window thread reads, whichever control has focus (the typing box, the chat,
/// a button, the window itself). Alt held alone shows the shortcut labels; any other key cancels them,
/// so Alt+Left and the like don't flash them. Only while LitePilot is the active window.
pub(super) unsafe extern "system" fn alt_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let a = app();
        let up = (lp >> 31) & 1 == 1;
        let ours = a.visible && GetForegroundWindow() == a.hwnd;
        if matches!(wp as u16, VK_MENU | VK_LMENU | VK_RMENU) {
            if ours || up { key_badges::alt(!up); }
        } else if !up && !(matches!(wp as u16, VK_CONTROL | VK_LCONTROL) && GetAsyncKeyState(VK_RMENU as i32) < 0) {
            // (Right Alt is AltGr on many layouts: Windows adds a fake Left Ctrl to it, which must not cancel the labels.)
            key_badges::stop();
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

pub(super) unsafe extern "system" fn child_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    let is_input = h == a.input;
    // Right-click: our own menu, and the selection stays as it was until the menu decides.
    match msg {
        WM_RBUTTONDOWN => return 0,
        WM_RBUTTONUP => { let mut p = POINT { x: lp as i16 as i32, y: (lp >> 16) as i16 as i32 }; ClientToScreen(h, &mut p); context_menu(h, Some((p.x, p.y))); return 0; }
        WM_CONTEXTMENU => { context_menu(h, if lp == -1 { None } else { Some((lp as i16 as i32, (lp >> 16) as i16 as i32)) }); return 0; }
        _ => {}
    }
    if msg == WM_PAINT && !is_input {
        // The whole chat is drawn off-screen (text, rounded bubbles, pictures, status) and shown in one go,
        // so there is no moment with square bubble corners or half-drawn pieces.
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, rc.right.max(1), rc.bottom.max(1));
        let old = SelectObject(mem, bmp as _);
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_view), h, 0x0318, mem as usize, 4 | 8); // WM_PRINTCLIENT: PRF_CLIENT | PRF_ERASEBKGND
        bubbles::begin(mem);
        bubbles::paint(h, a.theme.bg, rtf::cell_shade(a.theme.bubble));
        images::paint(h, pic_colors(a));
        scrollbar_paint(h, mem);
        stat_paint(h);
        status_paint(h);
        bubbles::end();
        BitBlt(dc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(h, &ps);
        return 0;
    }
    if msg == WM_PAINT {
        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_input), h, msg, wp, lp);
        paint_prompt(h);
        return r;
    }
    // Focus moving between the chat and the typing box changes the ring and the Tab hint.
    if matches!(msg, WM_SETFOCUS | WM_KILLFOCUS) {
        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(if is_input { a.old_input } else { a.old_view }), h, msg, wp, lp);
        CARET_HIDDEN.set(false); // the caret is recreated or destroyed with focus
        InvalidateRect(a.view, std::ptr::null(), 0);
        InvalidateRect(a.input, std::ptr::null(), 1);
        if msg == WM_KILLFOCUS { key_badges::stop(); }
        return r;
    }
    // Ctrl + mouse wheel zooms the chat: each notch of 120 is one 10% step.
    if !is_input && msg == WM_MOUSEWHEEL && GetKeyState(VK_CONTROL as i32) < 0 {
        let total = WHEEL_REST.get() + ((wp >> 16) as i16 as i32);
        WHEEL_REST.set(total % 120);
        let notches = total / 120;
        if notches != 0 { set_chat_zoom((a.cfg.chat_zoom as i32 + notches * 10).max(50) as u32); }
        return 0;
    }
    // Scrolling is done here, instantly and in whole pixels, instead of by RichEdit's animated scroll
    // (which copies pixels frame by frame and leaves bubble corners, pictures and status behind).
    if !is_input && scrollbar_mouse(h, msg, lp) { return 0; }
    if !is_input {
        match msg {
            WM_MOUSEWHEEL => {
                let delta = (wp >> 16) as i16 as i32;
                let mut lines = 3u32;
                SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut _, 0);
                let (_, _, page) = scroll_range(h);
                // Small deltas (smooth wheels, XMouse Button Control) add up instead of each moving a whole line.
                let units = if lines == u32::MAX { delta * page } else { delta * lines as i32 * line_px(h) };
                let total = WHEEL_REST.get() + units;
                WHEEL_REST.set(total % 120);
                if total / 120 != 0 { scroll_by(h, -(total / 120)); }
                return 0;
            }
            WM_VSCROLL => {
                let (_, _, page) = scroll_range(h);
                let l = line_px(h);
                match (wp & 0xFFFF) as i32 {
                    SB_LINEUP => scroll_by(h, -l),
                    SB_LINEDOWN => scroll_by(h, l),
                    SB_PAGEUP => scroll_by(h, -(page - l).max(l)),
                    SB_PAGEDOWN => scroll_by(h, (page - l).max(l)),
                    // Top, bottom and thumb drag use RichEdit's own numbers, then repaint at once.
                    _ => {
                        if matches!((wp & 0xFFFF) as i32, SB_THUMBTRACK | SB_TOP) { a.pin_user = false; }
                        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_view), h, msg, wp, lp);
                        let (pos, _, _) = scroll_range(h);
                        if pos > max_scroll(h) { set_pos(h, pos); } // dragged or jumped past the last reply: pull back
                        RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                        return r;
                    }
                }
                return 0;
            }
            // Holding the middle button and dragging moves the chat like a page under your finger.
            WM_MBUTTONDOWN => {
                DRAG_Y.set(Some((lp >> 16) as i16 as i32));
                SetCapture(h);
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZENS));
                return 0;
            }
            WM_MOUSEMOVE if DRAG_Y.get().is_some() => {
                let y = (lp >> 16) as i16 as i32;
                if let Some(last) = DRAG_Y.get() { scroll_by(h, last - y); }
                DRAG_Y.set(Some(y));
                return 0;
            }
            WM_MBUTTONUP | WM_CAPTURECHANGED | WM_KILLFOCUS if DRAG_Y.get().is_some() => {
                DRAG_Y.set(None);
                if msg == WM_MBUTTONUP { ReleaseCapture(); }
            }
            WM_SETCURSOR if SB_DRAG.get().is_some() => { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW)); return 1; }
            WM_SETCURSOR if DRAG_Y.get().is_some() => { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZENS)); return 1; }
            WM_MOUSEHWHEEL | WM_HSCROLL => return 0,
            _ => {}
        }
    }
    if !is_input && msg == WM_SETCURSOR && (lp & 0xFFFF) as u32 == 1 {
        // Hand cursor over picture tiles and gallery arrows instead of the text I-beam.
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        ScreenToClient(h, &mut pt);
        // The scroll bar strip (the same width that grabs clicks) keeps the normal arrow, not the I-beam or hand.
        if let Some((track, _)) = scrollbar(h) {
            if pt.x >= track.left - 6 * (GetDpiForWindow(h).max(96) as i32) / 96 {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW));
                return 1;
            }
        }
        if images::over_control(pt.x, pt.y) {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            return 1;
        }
    }
    // Holding Alt is watched by alt_hook; losing focus hides the labels.
    if msg == WM_KILLFOCUS { key_badges::stop(); }
    // While the model menu is open it gets every key; the typing box keeps focus underneath.
    if chip_menu::is_open() {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR => { chip_menu::key(msg, wp); return 0; }
            WM_KEYUP | WM_SYSKEYUP => return 0,
            WM_LBUTTONDOWN | WM_KILLFOCUS => chip_menu::close(),
            WM_MOUSEWHEEL => { chip_menu::wheel(wp); return 0; }
            _ => {}
        }
    }
    // The quick menu: opened by hover it only answers the navigation keys (so typing is left alone); otherwise it gets every key.
    if tile_menu::is_open() {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR => { if tile_menu::key(msg, wp) { return 0; } }
            WM_KEYUP | WM_SYSKEYUP if tile_menu::has_keys() => return 0,
            WM_LBUTTONDOWN | WM_KILLFOCUS => tile_menu::close(),
            _ => {}
        }
    }
    // RichEdit flips the text direction on Ctrl + Left/Right Shift. LitePilot is left-to-right, so those presses are ignored.
    if is_input && matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) && matches!(wp as u16, VK_SHIFT | VK_LSHIFT | VK_RSHIFT) && GetKeyState(VK_CONTROL as i32) < 0 { return 0; }
    if is_input && msg == WM_PASTE && attach_win::paste() { return 0; }
    if msg == WM_DROPFILES { attach_win::dropped(wp as HANDLE); windows_sys::Win32::UI::Shell::DragFinish(wp as HANDLE); return 0; }
    if msg == WM_LBUTTONDOWN && !is_input && tip_close_hit((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32) { dismiss_tip(); return 0; }
    if msg == WM_LBUTTONDOWN && !is_input && copy_link_hit((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32) { copy_link_click(); return 0; }
    if !is_input {
        // A click on a picture tile opens the viewer; on a row's arrow, pages the row.
        if msg == WM_LBUTTONDOWN && images::click(h, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32, pic_colors(a)) { return 0; }
    }
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        if msg == WM_KEYDOWN && is_input && wp as u16 == VK_RETURN && GetKeyState(VK_SHIFT as i32) >= 0 && GetKeyState(VK_CONTROL as i32) >= 0 {
            send();
            return 0;
        }
        // Ctrl+V / Shift+Insert with files or a picture on the clipboard attaches them instead of pasting.
        let (ctl, shf) = (GetKeyState(VK_CONTROL as i32) < 0, GetKeyState(VK_SHIFT as i32) < 0);
        if is_input && msg == WM_KEYDOWN && ((wp as u16 == 0x56 && ctl) || (wp as u16 == VK_INSERT && shf)) && attach_win::paste() { ATE_KEY.set(true); return 0; }
        if on_key(wp as u16) { ATE_KEY.set(true); return 0; }
        ATE_KEY.set(false);
        // In the typing box, Up/Down have nothing to move through when it is empty or holds a single line: they
        // scroll the chat instead (a setting, on by default). Shift, Ctrl and Alt keep their usual meaning.
        if is_input && msg == WM_KEYDOWN && !ctl && !shf && a.cfg.arrow_scroll && GetKeyState(VK_MENU as i32) >= 0 && matches!(wp as u16, VK_UP | VK_DOWN)
            && (text_len(h) == 0 || SendMessageW(h, 0xBA, 0, 0) <= 1) { // EM_GETLINECOUNT
            SendMessageW(a.view, WM_VSCROLL, if wp as u16 == VK_UP { SB_LINEUP } else { SB_LINEDOWN } as usize, 0);
            return 0;
        }
        // In the chat, plain Up/Down scroll it a line at a time instead of moving the caret through the text.
        // With Shift they still move the caret, so selecting text by keyboard works as before.
        if !is_input && msg == WM_KEYDOWN && !ctl && !shf && GetKeyState(VK_MENU as i32) >= 0 && matches!(wp as u16, VK_UP | VK_DOWN) {
            SendMessageW(h, WM_VSCROLL, if wp as u16 == VK_UP { SB_LINEUP } else { SB_LINEDOWN } as usize, 0);
            return 0;
        }
    }
    if msg == WM_CHAR && is_input && wp == 13 && GetKeyState(VK_SHIFT as i32) >= 0 { return 0; }
    // Ctrl+letter chars from handled shortcuts must not reach the edit control.
    if (msg == WM_CHAR || msg == WM_SYSCHAR) && ATE_KEY.replace(false) { return 0; }
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32 && !(is_input && (matches!(wp, 1 | 3 | 22 | 24 | 26) || ((wp == 8 || wp == 13) && GetKeyState(VK_CONTROL as i32) >= 0)))) { return 0; }
    if is_input && ((msg == WM_CHAR && wp == 8) || (msg == WM_KEYDOWN && wp as u16 == VK_DELETE)) && text_len(h) == 0 { return 0; }
    let old = if is_input { a.old_input } else { a.old_view };
    // Anything that can change the text: redraw when it goes between empty and not, for the prompt.
    let edits = is_input && matches!(msg, WM_CHAR | WM_KEYDOWN | WM_KEYUP | WM_PASTE | WM_CUT | WM_UNDO | WM_SETTEXT | WM_IME_COMPOSITION);
    let was_empty = edits && text_len(h) == 0;
    let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(old), h, msg, wp, lp);
    if edits && was_empty != (text_len(h) == 0) {
        // The "Ask anything" prompt is drawn straight onto the box, so its descenders (y, g) can outlive the
        // first typed letter. Wipe the whole box before it repaints.
        let dc = GetDC(h);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        FillRect(dc, &rc, a.field_brush);
        ReleaseDC(h, dc);
        InvalidateRect(h, std::ptr::null(), 0);
        UpdateWindow(h);
    }
    // The text direction follows what is typed: from the right for Arabic and similar, otherwise from the left.
    if edits { auto_direction(h); }
    if edits {
        let n = SendMessageW(h, 0xBA, 0, 0).max(1) as u32; // EM_GETLINECOUNT
        if n.min(6) != a.input_lines.min(6) {
            a.input_lines = n;
            place_children();
            SendMessageW(h, 0xB7, 0, 0); // EM_SCROLLCARET
        } else {
            a.input_lines = n;
        }
    }
    r
}



pub(super) unsafe fn paint_prompt(h: HWND) {
    if text_len(h) != 0 { return; }
    let a = app();
    let mut r: RECT = std::mem::zeroed();
    SendMessageW(h, 0xB2, 0, &mut r as *mut _ as isize); // EM_GETRECT
    let hdc = GetDC(h);
    let old = SelectObject(hdc, a.font as _);
    SetBkMode(hdc, 1);
    SetTextColor(hdc, a.theme.muted);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let s = |v: i32| v * dpi / 96;
    let t = w("Ask anything");
    let mut sz = std::mem::zeroed::<SIZE>();
    GetTextExtentPoint32W(hdc, t.as_ptr(), t.len() as i32 - 1, &mut sz);
    TextOutW(hdc, r.left + s(7), r.top, t.as_ptr(), t.len() as i32 - 1); // clear of the caret
    // Only while the chat has focus: a keycap and "to return to typing", right-aligned.
    if GetFocus() == a.view {
        // "Ask anything  |  [Tab] to return", the hint in a smaller font right after the prompt.
        let mut lf: LOGFONTW = std::mem::zeroed();
        GetObjectW(a.font as _, std::mem::size_of::<LOGFONTW>() as i32, &mut lf as *mut _ as *mut _);
        lf.lfHeight = lf.lfHeight * 3 / 4;
        lf.lfWidth = 0;
        let small = CreateFontIndirectW(&lf);
        SelectObject(hdc, small as _);
        let sep = w("|");
        let k = w("Tab");
        let rest = w("to return");
        let (mut ss, mut ks, mut rs) = (std::mem::zeroed::<SIZE>(), std::mem::zeroed::<SIZE>(), std::mem::zeroed::<SIZE>());
        GetTextExtentPoint32W(hdc, sep.as_ptr(), 1, &mut ss);
        GetTextExtentPoint32W(hdc, k.as_ptr(), 3, &mut ks);
        GetTextExtentPoint32W(hdc, rest.as_ptr(), rest.len() as i32 - 1, &mut rs);
        let (kw, kh) = (ks.cx + s(8), ks.cy);
        let mut x = r.left + s(7) + sz.cx + s(10);
        let ty = r.top + (sz.cy - ks.cy) / 2 + s(1);
        TextOutW(hdc, x, ty, sep.as_ptr(), 1);
        x += ss.cx + s(10);
        let top = ty;
        let pen = CreatePen(PS_SOLID, 1, a.theme.muted);
        let (op, ob) = (SelectObject(hdc, pen as _), SelectObject(hdc, GetStockObject(NULL_BRUSH)));
        if !images::smooth_round(hdc, &RECT { left: x, top, right: x + kw, bottom: top + kh }, None, Some((a.theme.muted, 255, 1.0)), s(5)) { RoundRect(hdc, x, top, x + kw, top + kh, s(5), s(5)); }
        TextOutW(hdc, x + s(4), top, k.as_ptr(), 3);
        TextOutW(hdc, x + kw + s(5), ty, rest.as_ptr(), rest.len() as i32 - 1);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        DeleteObject(pen as _);
        SelectObject(hdc, a.font as _);
        DeleteObject(small as _);
    }
    SelectObject(hdc, old);
    ReleaseDC(h, hdc);
}

/// Tracks hover for the toolbar buttons. Repaints only when hover changes; nothing runs while idle.
pub(super) unsafe extern "system" fn btn_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32) { return 0; }
    match msg {
        WM_MOUSEMOVE if h == a.btn => { tile_menu::hover_open(); }
        _ => {}
    }
    match msg {
        WM_MOUSEMOVE if a.hot != h => {
            if !a.hot.is_null() { InvalidateRect(a.hot, std::ptr::null(), 0); }
            a.hot = h;
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            InvalidateRect(h, std::ptr::null(), 0);
        }
        WM_MOUSELEAVE_ if a.hot == h => {
            a.hot = std::ptr::null_mut();
            InvalidateRect(h, std::ptr::null(), 0);
        }
        // Shortcuts work whichever button has focus, not only the typing box and the chat.
        WM_SETCURSOR => { crate::windows::hover::hand(); return 1; }
        WM_KEYDOWN | WM_SYSKEYDOWN if on_key(wp as u16) => return 0,
        _ => {}
    }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_btn), h, msg, wp, lp)
}

/// Adds hover tracking to any toolbar or settings button.
pub fn hook_button(h: HWND) {
    let a = app();
    let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, btn_proc as *const () as isize) };
    if a.old_btn == 0 { a.old_btn = old; }
}
