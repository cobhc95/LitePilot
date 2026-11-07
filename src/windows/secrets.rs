//! API keys live in Windows Credential Manager, never in the settings file.
use super::util::w;
use windows_sys::Win32::Security::Credentials::*;

fn target(id: &str) -> Vec<u16> {
    w(&format!("LitePilot/{id}"))
}

pub fn set(id: &str, key: &str) -> bool {
    let mut t = target(id);
    let mut user = w("LitePilot");
    let mut blob = key.as_bytes().to_vec();
    unsafe {
        let mut c: CREDENTIALW = std::mem::zeroed();
        c.Type = CRED_TYPE_GENERIC;
        c.TargetName = t.as_mut_ptr();
        c.UserName = user.as_mut_ptr();
        c.Persist = CRED_PERSIST_LOCAL_MACHINE;
        c.CredentialBlobSize = blob.len() as u32;
        c.CredentialBlob = blob.as_mut_ptr();
        CredWriteW(&c, 0) != 0
    }
}

pub fn get(id: &str) -> String {
    let t = target(id);
    unsafe {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
            return String::new();
        }
        let c = &*p;
        let s = String::from_utf8_lossy(std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)).into_owned();
        CredFree(p as _);
        s
    }
}

pub fn delete(id: &str) {
    unsafe { CredDeleteW(target(id).as_ptr(), CRED_TYPE_GENERIC, 0); }
}
