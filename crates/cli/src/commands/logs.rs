//! `clwd logs` — an app's log on the platform: what it wrote, and the
//! platform's own line about each request of it that failed.
//!
//! # Log text is untrusted, and a terminal obeys what it is sent
//!
//! An app writes whatever it likes, and that text reaches the author's
//! terminal. ANSI and OSC escape sequences in it could clear the screen, hide
//! lines, retitle the window or set the clipboard; a bare carriage return
//! could overwrite the line before it. [`inert`] renders every control
//! character visibly instead, so nothing an app writes is ever interpreted.
//! `--json` keeps the text exact, JSON-escaped, for a program to handle.
//!
//! # Where a line came from is the platform's to say
//!
//! Each line arrives with a `source` the platform set, never parsed from its
//! text. [`render`] draws the two apart in a column the app's text never
//! reaches: an app that prints a convincing platform line still shows up as
//! `app`.
//!
//! # Following
//!
//! `--follow` asks the platform to hold each request until a line arrives,
//! resumes from the last cursor it saw, and reconnects after a dropped
//! connection with a bounded backoff: no line is shown twice and none is
//! skipped.
//!
//! This returns a stream rather than a collection: #32's adapter consumes the
//! same type the binary renders.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::Progress;
use crate::error::CliError;
use crate::platform::{self, AppId, LogQuery, PlatformUrl, Token};

/// The longest the platform holds a follow before answering empty.
const WAIT_SECS: u64 = 20;
/// Reconnecting after a dropped connection: from this, doubling, to that.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// A position in an app's log: pass it back to read what follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogCursor(i64);

impl LogCursor {
    #[must_use]
    pub fn new(seq: i64) -> Self {
        Self(seq)
    }

    #[must_use]
    pub fn seq(self) -> i64 {
        self.0
    }
}

/// Where a line came from, as the platform decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    App,
    Platform,
}

/// One line of an app's log.
#[derive(Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LogLine {
    pub cursor: Option<LogCursor>,
    pub ts: String,
    pub source: Source,
    pub request_id: Option<String>,
    pub stream: Option<String>,
    /// Exactly what was written: untrusted text. Render it with [`inert`].
    pub text: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub runtime_code: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
}

impl LogLine {
    /// A line the app wrote, as the local dev server sees it.
    #[must_use]
    pub fn app(ts: String, stream: Option<&str>, request_id: Option<String>, text: String) -> Self {
        Self {
            cursor: None,
            ts,
            source: Source::App,
            request_id,
            stream: stream.map(str::to_owned),
            text,
            code: None,
            runtime_code: None,
            hint: None,
            status: None,
            latency_ms: None,
        }
    }

    /// A line the local dev server writes about a request.
    #[must_use]
    pub fn platform(ts: String, text: String) -> Self {
        Self {
            source: Source::Platform,
            stream: None,
            ..Self::app(ts, None, None, text)
        }
    }
}

impl fmt::Debug for LogLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogLine")
            .field("cursor", &self.cursor)
            .field("source", &self.source)
            .field("request_id", &self.request_id)
            .field("text", &format_args!("<{} bytes>", self.text.len()))
            .finish_non_exhaustive()
    }
}

impl fmt::Display for LogLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&render(self))
    }
}

