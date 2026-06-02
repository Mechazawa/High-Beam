//! Install / update capability-diff confirmation logic.
//!
//! Decoupled from the UI so the decision functions are testable without
//! spinning up Slint.

use tokio::sync::oneshot;

use crate::plugins::manifest::Manifest;
use crate::sdk::capability::explain_cap;

#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityEntry {
    pub cap: String,
    /// Human-readable explanation — falls back to the raw cap when unknown.
    pub explanation: String,
    /// True when this capability is not present in the currently-installed
    /// plugin (only meaningful during an update flow).
    pub is_new: bool,
}

impl CapabilityEntry {
    fn from_cap(cap: &str, is_new: bool) -> Self {
        Self {
            cap: cap.to_owned(),
            explanation: explain_cap(cap).to_owned(),
            is_new,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfirmationSummary {
    /// `display_name` when present, otherwise `name`.
    pub display_name: String,
    pub plugin_name: String,
    pub version: String,
    pub description: String,
    pub manifest_url: String,
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

pub struct PendingConfirmation {
    /// Send `true` → proceed, `false` → cancel.
    pub tx: oneshot::Sender<bool>,
    pub summary: ConfirmationSummary,
}

/// Whether a given update requires a confirmation prompt — i.e. whether
/// `remote_caps` introduces a capability that isn't already declared in
/// `installed_caps`.
#[must_use]
pub fn update_needs_prompt(remote_caps: &[String], installed_caps: &[String]) -> bool {
    remote_caps.iter().any(|cap| !installed_caps.contains(cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
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
