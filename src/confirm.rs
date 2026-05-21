//! Install / update capability-diff confirmation logic.
//!
//! Decoupled from the UI so the decision functions are testable without
//! spinning up Slint.

use tokio::sync::oneshot;

use crate::plugins::manifest::Manifest;
use crate::sdk::capability::KNOWN_CAPABILITIES;

/// Human-readable explanation for each known capability, shown in the
/// confirmation view next to the raw capability string.
static CAP_EXPLANATIONS: &[(&str, &str)] = &[
    ("actions", "open URLs, copy text, run commands, reveal files"),
    ("http", "make outbound HTTP requests"),
    ("clipboard.read", "read the system clipboard"),
    ("clipboard.write", "write to the system clipboard"),
    ("fs.read", "read files and list directories"),
    ("fs.cache", "read and write a per-plugin cache directory"),
    ("system.exec", "run shell commands and capture their output"),
    ("system.applescript", "execute AppleScript on macOS"),
    ("icons", "resolve native file / app icons"),
];

/// One row in the capability list the confirmation view renders.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityEntry {
    /// Raw capability string from the manifest.
    pub cap: String,
    /// Human-readable explanation — falls back to the raw cap when unknown.
    pub explanation: String,
    /// True when this capability is not present in the currently-installed
    /// plugin (only meaningful during an update flow).
    pub is_new: bool,
}

impl CapabilityEntry {
    fn from_cap(cap: &str, is_new: bool) -> Self {
        let explanation = CAP_EXPLANATIONS
            .iter()
            .find(|(k, _)| *k == cap)
            .map_or_else(|| cap.to_owned(), |(_, v)| (*v).to_owned());
        Self {
            cap: cap.to_owned(),
            explanation,
            is_new,
        }
    }
}

/// Everything the confirmation view needs to render itself.
#[derive(Debug, Clone)]
pub struct ConfirmationSummary {
    /// `display_name` when present, otherwise `name`.
    pub display_name: String,
    /// Raw plugin name (manifest `name`).
    pub plugin_name: String,
    /// Semver string, may be empty.
    pub version: String,
    /// Optional human-readable description.
    pub description: String,
    /// Source manifest URL.
    pub manifest_url: String,
    /// Capability list to render.
    pub capabilities: Vec<CapabilityEntry>,
}

impl ConfirmationSummary {
    /// Build from a fresh manifest.  `manifest_url` is the URL the caller
    /// fetched from — used as the display source URL.  `installed_caps` is
    /// `None` for a fresh install and `Some(slice)` for an update; caps not
    /// in `installed_caps` are flagged `is_new = true`.
    #[must_use]
    pub fn from_manifest(manifest: &Manifest, manifest_url: &str, installed_caps: Option<&[String]>) -> Self {
        let capabilities = manifest
            .capabilities
            .iter()
            .map(|cap| {
                let is_new = installed_caps.is_some_and(|existing| !existing.contains(cap));
                CapabilityEntry::from_cap(cap, is_new)
            })
            .collect();

        Self {
            display_name: manifest.display_name.clone().unwrap_or_else(|| manifest.name.clone()),
            plugin_name: manifest.name.clone(),
            version: manifest.version.clone().unwrap_or_default(),
            description: manifest.description.clone().unwrap_or_default(),
            manifest_url: manifest_url.to_owned(),
            capabilities,
        }
    }
}

/// Pending install/update waiting for the user's decision.
pub struct PendingConfirmation {
    /// Send `true` → proceed, `false` → cancel.
    pub tx: oneshot::Sender<bool>,
    pub summary: ConfirmationSummary,
}

/// Caps in `remote` that are not present in `installed`.
///
/// Used to decide whether an update needs a confirmation prompt and to
/// flag individual rows in the confirmation view.
#[must_use]
pub fn new_caps<'a>(remote: &'a [String], installed: &[String]) -> Vec<&'a str> {
    remote
        .iter()
        .filter(|cap| !installed.contains(cap))
        .map(String::as_str)
        .collect()
}

/// Whether a given update requires a confirmation prompt.
///
/// Returns `true` when `remote_caps` introduces at least one capability that
/// is not in `installed_caps`.  The decision is a pure function so it can be
/// unit-tested without touching the UI.
#[must_use]
pub fn update_needs_prompt(remote_caps: &[String], installed_caps: &[String]) -> bool {
    !new_caps(remote_caps, installed_caps).is_empty()
}

/// Human-readable explanation for a single known capability.  Falls back to
/// the raw capability string when it isn't in the known set.
#[must_use]
pub fn explain_cap(cap: &str) -> &str {
    CAP_EXPLANATIONS.iter().find(|(k, _)| *k == cap).map_or(cap, |(_, v)| v)
}

