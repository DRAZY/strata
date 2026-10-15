//! `latest.json`, the update feed: the newest version, its release notes, and
//! for each platform a download with its size, SHA-256 and signature. Also
//! the rules for which URLs Strata will fetch at all, and which download
//! fits the running build.

use super::version::Version;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The repository updates come from.
pub const REPO: &str = "DRAZY/strata";
/// The stable channel's feed: the newest release that isn't a prerelease.
pub const STABLE_FEED: &str = "https://github.com/DRAZY/strata/releases/latest/download/latest.json";
/// Every release, prereleases included, newest first (GitHub's API).
pub const RELEASES_API: &str = "https://api.github.com/repos/DRAZY/strata/releases?per_page=30";
/// The asset that holds the manifest in each release.
pub const MANIFEST_ASSET: &str = "latest.json";

/// Hosts Strata fetches from. github.com answers release downloads with a
/// redirect to its content hosts; those redirects must stay on HTTPS (curl
/// enforces that, see `fetch`).
const HOSTS: &[&str] = &["github.com", "api.github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// The platforms releases are built for.
pub const PLATFORMS: &[&str] = &["macos-universal", "macos-arm64", "windows-x64"];

/// Biggest manifest and releases list read, and biggest download accepted.
pub const MAX_MANIFEST_BYTES: u64 = 1 << 20;
pub const MAX_API_BYTES: u64 = 4 << 20;
pub const MAX_DOWNLOAD_BYTES: u64 = 512 << 20;
/// Release notes longer than this are cut (they're shown in a small notice).
const MAX_NOTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: Version,
    pub pub_date: Option<String>,
    pub notes: String,
    pub platforms: BTreeMap<String, Asset>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
}

#[derive(Deserialize)]
struct RawManifest {
    version: String,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    platforms: BTreeMap<String, Asset>,
}

impl Manifest {
    /// Parse and check a manifest. Platforms Strata doesn't know are ignored
    /// (a newer feed may add some); a known platform with a bad entry fails
    /// the whole manifest, since it's malformed or tampered with.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let raw: RawManifest = serde_json::from_slice(bytes).map_err(|e| format!("the update feed isn't valid: {e}"))?;
        let version = Version::parse(&raw.version).ok_or_else(|| format!("the update feed's version {:?} isn't a version", raw.version))?;
        let mut platforms = BTreeMap::new();
        for (key, asset) in raw.platforms {
            if !PLATFORMS.contains(&key.as_str()) {
                continue;
            }
            check_url(&asset.url).map_err(|e| format!("{key}: {e}"))?;
            if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return Err(format!("{key}: the sha256 isn't 64 lowercase hex digits"));
            }
            if asset.size == 0 || asset.size > MAX_DOWNLOAD_BYTES {
                return Err(format!("{key}: size {} is out of range", asset.size));
            }
            if super::sig::b64_decode(asset.signature.trim()).is_none_or(|s| s.len() != 64) {
                return Err(format!("{key}: the signature isn't 64 bytes of base64"));
            }
            platforms.insert(key, asset);
        }
        let mut notes = raw.notes;
        if notes.len() > MAX_NOTES {
            let mut cut = MAX_NOTES;
            while !notes.is_char_boundary(cut) {
                cut -= 1;
            }
            notes.truncate(cut);
            notes.push_str("\n\n…");
        }
        Ok(Self { version, pub_date: raw.pub_date.filter(|d| d.len() <= 64), notes, platforms })
    }

    /// The download for `platform`, if this release has one.
    pub fn asset(&self, platform: Option<&str>) -> Option<&Asset> {
        self.platforms.get(platform?)
    }
}

/// The release's page on GitHub (release notes, every download).
pub fn release_page(version: &Version) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{version}")
}

/// The releases page, for when there's no particular version to show.
pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases")
}

/// Only `https://` URLs on GitHub's hosts, with nothing unusual in them
/// (no user info, ports, spaces or control characters).
pub fn check_url(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| format!("refusing a non-HTTPS address: {}", shown(url)))?;
    if url.len() > 2048 || url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return Err(format!("refusing an unusual address: {}", shown(url)));
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if !HOSTS.contains(&host) {
        return Err(format!("refusing an address outside GitHub: {}", shown(url)));
    }
    Ok(())
}

