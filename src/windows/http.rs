//! Streaming POST over WinHTTP (OS TLS and proxy support, nothing bundled).
use super::util::w;
use crate::core::protocol::*;
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows_sys::Win32::Networking::WinHttp::*;

/// Lets another thread abort a blocking read by closing the request handle.
#[derive(Default)]
pub struct HttpCancel(AtomicPtr<c_void>);
impl HttpCancel {
    pub fn cancel(&self) {
        let h = self.0.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !h.is_null() {
            unsafe { WinHttpCloseHandle(h); }
        }
    }
}

struct Url { secure: bool, host: String, port: u16, path: String }

fn parse_url(u: &str) -> Option<Url> {
    let (secure, rest) = match u.split_once("://")? {
        ("https", r) => (true, r),
        ("http", r) => (false, r),
        _ => return None,
    };
    let (auth, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (auth, if secure { 443 } else { 80 }),
    };
    (!host.is_empty()).then(|| Url { secure, host: host.into(), port, path: path.into() })
}

/// Downloads a picture: HTTPS only, at most `max` bytes, short timeouts. None on any failure.
pub fn fetch(url: &str, max: usize) -> Option<Vec<u8>> {
    let u = parse_url(url).filter(|u| u.secure)?;
    unsafe {
        let agent = w(&format!("LitePilot/{} (Windows desktop chat app; shows pictures in replies)", crate::core::VERSION));
        let s = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0);
        if s.is_null() { return None; }
        WinHttpSetTimeouts(s, 0, 10_000, 10_000, 20_000);
        let c = WinHttpConnect(s, w(&u.host).as_ptr(), u.port, 0);
        let r = if c.is_null() { std::ptr::null_mut() } else {
            WinHttpOpenRequest(c, w("GET").as_ptr(), w(&u.path).as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_SECURE)
        };
        let mut out = Vec::new();
        let mut ok = !r.is_null()
            && WinHttpSendRequest(r, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) != 0
            && WinHttpReceiveResponse(r, std::ptr::null_mut()) != 0;
        if ok {
            let (mut status, mut sz) = (0u32, 4u32);
            WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut status as *mut _ as _, &mut sz, std::ptr::null_mut());
            ok = status == 200;
            let mut buf = vec![0u8; 16384];
            while ok {
                let mut n = 0u32;
                if WinHttpReadData(r, buf.as_mut_ptr() as _, buf.len() as u32, &mut n) == 0 { ok = false; break; }
                if n == 0 { break; }
                out.extend_from_slice(&buf[..n as usize]);
                if out.len() > max { ok = false; }
            }
        }
        for h in [r, c, s] { if !h.is_null() { WinHttpCloseHandle(h); } }
        ok.then_some(out)
    }
}

pub fn stream(req: &HttpRequest, f: ApiFormat, cancel: &HttpCancel, sink: &dyn Fn(Event)) {
    let Some(u) = parse_url(&req.url) else {
        return sink(Event::Error(format!("Invalid base URL: {}", req.url)));
    };
    let fail = |what: &str| {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        sink(Event::Error(format!("{what} failed (Windows error {code}). Check the base URL and your connection.")))
    };
    unsafe {
        let agent = w(&format!("LitePilot/{}", crate::core::VERSION));
        let s = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0);
        if s.is_null() { return fail("Network setup"); }
        WinHttpSetTimeouts(s, 0, 15_000, 30_000, 180_000);
        let c = WinHttpConnect(s, w(&u.host).as_ptr(), u.port, 0);
        if c.is_null() { WinHttpCloseHandle(s); return fail("Connection"); }
        let flags = if u.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let r = WinHttpOpenRequest(c, w("POST").as_ptr(), w(&u.path).as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), flags);
        if r.is_null() { WinHttpCloseHandle(c); WinHttpCloseHandle(s); return fail("Request"); }
        cancel.0.store(r, Ordering::SeqCst);

        let headers: String = req.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let hw = w(&headers);
        let body = req.body.as_bytes();
        let sent = WinHttpSendRequest(r, hw.as_ptr(), (hw.len() - 1) as u32, body.as_ptr() as _, body.len() as u32, body.len() as u32, 0) != 0
            && WinHttpReceiveResponse(r, std::ptr::null_mut()) != 0;
        if !sent {
            // A cancelled request has already had its handle closed and reports nothing.
            if !cancel.0.load(Ordering::SeqCst).is_null() { fail("Request"); }
        } else {
            let mut status = 0u32;
            let mut sz = 4u32;
            WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut status as *mut _ as _, &mut sz, std::ptr::null_mut());
            let mut buf = vec![0u8; 8192];
            let mut lines = LineBuf::default();
            let mut errbody = Vec::new();
            let mut finished = false;
            loop {
                // Read only what has arrived, so each piece of a slow stream shows at once
                // instead of waiting for a full buffer.
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(r, &mut avail) == 0 || avail == 0 { break; }
                let mut n = 0u32;
                if WinHttpReadData(r, buf.as_mut_ptr() as _, avail.min(buf.len() as u32), &mut n) == 0 || n == 0 { break; }
                if status >= 300 {
                    if errbody.len() < 16_384 { errbody.extend_from_slice(&buf[..n as usize]); }
                    continue;
                }
                lines.feed(&buf[..n as usize], |l| {
                    if finished { return; }
                    if let Some(d) = l.strip_prefix("data:") {
                        if let Some(e) = parse_sse(f, d) {
                            finished = !matches!(e, Event::Text(_) | Event::Status(_));
                            sink(e);
                        }
                    }
                });
                if finished { break; }
            }
            if status >= 300 {
                sink(Event::Error(http_error(status, &String::from_utf8_lossy(&errbody))));
            } else if !finished && !cancel.0.load(Ordering::SeqCst).is_null() {
                sink(Event::Done); // Some servers just close the stream at the end.
            }
        }
        cancel.cancel();
        WinHttpCloseHandle(c);
        WinHttpCloseHandle(s);
    }
}
