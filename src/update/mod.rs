//! Updates: finding out that a new version is out, and installing it.
//!
//! Strata talks to the network for this alone, and only when the user lets
//! it (the `update_check` setting) or asks (Help ▸ Check for Updates…). It
//! fetches one small file from GitHub, `latest.json` (see `manifest`), and
//! sends nothing but the request itself.
//!
//! Installing: download the file for this build (`fetch`), check its size,
//! SHA-256 and ed25519 signature against the key built into Strata (`sig`,
//! `key`), unpack it next to the app, check the app inside (on macOS its
//! code signature and team too), swap it in keeping the old app as a backup
//! (`install`), and on quit hand over to a helper that starts the new
//! version and puts the old one back if it doesn't start (`helper`).
//!
//! All of it runs on background threads. The `Updater` holds where things
//! stand (`Status`); the UI (src/app/update_ui.rs) shows it and calls in.

pub mod fetch;
pub mod helper;
pub mod install;
mod key;
pub mod manifest;
pub mod sig;
pub mod state;
pub mod version;

pub use manifest::Manifest;
pub use state::Mode;
pub use version::Version;

use fetch::Fetch;
use state::Saved;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// This build's version.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is semver")
}

/// A release found by a check.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub manifest: Manifest,
    /// Whether it can be installed from inside Strata, and if not, why.
    pub installable: Result<(), String>,
}