fn shown(url: &str) -> String {
    let s: String = url.chars().filter(|c| !c.is_control()).take(120).collect();
    if url.chars().count() > 120 { s + "…" } else { s }
}

/// Which download fits this build. Packaging sets `STRATA_DIST` when it
/// builds a release (`macos-universal`, `macos-arm64`, `windows-x64`), so a
/// universal app keeps getting universal updates and an Apple silicon one
/// the smaller arm64 build. Without it: universal on macOS, the x64 zip on
/// 64-bit Intel Windows, nothing elsewhere (Linux isn't packaged yet).
pub fn current_platform() -> Option<&'static str> {
    platform_for(option_env!("STRATA_DIST"), std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_for(dist: Option<&str>, os: &str, arch: &str) -> Option<&'static str> {
    let default = match (os, arch) {
        ("macos", _) => Some("macos-universal"),
        ("windows", "x86_64") => Some("windows-x64"),
        _ => None,
    };
    match dist.map(str::trim).filter(|d| !d.is_empty()) {
        // Only a build for this OS (an arm64 build can't run on Intel, so it
        // wouldn't be running there).
        Some(d) => PLATFORMS.iter().copied().find(|p| *p == d && p.starts_with(if os == "macos" { "macos-" } else { "windows-" })).or(default),
        None => default,
    }
}

