//! Install / update / reload pipeline driven by Core's `install <url>`,
//! `update`, and `reload` verbs.
//!
//! Each verb fires a single `HostTask` (see [`crate::plugins::actions`]) at
//! the runtime thread, which calls into [`handle_host_task`]. From there the
//! pipeline streams progress rows back into the launcher's result list
//! through [`ProgressEmitter`] under stable per-task keys so re-emitted lines
//! replace the previous row in place.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ModelRc, SharedString, VecModel};
use tokio::sync::oneshot;

use crate::QueryWindow;
use crate::confirm::{ConfirmationSummary, PendingConfirmation};
use crate::logging::LogErr;
use crate::plugins;
use crate::plugins::actions;
use crate::plugins::loader::{self, LoadOutcome};
use crate::plugins::registry::PluginRegistry;
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::{HookKind, LifecycleReason};
use crate::settings_ui::SettingsController;
use crate::ui::ConfirmCapRow;

use super::ConfirmState;

/// Stable-key stream of progress rows shared with the launcher's result
/// list. Re-emitting a key replaces the previous row in place, so the
/// install / update / reload tasks can drive a row through "installing →
/// installed" without inventing new rows.
///
/// The emitter holds a fixed `query_id` — re-using `latest_id` keeps these
/// rows from being clobbered by a stale yield from a previously-cancelled
/// JS query.
pub(super) struct ProgressEmitter {
    query_id: u64,
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
}

impl ProgressEmitter {
    pub(super) fn new(
        query_id: u64,
        weak: slint::Weak<QueryWindow>,
        latest: Arc<Mutex<Vec<RankedResult>>>,
        latest_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            query_id,
            weak,
            latest,
            latest_id,
        }
    }

    /// Push (or replace by `key`) one progress row and re-render. Progress
    /// rows are inert — Enter is a no-op — because they communicate
    /// background-task state, not actionable choices.
    fn emit(&self, key: &str, title: String, subtitle: Option<String>) {
        // Skip the paint if another query/task has already taken over —
        // late-arriving progress for an abandoned task shouldn't repaint.
        if self.query_id < self.latest_id.load(Ordering::Relaxed) {
            return;
        }
        let next = RankedResult {
            plugin_name: crate::plugins::builtin::core::NAME.to_owned(),
            result: plugins::result::PluginResult {
                key: key.to_owned(),
                title,
                subtitle,
                icon: None,
                weight: 100.0,
                pinned: true,
                action: plugins::result::Action::Noop,
                alt_action: None,
                alt_title: None,
                alt_subtitle: None,
            },
            order: 0,
        };
        let snapshot: Vec<RankedResult> = match self.latest.lock() {
            Ok(mut slot) => {
                if let Some(existing) = slot.iter_mut().find(|r| r.result.key == next.result.key) {
                    existing.result = next.result;
                } else {
                    let order = slot.len();
                    slot.push(RankedResult { order, ..next });
                }
                slot.clone()
            }
            Err(_) => return,
        };
        let weak = self.weak.clone();

        slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                super::query::render_results(&w, &snapshot);
            }
        })
        .log_debug("install: post progress render to event loop");
    }
}

pub(super) async fn handle_host_task(
    task: actions::HostTask,
    registry: &PluginRegistry,
    progress: ProgressEmitter,
    confirm_state: ConfirmState,
    weak: slint::Weak<QueryWindow>,
    settings: &SettingsController,
) {
    match task {
        actions::HostTask::Reload { name } => {
            run_reload(name, registry, &progress, settings).await;
        }
        actions::HostTask::Install { url } => {
            run_install(&url, registry, &progress, &confirm_state, &weak, settings).await;
        }
        actions::HostTask::UpdateAll => {
            run_update_all(registry, &progress, &confirm_state, &weak, settings).await;
        }
    }
}

/// Drive the install pipeline for a single manifest URL, streaming progress
/// rows under the stable key `install`. Returns the loaded plugin's name on
/// success — `run_update_all` re-uses this so each per-plugin update lands
/// progress under a stable per-plugin key.
async fn run_install(
    url: &str,
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
    settings: &SettingsController,
) -> Option<String> {
    progress.emit(
        "install",
        format!("Installing {url}…"),
        Some("fetching manifest".to_owned()),
    );
    install_pipeline(url, "install", None, registry, progress, confirm_state, weak, settings).await
}