/// `text` with every character a terminal would act on made visible: control
/// characters (C0, DEL, C1, so `ESC`, `BEL` and `CR` among them) and the
/// bidirectional overrides that reorder what is shown. A backslash is doubled,
/// so an escape written by the app reads differently from one made here. Tabs
/// are kept.
#[must_use]
pub fn inert(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(c),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() || is_bidi_control(c) => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// One line for a terminal: the time, the end of its request id, where it
/// came from, then the text, made [`inert`]. The source column is written
/// here, never by the line: a platform line is drawn as `▌platform`, and an
/// app line always as `app`, whatever its text says.
#[must_use]
pub fn render(line: &LogLine) -> String {
    let time = line.ts.get(11..19).unwrap_or(&line.ts);
    let request = line.request_id.as_deref().map_or("--------", |id| {
        id.get(id.len().saturating_sub(8)..).unwrap_or(id)
    });
    match line.source {
        Source::App => format!(
            "{time} {request:>8} app      {:<6} │ {}",
            line.stream.as_deref().unwrap_or(""),
            inert(&line.text)
        ),
        Source::Platform => {
            let mut out = format!(
                "{time} {request:>8} ▌platform        │ {}",
                inert(&line.text)
            );
            if let Some(hint) = &line.hint {
                out.push_str(&format!("\n{:>36}│ hint: {}", "", inert(hint)));
            }
            out
        }
    }
}

/// `30s`, `5m`, `2h` or `1d`, as seconds.
///
/// # Errors
///
/// When it is not a whole number followed by one of those units.
pub fn parse_since(raw: &str) -> Result<u64, CliError> {
    let bad = || CliError::InvalidArgument {
        what: format!("--since {raw:?}: use a whole number and a unit, like 30s, 5m, 2h or 1d"),
    };
    let (number, unit) = raw.split_at(raw.len().saturating_sub(1));
    let n: u64 = number.parse().map_err(|_| bad())?;
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    n.checked_mul(scale).ok_or_else(bad)
}

/// What to read.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Options {
    /// The app's host label, looked up in the org. Ignored when `app_id` is set.
    pub host_label: Option<String>,
    /// The app's id, when the caller has it.
    pub app_id: Option<String>,
    pub org: Option<String>,
    pub platform: Option<String>,
    pub follow: bool,
    pub since: Option<String>,
    pub request_id: Option<String>,
}

/// The lines, as they arrive.
#[derive(Debug)]
pub struct LogStream(mpsc::Receiver<Result<LogLine, CliError>>);

