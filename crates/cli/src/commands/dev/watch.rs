//! The file watcher: debounced rebuilds that swap the running component.
//!
//! Watches the project **source** only — never `target/` or the bundle output,
//! because a build writes there and a watcher that saw its own output would
//! rebuild forever. A burst of events (an editor save, a formatter) collapses
//! into one rebuild via a short quiet-window debounce.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::served::{Reloadable, build_active, store};
use crate::error::CliError;

/// How long to wait for changes to stop before rebuilding.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Start watching `project`, rebuilding and swapping on change.
///
/// The returned [`RecommendedWatcher`] must be kept alive for the watch to
/// continue; dropping it stops watching.
pub(super) fn spawn(
    project: PathBuf,
    reloadable: Reloadable,
) -> Result<RecommendedWatcher, CliError> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if res.is_ok_and(|event| is_source_change(&event)) {
            // The receiver only outlives the server; a send failing means it is
            // shutting down, which is not this thread's problem.
            let _ = tx.send(());
        }
    })
    .map_err(watch_error)?;

    // Source only. A cargo build touches `target/` thousands of times and writes
    // the `.clwd` there too; watching it would feed the loop its own output.
    watcher
        .watch(&project.join("src"), RecursiveMode::Recursive)
        .map_err(watch_error)?;
    watch_optional(&mut watcher, &project.join("Cargo.toml"));
    watch_optional(&mut watcher, &project.join("clwd.toml"));

    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            // Debounce: keep draining until the changes stop for one window, so
            // a save that fires five events is one rebuild, not five.
            loop {
                tokio::select! {
                    _ = rx.recv() => continue,
                    _ = tokio::time::sleep(DEBOUNCE) => break,
                }
            }
            let started = std::time::Instant::now();
            match build_active(&project).await {
                Ok(active) => {
                    if store(&reloadable, active) {
                        tracing::info!("reloaded in {} ms", started.elapsed().as_millis());
                    } else {
                        tracing::info!("no change");
                    }
                }
                // The one behaviour that matters: a failed rebuild keeps the last
                // good component serving. Nothing is swapped; the error is shown.
                Err(e) => {
                    tracing::warn!("rebuild failed — keeping the last good version:\n{e}");
                }
            }
        }
    });

    Ok(watcher)
}

/// Whether an event is a change worth rebuilding for. Access events and pure
/// metadata touches are ignored; creates, content modifies and removes count.
fn is_source_change(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Watch a path that may not exist — a project without a `clwd.toml` is valid —
/// ignoring its absence.
fn watch_optional(watcher: &mut RecommendedWatcher, path: &Path) {
    if path.exists() {
        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }
}

fn watch_error(e: notify::Error) -> CliError {
    CliError::Internal {
        source: anyhow::Error::new(e).context("watching the project for changes"),
    }
}
