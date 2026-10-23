//! HTML that cannot carry an unescaped value.
//!
//! # What this exists to prevent
//!
//! `format!("<p>{name}</p>")` is the easiest way to write a page, and the moment
//! `name` came from a request or from storage it is an XSS hole: a value of
//! `<script>…</script>` runs in every visitor's browser, on the app's origin.
//! Remembering to escape is not a defence, because forgetting it looks exactly
//! like remembering it.
//!
//! **So the mistake is made unrepresentable rather than documented.** [`Html`]
//! has two ways in:
//!
//! - [`Html::raw`] takes markup, and only as `&'static str` — a literal in the
//!   source. A value computed at run time cannot reach it without a deliberate
//!   `leak`, which reads as what it is.
//! - [`Html::text`] takes any string and escapes it.
//!
//! [`Response::html`](crate::Response::html) is the only way to send one, and it
//! sets the `Content-Type` and `nosniff` headers for you.
//!
//! # Contexts this does NOT cover
//!
//! Escaping makes a value safe as element text (`<p>…</p>`) and as a **quoted**
//! attribute value (`title="…"`). It does nothing for:
//!
//! - **`href` and `src`.** `javascript:alert(1)` contains nothing to escape.
//!   Never put a stored or submitted value in a URL attribute.
//! - **`<script>` and `<style>` bodies, and `on*` event attributes.** Their
//!   content is code, and escaping does not make code safe.
//! - **Unquoted attributes.** Always quote: `value="…"`, never `value=…`.
//!
//! ```
//! let mut page = clwd_sdk::Html::new();
//! page.raw("<p>hello, ").text("<b>ada</b>").raw("</p>");
//! assert_eq!(page.as_str(), "<p>hello, &lt;b&gt;ada&lt;/b&gt;</p>");
//! // In a handler: `Response::ok().html(out, &page)`.
//! ```

use alloc::string::String;

/// A page or fragment of HTML in which every run-time value was escaped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[must_use]
pub struct Html(String);

impl Html {
    /// An empty document.
    pub fn new() -> Self {
        Self(String::new())
    }

    /// Append markup written in the source.
    ///
    /// `&'static str` is the whole point: markup must be a literal, so nothing
    /// a caller sent can arrive here. See the module docs.
    pub fn raw(&mut self, markup: &'static str) -> &mut Self {
        self.0.push_str(markup);
        self
    }

    /// Append a value as text, escaped.
    ///
    /// Safe as element text and as a quoted attribute value; **not** in
    /// `href`, `src`, `<script>`, `<style>` or `on*` attributes. See the module
    /// docs.
    pub fn text(&mut self, value: &str) -> &mut Self {
        for c in value.chars() {
            match c {
                '&' => self.0.push_str("&amp;"),
                '<' => self.0.push_str("&lt;"),
                '>' => self.0.push_str("&gt;"),
                '"' => self.0.push_str("&quot;"),
                '\'' => self.0.push_str("&#39;"),
                c => self.0.push(c),
            }
        }
        self
    }

    /// The finished markup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant: SDK-2.
    #[test]
    fn text_escapes_every_character_that_can_end_a_context() {
        let mut page = Html::new();
        page.raw("<p title=\"")
            .text(r#"a"b'c"#)
            .raw("\">")
            .text("<script>alert('x') & </script>")
            .raw("</p>");
        assert_eq!(
            page.as_str(),
            "<p title=\"a&quot;b&#39;c\">&lt;script&gt;alert(&#39;x&#39;) &amp; &lt;/script&gt;</p>"
        );
    }

    #[test]
    fn text_leaves_ordinary_text_alone() {
        let mut page = Html::new();
        page.text("ada@example.com — café");
        assert_eq!(page.as_str(), "ada@example.com — café");
    }
}
