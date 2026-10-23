//! `clwd deploy` — build, create the app if it is missing, publish, print the URL.
//!
//! Three commands in one, each keeping its own error code. There is no local
//! state between the steps: creation is idempotent by host label and publishing
//! by digest, so a deploy that failed halfway is finished by running it again.
//!
//! # The host label is never guessed
//!
//! It comes from `[app] host_label` in `clwd.toml` and nowhere else — no flag,
//! no fallback to the directory name. A label is permanent once an app is
//! created under it, so it has to be something the author wrote down, and one
//! source means a re-run cannot reach a different app than the first run did.
//! Its syntax is checked before the build, so a typo costs nothing; whether the
//! org may use it is the platform's call.
//!
//! # One deploy of a project at a time
//!
//! A deploy holds an exclusive lock on `target/.clwd-deploy.lock` in the
//! project from before its build to after its publish. A second deploy of the
//! same project, from this process or another, is refused with
//! `cli.deploy_in_progress` rather than queued behind it: interleaved, either
//! one's bundle could be published last. The operating system releases the
//! lock when its holder exits, however it exits.
//!
//! # No waiting for the new version to serve
//!
//! The command returns once the publish is acknowledged. The version is ready
//! by then, but the host may keep serving the previous one for a short while,
//! and nothing reports when that ends — so the output says so instead of
//! polling for it.

use std::path::Path;

use serde::Serialize;

use crate::Progress;
use crate::build::{self, Cancel, Options, Source, Tail, manifest};
use crate::error::CliError;
use crate::platform::{self, Certificate, CreatedApp, PlatformError};

/// What a deploy did.
#[must_use]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Deployed {
    pub app_id: String,
    pub host_label: String,
    pub url: String,
    pub version: Version,
    pub digest: String,
    /// Whether this deploy created the app, or found it already there.
    pub created: bool,
    /// The org's certificate, only when it is not yet issued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<Certificate>,
}

/// The version a deploy published.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Version {
    pub id: String,
    /// Whether this deploy created the version, or the same bundle had been
    /// published before.
    pub created: bool,
}

impl std::fmt::Display for Deployed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.created {
            writeln!(f, "created app {} ({})", self.host_label, self.app_id)?;
        } else {
            writeln!(
                f,
                "app {} already exists ({})",
                self.host_label, self.app_id
            )?;
        }
        if self.version.created {
            writeln!(f, "published version {}", self.version.id)?;
        } else {
            // "Existed", never "live": publishing a bundle again does not, by
            // itself, make it the version the host serves.
            writeln!(f, "version {} already existed", self.version.id)?;
        }
        write!(f, "url: {}", self.url)?;
        if self.version.created {
            write!(f, "\nit may take up to ~30s for the new version to serve")?;
        }
        if let Some(c) = &self.certificate {
            write!(f, "\ncertificate {}: {}", c.status, c.message)?;
        }
        Ok(())
    }
}

/// What a caller with no terminal adds to a deploy.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Control {
    /// When set, the deploy is refused before anything is built unless it
    /// equals the label in `clwd.toml`: the caller names the label it is about
    /// to claim permanently.
    pub confirm_host_label: Option<String>,
    /// Keep the build's cargo stderr here instead of inheriting it.
    pub output: Option<Tail>,
    /// Lets another thread stop the build.
    pub cancel: Option<Cancel>,
}