impl Release {
    pub fn version(&self) -> &Version {
        &self.manifest.version
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking { manual: bool },
    /// The newest version is this one (a manual check says so).
    UpToDate { manual: bool },
    Available { release: Release, manual: bool },
    Downloading { release: Release, done: u64, total: u64 },
    /// Verifying and unpacking; `step` says which.
    Installing { release: Release, step: &'static str },
    /// In place; it starts when Strata is next opened (the UI quits and
    /// relaunches right away).
    Installed { release: Release, plan: helper::Plan },
    Failed { error: String, release: Option<Release>, manual: bool },
    /// A note for the user (an update finished, or was rolled back).
    Message(String),
}

impl Status {
    pub fn busy(&self) -> bool {
        matches!(self, Status::Checking { .. } | Status::Downloading { .. } | Status::Installing { .. })
    }
}

/// Where updates come from and what they're checked against (tests change these).
pub struct Source {
    pub fetch: Arc<dyn Fetch>,
    pub key: [u8; 32],
    pub current: Version,
    pub platform: Option<&'static str>,
    /// The app to replace; Err: why it can't be.
    pub target: Result<PathBuf, String>,
}

impl Source {
    fn real() -> Self {
        Self { fetch: Arc::new(fetch::Curl), key: key::PUBLIC_KEY, current: current_version(), platform: manifest::current_platform(), target: install::locate() }
    }
}

pub struct Updater {
    source: Source,
    config: Mutex<Option<PathBuf>>,
    status: Mutex<Status>,
    cancel: Mutex<Arc<AtomicBool>>,
    /// Called when the status changes (repaints the UI).
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Set by the UI when quitting to finish an update; `on_exit` starts it.
    relaunch: Mutex<Option<helper::Plan>>,
    /// The window showing the notices (a token it chose), and when it last did.
    owner: Mutex<Option<(u64, f64)>>,
    /// `startup` ran (once per process, whichever window is first).
    started: AtomicBool,
}

impl Updater {
    pub fn new(source: Source) -> Self {
        Self { source, config: Mutex::new(None), status: Mutex::new(Status::Idle), cancel: Mutex::new(Arc::new(AtomicBool::new(false))), waker: Mutex::new(None), relaunch: Mutex::new(None), owner: Mutex::new(None), started: AtomicBool::new(false) }
    }

    /// The one updater every window shares.
    pub fn global() -> Arc<Updater> {
        static G: OnceLock<Arc<Updater>> = OnceLock::new();
        G.get_or_init(|| Arc::new(Updater::new(Source::real()))).clone()
    }

    /// An updater that never reaches the network and can't install (tests).
    #[cfg(test)]
    pub fn for_tests() -> Arc<Updater> {
        Arc::new(Updater::new(Source { fetch: Arc::new(fetch::tests::FakeNet::default()), key: [0; 32], current: current_version(), platform: manifest::current_platform(), target: Err("tests don't install".into()) }))
    }

    #[cfg(test)]
    pub fn set_status(&self, s: Status) {
        self.set(s);
    }

    /// True the first time it's called in this process.
    pub fn first_start(&self) -> bool {
        !self.started.swap(true, Ordering::Relaxed)
    }

    /// Which window shows the notices. A window that asked (`take`) gets
    /// them; otherwise the first window, or the first one to draw when the
    /// owner hasn't for a second (it was closed). Whether `me` shows them.
    pub fn notice_owner(&self, me: u64, first: bool, now: f64, take: bool) -> bool {
        let mut o = self.owner.lock().unwrap();
        let mine = match *o {
            _ if take => true,
            None => first,
            Some((id, _)) if id == me => true,
            Some((_, seen)) => first && (now - seen > 1.0 || now < seen),
        };
        if mine {
            *o = Some((me, now));
        }
        mine
    }

    pub fn set_config(&self, dir: Option<PathBuf>) {
        *self.config.lock().unwrap() = dir;
    }

    pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.waker.lock().unwrap() = Some(waker);
    }

    fn config(&self) -> Option<PathBuf> {
        self.config.lock().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn current(&self) -> &Version {
        &self.source.current
    }

    fn set(&self, s: Status) {
        *self.status.lock().unwrap() = s;
        self.wake();
    }

    fn wake(&self) {
        let w = self.waker.lock().unwrap().clone();
        if let Some(w) = w {
            w();
        }
    }

    /// What's saved in update.json (nothing without a config folder).
    pub fn saved(&self) -> Saved {
        self.config().map(|d| Saved::load(&d)).unwrap_or_default()
    }

    fn save(&self, f: impl FnOnce(&mut Saved)) {
        if let Some(dir) = self.config()
            && let Err(e) = Saved::update(&dir, f)
        {
            log::warn!("couldn't save the update state: {e}");
        }
    }

    /// Show a note (from `startup`) unless something else is showing.
    pub fn set_status_message(&self, note: String) {
        let mut s = self.status.lock().unwrap();
        if *s == Status::Idle {
            *s = Status::Message(note);
        }
    }

    /// Back to idle: the notice was closed (Later, OK).
    pub fn dismiss(&self) {
        let mut s = self.status.lock().unwrap();
        if !s.busy() && !matches!(*s, Status::Installed { .. }) {
            *s = Status::Idle;
        }
    }

    /// Skip This Version: automatic checks won't offer it again.
    pub fn skip(&self, version: &Version) {
        let v = version.to_string();
        self.save(|s| s.skipped = Some(v));
        self.dismiss();
    }

    pub fn cancel(&self) {
        self.cancel.lock().unwrap().store(true, Ordering::Relaxed);
    }

    fn fresh_cancel(&self) -> Arc<AtomicBool> {
        let c = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = c.clone();
        c
    }

    /// Is an automatic check due now?
    /// `launch`: the first look after Strata started (a shorter gap applies).
    pub fn auto_check_due(&self, mode: Mode, launch: bool, interval: u64) -> bool {
        mode == Mode::Auto && !self.status().busy() && state::auto_check_due(mode, self.saved().last_check, state::now_secs(), launch, interval)
    }

    /// Check for a new version in the background. `manual` checks report
    /// "up to date" and errors, and offer a skipped version.
    pub fn check(self: &Arc<Self>, prerelease: bool, manual: bool) {
        {
            let mut s = self.status.lock().unwrap();
            if s.busy() || matches!(*s, Status::Installed { .. }) {
                return;
            }
            *s = Status::Checking { manual };
        }
        self.wake();
        self.save(|s| s.last_check = Some(state::now_secs()));
        let cancel = self.fresh_cancel();
        let me = self.clone();
        std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = me.find_release(prerelease, &cancel);
                let skipped = me.saved().skipped;
                let status = match result {
                    Err(e) if e == fetch::CANCELLED => Status::Idle,
                    Err(e) => {
                        log::warn!("update check failed: {e}");
                        if manual { Status::Failed { error: e, release: None, manual } } else { Status::Idle }
                    }
                    Ok(None) => Status::UpToDate { manual },
                    Ok(Some(m)) if state::should_offer(&m.version, &me.source.current, skipped.as_deref(), manual) => {
                        log::info!("Strata {} is available", m.version);
                        let installable = me.installable(&m);
                        Status::Available { release: Release { manifest: m, installable }, manual }
                    }
                    Ok(Some(_)) => Status::UpToDate { manual },
                };
                me.set(status);
            })
            .expect("spawn update-check");
    }

