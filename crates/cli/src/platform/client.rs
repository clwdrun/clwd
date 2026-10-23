//! The publish HTTP call, and the two shapes it turns a response into.
//!
//! A success (`201` or `200`) becomes a [`Published`]; every 4xx/5xx becomes a
//! [`CliError::Platform`] carrying the platform's own code; a failure with no
//! HTTP response at all — connection refused, timed out, TLS — becomes the one
//! CLI-owned network code, [`CliError::Net`], because nothing reached the
//! platform for it to describe.

use serde::{Deserialize, Serialize};

use super::{AppId, OrgId, PlatformError, PlatformUrl, Token};
use crate::error::CliError;

/// The version resource the platform returns on a successful publish.
///
/// The same body is sent for a freshly created version and one that already
/// existed; the platform API puts created-vs-existed in the status code and
/// has no `created` field, so this type carries no such field either — [`Published`]
/// derives it from the status, once, at the boundary.
#[derive(Debug, Deserialize)]
struct VersionResource {
    /// The version's own id.
    id: String,
    /// The app it belongs to.
    app_id: String,
    /// The content address, as `blake3:<64 hex>`.
    digest: String,
    /// `pending`, `ready`, or `failed` — whatever it was when the row returned.
    state: String,
    /// The org's certificate. Absent from a platform that predates it.
    #[serde(default)]
    certificate: Option<Certificate>,
}

/// Where the org's TLS certificate stands, as a publish reports it.
///
/// An org's first publish is what requests its certificate, so until it is
/// issued the app's URL fails the TLS handshake. That is worth saying loudly:
/// otherwise a deploy that succeeded hands back a URL that does not load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    /// `pending`, `issued` or `failed`.
    pub status: String,
    /// What the status means for reaching the org's apps, in the platform's words.
    pub message: String,
}

/// The shared error envelope, as it arrives on the wire.
#[derive(Debug, Deserialize)]
struct Envelope {
    code: String,
    message: String,
    hint: Option<String>,
    retryable: bool,
}

/// A published version, as the CLI reports it.
///
/// `created` is the CLI's own reading of the response status — `true` for a
/// `201`, `false` for a `200 already existed`. It is a presentation of the
/// platform's fact, derived at the boundary, not a second copy of it on the wire.
#[must_use]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Published {
    /// The app the version belongs to.
    pub app_id: String,
    /// The version's id.
    pub version_id: String,
    /// The content address the platform recorded.
    pub digest: String,
    /// The lifecycle state at the moment of the response.
    pub state: String,
    /// Whether this call created the version (`201`) or found it already there
    /// (`200`). Idempotency made visible, not an error.
    pub created: bool,
    /// The org's certificate, only when it is not yet `issued`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<Certificate>,
}

impl VersionResource {
    fn into_published(self, created: bool) -> Published {
        Published {
            app_id: self.app_id,
            version_id: self.id,
            digest: self.digest,
            state: self.state,
            created,
            certificate: self.certificate.filter(|c| c.status != "issued"),
        }
    }
}

impl std::fmt::Display for Published {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.created {
            writeln!(f, "published version {} ({})", self.version_id, self.state)?;
        } else {
            writeln!(
                f,
                "version {} already existed ({})",
                self.version_id, self.state
            )?;
        }
        writeln!(f, "  app:    {}", self.app_id)?;
        write!(f, "  digest: {}", self.digest)?;
        if let Some(c) = &self.certificate {
            write!(f, "\n  certificate {}: {}", c.status, c.message)?;
        }
        Ok(())
    }
}

/// Upload `bundle` as a new version of `app`.
///
/// The bundle is already fully in memory — `clwd-bundle` has no streaming API
/// (`#6`) — so it is sent as one body rather than pretending to stream. The
/// platform enforces its own size ceiling and returns `413` over it; the CLI
/// does not keep a second copy of that limit to check against.
pub async fn publish(
    url: &PlatformUrl,
    app: &AppId,
    token: &Token,
    bundle: Vec<u8>,
) -> Result<Published, CliError> {
    let endpoint = url.versions_endpoint(app)?;
    let client = url.client().build().map_err(pure_network)?;

    let response = client
        .post(endpoint)
        .bearer_auth(token.as_bearer())
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bundle)
        .send()
        .await
        .map_err(pure_network)?;

    let status = response.status();
    if status.is_success() {
        let created = status == reqwest::StatusCode::CREATED;
        let version = response
            .json::<VersionResource>()
            .await
            .map_err(|e| CliError::Internal {
                source: anyhow::Error::new(e)
                    .context("the platform's success response was not a version resource"),
            })?;
        Ok(version.into_published(created))
    } else {
        Err(platform_error(response).await)
    }
}

