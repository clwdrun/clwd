//! `clwd init` — scaffold a new app from a template.
//!
//! # Copied, not generated
//!
//! A template is a real project that CI builds. `init` copies one and rewrites
//! a few lines. That keeps one source of truth: what CI proves is byte-for-byte
//! what an author receives, minus the rewrites in [`substitutions`].
//!
//! A generator would be a second description of the same thing, and the two
//! would drift the first time someone edited a template without editing it.
//!
//! The rewrites are in [`substitutions`], and there are deliberately only three.
//! Every one is a way a scaffolded project can differ from the one CI built, so
//! each has to earn its place.
//!
//! # Embedded, not read from disk
//!
//! The files are `include_str!`d into the binary. Reading from disk would mean
//! `clwd init` only works inside a checkout — which is nowhere an author is —
//! and would let the copy CI builds and the copy a user gets diverge.
//!
//! The cost is that adding a file to a template means adding a line to
//! [`TEMPLATES`]. That is a compile error when forgotten for a new template,
//! and `init_output_matches_the_template_it_copied` catches it for a new file
//! in an existing one.

use std::path::{Path, PathBuf};

use crate::Progress;
use crate::error::CliError;

/// One file in a template.
struct TemplateFile {
    /// Where it goes, relative to the new project's root.
    path: &'static str,
    /// Its contents, as committed.
    contents: &'static str,
}

/// A template `clwd init` can scaffold from.
struct Template {
    name: &'static str,
    /// One line, shown by `--list`. What this template teaches.
    teaches: &'static str,
    /// Whether an app scaffolded from this works when deployed today. `false`
    /// is a promise the README explains: it runs under `clwd dev`, and names
    /// what deploying it waits on.
    works_deployed: bool,
    files: &'static [TemplateFile],
}

/// Every template, with every file it contains.
///
/// The paths here are relative to `templates/<name>/` in the repository, and
/// the contents are that file. A file present in the repository but missing
/// from this list would build in CI and be absent from a scaffolded project —
/// which is what `init_output_matches_the_template_it_copied` exists to catch.
const TEMPLATES: &[Template] = &[
    Template {
        name: "hello",
        teaches: "who is calling, with no auth code, and escaping by construction",
        works_deployed: true,
        files: &[
            TemplateFile {
                path: "Cargo.toml",
                contents: include_str!("../../../../templates/hello/Cargo.toml"),
            },
            TemplateFile {
                path: "clwd.toml",
                contents: include_str!("../../../../templates/hello/clwd.toml"),
            },
            TemplateFile {
                path: "src/lib.rs",
                contents: include_str!("../../../../templates/hello/src/lib.rs"),
            },
            TemplateFile {
                path: "README.md",
                contents: include_str!("../../../../templates/hello/README.md"),
            },
        ],
    },
    Template {
        name: "notes",
        teaches: "per-caller storage, forms that POST, and escaping stored values",
        works_deployed: false,
        files: &[
            TemplateFile {
                path: "Cargo.toml",
                contents: include_str!("../../../../templates/notes/Cargo.toml"),
            },
            TemplateFile {
                path: "clwd.toml",
                contents: include_str!("../../../../templates/notes/clwd.toml"),
            },
            TemplateFile {
                path: "src/lib.rs",
                contents: include_str!("../../../../templates/notes/src/lib.rs"),
            },
            TemplateFile {
                path: "README.md",
                contents: include_str!("../../../../templates/notes/README.md"),
            },
        ],
    },
];

/// The default when `--template` is not given.
const DEFAULT_TEMPLATE: &str = "hello";

/// The SDK path a template carries in the repository.
///
/// Rewritten to an absolute path on the way out, because a relative path only
/// resolves from inside `templates/`. This goes away when `clwd-sdk` is
/// published — see clwdrun/clwd#92.
const TEMPLATE_SDK_PATH: &str = "path = \"../../crates/sdk\"";

/// What a scaffolded project produced.
#[must_use]
#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct Scaffolded {
    /// Where it was written.
    pub project: PathBuf,
    /// Which template it came from.
    pub template: String,
    /// The app's name, as it appears in both manifests.
    pub name: String,
    /// Every file written, relative to the project root.
    pub files: Vec<String>,
    /// Whether the template works when deployed today. See [`Listed`].
    pub works_deployed: bool,
    /// The commands to run next, in order: into the project, run it locally,
    /// deploy it.
    pub next: Vec<String>,
}

/// One template, as `--list` shows it.
#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct Listed {
    pub name: &'static str,
    /// One line: what the template teaches.
    pub teaches: &'static str,
    /// Whether an app made from it works when deployed today. When `false`,
    /// its README names what deploying it waits on.
    pub works_deployed: bool,
}

/// Every template, for `--list`. Serializes as a bare array.
#[must_use]
#[derive(Debug, serde::Serialize)]
#[serde(transparent)]
pub struct Listing(pub Vec<Listed>);