impl Stream for LogStream {
    type Item = Result<LogLine, CliError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

/// Start reading. Must be called inside a tokio runtime: the reading runs as
/// a task feeding the stream.
///
/// # Errors
///
/// Configuration errors, and a host label the platform does not know, before
/// any line is read. Later failures arrive in the stream.
pub async fn execute(
    options: Options,
    progress: Option<&dyn Progress>,
) -> Result<LogStream, CliError> {
    let url = platform::config::platform_url(options.platform.as_deref())?;
    let token = platform::config::token()?;
    let since_secs = options.since.as_deref().map(parse_since).transpose()?;
    if let Some(p) = progress {
        p.step("finding the app");
    }
    let app = match (&options.app_id, &options.host_label) {
        (Some(id), _) => AppId::parse(id)?,
        (None, Some(label)) => {
            let org = platform::config::org(options.org.as_deref())?;
            AppId::parse(&platform::app_by_label(&url, &org, &token, label).await?.id)?
        }
        (None, None) => {
            return Err(CliError::InvalidArgument {
                what: "name the app: its host label, --app <id>, or run in a project whose \
                       clwd.toml declares host_label"
                    .to_owned(),
            });
        }
    };
    if let Some(p) = progress {
        p.finish();
    }

    let (tx, rx) = mpsc::channel(64);
    let query = LogQuery {
        request_id: options.request_id,
        since_secs,
        ..LogQuery::default()
    };
    tokio::spawn(read(tx, url, token, app, query, options.follow));
    Ok(LogStream(rx))
}

/// Read pages into `tx`: once, or, following, for as long as the reader
/// listens.
async fn read(
    tx: mpsc::Sender<Result<LogLine, CliError>>,
    url: PlatformUrl,
    token: Token,
    app: AppId,
    mut query: LogQuery,
    follow: bool,
) {
    let client = match url
        .client()
        .timeout(Duration::from_secs(WAIT_SECS + 15))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let _ = tx.send(Err(CliError::Internal { source: e.into() })).await;
            return;
        }
    };
    let mut backoff = BACKOFF_MIN;
    loop {
        match platform::read_logs(&client, &url, &app, &token, &query).await {
            Ok(page) => {
                backoff = BACKOFF_MIN;
                for line in page.lines {
                    if tx.send(Ok(line)).await.is_err() {
                        return;
                    }
                }
                if !follow {
                    return;
                }
                // An empty log has no cursor yet: follow it from the start.
                query.after = page.cursor.or(query.after).or(Some(LogCursor::new(0)));
                query.wait_secs = WAIT_SECS;
            }
            // A dropped connection, or a platform that says to try again:
            // resume from the same cursor, after a bounded wait.
            Err(error)
                if follow && (matches!(error, CliError::Net { .. }) || error.retryable()) =>
            {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
            Err(error) => {
                let _ = tx.send(Err(error)).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(source: Source, text: &str) -> LogLine {
        LogLine {
            cursor: Some(LogCursor::new(7)),
            ts: "2026-10-23T09:15:42.123456Z".to_owned(),
            source,
            request_id: Some("01a16605-0208-7273-a80a-f216a6a4c8b7".to_owned()),
            stream: (source == Source::App).then(|| "stdout".to_owned()),
            text: text.to_owned(),
            code: None,
            runtime_code: None,
            hint: None,
            status: None,
            latency_ms: None,
        }
    }

    /// The terminal test: clear-screen, an OSC 52 clipboard write, a carriage
    /// return and a fake platform prefix all render as visible text, and
    /// nothing a terminal would act on reaches it.
    ///
    /// Invariant: CLI-4.
    #[test]
    fn hostile_text_renders_inert() {
        let hostile = "\u{1b}[2J\u{1b}]52;c;cHduZWQ=\u{7}\rcleared ▌platform │ deploy ok \u{9b}31m \u{202e}gnp.exe";
        let shown = render(&line(Source::App, hostile));
        assert!(
            !shown.chars().any(|c| c.is_control() || is_bidi_control(c)),
            "a control character reached the terminal: {shown:?}"
        );
        assert!(shown.contains("\\u{1b}[2J"), "{shown}");
        assert!(shown.contains("\\u{1b}]52;c;cHduZWQ=\\u{7}"), "{shown}");
        assert!(shown.contains("\\rcleared"), "{shown}");
        assert!(shown.contains("\\u{9b}"), "C1 CSI: {shown}");
        assert!(shown.contains("\\u{202e}"), "bidi override: {shown}");
        // The fake prefix is text in the app's column, after the real one.
        assert!(
            shown.starts_with("09:15:42 a6a4c8b7 app      stdout │ "),
            "{shown}"
        );
    }

    /// `--json` keeps the text exact: escaped for JSON, not rewritten.
    #[test]
    fn json_keeps_the_text_raw_but_escaped() {
        let hostile = "\u{1b}[2J\rcleared";
        let json = serde_json::to_string(&line(Source::App, hostile)).expect("json");
        assert!(json.contains(r#""text":"\u001b[2J\rcleared""#), "{json}");
        assert!(!json.contains('\u{1b}'));
        let back: LogLine = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back.text, hostile);
    }

    #[test]
    fn a_platform_line_is_drawn_apart_with_its_hint() {
        let mut platform = line(
            Source::Platform,
            "request failed: gateway.app_failed (runtime.trap), 502 in 3 ms",
        );
        platform.hint = Some("The app trapped.\u{1b}[2J".to_owned());
        let shown = render(&platform);
        assert!(
            shown.starts_with("09:15:42 a6a4c8b7 ▌platform        │ request failed"),
            "{shown}"
        );
        assert!(
            shown.contains("hint: The app trapped.\\u{1b}[2J"),
            "{shown}"
        );
    }

    /// Invariant: CLI-4.
    #[test]
    fn a_backslash_is_doubled_so_a_written_escape_is_distinguishable() {
        assert_eq!(inert(r"a\rb"), r"a\\rb");
        assert_eq!(inert("a\rb"), r"a\rb");
        assert_eq!(inert("tab\tkept"), "tab\tkept");
    }

    #[test]
    fn since_is_a_number_and_a_unit() {
        assert_eq!(parse_since("30s").ok(), Some(30));
        assert_eq!(parse_since("5m").ok(), Some(300));
        assert_eq!(parse_since("2h").ok(), Some(7_200));
        assert_eq!(parse_since("1d").ok(), Some(86_400));
        for bad in ["", "5", "m", "5x", "-5m", "1.5h"] {
            assert!(parse_since(bad).is_err(), "{bad:?}");
        }
    }
}
