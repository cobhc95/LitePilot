//! The chat's pixel scrolling and its slim themed scroll bar.
use super::*;

/// Space kept clear on the chat's right, in pixels at 96 dpi: the slim scroll bar lives at the window's edge.
pub(super) const RIGHT_PAD: i32 = 31;

thread_local! {
    /// While the scroll bar thumb is dragged: (mouse y at the start, scroll position at the start).
    pub(super) static SB_DRAG: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
}

/// The chat's own slim scroll bar (the system one can't take the theme's colors): the thumb and the strip it
/// moves in. None when everything fits.
pub(super) unsafe fn scrollbar(h: HWND) -> Option<(RECT, RECT)> {
    let max = max_scroll(h);
    if max <= 0 { return None; }
    let (pos, _, page) = scroll_range(h);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let s = |v: i32| v * dpi / 96;
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let track = RECT { left: rc.right - s(12), top: s(6), right: rc.right - s(4), bottom: rc.bottom - s(6) };
    let th = (page * (track.bottom - track.top) / (max + page)).max(s(32)).min(track.bottom - track.top);
    let y = track.top + (pos.clamp(0, max) as i64 * (track.bottom - track.top - th) as i64 / max as i64) as i32;
    Some((track, RECT { left: track.left, top: y, right: track.right, bottom: y + th }))
}

/// Draws the thumb in the theme's muted text color, soft until you drag it.
pub(super) unsafe fn scrollbar_paint(h: HWND, dc: HDC) {
    let Some((_, thumb)) = scrollbar(h) else { return };
    let alpha = if SB_DRAG.get().is_some() { 200 } else { 120 };
    let rad = thumb.right - thumb.left;
    images::smooth_round(dc, &thumb, Some((app().theme.muted, alpha)), None, rad);
}

/// Clicks and drags on the scroll bar. True when the mouse message was used.
pub(super) unsafe fn scrollbar_mouse(h: HWND, msg: u32, lp: LPARAM) -> bool {
    let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    match msg {
        WM_LBUTTONDOWN => {
            let Some((track, thumb)) = scrollbar(h) else { return false };
            if x < track.left - 6 * dpi / 96 { return false; }
            let (pos, _, page) = scroll_range(h);
            if y >= thumb.top && y < thumb.bottom {
                SB_DRAG.set(Some((y, pos)));
                SetCapture(h);
                InvalidateRect(h, std::ptr::null(), 0);
            } else {
                scroll_to(h, pos + if y < thumb.top { -(page - 20) } else { page - 20 });
            }
            true
        }
        WM_MOUSEMOVE if SB_DRAG.get().is_some() => {
            let (Some((start_y, start_pos)), Some((track, thumb))) = (SB_DRAG.get(), scrollbar(h)) else { return true };
            let room = (track.bottom - track.top - (thumb.bottom - thumb.top)).max(1);
            scroll_to(h, start_pos + ((y - start_y) as i64 * max_scroll(h) as i64 / room as i64) as i32);
            true
        }
        WM_LBUTTONUP if SB_DRAG.get().is_some() => {
            SB_DRAG.set(None);
            ReleaseCapture();
            InvalidateRect(h, std::ptr::null(), 0);
            true
        }
        _ => false,
    }
}

/// The furthest the chat may scroll: the end of the last reply sits at the bottom edge, never above it, so the
/// chat can't be scrolled into empty space. (RichEdit itself lets the last line travel up to the top.)
pub(super) unsafe fn max_scroll(view: HWND) -> i32 {
    let (pos, _, page) = scroll_range(view);
    let end_y = bubbles::char_pos(view, text_len(view)).y;
    (pos + end_y - page + line_px(view) / 3).max(0)
}

/// Sets the chat's pixel scroll position directly (no repaint; callers paint once).
pub(super) unsafe fn set_pos(view: HWND, y: i32) {
    let mut pt = POINT { x: 0, y: y.clamp(0, max_scroll(view)) };
    SendMessageW(view, 0x0400 + 222, 0, &mut pt as *mut POINT as isize); // EM_SETSCROLLPOS
}

/// Focuses the chat where you are reading: the caret goes to the first character on screen and the scroll
/// position stays exactly as it was (it used to jump back to the latest question).
pub(super) unsafe fn focus_chat_at_question(view: HWND) {
    let (pos, _, _) = scroll_range(view);
    let d = GetDpiForWindow(view).max(96) as i32 / 96;
    let mut pt = POINT { x: 12 * d, y: 6 * d };
    let at = SendMessageW(view, 0xD7, 0, &mut pt as *mut POINT as isize); // EM_CHARFROMPOS (RichEdit: POINTL in lParam)
    SendMessageW(view, EM_SETSEL_, at as usize, at);
    SetFocus(view);
    set_pos(view, pos);
    InvalidateRect(view, std::ptr::null(), 0);
}

thread_local! {
    /// Where the chat was scrolled when the window hid, and which chat it was, so showing it again goes back there.
    pub(super) static SAVED_SCROLL: std::cell::RefCell<Option<(PathBuf, i32)>> = const { std::cell::RefCell::new(None) };
}

/// Scrolls so the latest question sits at the top of the view, as far as the text allows.
pub(super) unsafe fn show_question_at_top(view: HWND) {
    let (pos, _, _) = scroll_range(view);
    let y = bubbles::char_pos(view, app().user_start).y;
    set_pos(view, pos + y - line_px(view) / 2);
}

/// (true pixel position, visible height). RichEdit's own scrollbar numbers can be scaled for long
/// chats, so the position comes from EM_GETSCROLLPOS, which is always in pixels.
pub(super) unsafe fn scroll_range(h: HWND) -> (i32, i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    SendMessageW(h, 0x0400 + 221, 0, &mut pt as *mut POINT as isize); // EM_GETSCROLLPOS
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    (pt.y, i32::MAX, rc.bottom)
}

/// One wheel line in pixels, following the chat zoom and screen scaling.
pub(super) unsafe fn line_px(h: HWND) -> i32 {
    (22 * GetDpiForWindow(h).max(96) as i32 / 96 * app().cfg.chat_zoom as i32 / 100).max(8)
}

/// Moves the chat to `y` pixels from the top (clamped) at once and repaints the whole view.
pub(super) unsafe fn scroll_to(h: HWND, y: i32) {
    let (pos, _, _) = scroll_range(h);
    let y = y.clamp(0, max_scroll(h));
    app().pin_user = false; // the reader took over the scrolling
    if y == pos { return; }
    let mut pt = POINT { x: 0, y };
    SendMessageW(h, 0x0400 + 222, 0, &mut pt as *mut POINT as isize); // EM_SETSCROLLPOS
    RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
}

pub(super) unsafe fn scroll_by(h: HWND, dy: i32) {
    let (pos, _, _) = scroll_range(h);
    scroll_to(h, pos.saturating_add(dy));
}