impl std::fmt::Display for Listing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, t) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{:<8} {}", t.name, t.teaches)?;
            if !t.works_deployed {
                write!(f, " (local only for now; see its README)")?;
            }
        }
        Ok(())
    }
}

/// Names that work as a crate name **and** as an app name.
///
/// The two grammars differ: an app name may contain `.` (see
/// `clwd_cli::secrets::value`), and a crate name may not — cargo refuses
/// `my.app` with "invalid character `.` in package name". A scaffolded project
/// needs both, so this is the intersection rather than either one.
///
/// Narrower than both on purpose. It can widen later without breaking a name
/// anyone already used; it could not narrow.
fn validate_name(raw: &str) -> Result<(), CliError> {
    let rule = "a name is 1 to 64 characters of a-z, 0-9, '-' or '_', starting with a letter";

    if raw.is_empty() {
        return Err(CliError::InvalidArgument {
            what: format!("the name is empty; {rule}"),
        });
    }
    if raw.len() > 64 {
        return Err(CliError::InvalidArgument {
            what: format!("the name is {} characters; {rule}", raw.len()),
        });
    }
    if !raw.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err(CliError::InvalidArgument {
            what: format!("the name must start with a letter; {rule}"),
        });
    }
    if let Some(bad) = raw
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')))
    {
        return Err(CliError::InvalidArgument {
            what: format!(
                "the name contains {bad:?}, which cargo rejects in a package name; {rule}"
            ),
        });
    }
    Ok(())
}

