//! Plugins: separate programs, in any language, that Strata starts and
//! talks to with JSON-RPC over stdin and stdout (the framing language
//! servers use). A plugin can't crash Strata, and it can only ask Strata
//! for what the user granted it. Strata can't sandbox a native program,
//! though: what a plugin does on its own (open files, start programs, use
//! the network) is up to the operating system, and the consent question
//! says so.
//!
//! - `manifest`: `strata-plugin.json`, read and checked.
//! - `package` and `zip`: installing from a folder or a zip into the config
//!   folder, and a package's identity (SHA-256), which consent is tied to.
//! - `store`: what the user decided (enabled, granted permissions).
//! - `api`: the methods, the permission each needs, positions and edits.
//! - `workspace`: file access confined to the open folders.
//! - `host`: one running plugin (process, threads, bounded queues).
//! - `manager`: every installed plugin, starting on activation, consent,
//!   restarts, document events. `app::plugins_ui` puts it on screen.

pub mod api;
#[cfg(test)]
pub mod fake;
pub mod host;
pub mod manager;
pub mod manifest;
pub mod package;
pub mod store;
pub mod workspace;
pub mod zip;

use serde::{Deserialize, Serialize};

/// The API version; a manifest's `api` must say exactly this.
pub const API: &str = "strata.plugin/1";

/// What a plugin may do through Strata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Perm {
    ReadDocument,
    EditDocument,
    ReadWorkspace,
    WriteWorkspace,
    Panel,
    Notify,
    RunProcess,
    Network,
}

impl Perm {
    pub const ALL: [Perm; 8] = [Perm::ReadDocument, Perm::EditDocument, Perm::ReadWorkspace, Perm::WriteWorkspace, Perm::Panel, Perm::Notify, Perm::RunProcess, Perm::Network];

    pub fn id(self) -> &'static str {
        match self {
            Perm::ReadDocument => "read_document",
            Perm::EditDocument => "edit_document",
            Perm::ReadWorkspace => "read_workspace",
            Perm::WriteWorkspace => "write_workspace",
            Perm::Panel => "panel",
            Perm::Notify => "notify",
            Perm::RunProcess => "run_process",
            Perm::Network => "network",
        }
    }

    pub fn from_id(id: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.id() == id)
    }

    /// In plain words, for the consent question.
    pub fn describe(self) -> &'static str {
        match self {
            Perm::ReadDocument => "Read the text of the files you have open",
            Perm::EditDocument => "Change the text of files you have open (you can undo each change)",
            Perm::ReadWorkspace => "Read files in the folders open in the side bar",
            Perm::WriteWorkspace => "Create and replace files in the folders open in the side bar, including scripts and settings your tools run",
            Perm::Panel => "Show a panel of text, and lists to pick from",
            Perm::Notify => "Show messages and items in the status bar",
            Perm::RunProcess => "Run any program on your computer",
            Perm::Network => "Reach the internet and computers on your network",
        }
    }

    /// Things Strata can't limit at all: the plugin does them itself.
    pub fn unlimited(self) -> bool {
        matches!(self, Perm::RunProcess | Perm::Network)
    }
}
