//! The chat's pixel scrolling and its slim themed scroll bar.
use super::*;

/// Space kept clear on the chat's right, in pixels at 96 dpi: the slim scroll bar lives at the window's edge.
pub(super) const RIGHT_PAD: i32 = 31;

thread_local! {
    /// While the scroll bar thumb is dragged: (mouse y at the start, scroll position at the start).
    pub(super) static SB_DRAG: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// The chat's full height in pixels, measured once per layout (see `doc_height`).
    static DOC_H: Cell<HeightCache> = const { Cell::new(HeightCache { key: u64::MAX, h: 0 }) };
    /// Set when the pending repaint only comes from scrolling, so the chat's height needn't be measured again.
    static SCROLL_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// A measured height, kept until the layout it was measured for changes (`key`).
#[derive(Clone, Copy)]
pub(super) struct HeightCache { key: u64, h: i32 }

impl HeightCache {
    /// The height for `key`: the kept one, or a new measurement when the layout is different.
    pub(super) fn get(&mut self, key: u64, measure: impl FnOnce() -> i32) -> i32 {
        if self.key != key { *self = HeightCache { key, h: measure() }; }
        self.h
    }
}

/// The thumb inside a track `len` pixels long: (offset from the track's top, height). Its height depends only on
/// the page and the scrollable height (never on where you are), and its offset only on the position, so while
/// you scroll it slides without changing size and lands exactly at the track's ends at the top and the bottom.
pub(super) fn thumb_geom(len: i32, page: i32, max: i32, pos: i32, min_th: i32) -> (i32, i32) {
    if len <= 0 || max <= 0 { return (0, len.max(0)); }
    let th = ((page.max(0) as i64 * len as i64 / (max as i64 + page.max(0) as i64)) as i32).max(min_th).min(len);
    let off = (pos.clamp(0, max) as i64 * (len - th) as i64 / max as i64) as i32;
    (off, th)
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
    // Position and size come from the same `pos` and `max` the scrolling itself uses, so they always agree.
    let track = RECT { left: rc.right - s(12), top: s(6), right: rc.right - s(4), bottom: rc.bottom - s(6) };
    let (off, th) = thumb_geom(track.bottom - track.top, page, max, pos, s(32));
    let y = track.top + off;
    Some((track, RECT { left: track.left, top: y, right: track.right, bottom: y + th }))
}

/// The full height of the chat's text in pixels. RichEdit is asked once per layout (its text length, width, zoom
/// and screen scaling, and any repaint that didn't come from scrolling) instead of on every scroll step and paint,
/// so scrolling can't make the scroll bar's sums move.
unsafe fn doc_height(view: HWND) -> i32 {
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(view, &mut rc);
    let key = (text_len(view) as u64 & 0xFFFF_FFFF)
        ^ ((rc.right as u64 & 0xFFFF) << 32)
        ^ ((app().cfg.chat_zoom as u64 & 0x3FF) << 48)
        ^ ((GetDpiForWindow(view) as u64 & 0x3F) << 58);
    let mut c = DOC_H.get();
    let h = c.get(key, || {
        let (pos, _, _) = scroll_range(view);
        pos + bubbles::char_pos(view, text_len(view)).y
    });
    DOC_H.set(c);
    h
}

/// Forgets the measured height, so the next use measures the chat again (its content may have changed).
pub(super) fn forget_height() {
    DOC_H.set(HeightCache { key: u64::MAX, h: 0 });
}

/// Called as the chat starts a repaint: a repaint that wasn't asked for by scrolling may mean new content.
pub(super) fn before_paint() {
    if !SCROLL_ONLY.replace(false) { forget_height(); }
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
    let (_, _, page) = scroll_range(view);
    (doc_height(view) - page + line_px(view) / 3).max(0)
}

/// EM_SETSCROLLPOS without RichEdit drawing anything itself: on its own it repaints the moved text straight to
/// the screen at once, outside LitePilot's off-screen frame, so for a moment the slim scroll bar (and the
/// bubbles' corners) were overdrawn or carried along with the text, which made the thumb flicker as you scrolled.
/// With drawing held, nothing reaches the screen until the chat's own paint shows the whole new frame in one go.
unsafe fn move_text(view: HWND, y: i32) {
    // Only when drawing is on now: inside a refresh that has already held it, it must stay held until that ends.
    let drawing = GetWindowLongW(view, GWL_STYLE) as u32 & WS_VISIBLE != 0;
    if drawing { SendMessageW(view, WM_SETREDRAW, 0, 0); }
    let mut pt = POINT { x: 0, y };
    SendMessageW(view, 0x0400 + 222, 0, &mut pt as *mut POINT as isize); // EM_SETSCROLLPOS
    if drawing { SendMessageW(view, WM_SETREDRAW, 1, 0); }
}

/// Sets the chat's pixel scroll position directly (no repaint; callers paint once).
pub(super) unsafe fn set_pos(view: HWND, y: i32) {
    move_text(view, y.clamp(0, max_scroll(view)));
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

thread_local! {
    static CHAT_ARROW: Cell<HCURSOR> = const { Cell::new(std::ptr::null_mut()) };
}

/// The chat's arrow pointer: LitePilot's own copy of Windows' arrow, which looks the same.
///
/// Why: while any program is starting (a process created without STARTF_FORCEOFFFEEDBACK, such as the chat CLI
/// LitePilot keeps ready after each reply, or the helper tools that CLI starts in turn), Windows shows its
/// "app starting" pointer (arrow with a ring) in place of the *system* arrow, wherever that arrow is showing, for
/// up to several seconds. The swap happens inside Windows, so GetCursor still reports the plain arrow (which is
/// why checking it never caught anything). Windows only swaps its own shared arrow, so the chat's scroll bar and
/// any arrow the text control asks for use this copy instead, and stay a normal pointer while you scroll or zoom.
pub(super) unsafe fn chat_arrow() -> HCURSOR {
    let c = CHAT_ARROW.get();
    if !c.is_null() { return c; }
    // Not LR_SHARED: a separate cursor made from the current arrow at the default size.
    let mine = LoadImageW(std::ptr::null_mut(), 32512 as *const u16, IMAGE_CURSOR, 0, 0, LR_DEFAULTSIZE) as HCURSOR; // OCR_NORMAL
    let c = if mine.is_null() { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) } else { mine };
    CHAT_ARROW.set(c);
    c
}

/// After the text control picked its pointer: if it chose Windows' shared arrow, show the chat's own copy instead.
pub(super) unsafe fn keep_pointer_normal() {
    if GetCursor() == LoadCursorW(std::ptr::null_mut(), IDC_ARROW) { SetCursor(chat_arrow()); }
}

/// Moves the chat to `y` pixels from the top (clamped) at once and repaints the whole view.
pub(super) unsafe fn scroll_to(h: HWND, y: i32) {
    let (pos, _, _) = scroll_range(h);
    let y = y.clamp(0, max_scroll(h));
    app().pin_user = false; // the reader took over the scrolling
    if y == pos { return; }
    move_text(h, y);
    // Marked for repainting, not painted on the spot: wheel and drag events that arrive faster than a frame can
    // be drawn then share one repaint instead of each forcing its own. When only the position changed, that paint
    // keeps the measured height (a repaint already pending for another reason still measures again).
    let pending = GetUpdateRect(h, std::ptr::null_mut(), 0) != 0;
    InvalidateRect(h, std::ptr::null(), 0);
    if !pending { SCROLL_ONLY.set(true); }
}

pub(super) unsafe fn scroll_by(h: HWND, dy: i32) {
    let (pos, _, _) = scroll_range(h);
    scroll_to(h, pos.saturating_add(dy));
}

#[cfg(test)]
mod tests {
    use super::{thumb_geom, HeightCache};

    #[test]
    fn thumb_keeps_its_size_and_slides_end_to_end() {
        let (len, page, max, min) = (600, 700, 25_000, 48);
        let (top, th) = thumb_geom(len, page, max, 0, min);
        assert_eq!(top, 0);
        assert!(th >= min && th < len);
        let mut last = -1;
        for pos in (0..=max).step_by(37) {
            let (off, h) = thumb_geom(len, page, max, pos, min);
            assert_eq!(h, th, "the thumb's height must not change while scrolling");
            assert!(off >= last, "the thumb must never move back while scrolling down");
            assert!(off + h <= len);
            last = off;
        }
        assert_eq!(thumb_geom(len, page, max, max, min), (len - th, th));
        // Past either end (a stale position) it stays at that end.
        assert_eq!(thumb_geom(len, page, max, max + 500, min).0, len - th);
        assert_eq!(thumb_geom(len, page, max, -40, min).0, 0);
    }

    #[test]
    fn thumb_size_follows_the_page_share() {
        // Half the chat fits: half the track.
        assert_eq!(thumb_geom(400, 500, 500, 0, 10).1, 200);
        // A very long chat: never smaller than the minimum.
        assert_eq!(thumb_geom(400, 500, 5_000_000, 0, 32).1, 32);
        // Nothing to scroll or no room: whole track, no movement, no division by zero.
        assert_eq!(thumb_geom(400, 500, 0, 30, 32), (0, 400));
        assert_eq!(thumb_geom(0, 500, 900, 30, 32), (0, 0));
    }

    #[test]
    fn height_is_measured_once_per_layout() {
        let mut c = HeightCache { key: u64::MAX, h: 0 };
        let mut calls = 0;
        // Scrolling: the same layout asked many times, with measurements that would wobble.
        for wobble in [9000, 9003, 8996, 9001] {
            assert_eq!(c.get(7, || { calls += 1; wobble }), 9000);
        }
        assert_eq!(calls, 1);
        // New text (or width, zoom, scaling): measured afresh.
        assert_eq!(c.get(8, || { calls += 1; 9400 }), 9400);
        assert_eq!(calls, 2);
    }
}