/// Look up a template by name.
fn find(name: &str) -> Result<&'static Template, CliError> {
    TEMPLATES
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| CliError::NotFound {
            what: format!(
                "no template named {name}. Available: {}",
                TEMPLATES
                    .iter()
                    .map(|t| t.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
}

/// The substitutions applied to every file of `template` for a project called
/// `name`.
///
/// Public to the crate so the round-trip test can undo exactly these and
/// compare the result against the committed template.
pub(crate) fn substitutions(template: &str, name: &str, sdk: &Path) -> Vec<(String, String)> {
    vec![
        (
            format!("name = \"{template}\""),
            format!("name = \"{name}\""),
        ),
        // The name becomes the host label only when it is a valid one. When it
        // is not, the line is left as a commented example rather than a label
        // nobody chose: a host label is permanent once deployed.
        (
            format!("host_label = \"{template}\""),
            if crate::build::manifest::is_host_label(name) {
                format!("host_label = \"{name}\"")
            } else {
                format!(
                    "# host_label = \"your-app\"  ({name:?} is not a valid label: \
                     use a-z, 0-9 and '-')"
                )
            },
        ),
        (
            TEMPLATE_SDK_PATH.to_owned(),
            format!("path = {:?}", sdk.display().to_string()),
        ),
    ]
}

/// Where the SDK lives, for the path a scaffolded project needs.
///
/// Resolved from where this crate was compiled. That works for a binary built
/// from a checkout, which is every binary today: the repository is private and
/// `clwd-sdk` is unpublished, so a scaffolded project needs a path to a
/// checkout however it gets one.
fn sdk_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sdk")
        .components()
        .collect()
}

/// Scaffold `template` into `dir` as an app called `name`.
pub fn execute(
    dir: &Path,
    name: &str,
    template: Option<&str>,
    progress: Option<&dyn Progress>,
) -> Result<Scaffolded, CliError> {
    validate_name(name)?;
    let template = find(template.unwrap_or(DEFAULT_TEMPLATE))?;

    let project = dir.join(name);
    // An empty directory is fine — making one first is an ordinary thing to
    // do. Anything in it is refused rather than merged into or overwritten.
    let occupied = match std::fs::read_dir(&project) {
        Ok(mut entries) => entries.next().is_some(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        // Exists but cannot be listed: most often a file of that name.
        Err(_) => true,
    };
    if occupied {
        return Err(CliError::InvalidArgument {
            what: format!(
                "{} already exists and is not an empty directory. `clwd init` will not \
                 write into it",
                project.display()
            ),
        });
    }

    let rewrites = substitutions(template.name, name, &sdk_path());
    let mut written = Vec::new();

    for file in template.files {
        if let Some(p) = progress {
            p.step(file.path);
        }

        let mut contents = file.contents.to_owned();
        for (from, to) in &rewrites {
            contents = contents.replace(from, to);
        }

        let target = project.join(file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Config {
                what: format!("{} could not be created: {e}", parent.display()),
            })?;
        }
        std::fs::write(&target, contents).map_err(|e| CliError::Config {
            what: format!("{} could not be written: {e}", target.display()),
        })?;
        written.push(file.path.to_owned());
    }

    if let Some(p) = progress {
        p.finish();
    }

    let cd = project
        .strip_prefix(".")
        .unwrap_or(&project)
        .display()
        .to_string();
    Ok(Scaffolded {
        next: vec![
            format!("cd {cd}"),
            "clwd dev".to_owned(),
            "clwd deploy".to_owned(),
        ],
        project,
        template: template.name.to_owned(),
        name: name.to_owned(),
        files: written,
        works_deployed: template.works_deployed,
    })
}

/// Every template, what it teaches, and whether it works deployed, for
/// `--list`.
pub fn list() -> Listing {
    Listing(
        TEMPLATES
            .iter()
            .map(|t| Listed {
                name: t.name,
                teaches: t.teaches,
                works_deployed: t.works_deployed,
            })
            .collect(),
    )
}

impl std::fmt::Display for Scaffolded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.project.display())?;
        for file in &self.files {
            writeln!(f, "  {file}")?;
        }
        writeln!(f)?;
        writeln!(f, "next:")?;
        for command in &self.next {
            writeln!(f, "  {command}")?;
        }
        if !self.works_deployed {
            write!(
                f,
                "\n{} runs under `clwd dev`; deploying it waits on what its README names.",
                self.template
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cargo_would_reject_is_refused_with_the_rule() {
        // `.` is legal in an app name and illegal in a crate name. This is the
        // case the intersection exists for.
        let err = validate_name("my.app").expect_err("a dot must be refused");
        let msg = err.to_string();
        assert!(msg.contains('.'), "the offending character: {msg}");
        assert!(msg.contains("cargo"), "why it is refused: {msg}");
        assert!(msg.contains("a-z"), "the rule itself: {msg}");
    }

    #[test]
    fn names_outside_the_intersection_are_refused() {
        for bad in ["", "9lives", "-lead", "UPPER", "with space", "with/slash"] {
            assert!(validate_name(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn ordinary_names_are_accepted() {
        for good in ["app", "my-app", "my_app", "a1", "notes"] {
            assert!(validate_name(good).is_ok(), "{good:?} was refused");
        }
    }

    #[test]
    fn an_unknown_template_lists_the_ones_that_exist() {
        // `let else` rather than `expect_err`, which would require `Template`
        // to implement `Debug` only so a panic message could print an entire
        // template's contents.
        let Err(err) = find("nonexistent") else {
            panic!("an unknown template must be refused");
        };
        let msg = err.to_string();
        for t in TEMPLATES {
            assert!(msg.contains(t.name), "{} missing from: {msg}", t.name);
        }
    }

    /// Every template carries the release profile `clwd build` checks for. A
    /// template that warns on its first build is a template teaching the reader
    /// to ignore warnings.
    #[test]
    fn every_template_ships_the_release_profile_build_checks_for() {
        for t in TEMPLATES {
            let manifest = t
                .files
                .iter()
                .find(|f| f.path == "Cargo.toml")
                .unwrap_or_else(|| panic!("{} has no Cargo.toml", t.name));
            let warnings = crate::build::cargo::profile_warnings(manifest.contents);
            assert!(
                warnings.is_empty(),
                "{} would warn on build: {warnings:?}",
                t.name
            );
        }
    }

    /// Every template declares its app name, since secrets are scoped to it and
    /// `clwd secrets` refuses to run without one — and its host label, since
    /// `clwd deploy` does.
    #[test]
    fn every_template_declares_an_app_name() {
        for t in TEMPLATES {
            let manifest = t
                .files
                .iter()
                .find(|f| f.path == "clwd.toml")
                .unwrap_or_else(|| panic!("{} has no clwd.toml", t.name));
            let source = crate::build::Source::parse(manifest.contents)
                .unwrap_or_else(|e| panic!("{}'s clwd.toml does not parse: {e}", t.name));
            assert_eq!(
                source.app_name(),
                Some(t.name),
                "{}'s clwd.toml should name the app after the template",
                t.name
            );
            assert_eq!(source.host_label(), Some(t.name), "{}", t.name);
        }
    }

    /// Deny-by-default is written out rather than left to the build to fill in,
    /// so an author's first sight of the file shows the shape they will edit.
    #[test]
    fn every_template_spells_out_its_capabilities() {
        for t in TEMPLATES {
            let manifest = t
                .files
                .iter()
                .find(|f| f.path == "clwd.toml")
                .unwrap_or_else(|| panic!("{} has no clwd.toml", t.name));
            for key in [
                "egress",
                "secrets",
                "storage_namespaces",
                "cpu_ms",
                "memory_mb",
            ] {
                assert!(
                    manifest.contents.contains(key),
                    "{}'s clwd.toml does not mention {key}",
                    t.name
                );
            }
        }
    }

    /// No template teaches `.unwrap()`. An agent generating code from these
    /// reproduces their habits, and a panic in a guest traps the instance.
    #[test]
    fn no_template_teaches_unwrap() {
        for t in TEMPLATES {
            for file in t.files.iter().filter(|f| f.path.ends_with(".rs")) {
                for (n, line) in file.contents.lines().enumerate() {
                    let code = line.trim();
                    if code.starts_with("//") {
                        continue;
                    }
                    assert!(
                        !code.contains(".unwrap()") && !code.contains(".expect("),
                        "{}/{} line {} teaches a panic: {code}",
                        t.name,
                        file.path,
                        n + 1
                    );
                }
            }
        }
    }
}