    /// The feed for the channel, parsed; None if there's no release yet.
    fn find_release(&self, prerelease: bool, cancel: &AtomicBool) -> Result<Option<Manifest>, String> {
        let url = if prerelease {
            let list = self.source.fetch.get(manifest::RELEASES_API, manifest::MAX_API_BYTES, cancel)?;
            match manifest::pick_from_releases(&list, true)? {
                Some(u) => u,
                None => return Ok(None),
            }
        } else {
            manifest::STABLE_FEED.to_owned()
        };
        let bytes = self.source.fetch.get(&url, manifest::MAX_MANIFEST_BYTES, cancel)?;
        Manifest::parse(&bytes).map(Some)
    }

    /// Can `m` be installed from inside Strata here?
    fn installable(&self, m: &Manifest) -> Result<(), String> {
        if m.asset(self.source.platform).is_none() {
            return Err("This release has no download for this system that Strata can install itself.".into());
        }
        let target = self.source.target.as_ref().map_err(Clone::clone)?;
        install::check_writable(target)
    }

    /// Install and Relaunch: download, verify and swap in the background.
    /// Ends in `Installed` (then the UI quits) or `Failed`.
    pub fn install(self: &Arc<Self>) {
        let release = {
            let mut s = self.status.lock().unwrap();
            let release = match &*s {
                Status::Available { release, .. } | Status::Failed { release: Some(release), .. } => release.clone(),
                _ => return,
            };
            *s = Status::Downloading { release: release.clone(), done: 0, total: release.manifest.asset(self.source.platform).map_or(0, |a| a.size) };
            release
        };
        self.wake();
        let cancel = self.fresh_cancel();
        let me = self.clone();
        std::thread::Builder::new()
            .name("update-install".into())
            .spawn(move || {
                let status = match me.download_and_swap(&release, &cancel) {
                    Ok(plan) => Status::Installed { release, plan },
                    Err(e) if e == fetch::CANCELLED => Status::Available { release, manual: true },
                    Err(e) => {
                        log::error!("update to {} failed: {e}", release.version());
                        Status::Failed { error: e, release: Some(release), manual: true }
                    }
                };
                me.set(status);
            })
            .expect("spawn update-install");
    }

    fn download_and_swap(&self, release: &Release, cancel: &AtomicBool) -> Result<helper::Plan, String> {
        release.installable.clone()?;
        let platform = self.source.platform.ok_or("no download for this system")?;
        let asset = release.manifest.asset(Some(platform)).ok_or("no download for this system")?;
        let target = self.source.target.clone()?;
        install::sweep(&target, None);
        let staging = install::staging_dir(&target)?;
        let result = self.stage_and_swap(release, platform, asset, &target, &staging, cancel);
        install::remove(&staging);
        result
    }