/// Inner install pipeline, parameterised on the progress-row key so the
/// caller can decide between a single `install` row and per-plugin
/// `update-<name>` rows.
///
/// `installed_caps` is `Some` during an update and drives the capability diff
/// shown in the confirmation view.
#[allow(clippy::too_many_arguments)]
async fn install_pipeline(
    url: &str,
    progress_key: &str,
    installed_caps: Option<&[String]>,
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
    settings: &SettingsController,
) -> Option<String> {
    let manifest = match plugins::install::fetch_and_validate_manifest(url).await {
        Ok(m) => m,
        Err(err) => {
            progress.emit(progress_key, "Install failed".to_owned(), Some(err.to_string()));
            return None;
        }
    };

    // Gate on user confirmation before downloading anything.
    let confirmed = request_confirmation(&manifest, url, installed_caps, confirm_state, weak).await;

    if !confirmed {
        progress.emit(progress_key, format!("Install {} cancelled", manifest.name), None);
        return None;
    }

    let plugin_name = manifest.name.clone();
    let plugin_version = manifest.version.clone().unwrap_or_default();
    let staging = stage_payload(&manifest, url, progress_key, progress).await?;

    finalize_install(FinalizeCtx {
        plugin_name: &plugin_name,
        plugin_version: &plugin_version,
        staging_payload_root: &staging,
        progress_key,
        registry,
        progress,
        settings,
    })
    .await
}

/// Show the confirmation view and await the user's decision.
/// Returns `true` when the user pressed Install, `false` for Cancel.
async fn request_confirmation(
    manifest: &plugins::manifest::Manifest,
    manifest_url: &str,
    installed_caps: Option<&[String]>,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
) -> bool {
    let summary = ConfirmationSummary::from_manifest(manifest, manifest_url, installed_caps);

    let (tx, rx) = oneshot::channel::<bool>();

    // Stash the sender so the Slint callbacks can resolve it.
    match confirm_state.lock() {
        Ok(mut guard) => {
            *guard = Some(PendingConfirmation {
                tx,
                summary: summary.clone(),
            });
        }
        Err(err) => {
            tracing::error!(%err, "confirm: state lock poisoned; aborting install");
            return false;
        }
    }

    // Push the summary into Slint properties and flip to the confirm view.
    let weak = weak.clone();
    slint::invoke_from_event_loop(move || {
        let Some(w) = weak.upgrade() else { return };
        populate_confirm_view(&w, &summary);
        w.invoke_show_confirm();
    })
    .log_debug("install: post confirm view to event loop");

    rx.await.unwrap_or(false)
}

/// Populate the `confirm-*` Slint properties from a [`ConfirmationSummary`].
fn populate_confirm_view(window: &QueryWindow, summary: &ConfirmationSummary) {
    window.set_confirm_plugin_name(SharedString::from(summary.plugin_name.as_str()));
    window.set_confirm_display_name(SharedString::from(summary.display_name.as_str()));
    window.set_confirm_version(SharedString::from(summary.version.as_str()));
    window.set_confirm_description(SharedString::from(summary.description.as_str()));
    window.set_confirm_manifest_url(SharedString::from(summary.manifest_url.as_str()));
    let rows: Vec<ConfirmCapRow> = summary
        .capabilities
        .iter()
        .map(|c| ConfirmCapRow {
            cap: SharedString::from(c.cap.as_str()),
            explanation: SharedString::from(c.explanation.as_str()),
            is_new: c.is_new,
        })
        .collect();
    window.set_confirm_capabilities(ModelRc::new(VecModel::from(rows)));
}