/// Map a response the platform decided was a failure onto its own code.
///
/// The `Retry-After` header rides alongside a `429`'s `retryable: true`. When
/// the platform gives no hint of its own, the header becomes one so the caller
/// still learns how long to wait; when it does, the platform's hint is kept
/// verbatim.
/// An app, as creating it returned it.
///
/// `created` is `false` when the org already had a live app with this host
/// label: creation is idempotent by label, so running the same command again
/// returns the same app rather than failing.
#[must_use]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CreatedApp {
    pub id: String,
    pub org_id: String,
    /// The app's permanent label: its URL never changes.
    pub host_label: String,
    pub name: String,
    pub url: String,
    pub created: bool,
}

impl std::fmt::Display for CreatedApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.created {
            writeln!(f, "created app {} ({})", self.host_label, self.id)?;
        } else {
            writeln!(f, "app {} already exists ({})", self.host_label, self.id)?;
        }
        writeln!(f, "url:  {}", self.url)?;
        // The follow-up command, spelled out, so the reader — often an agent —
        // does not have to work out where the id goes.
        write!(f, "next: clwd publish <bundle> --app {}", self.id)
    }
}

/// Create the app `host_label` names in `org`, or get the one that already
/// holds that label.
pub async fn create_app(
    url: &PlatformUrl,
    org: &OrgId,
    token: &Token,
    host_label: &str,
) -> Result<CreatedApp, CliError> {
    let endpoint = url.apps_endpoint(org)?;
    let client = url.client().build().map_err(pure_network)?;

    let response = client
        .post(endpoint)
        .bearer_auth(token.as_bearer())
        .json(&serde_json::json!({ "host_label": host_label }))
        .send()
        .await
        .map_err(pure_network)?;

    if response.status().is_success() {
        response
            .json::<CreatedApp>()
            .await
            .map_err(|e| CliError::Internal {
                source: anyhow::Error::new(e)
                    .context("the platform's success response was not an app resource"),
            })
    } else {
        Err(platform_error(response).await)
    }
}

/// An app, as looking it up by its label returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AppSummary {
    pub id: String,
    pub host_label: String,
    pub url: String,
}

/// The app in `org` with this host label.
pub async fn app_by_label(
    url: &PlatformUrl,
    org: &OrgId,
    token: &Token,
    host_label: &str,
) -> Result<AppSummary, CliError> {
    let endpoint = url.app_by_label_endpoint(org, host_label)?;
    let client = url.client().build().map_err(pure_network)?;
    get_json(client.get(endpoint), token).await
}

/// One page of an app's log, as the platform returns it.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct LogPage {
    pub lines: Vec<crate::commands::logs::LogLine>,
    pub cursor: Option<crate::commands::logs::LogCursor>,
}

/// What to read: after a cursor, one request's lines, recent lines only, at
/// most how many, and how long to wait for a line when there is none.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct LogQuery {
    pub after: Option<crate::commands::logs::LogCursor>,
    pub request_id: Option<String>,
    pub since_secs: Option<u64>,
    pub limit: Option<u32>,
    pub wait_secs: u64,
}

/// Read a page of `app`'s log.
pub async fn read_logs(
    client: &reqwest::Client,
    url: &PlatformUrl,
    app: &AppId,
    token: &Token,
    query: &LogQuery,
) -> Result<LogPage, CliError> {
    let mut endpoint = url.logs_endpoint(app)?;
    {
        let mut pairs = endpoint.query_pairs_mut();
        if let Some(after) = query.after {
            pairs.append_pair("after", &after.seq().to_string());
        }
        if let Some(id) = &query.request_id {
            pairs.append_pair("request_id", id);
        }
        if let Some(since) = query.since_secs {
            pairs.append_pair("since", &since.to_string());
        }
        if let Some(limit) = query.limit {
            pairs.append_pair("limit", &limit.to_string());
        }
        if query.wait_secs > 0 {
            pairs.append_pair("wait", &query.wait_secs.to_string());
        }
    }
    // `query_pairs_mut` leaves a bare `?` when nothing was added.
    if endpoint.query() == Some("") {
        endpoint.set_query(None);
    }
    get_json(client.get(endpoint), token).await
}

