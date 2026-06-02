//! Install / update / reload pipeline driven by Core's `install <url>`,
//! `update`, and `reload` verbs.
//!
//! Install + reload fire a [`actions::HostTask`] at the runtime thread,
//! which calls into [`handle_host_task`]. From there the pipeline streams
//! progress rows back into the launcher's result list through
//! [`ProgressEmitter`] under stable per-task keys so re-emitted lines
//! replace the previous row in place.
//!
//! Update is different: it opens a dedicated host view (see
//! [`super::host_view`]) and the per-plugin progress lives as state
//! mutations on that view instead of rows. [`run_update_view`] is the
//! entry point.

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
use super::host_view::{self, EntryStatus, HostView, UpdateEntry, UpdateSummary};

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
    }
}

/// Drive the install pipeline for a single manifest URL, streaming progress
/// rows under the stable key `install`. Returns the loaded plugin's name on
/// success. The update flow drives [`install_pipeline`] directly (under a
/// per-plugin key), so it doesn't route through here.
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

/// Update entry point — runs against a host-driven update view (see
/// [`super::host_view`]). The Slint thread already seeded the slot before
/// posting `HostMessage::UpdateAll`; here we discover the plugin list,
/// fill in entries, then walk each one updating the view state in place.
/// Cancellation: the slot's cancel token fires on Esc — we abort between
/// plugins (and skip pending ones with `is_cancelled()` checks).
pub(super) async fn run_update_view(
    registry: &PluginRegistry,
    host_view: HostView,
    weak: slint::Weak<QueryWindow>,
    confirm_state: ConfirmState,
    settings: &SettingsController,
) {
    let plugins = registry.snapshot().await;
    let cancel = match host_view.lock() {
        Ok(g) => g.as_ref().map(|s| s.cancel.clone()),
        Err(err) => {
            tracing::error!(%err, "update: host_view lock poisoned at start");
            return;
        }
    };
    let Some(cancel) = cancel else {
        tracing::warn!("update: host view slot empty at start; aborting");
        return;
    };

    seed_update_entries(&host_view, &weak, &plugins);

    let mut tally = UpdateTally::default();
    // install_pipeline writes progress rows through ProgressEmitter; we
    // wire a silent one so those writes don't clobber the real launcher
    // result list (which becomes visible again the moment the user Escs
    // out of the update view). `query_id = 0` against a pre-bumped
    // latest_id of 1 makes every emit() short-circuit on the stale-check
    // at the top of `ProgressEmitter::emit`. Hoisting it out of the loop
    // avoids 16+ pointless allocations per update run.
    let silent_progress = ProgressEmitter::new(
        0,
        weak.clone(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicU64::new(1)),
    );

    for plugin in plugins {
        if cancel.is_cancelled() {
            tracing::info!("update: cancelled mid-loop");
            return;
        }
        process_one_update(
            &plugin,
            &host_view,
            &weak,
            registry,
            &confirm_state,
            settings,
            &silent_progress,
            &mut tally,
        )
        .await;
        // The install pipeline switches the window to VIEW-CONFIRM mid-flow
        // for capability prompts. After the prompt resolves, the confirm
        // callback returns the window to VIEW-QUERY by default — repaint
        // the host view so we come back to it instead of a blank launcher.
        host_view::schedule_paint(&host_view, &weak);
    }

    update_view_state(&host_view, &weak, |s| {
        s.summary = Some(UpdateSummary {
            updated: tally.updated,
            up_to_date: tally.up_to_date,
            skipped: tally.skipped,
            failed: tally.failed,
        });
    });
}

#[derive(Default)]
struct UpdateTally {
    updated: usize,
    up_to_date: usize,
    skipped: usize,
    failed: usize,
}

/// Seed one entry per plugin so the view paints "N queued" immediately.
/// Plugins without `manifestUrl` get Skipped up front.
fn seed_update_entries(
    host_view: &HostView,
    weak: &slint::Weak<QueryWindow>,
    plugins: &[Arc<crate::plugins::runtime::LoadedPlugin>],
) {
    let entries: Vec<UpdateEntry> = plugins
        .iter()
        .map(|p| UpdateEntry {
            name: p.manifest.name.clone(),
            local_version: p.manifest.version.clone().unwrap_or_default(),
            status: if p.manifest.manifest_url.is_none() {
                EntryStatus::Skipped {
                    reason: "no manifestUrl".to_owned(),
                }
            } else {
                EntryStatus::Queued
            },
        })
        .collect();

    update_view_state(host_view, weak, |s| s.entries = entries);
}