/// Download + (extract or write) into a fresh staging dir. Returns the
/// path of the staged payload root. Branches on whichever distribution
/// shape the manifest declared — `archiveUrl` (multi-file archive) or
/// `entryUrl` (single JS file).
async fn stage_payload(
    manifest: &plugins::manifest::Manifest,
    url: &str,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    let plugin_name = manifest.name.clone();
    let plugin_version = manifest.version.clone().unwrap_or_default();

    let staging = match temp_dir_for_install(&plugin_name) {
        Ok(dir) => dir,
        Err(err) => {
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                &format!("create staging dir: {err}"),
            );

            return None;
        }
    };

    let payload_root = match (manifest.archive_url.as_deref(), manifest.entry_url.as_deref()) {
        (Some(archive_url), None) => {
            stage_from_archive(
                manifest,
                url,
                archive_url,
                &plugin_name,
                &plugin_version,
                &staging,
                progress_key,
                progress,
            )
            .await?
        }
        (None, Some(entry_url)) => {
            stage_from_entry(
                manifest,
                entry_url,
                &plugin_name,
                &plugin_version,
                &staging,
                progress_key,
                progress,
            )
            .await?
        }
        (Some(_), Some(_)) => {
            cleanup_staging(&staging);
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                "manifest declares both archiveUrl and entryUrl — pick one",
            );

            return None;
        }
        (None, None) => {
            cleanup_staging(&staging);
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                "manifest missing archiveUrl or entryUrl",
            );

            return None;
        }
    };

    let writeable = plugins::install::manifest_for_write(manifest, url);
    let payload_root_for_write = payload_root.clone();
    let write_result =
        tokio::task::spawn_blocking(move || plugins::install::write_manifest_json(&payload_root_for_write, &writeable))
            .await;

    if let Err(err) = unwrap_spawn_blocking(write_result, "write_manifest_json") {
        cleanup_staging(&staging);
        emit_install_failure(progress, progress_key, &plugin_name, &err);

        return None;
    }
    Some(payload_root)
}

#[allow(clippy::too_many_arguments)]
async fn stage_from_archive(
    manifest: &plugins::manifest::Manifest,
    install_url: &str,
    archive_url: &str,
    plugin_name: &str,
    plugin_version: &str,
    staging: &Path,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    progress.emit(
        progress_key,
        format!("Installing {plugin_name} v{plugin_version}…"),
        Some(format!("downloading {archive_url}")),
    );
    let (bytes, format) = match plugins::install::download_archive(archive_url).await {
        Ok(pair) => pair,
        Err(err) => {
            cleanup_staging(staging);
            emit_install_failure(progress, progress_key, plugin_name, &err.to_string());

            return None;
        }
    };
    let staging_for_extract = staging.to_path_buf();
    let extract_result =
        tokio::task::spawn_blocking(move || plugins::install::extract_archive(&bytes, format, &staging_for_extract))
            .await;

    if let Err(err) = unwrap_spawn_blocking(extract_result, "extract") {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err);

        return None;
    }
    let payload_root = plugins::install::find_payload_root(staging);

    if let Err(err) = plugins::install::cross_check_embedded(&payload_root, manifest, install_url) {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err.to_string());

        return None;
    }
    Some(payload_root)
}

#[allow(clippy::too_many_arguments)]
async fn stage_from_entry(
    manifest: &plugins::manifest::Manifest,
    entry_url: &str,
    plugin_name: &str,
    plugin_version: &str,
    staging: &Path,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    progress.emit(
        progress_key,
        format!("Installing {plugin_name} v{plugin_version}…"),
        Some(format!("downloading {entry_url}")),
    );
    let bytes = match plugins::install::download_entry(entry_url).await {
        Ok(b) => b,
        Err(err) => {
            cleanup_staging(staging);
            emit_install_failure(progress, progress_key, plugin_name, &err.to_string());

            return None;
        }
    };
    // The single-file shape mirrors what an archive would have produced:
    // `<staging>/<plugin>/<entry>` so `finalize_install`'s rename-into-place
    // doesn't need to know which path got us here.
    let payload_root = staging.join(plugin_name);
    let entry_filename = manifest.entry.clone();
    let payload_root_for_write = payload_root.clone();
    let write_result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&payload_root_for_write)?;
        std::fs::write(payload_root_for_write.join(&entry_filename), &bytes)
    })
    .await;

    if let Err(err) = unwrap_spawn_blocking(write_result, "write_entry") {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err);

        return None;
    }
    Some(payload_root)
}

/// Flatten a `JoinResult<Result<T, E>>` from `spawn_blocking` into one
/// stringly-typed error so callers can pipe the message straight into
/// `emit_install_failure`. `op` names the operation for panicked-task logs.
fn unwrap_spawn_blocking<T, E: std::fmt::Display>(
    result: Result<Result<T, E>, tokio::task::JoinError>,
    op: &str,
) -> Result<T, String> {
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(err)) => Err(err.to_string()),
        Err(join_err) => Err(format!("{op} task panicked: {join_err}")),
    }
}

/// Bundle of references `finalize_install` consumes — purely a packaging
/// concession to clippy's arg-count lint; every field is borrowed.
struct FinalizeCtx<'a> {
    plugin_name: &'a str,
    plugin_version: &'a str,
    staging_payload_root: &'a Path,
    progress_key: &'a str,
    registry: &'a PluginRegistry,
    progress: &'a ProgressEmitter,
    settings: &'a SettingsController,
}

