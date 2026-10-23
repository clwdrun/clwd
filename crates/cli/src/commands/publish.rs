//! `clwd publish` — upload a built bundle to a platform as a new version.
//!
//! The command consumes an already-built `.clwd` rather than running `build`
//! itself: publishing is a network concern and building is a toolchain one, and
//! keeping them apart means `publish` never drags a compiler onto the path of an
//! HTTP request. `clwd build` is one command away when a bundle is stale.
//!
//! Idempotency is the platform's, not the CLI's. The platform API keys a
//! version on `(app_id, digest)`, so re-publishing identical bytes returns the existing
//! version as a `200` rather than creating a second — surfaced here as
//! `already existed`, not an error. A publish that times out after the platform
//! committed but before the response arrived is safe to re-run for the same
//! reason; the CLI keeps no retry bookkeeping of its own.

use std::path::Path;

use crate::Progress;
use crate::error::CliError;
use crate::platform::{self, AppId, Published};

/// Publish `bundle` as a new version of `app`.
///
/// `platform_url` overrides `$CLWD_PLATFORM_URL`; the token comes from the
/// environment or the `0600` credentials file. The returned [`Published`] says
/// whether the version was created or already existed.
pub fn execute(
    bundle: &Path,
    app: &str,
    platform_url: Option<&str>,
    progress: Option<&dyn Progress>,
) -> Result<Published, CliError> {
    let app = AppId::parse(app)?;
    let url = platform::config::platform_url(platform_url)?;
    let token = platform::config::token()?;
    let bytes = read_bundle(bundle)?;

    if let Some(p) = progress {
        p.step("uploading the bundle");
    }

    // A one-shot runtime, built here rather than in the binary: `publish` is a
    // library function `clwd mcp serve` (#32) will call too, so the async
    // boundary belongs to the command, not to `main`.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Internal { source: e.into() })?;

    let published = runtime.block_on(platform::publish(&url, &app, &token, bytes))?;

    if let Some(p) = progress {
        p.finish();
    }
    Ok(published)
}

/// Read the bundle bytes, mapping a missing file to `cli.not_found`.
///
/// The bundle is not validated here: the platform decodes it and returns a
/// `bundle.*` code if it is malformed, which the envelope surfaces verbatim.
/// A second copy of that check would be a second place for it to drift.
pub(crate) fn read_bundle(path: &Path) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CliError::NotFound {
            what: format!("no bundle at {}", path.display()),
        },
        _ => CliError::Internal {
            source: anyhow::Error::new(e).context(format!("reading {}", path.display())),
        },
    })
}
