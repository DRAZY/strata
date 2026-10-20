//! What the user decided about each plugin, kept in `plugins.json` in the
//! config folder: whether it's enabled, and the permissions granted to one
//! exact package (its name and SHA-256). A grant for any other package (an
//! update, an edited file) doesn't count, so Strata asks again. A file that
//! can't be read grants nothing.

use super::Perm;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const FILE: &str = "plugins.json";

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct State {
    #[serde(default)]
    pub plugins: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub grant: Option<Grant>,
}

impl Default for Entry {
    fn default() -> Self {
        Self { enabled: true, grant: None }
    }
}

fn yes() -> bool {
    true
}

/// Permissions the user approved for one package.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Grant {
    pub sha256: String,
    pub permissions: BTreeSet<Perm>,
}

pub struct Store {
    path: Option<PathBuf>,
    pub state: State,
}

impl Store {
    /// The store in `config` (None: nothing is remembered, as in tests
    /// without a config folder).
    pub fn load(config: Option<&Path>) -> Self {
        let path = config.map(|c| c.join(FILE));
        let state = match path.as_deref().map(std::fs::read_to_string) {
            Some(Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("{FILE} can't be read ({e}); every plugin will ask for permission again");
                State::default()
            }),
            _ => State::default(),
        };
        Self { path, state }
    }

    pub fn enabled(&self, name: &str) -> bool {
        self.state.plugins.get(name).is_none_or(|e| e.enabled)
    }

    /// The package `sha256` with exactly `permissions` was approved.
    pub fn granted(&self, name: &str, sha256: &str, permissions: &BTreeSet<Perm>) -> bool {
        self.state.plugins.get(name).and_then(|e| e.grant.as_ref()).is_some_and(|g| g.sha256 == sha256 && &g.permissions == permissions)
    }

    pub fn grant(&mut self, name: &str, sha256: &str, permissions: &BTreeSet<Perm>) -> Result<(), String> {
        let e = self.state.plugins.entry(name.to_owned()).or_default();
        e.grant = Some(Grant { sha256: sha256.to_owned(), permissions: permissions.clone() });
        e.enabled = true;
        self.save()
    }

    pub fn set_enabled(&mut self, name: &str, on: bool) -> Result<(), String> {
        self.state.plugins.entry(name.to_owned()).or_default().enabled = on;
        self.save()
    }

    pub fn forget(&mut self, name: &str) -> Result<(), String> {
        self.state.plugins.remove(name);
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&self.state).map_err(|e| e.to_string())?;
        // Written whole and renamed into place: a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        crate::private::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("couldn't save {FILE}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_tied_to_the_exact_package_and_permissions() {
        let dir = super::super::package::tests::temp("store");
        let perms = BTreeSet::from([Perm::ReadDocument]);
        let mut s = Store::load(Some(&dir));
        assert!(!s.granted("wc", "aa", &perms));
        assert!(s.enabled("wc"), "enabled until turned off");
        s.grant("wc", "aa", &perms).unwrap();
        assert!(s.granted("wc", "aa", &perms));
        assert!(!s.granted("wc", "bb", &perms), "another package asks again");
        assert!(!s.granted("wc", "aa", &BTreeSet::from([Perm::ReadDocument, Perm::Network])), "more permissions ask again");
        assert!(!s.granted("other", "aa", &perms), "another name asks again");
        s.set_enabled("wc", false).unwrap();
        // Remembered across runs.
        let s2 = Store::load(Some(&dir));
        assert!(s2.granted("wc", "aa", &perms) && !s2.enabled("wc"));
        // A damaged file grants nothing.
        std::fs::write(dir.join(FILE), "{nope").unwrap();
        let s3 = Store::load(Some(&dir));
        assert!(!s3.granted("wc", "aa", &perms));
        let mut s4 = Store::load(Some(&dir));
        s4.grant("wc", "aa", &perms).unwrap();
        s4.forget("wc").unwrap();
        assert!(!Store::load(Some(&dir)).granted("wc", "aa", &perms));
        std::fs::remove_dir_all(dir).ok();
    }
}