/// From GitHub's releases list: the newest release (by version) that isn't a
/// draft, is allowed by the channel, and has a `latest.json`; returns that
/// asset's download URL.
pub fn pick_from_releases(json: &[u8], prerelease: bool) -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        assets: Vec<GhAsset>,
    }
    #[derive(Deserialize)]
    struct GhAsset {
        name: String,
        browser_download_url: String,
    }
    let list: Vec<Release> = serde_json::from_slice(json).map_err(|e| format!("GitHub's list of releases isn't valid: {e}"))?;
    let best = list
        .into_iter()
        .filter(|r| !r.draft && (prerelease || !r.prerelease))
        .filter_map(|r| {
            let v = Version::parse(&r.tag_name)?;
            let url = r.assets.into_iter().find(|a| a.name == MANIFEST_ASSET)?.browser_download_url;
            Some((v, url))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    match best {
        Some((_, url)) => check_url(&url).map(|()| Some(url)),
        None => Ok(None),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample(version: &str, platforms: &[(&str, &str)]) -> String {
        let p: Vec<String> = platforms
            .iter()
            .map(|(k, url)| format!(r#""{k}": {{"url": "{url}", "size": 1234, "sha256": "{}", "signature": "{}"}}"#, "a1".repeat(32), super::super::sig::b64_encode(&[3u8; 64])))
            .collect();
        format!(r##"{{"version": "{version}", "pub_date": "2026-10-01T00:00:00Z", "notes": "# What's new\n\n- Faster", "platforms": {{{}}}}}"##, p.join(", "))
    }

    const URL: &str = "https://github.com/DRAZY/strata/releases/download/v1.2.0/Strata-1.2.0-universal.app.zip";

    #[test]
    fn parses_a_manifest() {
        let m = Manifest::parse(sample("1.2.0", &[("macos-universal", URL), ("windows-x64", URL), ("linux-riscv", "http://x")]).as_bytes()).unwrap();
        assert_eq!(m.version.to_string(), "1.2.0");
        assert!(m.notes.contains("Faster"));
        assert_eq!(m.platforms.len(), 2, "unknown platforms are ignored, even with bad URLs");
        assert_eq!(m.asset(Some("macos-universal")).unwrap().size, 1234);
        assert!(m.asset(Some("macos-arm64")).is_none() && m.asset(None).is_none());
    }

    #[test]
    fn rejects_bad_manifests() {
        for bad in [
            sample("one", &[]),
            sample("1.2.0", &[("macos-universal", "http://github.com/x.zip")]),
            sample("1.2.0", &[("macos-universal", "https://evil.example/x.zip")]),
            sample("1.2.0", &[("macos-universal", "https://github.com.evil.example/x.zip")]),
            sample("1.2.0", &[("macos-universal", "https://user@github.com/x.zip")]),
            sample("1.2.0", &[("macos-universal", "https://github.com:8443/x.zip")]),
            sample("1.2.0", &[("macos-universal", URL)]).replace(&"a1".repeat(32), "ZZ"),
            sample("1.2.0", &[("macos-universal", URL)]).replace("1234", "0"),
            sample("1.2.0", &[("macos-universal", URL)]).replace("1234", "999999999999"),
            "not json".into(),
            r#"{"notes": "x"}"#.into(),
        ] {
            assert!(Manifest::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let long = sample("1.2.0", &[]).replace("- Faster", &"word ".repeat(20_000));
        assert!(Manifest::parse(long.as_bytes()).unwrap().notes.len() < 33_000, "long notes are cut");
    }

    #[test]
    fn https_only_on_github() {
        assert!(check_url(URL).is_ok());
        assert!(check_url("https://api.github.com/repos/DRAZY/strata/releases").is_ok());
        assert!(check_url("https://objects.githubusercontent.com/a?b=c").is_ok());
        for bad in ["http://github.com/a", "ftp://github.com/a", "file:///etc/passwd", "https://github.com /a", "https://evil.com/github.com", "https://github.com@evil.com/", "-https://github.com", "https://github.com\\@evil.com", "HTTPS://github.com/a", ""] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn picks_the_download_for_this_build() {
        assert_eq!(platform_for(None, "macos", "aarch64"), Some("macos-universal"));
        assert_eq!(platform_for(Some("macos-arm64"), "macos", "aarch64"), Some("macos-arm64"));
        assert_eq!(platform_for(Some("macos-universal"), "macos", "x86_64"), Some("macos-universal"));
        assert_eq!(platform_for(Some("windows-x64"), "macos", "aarch64"), Some("macos-universal"), "another OS's dist is ignored");
        assert_eq!(platform_for(Some("bogus"), "macos", "aarch64"), Some("macos-universal"));
        assert_eq!(platform_for(None, "windows", "x86_64"), Some("windows-x64"));
        assert_eq!(platform_for(None, "windows", "aarch64"), None);
        assert_eq!(platform_for(None, "linux", "x86_64"), None);
        assert!(current_platform().is_some() || !cfg!(any(target_os = "macos", all(windows, target_arch = "x86_64"))));
    }

    #[test]
    fn picks_the_newest_release_for_the_channel() {
        let list = r#"[
            {"tag_name": "v1.1.0-rc.1", "prerelease": true, "assets": [{"name": "latest.json", "browser_download_url": "https://github.com/DRAZY/strata/releases/download/v1.1.0-rc.1/latest.json"}]},
            {"tag_name": "v1.2.0", "draft": true, "assets": [{"name": "latest.json", "browser_download_url": "https://github.com/DRAZY/strata/releases/download/v1.2.0/latest.json"}]},
            {"tag_name": "v1.0.0", "assets": [{"name": "latest.json", "browser_download_url": "https://github.com/DRAZY/strata/releases/download/v1.0.0/latest.json"}]},
            {"tag_name": "v1.0.1", "assets": [{"name": "other.zip", "browser_download_url": "https://github.com/x"}]},
            {"tag_name": "nightly", "assets": [{"name": "latest.json", "browser_download_url": "https://github.com/n"}]}
        ]"#;
        assert_eq!(pick_from_releases(list.as_bytes(), true).unwrap().unwrap(), "https://github.com/DRAZY/strata/releases/download/v1.1.0-rc.1/latest.json");
        assert_eq!(pick_from_releases(list.as_bytes(), false).unwrap().unwrap(), "https://github.com/DRAZY/strata/releases/download/v1.0.0/latest.json");
        assert_eq!(pick_from_releases(b"[]", true).unwrap(), None);
        assert!(pick_from_releases(b"{}", true).is_err());
        let evil = r#"[{"tag_name": "v9.0.0", "assets": [{"name": "latest.json", "browser_download_url": "http://evil.example/latest.json"}]}]"#;
        assert!(pick_from_releases(evil.as_bytes(), false).is_err());
    }
}