    fn stage_and_swap(&self, release: &Release, platform: &str, asset: &manifest::Asset, target: &Path, staging: &Path, cancel: &AtomicBool) -> Result<helper::Plan, String> {
        let version = release.version();
        let zip = staging.join("download.zip");
        let total = asset.size;
        self.source.fetch.download(&asset.url, &zip, asset.size, cancel, &|done| {
            let mut s = self.status.lock().unwrap();
            if let Status::Downloading { done: d, .. } = &mut *s {
                *d = done.min(total);
            }
            drop(s);
            self.wake();
        })?;
        self.set(Status::Installing { release: release.clone(), step: "Checking the signature…" });
        verify_download(&zip, platform, version, asset, &self.source.key)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(fetch::CANCELLED.into());
        }
        self.set(Status::Installing { release: release.clone(), step: "Unpacking…" });
        let app = install::unpack(&zip, staging)?;
        self.set(Status::Installing { release: release.clone(), step: "Checking the new version…" });
        install::verify_staged(&app, version, install::self_passes_gatekeeper(target))?;
        if cancel.load(Ordering::Relaxed) {
            return Err(fetch::CANCELLED.into());
        }
        let backup = install::backup_path(target);
        install::swap(target, &app, &backup)?;
        let (v, from) = (version.to_string(), self.source.current.to_string());
        let (t, b) = (target.to_path_buf(), backup.clone());
        self.save(|s| s.pending = Some(state::Pending { version: v, from_version: from, target: t, backup: b, launched: false }));
        log::info!("installed Strata {version} at {}; the previous version is at {}", target.display(), backup.display());
        Ok(helper::Plan { pid: std::process::id(), target: target.to_path_buf(), backup, version: version.to_string() })
    }

    /// The UI is quitting to finish `plan`; `None` takes that back (the quit
    /// was cancelled).
    pub fn set_relaunch(&self, plan: Option<helper::Plan>) {
        *self.relaunch.lock().unwrap() = plan;
    }

    /// Strata is exiting: start the helper if an update is waiting for it.
    pub fn on_exit(&self) {
        if let Some(plan) = self.relaunch.lock().unwrap().take() {
            match helper::spawn(&plan) {
                Ok(()) => log::info!("relaunching as Strata {}", plan.version),
                Err(e) => log::error!("couldn't start the relaunch helper: {e}; Strata {} starts next time", plan.version),
            }
        }
    }

    /// At startup, once the first window is up: if this is a just-installed
    /// version, mark it started and delete the previous version's backup;
    /// tidy leftovers; return a note to show (an update finished or rolled back).
    pub fn startup(&self) -> Option<String> {
        let dir = self.config()?;
        let saved = Saved::load(&dir);
        let mut note = saved.notice.clone();
        match &saved.pending {
            Some(p) if Version::parse(&p.version).as_ref() == Some(&self.source.current) => {
                let p = p.clone();
                let _ = Saved::update(&dir, |s| {
                    if let Some(q) = s.pending.as_mut() {
                        q.launched = true;
                    }
                    s.notice = None;
                });
                note = Some(format!("Strata was updated to {}.", p.version));
                // The helper may still be running from the backup (Windows
                // keeps it locked): retry for a while, off the UI thread.
                std::thread::spawn(move || {
                    for _ in 0..60 {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        if install::remove(&p.backup) {
                            let _ = Saved::update(&dir, |s| s.pending = None);
                            return;
                        }
                    }
                    log::warn!("couldn't delete the previous version at {}", p.backup.display());
                });
            }
            Some(p) if !p.backup.exists() => {
                let _ = Saved::update(&dir, |s| s.pending = None);
            }
            _ => {
                if saved.notice.is_some() {
                    let _ = Saved::update(&dir, |s| s.notice = None);
                }
            }
        }
        if let Ok(target) = &self.source.target {
            let target = target.clone();
            std::thread::spawn(move || install::sweep(&target, None));
        }
        note
    }
}