async fn get_json<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
    token: &Token,
) -> Result<T, CliError> {
    let response = request
        .bearer_auth(token.as_bearer())
        .send()
        .await
        .map_err(pure_network)?;
    if response.status().is_success() {
        response.json::<T>().await.map_err(|e| {
            // A body cut off mid-read is the network's doing, not a broken
            // contract: report it as one, so a follower retries.
            if e.is_body() {
                pure_network(e)
            } else {
                CliError::Internal {
                    source: anyhow::Error::new(e)
                        .context("the platform's success response was not the expected shape"),
                }
            }
        })
    } else {
        Err(platform_error(response).await)
    }
}

async fn platform_error(response: reqwest::Response) -> CliError {
    let status = response.status();
    // The platform API never redirects, and the client follows none, so a
    // redirect is something between here and the platform, or a wrong URL. It
    // is reported rather than obeyed: following it would carry the token to
    // wherever the response pointed.
    if status.is_redirection() {
        let to = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("nowhere");
        return CliError::Net {
            retryable: false,
            detail: format!(
                "the platform answered {status}, a redirect to {to:?}, which clwd does not \
                 follow; check the platform URL"
            ),
        };
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());

    match response.json::<Envelope>().await {
        Ok(envelope) => {
            let hint = envelope
                .hint
                .or_else(|| retry_after.map(|s| format!("retry after {s}s")));
            CliError::Platform {
                source: PlatformError::new(
                    envelope.code,
                    envelope.message,
                    hint,
                    envelope.retryable,
                ),
            }
        }
        // The platform API sends the envelope on every 4xx/5xx, so a body that
        // is not one is a broken contract, not a normal outcome — an internal
        // error naming the status, rather than a delegated code we cannot read.
        Err(e) => CliError::Internal {
            source: anyhow::Error::new(e).context(format!(
                "the platform returned {status} without the shared error envelope"
            )),
        },
    }
}