/// Whether `cap` is in the host's known capability table.  Used to emit a
/// warning in the confirmation view for unrecognised caps.
#[must_use]
pub fn is_known_cap(cap: &str) -> bool {
    KNOWN_CAPABILITIES.contains(&cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    // --- new_caps ---

    #[test]
    fn new_caps_empty_when_remote_subset_of_installed() {
        let installed = strs(&["actions", "http"]);
        let remote = strs(&["actions"]);
        assert!(new_caps(&remote, &installed).is_empty());
    }

    #[test]
    fn new_caps_empty_when_equal() {
        let caps = strs(&["actions", "http"]);
        assert!(new_caps(&caps, &caps).is_empty());
    }

    #[test]
    fn new_caps_returns_caps_in_remote_not_in_installed() {
        let installed = strs(&["actions"]);
        let remote = strs(&["actions", "http", "fs.read"]);
        let mut result = new_caps(&remote, &installed);
        result.sort_unstable();
        assert_eq!(result, vec!["fs.read", "http"]);
    }

    #[test]
    fn new_caps_all_new_when_installed_empty() {
        let installed: Vec<String> = Vec::new();
        let remote = strs(&["actions", "http"]);
        let mut result = new_caps(&remote, &installed);
        result.sort_unstable();
        assert_eq!(result, vec!["actions", "http"]);
    }

    #[test]
    fn new_caps_empty_remote_returns_empty() {
        let installed = strs(&["actions"]);
        let remote: Vec<String> = Vec::new();
        assert!(new_caps(&remote, &installed).is_empty());
    }

    // --- update_needs_prompt ---

    #[test]
    fn update_needs_prompt_true_when_new_cap_added() {
        let installed = strs(&["actions"]);
        let remote = strs(&["actions", "http"]);
        assert!(update_needs_prompt(&remote, &installed));
    }

    #[test]
    fn update_needs_prompt_false_when_remote_subset() {
        let installed = strs(&["actions", "http", "fs.read"]);
        let remote = strs(&["actions", "http"]);
        assert!(!update_needs_prompt(&remote, &installed));
    }

    #[test]
    fn update_needs_prompt_false_when_equal() {
        let caps = strs(&["actions", "http"]);
        assert!(!update_needs_prompt(&caps, &caps));
    }

    #[test]
    fn update_needs_prompt_false_when_both_empty() {
        assert!(!update_needs_prompt(&[], &[]));
    }

    // --- CapabilityEntry ---

    #[test]
    fn capability_entry_explains_known_cap() {
        let entry = CapabilityEntry::from_cap("http", false);
        assert_eq!(entry.cap, "http");
        assert!(!entry.explanation.is_empty());
        assert_ne!(entry.explanation, "http", "should use human text, not raw cap");
    }

    #[test]
    fn capability_entry_falls_back_for_unknown_cap() {
        let entry = CapabilityEntry::from_cap("future.capability", false);
        assert_eq!(entry.cap, "future.capability");
        assert_eq!(entry.explanation, "future.capability");
    }

    #[test]
    fn capability_entry_is_new_flag_propagates() {
        let new_entry = CapabilityEntry::from_cap("http", true);
        let old_entry = CapabilityEntry::from_cap("http", false);
        assert!(new_entry.is_new);
        assert!(!old_entry.is_new);
    }

    // --- ConfirmationSummary ---

    #[test]
    fn confirmation_summary_fresh_install_no_caps_new() {
        let manifest = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz",
                "capabilities": ["actions", "http"]
            }"#,
        )
        .unwrap();
        let summary = ConfirmationSummary::from_manifest(&manifest, "https://example.com/m.json", None);
        assert!(
            summary.capabilities.iter().all(|c| !c.is_new),
            "fresh install: no caps marked new"
        );
        assert_eq!(summary.capabilities.len(), 2);
    }

    #[test]
    fn confirmation_summary_update_marks_new_caps() {
        let manifest = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "2.0.0",
                "archiveUrl": "https://example.com/h.tar.gz",
                "capabilities": ["actions", "http"]
            }"#,
        )
        .unwrap();
        let installed = strs(&["actions"]);
        let summary = ConfirmationSummary::from_manifest(&manifest, "https://example.com/m.json", Some(&installed));
        let new_count = summary.capabilities.iter().filter(|c| c.is_new).count();
        assert_eq!(new_count, 1, "only `http` is new");
        let http = summary.capabilities.iter().find(|c| c.cap == "http").unwrap();
        assert!(http.is_new);
        let actions = summary.capabilities.iter().find(|c| c.cap == "actions").unwrap();
        assert!(!actions.is_new);
    }

    #[test]
    fn confirmation_summary_display_name_falls_back_to_name() {
        let manifest = Manifest::parse(br#"{ "name": "plain", "archiveUrl": "https://x.com/a.zip" }"#).unwrap();
        let summary = ConfirmationSummary::from_manifest(&manifest, "https://x.com/m.json", None);
        assert_eq!(summary.display_name, "plain");
    }
}