/// Size, SHA-256 and signature of a downloaded file.
pub fn verify_download(file: &Path, platform: &str, version: &Version, asset: &manifest::Asset, key: &[u8; 32]) -> Result<(), String> {
    let f = std::fs::File::open(file).map_err(|e| format!("couldn't read the download: {e}"))?;
    let (sha256, size) = sig::sha256_hex(std::io::BufReader::new(f)).map_err(|e| format!("couldn't read the download: {e}"))?;
    if size != asset.size {
        return Err(format!("the download is {size} bytes, not the {} the update feed says", asset.size));
    }
    if sha256 != asset.sha256 {
        return Err("the download's SHA-256 doesn't match the update feed".into());
    }
    sig::verify(key, &sig::message(platform, &version.to_string(), &sha256, size), &asset.signature).map_err(|e| format!("the download isn't signed by Strata's developer: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::fetch::tests::FakeNet;
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub const SEED: [u8; 32] = [11u8; 32];
    const ZIP_URL: &str = "https://github.com/DRAZY/strata/releases/download/v1.1.0/Strata-1.1.0-universal.app.zip";

    pub fn signed_manifest(version: &str, platform: &str, body: &[u8], url: &str) -> String {
        let (sha, size) = sig::sha256_hex(body).unwrap();
        let sk = SigningKey::from_bytes(&SEED);
        let signature = sig::b64_encode(&sk.sign(&sig::message(platform, version, &sha, size)).to_bytes());
        serde_json::json!({"version": version, "notes": "## New\n\n- Things", "platforms": {platform: {"url": url, "size": size, "sha256": sha, "signature": signature}}}).to_string()
    }

    pub fn updater(net: FakeNet, current: &str, target: Result<PathBuf, String>) -> Arc<Updater> {
        let key = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
        Arc::new(Updater::new(Source { fetch: Arc::new(net), key, current: Version::parse(current).unwrap(), platform: Some("macos-universal"), target }))
    }

    pub fn wait(u: &Updater) -> Status {
        for _ in 0..500 {
            let s = u.status();
            if !s.busy() {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("still busy: {:?}", u.status());
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-updater-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_a_newer_version_and_remembers_the_check() {
        let feed = signed_manifest("1.1.0", "macos-universal", b"zip", ZIP_URL);
        let dir = temp("check");
        let u = updater(FakeNet::with(&[(manifest::STABLE_FEED, feed.into_bytes())]), "1.0.0", Err("not here".into()));
        u.set_config(Some(dir.clone()));
        assert!(u.auto_check_due(Mode::Auto, true, state::interval_secs(4)) && !u.auto_check_due(Mode::Ask, true, state::interval_secs(4)));
        u.check(false, false);
        let Status::Available { release, manual: false } = wait(&u) else { panic!("{:?}", u.status()) };
        assert_eq!(release.version().to_string(), "1.1.0");
        assert_eq!(release.installable, Err("not here".into()), "found, but only the browser can install it");
        assert!(!u.auto_check_due(Mode::Auto, false, state::interval_secs(4)), "just checked");
        // Skipped: an automatic check stays quiet, a manual one offers it.
        u.skip(release.version());
        assert_eq!(u.status(), Status::Idle);
        u.check(false, false);
        assert_eq!(wait(&u), Status::UpToDate { manual: false });
        u.check(false, true);
        assert!(matches!(wait(&u), Status::Available { manual: true, .. }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn up_to_date_and_errors() {
        let feed = signed_manifest("1.0.0", "macos-universal", b"zip", ZIP_URL);
        let u = updater(FakeNet::with(&[(manifest::STABLE_FEED, feed.into_bytes())]), "1.0.0", Err("x".into()));
        u.check(false, true);
        assert_eq!(wait(&u), Status::UpToDate { manual: true });
        // No feed: a manual check says why, an automatic one is quiet.
        let u = updater(FakeNet::default(), "1.0.0", Err("x".into()));
        u.check(false, true);
        let Status::Failed { error, .. } = wait(&u) else { panic!() };
        assert!(error.contains("404"), "{error}");
        u.check(false, false);
        assert_eq!(wait(&u), Status::Idle);
    }

    #[test]
    fn the_prerelease_channel_reads_the_releases_list() {
        let url = "https://github.com/DRAZY/strata/releases/download/v1.1.0-rc.1/latest.json";
        let list = format!(r#"[{{"tag_name": "v1.1.0-rc.1", "prerelease": true, "assets": [{{"name": "latest.json", "browser_download_url": "{url}"}}]}}]"#);
        let feed = signed_manifest("1.1.0-rc.1", "macos-universal", b"zip", ZIP_URL);
        let u = updater(FakeNet::with(&[(manifest::RELEASES_API, list.into_bytes()), (url, feed.into_bytes())]), "1.0.0", Err("x".into()));
        u.check(true, true);
        let Status::Available { release, .. } = wait(&u) else { panic!("{:?}", u.status()) };
        assert_eq!(release.version().to_string(), "1.1.0-rc.1");
        // The stable channel never asked the API.
        let u = updater(FakeNet::default(), "1.0.0", Err("x".into()));
        u.check(false, true);
        wait(&u);
    }

    #[test]
    fn downloads_are_verified() {
        let dir = temp("verify");
        let body = b"pretend zip".to_vec();
        let feed = signed_manifest("1.1.0", "macos-universal", &body, ZIP_URL);
        let m = Manifest::parse(feed.as_bytes()).unwrap();
        let asset = m.asset(Some("macos-universal")).unwrap();
        let key = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
        let file = dir.join("d.zip");
        std::fs::write(&file, &body).unwrap();
        assert_eq!(verify_download(&file, "macos-universal", &m.version, asset, &key), Ok(()));
        // Signed for another platform or version: refused.
        assert!(verify_download(&file, "macos-arm64", &m.version, asset, &key).is_err());
        assert!(verify_download(&file, "macos-universal", &Version::parse("1.2.0").unwrap(), asset, &key).is_err());
        // Another key.
        let other = SigningKey::from_bytes(&[1u8; 32]).verifying_key().to_bytes();
        assert!(verify_download(&file, "macos-universal", &m.version, asset, &other).unwrap_err().contains("signed"));
        // Tampered (same size), truncated.
        std::fs::write(&file, b"pretend zap").unwrap();
        assert!(verify_download(&file, "macos-universal", &m.version, asset, &key).unwrap_err().contains("SHA-256"));
        std::fs::write(&file, b"pretend").unwrap();
        assert!(verify_download(&file, "macos-universal", &m.version, asset, &key).unwrap_err().contains("bytes"));
        // A tampered feed: the hash changed to match tampered bytes, signature not.
        let mut forged = asset.clone();
        forged.sha256 = sig::sha256_hex(&b"pretend zap"[..]).unwrap().0;
        std::fs::write(&file, b"pretend zap").unwrap();
        assert!(verify_download(&file, "macos-universal", &m.version, &forged, &key).unwrap_err().contains("signed"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_bad_download_never_touches_the_app() {
        let dir = temp("install");
        let target = dir.join("Strata.app");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("v"), "1.0.0").unwrap();
        let feed = signed_manifest("1.1.0", "macos-universal", b"the real zip", ZIP_URL);
        let net = FakeNet::with(&[(manifest::STABLE_FEED, feed.into_bytes()), (ZIP_URL, b"an evil zip!".to_vec())]);
        let u = updater(net, "1.0.0", Ok(target.clone()));
        u.set_config(Some(dir.join("config")));
        u.check(false, true);
        let Status::Available { release, .. } = wait(&u) else { panic!("{:?}", u.status()) };
        assert_eq!(release.installable, Ok(()));
        u.install();
        let Status::Failed { error, release: Some(_), .. } = wait(&u) else { panic!("{:?}", u.status()) };
        assert!(error.contains("SHA-256"), "{error}");
        assert_eq!(std::fs::read_to_string(target.join("v")).unwrap(), "1.0.0");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(left.iter().all(|n| !n.to_string_lossy().starts_with(install::STAGING_PREFIX)), "staging removed: {left:?}");
        assert!(u.saved().pending.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_download_can_be_cancelled() {
        let dir = temp("cancel");
        let target = dir.join("Strata.app");
        std::fs::create_dir_all(&target).unwrap();
        let body = b"zip".to_vec();
        let feed = signed_manifest("1.1.0", "macos-universal", &body, ZIP_URL);
        let net = FakeNet::with(&[(manifest::STABLE_FEED, feed.into_bytes()), (ZIP_URL, body)]);
        net.stall.store(true, Ordering::Relaxed);
        let u = updater(net, "1.0.0", Ok(target.clone()));
        u.check(false, true);
        wait(&u);
        u.install();
        for _ in 0..200 {
            if matches!(u.status(), Status::Downloading { done, .. } if done > 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Status::Downloading { done, total, .. } = u.status() else { panic!("{:?}", u.status()) };
        assert!(done > 0 && total == 3, "progress {done}/{total}");
        u.cancel();
        assert!(matches!(wait(&u), Status::Available { .. }), "back to the offer");
        assert!(target.exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn startup_finishes_an_update_and_reports_a_rollback() {
        let dir = temp("startup");
        let config = dir.join("config");
        let backup = dir.join(".Strata.app.old");
        std::fs::create_dir_all(&backup).unwrap();
        Saved::update(&config, |s| s.pending = Some(state::Pending { version: "1.1.0".into(), from_version: "1.0.0".into(), target: dir.join("Strata.app"), backup: backup.clone(), launched: false })).unwrap();
        // The new version starts.
        let u = updater(FakeNet::default(), "1.1.0", Err("x".into()));
        u.set_config(Some(config.clone()));
        assert_eq!(u.startup().unwrap(), "Strata was updated to 1.1.0.");
        assert!(Saved::load(&config).pending.unwrap().launched, "the helper sees it started");
        // The previous version after a rollback: the helper's note, once.
        Saved::update(&config, |s| {
            s.pending = None;
            s.notice = Some("Strata 1.1.0 didn't start".into());
        })
        .unwrap();
        let u = updater(FakeNet::default(), "1.0.0", Err("x".into()));
        u.set_config(Some(config.clone()));
        assert_eq!(u.startup().unwrap(), "Strata 1.1.0 didn't start");
        assert_eq!(u.startup(), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
