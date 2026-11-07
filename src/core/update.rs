//! Update rules that do not depend on Windows: which release is newer, and which download to use.
use serde_json::Value;

/// The newest releases of this project, newest first. Pre-releases are included on purpose: every release here is one.
pub const RELEASES_API: &str = "https://api.github.com/repos/cobhc95/LitePilot/releases?per_page=5";
/// Downloads are only taken from this project's own release folder.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/cobhc95/LitePilot/releases/download/";
/// The portable ZIP, which holds LitePilot.exe. Both the installed and the portable copy update from it.
pub const PORTABLE_SUFFIX: &str = "-Portable.zip";

/// A downloadable file with the SHA-256 that GitHub reports for it.
#[derive(Debug, PartialEq)]
pub struct Download {
    pub tag: String,
    pub url: String,
    pub sha256: String,
}

/// "v0.1.2" or "0.1.2-alpha" → ([0, 1, 2], "") and ([0, 1, 2], "alpha").
fn parse(v: &str) -> (Vec<u64>, &str) {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    (core.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre)
}

/// True when `remote` is a later version than `local`. A release without a suffix is later than the same
/// numbers with one ("0.1.2" > "0.1.2-alpha"). Missing numbers count as zero ("0.1" == "0.1.0").
pub fn is_newer(remote: &str, local: &str) -> bool {
    let ((a, pa), (b, pb)) = (parse(remote), parse(local));
    let n = a.len().max(b.len());
    let num = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..n {
        if num(&a, i) != num(&b, i) { return num(&a, i) > num(&b, i); }
    }
    match (pa.is_empty(), pb.is_empty()) {
        (true, false) => true,
        (false, true) => false,
        _ => pa > pb,
    }
}

/// The portable ZIP of the newest release, when that release is newer than `local`. `json` is the body of
/// the releases API. Anything missing, unexpected, or not from this project's release folder gives None.
pub fn portable_update(json: &str, local: &str) -> Option<Download> {
    let releases: Value = serde_json::from_str(json).ok()?;
    let release = releases.as_array()?.iter().find(|r| !r["draft"].as_bool().unwrap_or(false))?;
    let tag = release["tag_name"].as_str()?;
    if !is_newer(tag, local) { return None; }
    let asset = release["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(PORTABLE_SUFFIX)))?;
    let url = asset["browser_download_url"].as_str()?;
    let sha256 = asset["digest"].as_str()?.strip_prefix("sha256:")?;
    if !url.starts_with(DOWNLOAD_PREFIX) { return None; }
    Some(Download { tag: tag.into(), url: url.into(), sha256: sha256.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_then_suffix() {
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("0.2", "0.1.9"));
        assert!(is_newer("0.1.2", "0.1.2-alpha"));
        assert!(!is_newer("0.1.2-alpha", "0.1.2"));
        assert!(!is_newer("v0.1-alpha", "0.1.2"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1", "0.1.0"));
    }

    #[test]
    fn picks_the_portable_zip_of_a_newer_release() {
        let json = r#"[{"tag_name":"v0.1.3","draft":false,"assets":[
            {"name":"LitePilot-0.1.3-Setup.exe","browser_download_url":"https://github.com/cobhc95/LitePilot/releases/download/v0.1.3/LitePilot-0.1.3-Setup.exe","digest":"sha256:aa"},
            {"name":"LitePilot-0.1.3-Portable.zip","browser_download_url":"https://github.com/cobhc95/LitePilot/releases/download/v0.1.3/LitePilot-0.1.3-Portable.zip","digest":"sha256:bb"}]}]"#;
        let d = portable_update(json, "0.1.2").unwrap();
        assert_eq!((d.tag.as_str(), d.sha256.as_str()), ("v0.1.3", "bb"));
        assert!(portable_update(json, "0.1.3").is_none());
    }

    #[test]
    fn refuses_other_hosts_drafts_and_missing_digests() {
        let other = r#"[{"tag_name":"v0.2","draft":false,"assets":[{"name":"x-Portable.zip","browser_download_url":"https://example.com/x.zip","digest":"sha256:cc"}]}]"#;
        assert!(portable_update(other, "0.1").is_none());
        let draft = r#"[{"tag_name":"v0.2","draft":true,"assets":[]}]"#;
        assert!(portable_update(draft, "0.1").is_none());
        let nodigest = r#"[{"tag_name":"v0.2","draft":false,"assets":[{"name":"x-Portable.zip","browser_download_url":"https://github.com/cobhc95/LitePilot/releases/download/v0.2/x-Portable.zip"}]}]"#;
        assert!(portable_update(nodigest, "0.1").is_none());
        assert!(portable_update("not json", "0.1").is_none());
    }
}
