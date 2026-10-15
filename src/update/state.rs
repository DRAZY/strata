//! What the updater remembers between runs, in `update.json` in the config
//! folder: when it last checked, a version the user skipped, and an
//! installed update waiting to prove it starts. Its own small file rather
//! than part of the session: the background check and the relaunch helper
//! (a separate process) write it, and the session is rewritten as a whole
//! every few seconds.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "update.json";
/// While Strata is open, automatic checks happen this often (plus a small
/// per-install offset, `jitter_secs`, so installs don't all ask at once).
pub const CHECK_INTERVAL_SECS: u64 = 4 * 60 * 60;
/// At launch, check unless the last check was more recent than this, so
/// opening and closing Strata repeatedly doesn't ask GitHub every time.
pub const LAUNCH_MIN_GAP_SECS: u64 = 60 * 60;

/// A fixed offset of up to 15 minutes, different per install (from the
/// config path), added to the interval.
pub fn jitter_secs(seed: &str) -> u64 {
    let h = seed.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    h % (15 * 60)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Saved {
    /// Seconds since 1970 of the last automatic or manual check (tried,
    /// whether or not it reached GitHub).
    pub last_check: Option<u64>,
    /// "Skip This Version": automatic checks don't offer this one.
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// Something to tell the user at the next start (the helper writes it
    /// when it had to put the previous version back).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// An update swapped into place: the old app is kept at `backup` until the
/// new one has started once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    pub version: String,
    pub from_version: String,
    pub target: PathBuf,
    pub backup: PathBuf,
    /// The new version drew its first window.
    #[serde(default)]
    pub launched: bool,
}

impl Saved {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// The saved state; a missing or unreadable file is a fresh start.
    pub fn load(dir: &Path) -> Self {
        std::fs::read(Self::path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Write it whole (to a temporary file, then renamed over the old one).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        crate::private::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
        crate::private::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, Self::path(dir)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Load, change and save.
    pub fn update(dir: &Path, f: impl FnOnce(&mut Saved)) -> std::io::Result<Saved> {
        let mut s = Self::load(dir);
        f(&mut s);
        s.save(dir)?;
        Ok(s)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The `update_check` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ask once whether to check automatically (the default).
    Ask,
    Auto,
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" | "true" => Mode::Auto,
            "off" | "never" | "false" => Mode::Off,
            _ => Mode::Ask,
        }
    }
}

/// Is an automatic check due? Only with `auto`: at launch when the last check
/// is over an hour old, then every `interval` seconds while open. A last
/// check in the future (the clock went back) counts as long ago.
pub fn auto_check_due(mode: Mode, last_check: Option<u64>, now: u64, launch: bool, interval: u64) -> bool {
    let gap = if launch { LAUNCH_MIN_GAP_SECS } else { interval };
    mode == Mode::Auto && last_check.is_none_or(|t| t > now || now - t >= gap)
}

/// Should a found version be offered? It must be newer than this one, and
/// an automatic check doesn't offer a skipped version (a manual check does).
pub fn should_offer(found: &super::Version, current: &super::Version, skipped: Option<&str>, manual: bool) -> bool {
    found > current && (manual || skipped.and_then(super::Version::parse).is_none_or(|s| s != *found))
}

#[cfg(test)]
mod tests {
    use super::super::Version;
    use super::*;

    #[test]
    fn checks_at_launch_then_every_few_hours_and_only_when_on() {
        let (hour, every) = (LAUNCH_MIN_GAP_SECS, CHECK_INTERVAL_SECS);
        let now = 1_800_000_000;
        assert!(auto_check_due(Mode::Auto, None, now, true, every), "never checked");
        assert!(auto_check_due(Mode::Auto, None, now, false, every), "never checked");
        // At launch: over an hour since the last check.
        assert!(!auto_check_due(Mode::Auto, Some(now - 60), now, true, every));
        assert!(!auto_check_due(Mode::Auto, Some(now - hour + 1), now, true, every));
        assert!(auto_check_due(Mode::Auto, Some(now - hour), now, true, every));
        // While open: every interval.
        assert!(!auto_check_due(Mode::Auto, Some(now - hour), now, false, every));
        assert!(!auto_check_due(Mode::Auto, Some(now - every + 1), now, false, every));
        assert!(auto_check_due(Mode::Auto, Some(now - every), now, false, every));
        assert!(auto_check_due(Mode::Auto, Some(now + 3600), now, false, every), "clock went back");
        for mode in [Mode::Ask, Mode::Off] {
            for launch in [true, false] {
                assert!(!auto_check_due(mode, None, now, launch, every));
                assert!(!auto_check_due(mode, Some(now - 10 * every), now, launch, every));
            }
        }
        // The offset is under 15 minutes and stable per install.
        assert!(jitter_secs("/a") < 15 * 60);
        assert_eq!(jitter_secs("/a"), jitter_secs("/a"));
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse(" OFF "), Mode::Off);
        assert_eq!(Mode::parse("ask"), Mode::Ask);
        assert_eq!(Mode::parse("bogus"), Mode::Ask, "anything else asks");
    }

    #[test]
    fn offers_newer_versions_except_a_skipped_one() {
        let v = |s| Version::parse(s).unwrap();
        let current = v("1.0.0");
        assert!(should_offer(&v("1.0.1"), &current, None, false));
        assert!(!should_offer(&v("1.0.0"), &current, None, true), "same version");
        assert!(!should_offer(&v("0.9.0"), &current, None, true), "older");
        assert!(!should_offer(&v("1.0.0-rc.1"), &current, None, true), "its own release candidate is older");
        assert!(should_offer(&v("1.1.0-rc.1"), &current, None, false));
        assert!(!should_offer(&v("1.0.1"), &current, Some("1.0.1"), false), "skipped");
        assert!(should_offer(&v("1.0.1"), &current, Some("v1.0.1"), true), "a manual check offers it anyway");
        assert!(should_offer(&v("1.0.2"), &current, Some("1.0.1"), false), "a newer one than the skipped one");
    }

    #[test]
    fn saves_and_loads() {
        let dir = std::env::temp_dir().join(format!("strata-update-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Saved::load(&dir), Saved::default(), "no file yet");
        let s = Saved::update(&dir, |s| {
            s.last_check = Some(5);
            s.skipped = Some("1.2.0".into());
            s.pending = Some(Pending { version: "1.2.0".into(), from_version: "1.1.0".into(), target: "/A/Strata.app".into(), backup: "/A/.Strata.app.old".into(), launched: false });
        })
        .unwrap();
        assert_eq!(Saved::load(&dir), s);
        std::fs::write(Saved::path(&dir), "{broken").unwrap();
        assert_eq!(Saved::load(&dir), Saved::default(), "a broken file starts over");
        std::fs::remove_dir_all(&dir).ok();
    }
}
