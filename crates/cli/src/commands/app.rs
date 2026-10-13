//! `clwd app` — apps on a platform.
//!
//! `create` is idempotent by host label: the platform returns the existing
//! app when the org already has a live one with that label, so running the
//! command again is safe and prints the same id. A label that belonged to a
//! deleted app is refused, because a label is never reused.

use crate::Progress;
use crate::error::CliError;
use crate::platform::{self, CreatedApp};

/// Create the app `host_label` names in the org, or get the existing one.
///
/// `org` overrides `$CLWD_ORG` and `platform_url` overrides
/// `$CLWD_PLATFORM_URL`; the token comes from the environment or the `0600`
/// credentials file.
pub fn create(
    host_label: &str,
    org: Option<&str>,
    platform_url: Option<&str>,
    progress: Option<&dyn Progress>,
) -> Result<CreatedApp, CliError> {
    let org = platform::config::org(org)?;
    let url = platform::config::platform_url(platform_url)?;
    let token = platform::config::token()?;
    if let Some(p) = progress {
        p.step("creating the app");
    }
    // A one-shot runtime, as in `publish`: the async boundary belongs to the
    // command, so a caller that is not `main` can use it too.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Internal { source: e.into() })?;
    let app = runtime.block_on(platform::create_app(&url, &org, &token, host_label))?;
    if let Some(p) = progress {
        p.finish();
    }
    Ok(app)
}