/// Deploy the project in `project`.
///
/// `org` overrides `$CLWD_ORG` and `platform_url` overrides
/// `$CLWD_PLATFORM_URL`; the token comes from the environment or the `0600`
/// credentials file.
pub fn execute(
    project: &Path,
    org: Option<&str>,
    platform_url: Option<&str>,
    control: Control,
    progress: Option<&dyn Progress>,
) -> Result<Deployed, CliError> {
    // Everything that can be wrong without a build is checked before one.
    let host_label = host_label(project)?;
    if let Some(confirmed) = &control.confirm_host_label
        && *confirmed != host_label
    {
        return Err(CliError::InvalidArgument {
            what: format!(
                "confirm_host_label is {confirmed:?} but clwd.toml declares host_label \
                 {host_label:?}; nothing was built or deployed"
            ),
        });
    }
    let org = platform::config::org(org)?;
    let url = platform::config::platform_url(platform_url)?;
    let token = platform::config::token()?;
    let _lock = lock(project)?;

    let mut options = Options::new(project.to_path_buf());
    options.output = control.output;
    options.cancel = control.cancel;
    let built = build::run(&options, progress)?;
    let bytes = super::publish::read_bundle(&built.bundle)?;

    // A one-shot runtime, as in `publish`: the async boundary belongs to the
    // command, so a caller that is not `main` can use it too.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Internal { source: e.into() })?;

    if let Some(p) = progress {
        p.step("creating the app");
    }
    let app = runtime.block_on(platform::create_app(&url, &org, &token, &host_label))?;
    let app_id = platform::AppId::parse(&app.id)?;

    if let Some(p) = progress {
        p.step("uploading the bundle");
    }
    let published = runtime
        .block_on(platform::publish(&url, &app_id, &token, bytes))
        .map_err(|e| after_create(e, &app))?;

    Ok(Deployed {
        app_id: app.id,
        host_label: app.host_label,
        url: app.url,
        version: Version {
            id: published.version_id,
            created: published.created,
        },
        digest: published.digest,
        created: app.created,
        certificate: published.certificate,
    })
}

/// Hold the project's deploy lock until the returned file is dropped.
fn lock(project: &Path) -> Result<std::fs::File, CliError> {
    let dir = project.join("target");
    let path = dir.join(".clwd-deploy.lock");
    let io = |e: std::io::Error| CliError::Internal {
        source: anyhow::Error::new(e).context(format!("locking {}", path.display())),
    };
    std::fs::create_dir_all(&dir).map_err(io)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(CliError::DeployInProgress {
            project: project.display().to_string(),
        }),
        Err(std::fs::TryLockError::Error(e)) => Err(io(e)),
    }
}

/// The host label `clwd.toml` declares, checked for syntax.
pub fn host_label(project: &Path) -> Result<String, CliError> {
    let path = project.join(manifest::FILENAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(CliError::ManifestInvalid {
                what: format!("{} could not be read: {e}", path.display()),
            });
        }
    };
    let remedy = "add host_label = \"<label>\" under [app] in clwd.toml; it becomes \
                  https://<label>.<org>.<apps domain>, and can never be changed or reused";
    match Source::parse(&text)?.host_label() {
        Some(label) if manifest::is_host_label(label) => Ok(label.to_owned()),
        Some(label) => Err(CliError::ManifestInvalid {
            what: format!(
                "host_label {label:?} in {} is not a DNS label: use 1-63 lowercase letters, \
                 digits and hyphens, not starting or ending with a hyphen",
                path.display()
            ),
        }),
        None => Err(CliError::ManifestInvalid {
            what: format!(
                "{} has no host_label, which deploy needs: {remedy}",
                path.display()
            ),
        }),
    }
}

/// A publish failure, said to come after the app exists.
///
/// The code, hint and retryability are untouched; the message gains the one
/// fact the caller needs to recover — the app is in place, so running the
/// command again continues rather than starting over.
fn after_create(err: CliError, app: &CreatedApp) -> CliError {
    let prefix = format!(
        "app {} ({}) is in place but publishing failed; run clwd deploy again to continue",
        app.host_label, app.id
    );
    match err {
        CliError::Platform { source } => CliError::Platform {
            source: PlatformError::new(
                source.code().to_owned(),
                format!("{prefix}: {source}"),
                source.hint().map(str::to_owned),
                source.retryable(),
            ),
        },
        CliError::Net { detail, retryable } => CliError::Net {
            detail: format!("{prefix}: {detail}"),
            retryable,
        },
        other => other,
    }
}