/// Atomically swap the staged payload into the user's plugin dir, hot-reload
/// it, and report the result. `staging_payload_root` is the path returned by
/// `stage_payload` — its parent (the tmpdir) is cleaned up before returning.
async fn finalize_install(ctx: FinalizeCtx<'_>) -> Option<String> {
    let FinalizeCtx {
        plugin_name,
        plugin_version,
        staging_payload_root,
        progress_key,
        registry,
        progress,
        settings,
    } = ctx;
    let plugins_dir = registry.options().plugins_dir.clone();
    let staging_path = staging_payload_root.to_path_buf();
    let plugin_name_owned = plugin_name.to_owned();
    let move_result = tokio::task::spawn_blocking(move || {
        plugins::install::move_into_plugins_dir(&staging_path, &plugins_dir, &plugin_name_owned)
    })
    .await;
    let installed_path = match unwrap_spawn_blocking(move_result, "move_into_plugins_dir") {
        Ok(p) => p,
        Err(err) => {
            cleanup_staging(staging_payload_root);
            emit_install_failure(progress, progress_key, plugin_name, &err);

            return None;
        }
    };

    // The payload root's parent is the staging tmpdir — best-effort wipe.
    if let Some(staging_parent) = staging_payload_root.parent() {
        cleanup_staging(staging_parent);
    }

    progress.emit(
        progress_key,
        format!("Loading {plugin_name} v{plugin_version}…"),
        Some(installed_path.display().to_string()),
    );

    match loader::load_one_for_reload(&installed_path, &settings.snapshot()).await {
        Ok(outcome) => {
            let plugin = outcome.plugin;
            let reason = outcome.reason;
            registry.install(Arc::clone(&plugin)).await;
            fire_enable_hooks(
                &[LoadOutcome {
                    plugin: Arc::clone(&plugin),
                    reason,
                }],
                settings,
            );
            progress.emit(
                progress_key,
                format!("Installed {plugin_name} v{plugin_version}"),
                Some("ready to use".to_owned()),
            );
            Some(plugin_name.to_owned())
        }
        Err(err) => {
            progress.emit(
                progress_key,
                format!("Installed {plugin_name} but load failed"),
                Some(err),
            );
            None
        }
    }
}

fn emit_install_failure(progress: &ProgressEmitter, progress_key: &str, plugin_name: &str, detail: &str) {
    progress.emit(
        progress_key,
        format!("Install {plugin_name} failed"),
        Some(detail.to_owned()),
    );
}

async fn run_update_all(
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
    settings: &SettingsController,
) {
    let plugins = registry.snapshot().await;

    if plugins.is_empty() {
        progress.emit(
            "update-summary",
            "No plugins loaded".to_owned(),
            Some("nothing to update".to_owned()),
        );

        return;
    }
    let mut updated = 0usize;
    let mut up_to_date = 0usize;
    let mut failed = 0usize;

    for plugin in plugins {
        let local_version = plugin.manifest.version.clone().unwrap_or_default();
        let installed_caps = plugin.manifest.capabilities.clone();
        let Some(manifest_url) = plugin.manifest.manifest_url.clone() else {
            let key = format!("update-{}", plugin.manifest.name);
            progress.emit(
                &key,
                format!("Skipped {}", plugin.manifest.name),
                Some("no manifestUrl — plugin opts out of updates".to_owned()),
            );

            continue;
        };

        let name = plugin.manifest.name.clone();
        let key = format!("update-{name}");

        progress.emit(&key, format!("Checking {name}…"), Some(manifest_url.clone()));

        let remote = match plugins::install::fetch_and_validate_manifest(&manifest_url).await {
            Ok(m) => m,
            Err(err) => {
                failed += 1;
                progress.emit(&key, format!("Update check failed: {name}"), Some(err.to_string()));

                continue;
            }
        };

        let remote_version = remote.version.clone().unwrap_or_default();

        if !plugins::manifest::is_newer_version(&remote_version, &local_version) {
            up_to_date += 1;
            progress.emit(&key, format!("Up to date: {name} v{local_version}"), None);

            continue;
        }

        progress.emit(
            &key,
            format!("Updating {name} v{local_version} → v{remote_version}…"),
            None,
        );

        // Only prompt if the update introduces new capabilities.
        let needs_prompt = crate::confirm::update_needs_prompt(&remote.capabilities, &installed_caps);
        let caps_arg: Option<&[String]> = needs_prompt.then_some(&installed_caps);

        if install_pipeline(
            &manifest_url,
            &key,
            caps_arg,
            registry,
            progress,
            confirm_state,
            weak,
            settings,
        )
        .await
        .is_some()
        {
            updated += 1;
        } else {
            failed += 1;
        }
    }
    progress.emit(
        "update-summary",
        format!("Update complete — {updated} updated, {up_to_date} up to date, {failed} failed"),
        None,
    );
}

