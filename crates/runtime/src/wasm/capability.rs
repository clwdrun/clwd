//! Deny-by-default enforcement of the manifest's egress and storage grants.
//!
//! The bundle format carries `capabilities.egress` and
//! `capabilities.storage_namespaces` as opaque string lists — spec §5.3 assigns
//! them no meaning and defers their grammar and enforcement to here. This module
//! is that grammar and that enforcement.
//!
//! # Egress: the sandbox enforces by name, the edge by address
//!
//! Egress is enforced in two places, and the division is deliberate (clwdrun/clwd#33,
//! commented onto the platform edge issue). **The sandbox — here — checks the
//! hostname the guest names, at the outbound-request seam, before the request
//! leaves the host process.** It sees the requested name, not the address it
//! resolves to. A name-only allowlist therefore genuinely *cannot* stop DNS
//! rebinding or an allowed name pointing at an internal IP, and it does not
//! pretend to: restricting resolution in step with the allowlist is the edge's
//! job. The rule is: the sandbox denies by the hostname the guest names; the
//! edge denies by the address that name resolves to.
//!
//! # What a denied guest learns: nothing specific
//!
//! A denied outbound request surfaces to the guest as a generic
//! `wasi:http` `HttpRequestDenied`, and a denied storage key as a generic
//! `invalid-key`. The precise reason — which host, which namespace — is logged
//! host-side under `runtime.capability_denied`, never returned to the guest. The
//! guest is untrusted and must not be able to probe which specific host or
//! namespace it was blocked from, the same reasoning that makes an ungranted
//! secret *absent* rather than *denied*. Do not "improve" this by handing the
//! denied host or namespace back to the guest.

use std::collections::HashSet;
use std::future::Future;

use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody, WasiHttpHooks};

use crate::error::Error;

/// The hostname allowlist, precomputed once at spawn.
///
/// Two forms, from the grammar this issue defines for the opaque strings:
/// an **exact** host (`api.example.com`) and a **suffix wildcard**
/// (`*.example.com`). A suffix matches any name strictly *under* it
/// (`a.example.com`, `a.b.example.com`) but **not** the apex (`example.com`)
/// unless the apex is also listed. Matching is ASCII-case-insensitive.
///
/// Deny-by-default falls out of the representation: an empty allowlist has no
/// exact entries and no suffixes, so [`EgressPolicy::allows`] returns `false`
/// for every host.
#[derive(Debug, Clone, Default)]
pub(super) struct EgressPolicy {
    exact: HashSet<String>,
    /// Each entry is the part after `*.`, e.g. `example.com` for `*.example.com`.
    suffixes: Vec<String>,
}

impl EgressPolicy {
    /// Build the allowlist from the manifest's `egress` list.
    ///
    /// Cheap and done once per spawn: the check on the request path is then a
    /// hash lookup plus a short suffix scan, with nothing parsed or resolved per
    /// request.
    pub(super) fn from_manifest(egress: &[String]) -> Self {
        let mut exact = HashSet::new();
        let mut suffixes = Vec::new();
        for grant in egress {
            match grant.strip_prefix("*.") {
                Some(suffix) => suffixes.push(suffix.to_ascii_lowercase()),
                None => {
                    exact.insert(grant.to_ascii_lowercase());
                }
            }
        }
        Self { exact, suffixes }
    }

    /// Whether an outbound request to `host` is allowed.
    pub(super) fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if self.exact.contains(&host) {
            return true;
        }
        // `*.example.com` matches `a.example.com` but not `example.com`: the
        // name must be strictly longer than the suffix and joined to it by a
        // dot, so `notexample.com` never matches `*.example.com`.
        self.suffixes.iter().any(|suffix| {
            host.len() > suffix.len() + 1
                && host.ends_with(suffix.as_str())
                && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
        })
    }
}

/// Whether the app may use persistent storage at all.
///
/// app-data is a **flat per-app key-value store** (`clwd:app-data` DECISION 1):
/// the host sees opaque keys with no namespace structure, and any per-caller or
/// logical scoping is an SDK-side key-prefix convention (`u.<subject>.…`) the
/// host neither sees structure in nor should couple to — gating on it would
/// break the moment the SDK changed its prefix. So the only capability the host
/// can honestly enforce is on/off.
///
/// Deny-by-default: an **empty** `storage_namespaces` grant denies all app-data;
/// a **non-empty** grant allows it. The grant's string *contents* are
/// deliberately not interpreted — a real per-namespace capability needs a
/// `namespace` concept the app-data WIT does not yet have, and is filed as its
/// own issue. This enforces the boolean the current data model actually
/// supports, with no manifest format change.
#[derive(Debug, Clone, Default)]
pub(super) struct StoragePolicy {
    granted: bool,
}

impl StoragePolicy {
    /// Build from the manifest's `storage_namespaces` list: non-empty grants
    /// app-data, empty denies it.
    pub(super) fn from_manifest(namespaces: &[String]) -> Self {
        Self {
            granted: !namespaces.is_empty(),
        }
    }

    /// Whether an app-data operation is allowed. The key is not inspected — the
    /// grant is on/off (see the type docs); it is taken so a future
    /// per-namespace model can start gating without touching the call sites.
    pub(super) fn allows(&self, _key: &str) -> bool {
        self.granted
    }
}

/// Empty hooks whose only job is to reach `wasmtime-wasi-http`'s default
/// outbound sender for a request the policy allowed.
#[derive(Default)]
struct Permissive;
impl WasiHttpHooks for Permissive {}

