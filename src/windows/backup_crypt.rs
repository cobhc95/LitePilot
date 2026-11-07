//! Passphrase encryption for backup files, with Windows' own cryptography (CNG): a key from the passphrase by
//! PBKDF2-SHA256, then AES-256-GCM, which also detects a wrong passphrase or a damaged file. Unlike the
//! memory file (DPAPI), a backup opens on any PC with the passphrase.
use super::util::w;
use windows_sys::Win32::Security::Cryptography::*;

const MAGIC: &[u8; 4] = b"LPB1";
const ITERATIONS: u64 = 200_000;
const SALT: usize = 16;
const NONCE: usize = 12;
const TAG: usize = 16;

fn ok(status: i32) -> Option<()> { (status >= 0).then_some(()) }

fn random(n: usize) -> Option<Vec<u8>> {
    let mut v = vec![0u8; n];
    ok(unsafe { BCryptGenRandom(std::ptr::null_mut(), v.as_mut_ptr(), n as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) })?;
    Some(v)
}

fn derive(pass: &str, salt: &[u8]) -> Option<[u8; 32]> {
    let mut key = [0u8; 32];
    unsafe {
        let mut alg: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
        ok(BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), BCRYPT_ALG_HANDLE_HMAC_FLAG))?;
        let r = BCryptDeriveKeyPBKDF2(alg, pass.as_ptr(), pass.len() as u32, salt.as_ptr(), salt.len() as u32, ITERATIONS, key.as_mut_ptr(), 32, 0);
        BCryptCloseAlgorithmProvider(alg, 0);
        ok(r)?;
    }
    Some(key)
}

/// Runs AES-256-GCM one way or the other. `tag` is written when encrypting and checked when decrypting.
unsafe fn gcm(key: &[u8; 32], nonce: &[u8], tag: &mut [u8], input: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let mut alg: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
    ok(BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_AES_ALGORITHM, std::ptr::null(), 0))?;
    let mode = w("ChainingModeGCM");
    let result = (|| {
        ok(BCryptSetProperty(alg, BCRYPT_CHAINING_MODE, mode.as_ptr() as *const u8, (mode.len() * 2) as u32, 0))?;
        let mut hk: BCRYPT_KEY_HANDLE = std::ptr::null_mut();
        ok(BCryptGenerateSymmetricKey(alg, &mut hk, std::ptr::null_mut(), 0, key.as_ptr(), 32, 0))?;
        let mut info: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32;
        info.dwInfoVersion = 1;
        info.pbNonce = nonce.as_ptr() as *mut u8;
        info.cbNonce = nonce.len() as u32;
        info.pbTag = tag.as_mut_ptr();
        info.cbTag = tag.len() as u32;
        let mut out = vec![0u8; input.len()];
        let mut n = 0u32;
        let info_ptr = &info as *const _ as *const std::ffi::c_void;
        let r = if encrypt {
            BCryptEncrypt(hk, input.as_ptr(), input.len() as u32, info_ptr, std::ptr::null_mut(), 0, out.as_mut_ptr(), out.len() as u32, &mut n, 0)
        } else {
            BCryptDecrypt(hk, input.as_ptr(), input.len() as u32, info_ptr, std::ptr::null_mut(), 0, out.as_mut_ptr(), out.len() as u32, &mut n, 0)
        };
        BCryptDestroyKey(hk);
        ok(r)?;
        out.truncate(n as usize);
        Some(out)
    })();
    BCryptCloseAlgorithmProvider(alg, 0);
    result
}

pub fn encrypt(pass: &str, plain: &[u8]) -> Option<Vec<u8>> {
    let (salt, nonce) = (random(SALT)?, random(NONCE)?);
    let key = derive(pass, &salt)?;
    let mut tag = [0u8; TAG];
    let ct = unsafe { gcm(&key, &nonce, &mut tag, plain, true)? };
    Some([&MAGIC[..], &salt, &nonce, &tag, &ct].concat())
}

pub fn decrypt(pass: &str, file: &[u8]) -> Result<Vec<u8>, String> {
    let head = MAGIC.len() + SALT + NONCE + TAG;
    if file.len() < head || &file[..4] != MAGIC { return Err("That isn't a LitePilot backup file.".into()); }
    let (salt, rest) = file[4..].split_at(SALT);
    let (nonce, rest) = rest.split_at(NONCE);
    let (tag, ct) = rest.split_at(TAG);
    let key = derive(pass, salt).ok_or("Windows couldn't prepare the decryption.")?;
    let mut tag = tag.to_vec();
    unsafe { gcm(&key, nonce, &mut tag, ct, false) }.ok_or_else(|| "Wrong passphrase, or the file is damaged.".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip_and_wrong_passphrase() {
        let f = super::encrypt("correct horse", b"{\"hello\":1}").unwrap();
        assert!(!String::from_utf8_lossy(&f).contains("hello"));
        assert_eq!(super::decrypt("correct horse", &f).unwrap(), b"{\"hello\":1}");
        assert!(super::decrypt("wrong horse", &f).is_err());
        let mut bad = f.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(super::decrypt("correct horse", &bad).is_err());
        assert!(super::decrypt("x", b"not a backup").is_err());
    }
}