/// A failure with no HTTP response: connection refused, timed out, TLS handshake
/// failed. The one publish failure the platform never describes, so the CLI owns
/// the code. `retryable` follows the transport: a timeout or a refused connection
/// may succeed on a later attempt.
fn pure_network(e: reqwest::Error) -> CliError {
    CliError::Net {
        retryable: e.is_timeout() || e.is_connect(),
        detail: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};

    const TOKEN: &str = "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const APP: &str = "018f5b2a-0000-7000-8000-000000000000";

    /// What the mock server saw the CLI send. Asserting on this is how a test of
    /// what the CLI *sends* stays honest — it is captured from a real socket, not
    /// from the request object the CLI built.
    struct Captured {
        method: String,
        path: String,
        authorization: Option<String>,
        content_type: Option<String>,
        body: Vec<u8>,
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn read_request(stream: &mut TcpStream) -> Captured {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        let header_end = loop {
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                break pos;
            }
            let n = stream.read(&mut tmp).await.expect("read headers");
            if n == 0 {
                break buf.len();
            }
            buf.extend_from_slice(&tmp[..n]);
        };

        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();

        let mut authorization = None;
        let mut content_type = None;
        let mut content_length = 0usize;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
                match k.as_str() {
                    "authorization" => authorization = Some(v.to_owned()),
                    "content-type" => content_type = Some(v.to_owned()),
                    "content-length" => content_length = v.parse().unwrap_or(0),
                    _ => {}
                }
            }
        }

        let mut body = buf[header_end + 4..].to_vec();
        while body.len() < content_length {
            let n = stream.read(&mut tmp).await.expect("read body");
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }

        Captured {
            method,
            path,
            authorization,
            content_type,
            body,
        }
    }

    /// Start a one-shot server that returns `(status, extra_headers, body)` and
    /// hands back what it received. `extra_headers` is raw, e.g. `"Retry-After: 30\r\n"`.
    async fn serve_once(
        status: u16,
        reason: &'static str,
        extra_headers: &'static str,
        body: String,
    ) -> (String, tokio::task::JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let captured = read_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Type: application/json\r\n\
                 {extra_headers}\
                 Content-Length: {len}\r\n\
                 Connection: close\r\n\r\n{body}",
                len = body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            stream.flush().await.expect("flush");
            captured
        });
        (format!("http://{addr}"), handle)
    }

    fn version_body(state: &str) -> String {
        format!(
            r#"{{"id":"018f5b2a-1111-7000-8000-000000000001","app_id":"{APP}",
                "digest":"blake3:{hex}","state":"{state}",
                "created_at":"2026-09-20T00:00:00Z"}}"#,
            hex = "a".repeat(64)
        )
    }

    async fn call(base: &str, bundle: &[u8]) -> Result<Published, CliError> {
        let url = PlatformUrl::parse(base).expect("valid base");
        let app = AppId::parse(APP).expect("valid app");
        let token = Token::parse(TOKEN).expect("valid token");
        publish(&url, &app, &token, bundle.to_vec()).await
    }

    #[tokio::test]
    async fn a_201_creates_a_version_and_the_request_is_well_formed() {
        let (base, server) = serve_once(201, "Created", "", version_body("ready")).await;
        let bundle = b"the actual bundle bytes".to_vec();

        let published = call(&base, &bundle).await.expect("publish succeeds");
        assert!(published.created, "a 201 is a creation");
        assert_eq!(published.state, "ready");
        assert_eq!(published.version_id, "018f5b2a-1111-7000-8000-000000000001");

        // What the platform actually received — not what the CLI thinks it sent.
        let req = server.await.expect("server joined");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, format!("/v1/apps/{APP}/versions"));
        assert_eq!(
            req.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(
            req.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(
            req.body, bundle,
            "the bundle bytes must reach the platform intact"
        );
    }

    const ORG: &str = "018f5b2a-2222-7000-8000-000000000002";

    fn app_body(created: bool) -> String {
        format!(
            r#"{{"id":"{APP}","org_id":"{ORG}","host_label":"notes","name":"notes",
                "url":"https://notes.acme.apps.example","created":{created}}}"#
        )
    }

    async fn create(base: &str) -> Result<CreatedApp, CliError> {
        let url = PlatformUrl::parse(base).expect("valid base");
        let org = OrgId::parse(ORG).expect("valid org");
        let token = Token::parse(TOKEN).expect("valid token");
        create_app(&url, &org, &token, "notes").await
    }

    #[tokio::test]
    async fn a_201_creates_an_app_and_the_request_is_well_formed() {
        let (base, server) = serve_once(201, "Created", "", app_body(true)).await;

        let app = create(&base).await.expect("create succeeds");
        assert!(app.created);
        assert_eq!(app.id, APP);
        assert_eq!(app.url, "https://notes.acme.apps.example");
        assert_eq!(
            app.to_string(),
            format!(
                "created app notes ({APP})\nurl:  https://notes.acme.apps.example\n\
                 next: clwd publish <bundle> --app {APP}"
            )
        );

        let req = server.await.expect("server joined");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, format!("/v1/orgs/{ORG}/apps"));
        assert_eq!(
            req.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(req.content_type.as_deref(), Some("application/json"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).expect("JSON body");
        assert_eq!(body, serde_json::json!({ "host_label": "notes" }));
    }

    /// Idempotent by label: the same command again returns the same app.
    #[tokio::test]
    async fn a_200_is_the_existing_app_not_an_error() {
        let (base, _server) = serve_once(200, "OK", "", app_body(false)).await;
        let app = create(&base).await.expect("an existing app is a success");
        assert!(!app.created);
        assert!(
            app.to_string()
                .starts_with(&format!("app notes already exists ({APP})")),
            "{app}"
        );
    }

    /// A redirect is reported, never followed: following it would carry the
    /// token to wherever the response pointed. The target sees no request.
    ///
    /// Invariant: CLI-11.
    #[tokio::test]
    async fn a_redirect_is_reported_and_not_followed() {
        let (target, target_server) = serve_once(200, "OK", "", app_body(true)).await;
        let location: &'static str =
            Box::leak(format!("Location: {target}/elsewhere\r\n").into_boxed_str());
        let (base, _server) = serve_once(302, "Found", location, "{}".to_owned()).await;
        let err = create(&base).await.expect_err("a redirect is an error");
        assert_eq!(err.code(), "net.unreachable");
        assert!(!err.retryable(), "a redirect does not go away on retry");
        let message = err.to_string();
        assert!(message.contains("302"), "{message}");
        assert!(
            message.contains(&target),
            "names where it pointed: {message}"
        );
        assert!(!message.contains(TOKEN), "{message}");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), target_server)
                .await
                .is_err(),
            "the redirect target received no request"
        );
    }

    /// A retired label keeps the platform's code, message and hint.
    #[tokio::test]
    async fn a_retired_label_surfaces_the_platform_conflict() {
        let (base, _server) = serve_once(
            409,
            "Conflict",
            "",
            r#"{"code":"control-plane.conflict","message":"conflict: host_label \"notes\" belonged to a deleted app","hint":"choose another","retryable":false,"request_id":"r"}"#.to_owned(),
        )
        .await;
        let err = create(&base).await.expect_err("a conflict is an error");
        assert_eq!(err.code(), "control-plane.conflict");
        assert_eq!(err.hint(), Some("choose another"));
        assert!(!err.retryable());
    }

    /// A certificate is reported only while it is not yet issued: an issued one
    /// is the normal case and would be noise on every publish.
    #[tokio::test]
    async fn only_an_unissued_certificate_is_reported() {
        let with = |status: &str| {
            version_body("ready").replacen(
                '{',
                &format!(r#"{{"certificate":{{"status":"{status}","message":"m"}},"#),
                1,
            )
        };
        let (base, _server) = serve_once(201, "Created", "", with("pending")).await;
        let published = call(&base, b"bytes").await.expect("succeeds");
        let cert = published.certificate.as_ref().expect("pending is reported");
        assert_eq!(cert.status, "pending");
        assert!(
            published.to_string().ends_with("certificate pending: m"),
            "{published}"
        );

        let (base, _server) = serve_once(201, "Created", "", with("issued")).await;
        let published = call(&base, b"bytes").await.expect("succeeds");
        assert!(published.certificate.is_none());
        let json = serde_json::to_value(&published).expect("serializes");
        assert!(json.get("certificate").is_none(), "{json}");
    }

    #[tokio::test]
    async fn a_200_is_already_existed_not_an_error() {
        let (base, server) = serve_once(200, "OK", "", version_body("ready")).await;

        let published = call(&base, b"identical bytes")
            .await
            .expect("200 is success");
        assert!(
            !published.created,
            "a 200 means the version already existed"
        );
        server.await.expect("server joined");
    }

    /// The test that matters: a well-formed UUID that names no app returns 404,
    /// and the CLI must surface the platform's code, not mangle or wrap it.
    #[tokio::test]
    async fn a_missing_app_surfaces_the_platform_not_found() {
        let body = r#"{"code":"control-plane.not_found","message":"no such app",
                       "hint":"check the app id","retryable":false}"#
            .to_owned();
        let (base, server) = serve_once(404, "Not Found", "", body).await;

        let err = call(&base, b"bytes").await.expect_err("404 is an error");
        assert_eq!(err.code(), "control-plane.not_found");
        assert!(!err.retryable());
        server.await.expect("server joined");
    }

    #[tokio::test]
    async fn a_forbidden_token_surfaces_the_platform_code_verbatim() {
        let body = r#"{"code":"identity.forbidden","message":"token lacks publish scope",
                       "hint":"request a token with the publish scope","retryable":false}"#
            .to_owned();
        let (base, server) = serve_once(403, "Forbidden", "", body).await;

        let err = call(&base, b"bytes").await.expect_err("403 is an error");
        assert_eq!(err.code(), "identity.forbidden");
        assert_ne!(err.code(), "cli.internal", "must not be wrapped");
        server.await.expect("server joined");
    }

    #[tokio::test]
    async fn a_429_preserves_retryable_and_the_retry_after() {
        // Platform gives no hint, so the Retry-After becomes one.
        let body = r#"{"code":"control-plane.rate_limited","message":"slow down",
                       "hint":null,"retryable":true}"#
            .to_owned();
        let (base, server) =
            serve_once(429, "Too Many Requests", "Retry-After: 30\r\n", body).await;

        let err = call(&base, b"bytes").await.expect_err("429 is an error");
        assert_eq!(err.code(), "control-plane.rate_limited");
        assert!(err.retryable(), "the back-off signal must survive");
        assert_eq!(err.hint(), Some("retry after 30s"));
        server.await.expect("server joined");
    }

    /// A connection refused — nothing answered — is the CLI-owned network code.
    #[tokio::test]
    async fn a_connection_refused_is_a_cli_network_error() {
        // Port 1 on loopback refuses; no server is listening.
        let err = call("http://127.0.0.1:1", b"bytes")
            .await
            .expect_err("nothing is listening");
        assert_eq!(err.code(), "net.unreachable");
        assert!(err.retryable(), "a refused connection may succeed on retry");
    }

    /// The positive control: no token value ever reaches rendered output.
    ///
    /// Invariant: CLI-2.
    #[tokio::test]
    async fn the_token_never_appears_in_a_rendered_result() {
        let (base, server) = serve_once(201, "Created", "", version_body("pending")).await;

        let published = call(&base, b"bytes").await.expect("succeeds");
        let json = serde_json::to_string(&published).expect("serializes");
        assert!(
            !json.contains("clwd_tok"),
            "a token leaked into JSON output: {json}"
        );
        assert!(
            !format!("{published}").contains("clwd_tok"),
            "a token leaked into Display"
        );
        server.await.expect("server joined");
    }
}