/// The `WasiHttpHooks` the sandbox installs: allow-listed outbound, deny closed.
pub(super) struct EgressHooks {
    policy: EgressPolicy,
}

impl EgressHooks {
    pub(super) fn new(policy: EgressPolicy) -> Self {
        Self { policy }
    }
}

impl WasiHttpHooks for EgressHooks {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = Result<(), HttpError>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (
                        http::Response<WasiBody>,
                        Box<dyn Future<Output = Result<(), HttpError>> + Send>,
                    ),
                    HttpError,
                >,
            > + Send,
    > {
        let host = request.uri().host().unwrap_or_default().to_owned();
        if !host.is_empty() && self.policy.allows(&host) {
            return Permissive.send_request(request, options, fut);
        }

        // Denied. Log the specifics host-side for the app owner; the guest gets
        // only a generic denial (see the module docs — the host name must not
        // leak back to an untrusted guest). A missing host is treated as a
        // denial too: fail closed rather than guess.
        let denied = Error::CapabilityDenied {
            detail: format!(
                "egress to {:?} is not in the manifest allowlist",
                request.uri().host().unwrap_or("<no host>")
            ),
        };
        tracing::warn!(code = denied.code(), detail = %denied, "outbound request denied");
        Box::new(async move { Err(HttpError::HttpRequestDenied) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(grants: &[&str]) -> EgressPolicy {
        EgressPolicy::from_manifest(&grants.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn an_empty_egress_list_denies_every_host() {
        let p = EgressPolicy::default();
        assert!(!p.allows("example.com"));
        assert!(!p.allows("anything.at.all"));
        // The from_manifest path with an empty slice is the same posture.
        assert!(!policy(&[]).allows("example.com"));
    }

    #[test]
    fn an_exact_host_matches_only_itself() {
        let p = policy(&["api.example.com"]);
        assert!(p.allows("api.example.com"));
        assert!(!p.allows("example.com"));
        assert!(!p.allows("other.example.com"));
        assert!(!p.allows("api.example.com.evil.com"));
    }

    /// The rule most likely to be gotten wrong later: `*.example.com` matches a
    /// subdomain but NOT the apex. Both directions asserted.
    #[test]
    fn a_suffix_wildcard_matches_subdomains_but_not_the_apex() {
        let p = policy(&["*.example.com"]);
        assert!(p.allows("a.example.com"), "subdomain must match");
        assert!(p.allows("a.b.example.com"), "deeper subdomain must match");
        assert!(
            !p.allows("example.com"),
            "the apex must NOT match *.example.com"
        );
        assert!(
            !p.allows("notexample.com"),
            "a non-dot boundary must not match"
        );
        assert!(
            !p.allows("example.com.evil.com"),
            "suffix must be anchored at the end"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(policy(&["API.Example.COM"]).allows("api.example.com"));
        assert!(policy(&["*.Example.com"]).allows("A.EXAMPLE.COM"));
    }

    #[test]
    fn the_apex_can_be_granted_alongside_the_wildcard() {
        let p = policy(&["*.example.com", "example.com"]);
        assert!(p.allows("example.com"));
        assert!(p.allows("a.example.com"));
    }

    fn storage(grants: &[&str]) -> StoragePolicy {
        StoragePolicy::from_manifest(&grants.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    /// Deny-by-default: an empty grant refuses every key, whatever its shape.
    #[test]
    fn an_empty_storage_grant_denies_all_app_data() {
        let p = StoragePolicy::default();
        assert!(!p.allows("counter"));
        assert!(!p.allows("u.local.note"), "even a per-caller key is denied");
        assert!(!storage(&[]).allows("counter"));
    }

    /// A non-empty grant is on/off: it allows any key. The contents are not
    /// interpreted — that is deferred to the future per-namespace model — so a
    /// key's shape (flat, or the SDK's `u.<subject>.<key>`) does not matter.
    #[test]
    fn a_non_empty_storage_grant_allows_app_data_regardless_of_key_shape() {
        let p = storage(&["notes"]);
        assert!(p.allows("counter"));
        assert!(
            p.allows("u.local.note"),
            "the SDK per-caller key shape is allowed"
        );
        // The string contents do not gate anything yet: a grant of some other
        // word still turns storage on.
        assert!(storage(&["anything"]).allows("u.local.note"));
    }

    /// The boundary proof for deny-by-default: an outbound request to a host the
    /// policy does not allow is stopped at the hook and resolves to
    /// `HttpRequestDenied` — **before any connection is attempted**. A hook that
    /// failed to deny would instead attempt to send and fail with a connect/DNS
    /// error, so asserting the exact denial code proves the request never left.
    #[tokio::test]
    async fn a_disallowed_host_is_denied_before_the_request_leaves() {
        use http_body_util::BodyExt as _;

        let mut hooks = EgressHooks::new(EgressPolicy::default()); // deny all
        let body: WasiBody = http_body_util::Empty::<bytes::Bytes>::new()
            .map_err(|never| match never {})
            .boxed_unsync();
        let request = http::Request::builder()
            .method("GET")
            .uri("http://blocked.example/")
            .body(body)
            .expect("a valid request");

        let fut = hooks.send_request(request, None, Box::new(async { Ok(()) }));
        let result = Box::into_pin(fut).await;

        match result {
            Err(HttpError::HttpRequestDenied) => {}
            Err(other) => panic!("denied host should be HttpRequestDenied, got {other:?}"),
            Ok(_) => panic!("a disallowed host must not produce a response"),
        }
    }
}