fn temp_dir_for_install(name: &str) -> std::io::Result<PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("high-beam-install-{name}-{now}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn cleanup_staging(path: &Path) {
    std::fs::remove_dir_all(path).log_debug("install: cleanup staging dir");
}

async fn run_reload(
    name: Option<String>,
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    settings: &SettingsController,
) {
    let settings_snapshot = settings.snapshot();

    match name {
        None => {
            progress.emit("reload-all", "Reloading all plugins…".to_owned(), None);
            let outcomes = registry.reload_all(&settings_snapshot).await;
            // The `reload` verb forces every fresh plugin's hook to fire
            // with `Reload`, regardless of whether the manifest version
            // moved — that's what the user just asked for.
            let names: Vec<String> = outcomes.iter().map(|o| o.plugin.manifest.name.clone()).collect();

            persist_versions_and_fire(&outcomes, LifecycleReason::Reload, settings);
            progress.emit(
                "reload-all",
                format!("Reloaded {} plugin(s)", names.len()),
                Some(if names.is_empty() {
                    "no plugins on disk".to_owned()
                } else {
                    names.join(", ")
                }),
            );
        }
        Some(target) => {
            let key = format!("reload-{target}");
            progress.emit(&key, format!("Reloading {target}…"), None);

            match registry.reload_one(&target, &settings_snapshot).await {
                Ok(plugin) => {
                    let version = plugin.manifest.version.clone();
                    persist_versions_and_fire(
                        &[LoadOutcome {
                            plugin: Arc::clone(&plugin),
                            reason: Some(LifecycleReason::Reload),
                        }],
                        LifecycleReason::Reload,
                        settings,
                    );
                    progress.emit(&key, format!("Reloaded {target}"), version.map(|v| format!("v{v}")));
                }
                Err(err) => {
                    progress.emit(&key, format!("Failed to reload {target}"), Some(err.to_string()));
                }
            }
        }
    }
}

/// Variant of [`fire_enable_hooks`] that overrides the loader's reason —
/// used by the `reload` verb, where the user's intent is "reload" no matter
/// whether the manifest version on disk also happened to move.
fn persist_versions_and_fire(outcomes: &[LoadOutcome], reason: LifecycleReason, settings: &SettingsController) {
    let entries: Vec<(String, Option<String>)> = outcomes
        .iter()
        .filter_map(|o| {
            o.plugin
                .manifest
                .version
                .clone()
                .map(|v| (o.plugin.manifest.name.clone(), Some(v)))
        })
        .collect();
    settings.record_loaded_versions(&entries);

    // Bookkeeping persists before the hook fires so a crash mid-hook
    // doesn't replay the work on next boot.
    for outcome in outcomes {
        drop(outcome.plugin.run_lifecycle_hook(HookKind::Enable, reason));
    }
}

/// For each newly-loaded plugin the loader flagged with an `Install` /
/// `Update` reason, record the manifest version in settings and spawn the
/// `onEnable` hook task. Settings are saved once at the end so multiple
/// fires don't each round-trip the TOML file.
pub(super) fn fire_enable_hooks(outcomes: &[LoadOutcome], settings: &SettingsController) {
    let entries: Vec<(String, Option<String>)> = outcomes
        .iter()
        .filter(|o| o.reason.is_some())
        .filter_map(|o| {
            o.plugin
                .manifest
                .version
                .clone()
                .map(|v| (o.plugin.manifest.name.clone(), Some(v)))
        })
        .collect();
    settings.record_loaded_versions(&entries);

    // Bookkeeping persists before the hook fires so a crash mid-hook
    // doesn't replay the work on next boot.
    for outcome in outcomes {
        let Some(reason) = outcome.reason else { continue };
        drop(outcome.plugin.run_lifecycle_hook(HookKind::Enable, reason));
    }
}
