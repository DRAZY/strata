//! Dev aids switched on by environment variables (`STRATA_SCREENSHOT`,
//! `STRATA_DEV_*`; the README lists them). Every one is read through here so
//! a build decides which work at all:
//!
//! - `var`: aids that only look or open panels (screenshots, a palette or
//!   find bar on the first frame, extra windows, a right-click). They need
//!   the `devtools` feature, which is on by default; a release package built
//!   with `--no-default-features` has none of them.
//! - `risky_var`: aids that run or change things (type a command into a new
//!   terminal, crash on purpose, replace across files, run a command by id).
//!   They work only in debug builds, or with the `dev-hooks` feature, which
//!   no release build turns on, so an environment variable set by someone
//!   else (a launch agent, a login script) can't make a release Strata act.

/// A dev aid that only looks: on with the `devtools` feature.
pub fn var(name: &str) -> Option<String> {
    if cfg!(feature = "devtools") { std::env::var(name).ok() } else { None }
}

/// A dev aid that runs or changes things: debug builds or `dev-hooks` only.
pub fn risky_var(name: &str) -> Option<String> {
    if cfg!(any(debug_assertions, feature = "dev-hooks")) { std::env::var(name).ok() } else { None }
}

/// The dev-palette actions that run or change things (see `risky_var`).
pub fn palette_mode_allowed(mode: &str) -> bool {
    mode != "run" || cfg!(any(debug_assertions, feature = "dev-hooks"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_aids_follow_the_build() {
        // SAFETY (set_var): tests in this binary don't read this variable elsewhere.
        unsafe { std::env::set_var("STRATA_DEV_TEST_ONLY", "x") };
        assert_eq!(risky_var("STRATA_DEV_TEST_ONLY").is_some(), cfg!(any(debug_assertions, feature = "dev-hooks")));
        assert_eq!(var("STRATA_DEV_TEST_ONLY").is_some(), cfg!(feature = "devtools"));
        assert_eq!(palette_mode_allowed("run"), cfg!(any(debug_assertions, feature = "dev-hooks")));
        assert!(palette_mode_allowed("p"));
        unsafe { std::env::remove_var("STRATA_DEV_TEST_ONLY") };
    }
}
