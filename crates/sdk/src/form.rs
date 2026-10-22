//! Reading an HTML form submission.
//!
//! A `<form method="post">` sends its fields as
//! `application/x-www-form-urlencoded`: `name=value&other=value`, with spaces as
//! `+` and everything else outside a small safe set as `%XX`. Decoding that by
//! hand is a few lines that are easy to get subtly wrong, so it is here once.
//!
//! **A form field is never identity.** Who submitted the form is
//! [`caller()`](crate::caller), resolved by the host before the app runs. A
//! field named `user` or `email` is whatever the browser — or anyone with
//! `curl` — chose to send.

use alloc::string::String;
use alloc::vec::Vec;

/// The value of the first field called `name` in a form-encoded `body`.
///
/// `None` when the field is absent, when an escape is malformed, or when the
/// decoded bytes are not UTF-8: a value that cannot be decoded exactly is not
/// guessed at.
#[must_use]
pub fn field(body: &[u8], name: &str) -> Option<String> {
    body.split(|&b| b == b'&').find_map(|pair| {
        let (key, value) = match pair.iter().position(|&b| b == b'=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, &[][..]),
        };
        (decode(key)? == name).then(|| decode(value))?
    })
}

/// Decode one form-encoded component.
fn decode(raw: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut bytes = raw.iter();
    while let Some(&b) = bytes.next() {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let hi = hex(*bytes.next()?)?;
                let lo = hex(*bytes.next()?)?;
                out.push(hi << 4 | lo);
            }
            b => out.push(b),
        }
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_found_and_decoded() {
        let body = b"a=1&text=hello+world%21+%3Cb%3E&b=2";
        assert_eq!(field(body, "text").as_deref(), Some("hello world! <b>"));
        assert_eq!(field(body, "b").as_deref(), Some("2"));
    }

    #[test]
    fn utf8_survives_the_round_trip() {
        assert_eq!(field(b"t=caf%C3%A9", "t").as_deref(), Some("café"));
    }

    #[test]
    fn an_absent_or_undecodable_field_is_none() {
        assert_eq!(field(b"a=1", "text"), None);
        assert_eq!(field(b"text=%zz", "text"), None);
        assert_eq!(field(b"text=%4", "text"), None);
        assert_eq!(field(b"text=%FF", "text"), None, "not UTF-8");
    }

    #[test]
    fn a_field_with_no_value_is_empty() {
        assert_eq!(field(b"text=&a=1", "text").as_deref(), Some(""));
        assert_eq!(field(b"text", "text").as_deref(), Some(""));
    }
}
