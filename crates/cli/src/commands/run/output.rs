//! What `clwd run` returns, and how it renders.
//!
//! # Why the body has two possible keys
//!
//! An app may reasonably return an image, and JSON has no byte string. The three
//! alternatives are all worse than a second key: lossy UTF-8 conversion silently
//! corrupts binary, erroring means `clwd run` cannot run an app that serves a
//! PNG, and always-base64 makes `| jq -r .body` useless for the text case that
//! is almost all of them.
//!
//! So exactly one of `body` and `body_base64` is present, and a caller checks
//! which. That is discriminating on a key rather than parsing a flag, which is
//! what an agent wants.

use base64::Engine as _;
use serde::Serialize;

use clwd_runtime::Response;

/// One local run: the response, and the identity it ran as.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub status: u16,

    /// The response body, when it is text.
    ///
    /// Exactly one of this and `body_base64` is present. Both are
    /// `skip_serializing_if`, so the absent one is omitted rather than null —
    /// unlike the error envelope's `hint`, where a null carries meaning. Here
    /// the presence of the key *is* the type discriminant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// The response body base64-encoded, when it is not valid UTF-8.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,

    /// The subject the app saw. Always the local one — see `run.rs`.
    ///
    /// Reported rather than assumed: an app author reading `ctx().caller()`
    /// should be able to see what their code was given, without inferring it.
    pub caller: String,

    /// The org the app saw.
    pub org: String,
}

impl Outcome {
    pub fn new(response: Response, subject: &str, org: &str) -> Self {
        let status = response.status().as_u16();
        let bytes = response.body().as_bytes().to_vec();

        match String::from_utf8(bytes) {
            Ok(text) => Self {
                status,
                body: Some(text),
                body_base64: None,
                caller: subject.to_owned(),
                org: org.to_owned(),
            },
            Err(e) => Self {
                status,
                body: None,
                body_base64: Some(base64::engine::general_purpose::STANDARD.encode(e.into_bytes())),
                caller: subject.to_owned(),
                org: org.to_owned(),
            },
        }
    }
}

/// Human rendering: the body, and nothing else on stdout.
///
/// The status and identity go to stderr via the caller, so `clwd run ... | jq`
/// and `clwd run ... > out.png` both do the obvious thing. A binary body is not
/// written to a terminal as raw bytes — a description is written instead, since
/// dumping a PNG into a TTY corrupts the terminal state.
impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.body, &self.body_base64) {
            (Some(text), _) => f.write_str(text),
            (None, Some(b64)) => write!(
                f,
                "<{} bytes of non-text response; use --json for body_base64>",
                b64.len() * 3 / 4
            ),
            (None, None) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clwd_runtime::Body;

    fn response(bytes: &[u8]) -> Response {
        http::Response::builder()
            .status(200)
            .body(Body::from(bytes.to_vec()))
            .expect("a valid response")
    }

    #[test]
    fn a_text_body_serializes_as_body_and_omits_body_base64() {
        let outcome = Outcome::new(response(b"hello"), "local", "local");
        let json = serde_json::to_value(&outcome).expect("serializes");
        let object = json.as_object().expect("an object");

        assert_eq!(object["body"], "hello");
        assert!(
            !object.contains_key("body_base64"),
            "the absent key must be omitted, not null: {object:?}"
        );
    }

    /// The case the second key exists for: an app returning an image.
    #[test]
    fn a_binary_body_serializes_as_body_base64_and_omits_body() {
        // A PNG magic number: valid bytes, invalid UTF-8.
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let outcome = Outcome::new(response(&png), "local", "local");
        let json = serde_json::to_value(&outcome).expect("serializes");
        let object = json.as_object().expect("an object");

        assert!(
            !object.contains_key("body"),
            "a non-UTF-8 body must not appear under `body`: {object:?}"
        );
        let encoded = object["body_base64"].as_str().expect("base64 is a string");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("valid base64");
        assert_eq!(decoded, png, "the bytes must survive the round trip");
    }

    /// Exactly one, always. Neither both nor neither.
    #[test]
    fn exactly_one_body_key_is_always_present() {
        for bytes in [b"text".to_vec(), vec![0xff, 0xfe], Vec::new()] {
            let outcome = Outcome::new(response(&bytes), "local", "local");
            let json = serde_json::to_value(&outcome).expect("serializes");
            let object = json.as_object().expect("an object");
            let present =
                object.contains_key("body") as u8 + object.contains_key("body_base64") as u8;
            assert_eq!(present, 1, "for {bytes:?}: got {object:?}");
        }
    }

    #[test]
    fn a_binary_body_is_described_rather_than_dumped_to_a_terminal() {
        let outcome = Outcome::new(response(&[0xff, 0xfe, 0xfd]), "local", "local");
        let rendered = outcome.to_string();
        assert!(
            rendered.contains("non-text") && rendered.contains("--json"),
            "got {rendered:?}"
        );
    }
}
