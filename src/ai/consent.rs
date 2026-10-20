//! Providers the user said "always" to, remembered in `ai-consent.json` in
//! Strata's config folder (a provider is its kind and origin, or its
//! program: see `Provider::consent_id`). Read fresh at every check, so
//! another window's answer counts and a deleted file means "ask again".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "ai-consent.json";

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    always_allowed: Vec<String>,
}

pub struct Consent {
    /// None: nothing is saved (tests, no config folder); `memory` holds it.
    path: Option<PathBuf>,
    memory: Vec<String>,
}

impl Consent {
    pub fn new(config: Option<&Path>) -> Self {
        Consent { path: config.map(|d| d.join(FILE)), memory: Vec::new() }
    }

    fn load(&self) -> Vec<String> {
        match &self.path {
            Some(p) => std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Saved>(&t).ok()).map(|s| s.always_allowed).unwrap_or_default(),
            None => self.memory.clone(),
        }
    }

    pub fn allowed(&self, id: &str) -> bool {
        self.load().iter().any(|a| a == id)
    }

    /// Remember "always" for `id`.
    pub fn allow(&mut self, id: &str) -> Result<(), String> {
        let mut all = self.load();
        if !all.iter().any(|a| a == id) {
            all.push(id.to_owned());
        }
        self.save(all)
    }

    /// Forget every "always": each provider asks again.
    pub fn forget_all(&mut self) -> Result<usize, String> {
        let n = self.load().len();
        self.save(Vec::new())?;
        Ok(n)
    }

    fn save(&mut self, all: Vec<String>) -> Result<(), String> {
        let Some(path) = &self.path else {
            self.memory = all;
            return Ok(());
        };
        let text = serde_json::to_string_pretty(&Saved { always_allowed: all }).map_err(|e| e.to_string())?;
        // Write a sibling and rename it over, so a crash can't leave half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text + "\n").and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Couldn't save {}: {e}", path.display())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_per_provider_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("strata-ai-consent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Consent::new(Some(&dir));
        assert!(!c.allowed("openai https://openrouter.ai"));
        c.allow("openai https://openrouter.ai").unwrap();
        c.allow("openai https://openrouter.ai").unwrap();
        // Another window sees it; another host doesn't count.
        let other = Consent::new(Some(&dir));
        assert!(other.allowed("openai https://openrouter.ai"));
        assert!(!other.allowed("openai https://api.openai.com"));
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text.matches("openrouter").count(), 1);
        assert_eq!(c.forget_all().unwrap(), 1);
        assert!(!other.allowed("openai https://openrouter.ai"));
        // A damaged file means "ask".
        std::fs::write(dir.join(FILE), "{oops").unwrap();
        assert!(!other.allowed("openai https://openrouter.ai"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
