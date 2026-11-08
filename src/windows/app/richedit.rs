//! RichEdit plumbing the chat and typing box use: message ids, richedit.h structures, streaming RTF in.
#![allow(non_snake_case, clippy::upper_case_acronyms)] // names as in richedit.h
use super::*;

pub(super) const EM_STREAMIN_: u32 = 0x449;
pub(super) const EM_SETBKGNDCOLOR_: u32 = 0x443;
pub(super) const EM_SETCHARFORMAT_: u32 = 0x444;
pub(super) const EM_SETEVENTMASK_: u32 = 0x445;
pub(super) const EM_EXLIMITTEXT_: u32 = 0x435;
pub(super) const EM_GETTEXTRANGE_: u32 = 0x44B;
pub(super) const EM_SETTEXTMODE_: u32 = 0x459;
pub(super) const EM_GETTEXTLENGTHEX_: u32 = 0x45F;
pub(super) const EM_SETSEL_: u32 = 0xB1;
pub(super) const ES_MULTI: u32 = 0x4;
pub(super) const ES_READ: u32 = 0x800;
pub(super) const ES_NOHIDE: u32 = 0x100;
pub(super) const ES_AUTOV: u32 = 0x40;
pub(super) const EN_LINK_: u32 = 0x70B;
pub(super) const SF_RTF: u32 = 2;
pub(super) const SFF_SELECTION: u32 = 0x8000;
pub(super) const CFM_COLOR: u32 = 0x4000_0000;

// richedit.h structures (4-byte packing); windows-sys doesn't include them.
#[repr(C, packed(4))]
pub(super) struct EDITSTREAM {
    dwCookie: usize,
    dwError: u32,
    pfnCallback: Option<unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32>,
}
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct CHARRANGE { pub(super) cpMin: i32, pub(super) cpMax: i32 }
#[repr(C, packed(4))]
pub(super) struct TEXTRANGEW { pub(super) chrg: CHARRANGE, pub(super) lpstrText: *mut u16 }
#[repr(C, packed(4))]
pub(super) struct ENLINK { nmhdr: NMHDR, pub(super) msg: u32, wParam: usize, lParam: isize, pub(super) chrg: CHARRANGE }
#[repr(C, packed(4))]
pub(super) struct CHARFORMATW {
    pub(super) cbSize: u32, pub(super) dwMask: u32, dwEffects: u32, yHeight: i32, yOffset: i32, pub(super) crTextColor: u32,
    bCharSet: u8, bPitchAndFamily: u8, szFaceName: [u16; 32],
}

pub(super) struct Cursor<'a>(&'a [u8]);
pub(super) unsafe extern "system" fn stream_cb(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let c = &mut *(cookie as *mut Cursor);
    let n = c.0.len().min(cb as usize);
    std::ptr::copy_nonoverlapping(c.0.as_ptr(), buf, n);
    c.0 = &c.0[n..];
    *pcb = n as i32;
    0
}

pub(super) fn text_len(h: HWND) -> i32 {
    let q: [u32; 2] = [2 | 8, 1200]; // GTL_PRECISE | GTL_NUMCHARS, UTF-16
    unsafe { SendMessageW(h, EM_GETTEXTLENGTHEX_, q.as_ptr() as usize, 0) as i32 }
}

pub(super) fn stream_in(view: HWND, start: i32, doc: &str) {
    let mut c = Cursor(doc.as_bytes());
    unsafe {
        SendMessageW(view, EM_SETSEL_, start as usize, -1);
        let mut es: EDITSTREAM = std::mem::zeroed();
        es.dwCookie = &mut c as *mut _ as usize;
        es.pfnCallback = Some(stream_cb);
        SendMessageW(view, EM_STREAMIN_, (SF_RTF | SFF_SELECTION) as usize, &es as *const _ as isize);
    }
}

pub(super) fn scroll_bottom(view: HWND) {
    unsafe { set_pos(view, i32::MAX); }
}
