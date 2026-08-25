//! `clwd.toml`: schema, strict parse, canonical serialization.
//! See docs/spec/bundle-v1.md §5.

use std::fmt::Write as _;

use crate::digest::Digest;
use crate::error::Error;

pub const FILENAME: &str = "clwd.toml";
pub const SCHEMA_VERSION: i64 = 1;
pub const CAPABILITIES_VERSION: i64 = 1;

const MAX_MEMBER_SIZE: u64 = 8 * 1024 * 1024 * 1024;
const MAX_CPU_MS: u64 = 3_600_000;
const MAX_MEMORY_MB: u64 = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Wasm,
    MicroVm,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wasm => "wasm",
            Self::MicroVm => "microvm",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub path: String,
    pub digest: Digest,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub cpu_ms: u64,
    pub memory_mb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub version: i64,
    pub egress: Vec<String>,
    pub secrets: Vec<String>,
    pub storage_namespaces: Vec<String>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: i64,
    pub runtime: Runtime,
    pub entrypoint: String,
    pub members: Vec<Member>,
    pub capabilities: Capabilities,
}

// ---------------------------------------------------------------- parsing

fn missing(key: &str) -> Error {
    Error::FieldMissing { key: key.to_owned() }
}

fn wrong_type(key: &str, expected: &'static str) -> Error {
    Error::FieldTypeInvalid {
        key: key.to_owned(),
        expected,
    }
}

fn bad_value(key: &str, reason: impl Into<String>) -> Error {
    Error::FieldValueInvalid {
        key: key.to_owned(),
        reason: reason.into(),
    }
}

/// §5.5: no control character may appear in any manifest string.
fn check_string(key: &str, value: &str) -> Result<(), Error> {
    if value.chars().any(char::is_control) {
        return Err(bad_value(key, "contains a control character"));
    }
    Ok(())
}

fn get_str(table: &toml::Table, key: &str, path: &str) -> Result<String, Error> {
    let value = table.get(key).ok_or_else(|| missing(path))?;
    let s = value.as_str().ok_or_else(|| wrong_type(path, "a string"))?;
    check_string(path, s)?;
    Ok(s.to_owned())
}

fn get_int(table: &toml::Table, key: &str, path: &str) -> Result<i64, Error> {
    let value = table.get(key).ok_or_else(|| missing(path))?;
    value.as_integer().ok_or_else(|| wrong_type(path, "an integer"))
}

fn get_table<'a>(table: &'a toml::Table, key: &str, path: &str) -> Result<&'a toml::Table, Error> {
    let value = table.get(key).ok_or_else(|| missing(path))?;
    value.as_table().ok_or_else(|| wrong_type(path, "a table"))
}

/// A sorted, duplicate-free array of non-empty strings. §5.3.
fn get_grants(table: &toml::Table, key: &str) -> Result<Vec<String>, Error> {
    let path = format!("capabilities.{key}");
    let value = table.get(key).ok_or_else(|| missing(&path))?;
    let array = value
        .as_array()
        .ok_or_else(|| wrong_type(&path, "an array of strings"))?;

    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item
            .as_str()
            .ok_or_else(|| wrong_type(&path, "an array of strings"))?;
        if s.is_empty() {
            return Err(bad_value(&path, "contains an empty string"));
        }
        check_string(&path, s)?;
        out.push(s.to_owned());
    }
    for pair in out.windows(2) {
        if pair[0] == pair[1] {
            return Err(bad_value(&path, format!("duplicate entry {:?}", pair[0])));
        }
        if pair[0].as_bytes() > pair[1].as_bytes() {
            return Err(bad_value(&path, "not sorted byte-wise ascending"));
        }
    }
    Ok(out)
}

fn reject_unknown(
    table: &toml::Table,
    known: &[&str],
    prefix: &str,
    capability: bool,
) -> Result<(), Error> {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            let full = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            return Err(if capability {
                Error::CapabilityUnknownKey { key: full }
            } else {
                Error::UnknownKey { key: full }
            });
        }
    }
    Ok(())
}

impl Manifest {
    /// Parse and validate, in the order §8 steps 11 to 15 require.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let root: toml::Table = text
            .parse::<toml::Table>()
            .map_err(|e| Error::ManifestNotToml { reason: e.to_string() })?;

        // Step 11. Nothing else can be read until the schema is known.
        let schema_version = match root.get("schema_version") {
            None => return Err(Error::SchemaVersionMissing {}),
            Some(v) => v
                .as_integer()
                .ok_or_else(|| wrong_type("schema_version", "an integer"))?,
        };
        if schema_version != SCHEMA_VERSION {
            return Err(Error::SchemaVersionUnsupported {
                found: schema_version,
            });
        }

        // Step 12: unknown keys at the top level and in every table outside
        // [capabilities]. §7.1.
        reject_unknown(
            &root,
            &["schema_version", "runtime", "app", "members", "capabilities"],
            "",
            false,
        )?;
        let app = get_table(&root, "app", "app")?;
        reject_unknown(app, &["entrypoint"], "app", false)?;

        let members_value = root.get("members").ok_or_else(|| missing("members"))?;
        let members_array = members_value
            .as_array()
            .ok_or_else(|| wrong_type("members", "an array of tables"))?;
        for (i, entry) in members_array.iter().enumerate() {
            let table = entry
                .as_table()
                .ok_or_else(|| wrong_type(&format!("members[{i}]"), "a table"))?;
            reject_unknown(
                table,
                &["path", "digest", "size"],
                &format!("members[{i}]"),
                false,
            )?;
        }