/// Run one plugin's update sub-flow: fetch manifest, version-compare,
/// optionally drive `install_pipeline`. Mutates `tally` and the matching
/// view entry's status as it goes.
#[allow(clippy::too_many_arguments)]
async fn process_one_update(
    plugin: &Arc<crate::plugins::runtime::LoadedPlugin>,
    host_view: &HostView,
    weak: &slint::Weak<QueryWindow>,
    registry: &PluginRegistry,
    confirm_state: &ConfirmState,
    settings: &SettingsController,
    silent_progress: &ProgressEmitter,
    tally: &mut UpdateTally,
) {
    let name = plugin.manifest.name.clone();
    let local_version = plugin.manifest.version.clone().unwrap_or_default();
    let installed_caps = plugin.manifest.capabilities.clone();

    let Some(manifest_url) = plugin.manifest.manifest_url.clone() else {
        tally.skipped += 1;
        // Entry was already seeded with Skipped — nothing to update.
        return;
    };

    set_entry_status(host_view, weak, &name, EntryStatus::Checking);

    let remote = match plugins::install::fetch_and_validate_manifest(&manifest_url).await {
        Ok(m) => m,
        Err(err) => {
            tally.failed += 1;
            set_entry_status(host_view, weak, &name, EntryStatus::Failed { error: err.to_string() });

            return;
        }
    };

    let remote_version = remote.version.clone().unwrap_or_default();

    if !plugins::manifest::is_newer_version(&remote_version, &local_version) {
        tally.up_to_date += 1;
        set_entry_status(host_view, weak, &name, EntryStatus::UpToDate);

        return;
    }
    set_entry_status(
        host_view,
        weak,
        &name,
        EntryStatus::Updating {
            new_version: remote_version.clone(),
        },
    );

    let needs_prompt = crate::confirm::update_needs_prompt(&remote.capabilities, &installed_caps);
    let caps_arg: Option<&[String]> = needs_prompt.then_some(&installed_caps);

    let result = install_pipeline(
        &manifest_url,
        &format!("update-{name}"),
        caps_arg,
        registry,
        silent_progress,
        confirm_state,
        weak,
        settings,
    )
    .await;

    if result.is_some() {
        tally.updated += 1;
        set_entry_status(
            host_view,
            weak,
            &name,
            EntryStatus::Updated {
                new_version: remote_version,
            },
        );
    } else {
        tally.failed += 1;
        set_entry_status(
            host_view,
            weak,
            &name,
            EntryStatus::Failed {
                error: "install pipeline failed (see logs)".to_owned(),
            },
        );
    }
}

/// Mutate the live update-view state under the slot lock and trigger a
/// repaint. No-op when the slot is empty (the user closed the view
/// mid-run). Runs on the runtime thread; the repaint hops to the Slint
/// thread.
fn update_view_state<F>(host_view: &HostView, weak: &slint::Weak<QueryWindow>, mutate: F)
where
    F: FnOnce(&mut host_view::UpdateViewState),
{
    {
        let Ok(mut guard) = host_view.lock() else {
            tracing::error!("update: host_view lock poisoned during update");
            return;
        };
        let Some(state) = guard.as_mut() else {
            return;
        };
        mutate(state);
    }
    host_view::schedule_paint(host_view, weak);
}

/// Find the entry by name and replace its status. Logs a debug line when
/// the entry isn't found (would mean a race between seeding and the
/// per-plugin loop, but bail safely rather than corrupting state).
fn set_entry_status(host_view: &HostView, weak: &slint::Weak<QueryWindow>, name: &str, status: EntryStatus) {
    update_view_state(host_view, weak, |s| {
        let Some(idx) = s.position(name) else {
            tracing::debug!(%name, "update: entry not found for status update");
            return;
        };
        s.entries[idx].status = status;
    });
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