        let capabilities = get_table(&root, "capabilities", "capabilities")?;

        // Step 13: the capability vocabulary version, before any key in the
        // subtree is read. §7.2.
        let cap_version = get_int(capabilities, "version", "capabilities.version")?;
        if cap_version != CAPABILITIES_VERSION {
            return Err(Error::CapabilitiesVersionUnsupported { found: cap_version });
        }

        // Step 14: unknown keys anywhere under [capabilities]. A key here is a
        // permission or a limit this runtime cannot enforce, which is why it
        // carries its own code.
        reject_unknown(
            capabilities,
            &[
                "version",
                "egress",
                "secrets",
                "storage_namespaces",
                "limits",
            ],
            "capabilities",
            true,
        )?;
        let limits = get_table(capabilities, "limits", "capabilities.limits")?;
        reject_unknown(
            limits,
            &["cpu_ms", "memory_mb"],
            "capabilities.limits",
            true,
        )?;

        // Step 15: required keys, types, values.
        let runtime = match get_str(&root, "runtime", "runtime")?.as_str() {
            "wasm" => Runtime::Wasm,
            "microvm" => Runtime::MicroVm,
            other => {
                return Err(Error::RuntimeUnknown {
                    found: other.to_owned(),
                })
            }
        };
        let entrypoint = get_str(app, "entrypoint", "app.entrypoint")?;

        if members_array.is_empty() {
            return Err(bad_value("members", "must have at least one element"));
        }
        let mut members = Vec::with_capacity(members_array.len());
        for (i, entry) in members_array.iter().enumerate() {
            let table = entry.as_table().expect("checked a table above");
            let key = |k: &str| format!("members[{i}].{k}");
            let path = get_str(table, "path", &key("path"))?;
            crate::path::validate(&path)?;

            let digest_text = get_str(table, "digest", &key("digest"))?;
            let digest = digest_text.parse::<Digest>()?;

            let size = get_int(table, "size", &key("size"))?;
            if size < 0 {
                return Err(bad_value(&key("size"), "negative"));
            }
            if size as u64 >= MAX_MEMBER_SIZE {
                return Err(Error::MemberTooLarge { size: size as u64 });
            }
            members.push(Member {
                path,
                digest,
                size: size as u64,
            });
        }
        for pair in members.windows(2) {
            if pair[0].path == pair[1].path {
                return Err(Error::PathDuplicate {
                    path: pair[0].path.clone(),
                });
            }
            if pair[0].path.as_bytes() > pair[1].path.as_bytes() {
                return Err(bad_value("members", "not sorted by path"));
            }
        }

        let cpu_ms = get_int(limits, "cpu_ms", "capabilities.limits.cpu_ms")?;
        if cpu_ms < 1 || cpu_ms as u64 > MAX_CPU_MS {
            return Err(bad_value("capabilities.limits.cpu_ms", "outside 1..=3600000"));
        }
        let memory_mb = get_int(limits, "memory_mb", "capabilities.limits.memory_mb")?;
        if memory_mb < 1 || memory_mb as u64 > MAX_MEMORY_MB {
            return Err(bad_value(
                "capabilities.limits.memory_mb",
                "outside 1..=65536",
            ));
        }

        Ok(Self {
            schema_version,
            runtime,
            entrypoint,
            members,
            capabilities: Capabilities {
                version: cap_version,
                egress: get_grants(capabilities, "egress")?,
                secrets: get_grants(capabilities, "secrets")?,
                storage_namespaces: get_grants(capabilities, "storage_namespaces")?,
                limits: Limits {
                    cpu_ms: cpu_ms as u64,
                    memory_mb: memory_mb as u64,
                },
            },
        })
    }

    /// Emit canonical TOML. §5.4.
    ///
    /// Hand-written rather than delegated to a serializer: this is a byte
    /// format, not a convenience, and no serializer promises stability of its
    /// output across versions.
    pub fn to_canonical_string(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "schema_version = {}", self.schema_version);
        let _ = writeln!(out, "runtime = {}", quote(self.runtime.as_str()));
        let _ = writeln!(out, "\n[app]");
        let _ = writeln!(out, "entrypoint = {}", quote(&self.entrypoint));
        for member in &self.members {
            let _ = writeln!(out, "\n[[members]]");
            let _ = writeln!(out, "path = {}", quote(&member.path));
            let _ = writeln!(out, "digest = {}", quote(&member.digest.to_string()));
            let _ = writeln!(out, "size = {}", member.size);
        }
        let caps = &self.capabilities;
        let _ = writeln!(out, "\n[capabilities]");
        let _ = writeln!(out, "version = {}", caps.version);
        let _ = writeln!(out, "egress = {}", array(&caps.egress));
        let _ = writeln!(out, "secrets = {}", array(&caps.secrets));
        let _ = writeln!(
            out,
            "storage_namespaces = {}",
            array(&caps.storage_namespaces)
        );
        let _ = writeln!(out, "\n[capabilities.limits]");
        let _ = writeln!(out, "cpu_ms = {}", caps.limits.cpu_ms);
        let _ = writeln!(out, "memory_mb = {}", caps.limits.memory_mb);
        out
    }
}

/// §5.4: a basic string escaping only the quote and the backslash. Control
/// characters cannot appear — §5.5 rejects them — so no other escape is needed.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn array(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".to_owned();
    }
    let items: Vec<String> = values.iter().map(|v| quote(v)).collect();
    format!("[{}]", items.join(", "))
}
