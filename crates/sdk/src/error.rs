//! Application errors, and the status each becomes.
//!
//! # Why the message is `&'static str`
//!
//! An [`Error`]'s message goes into a response body, which leaves the sandbox
//! and reaches whoever made the request. If a variant could carry formatted
//! context — `BadRequest(String)`, say — then the first author who writes
//! `Error::BadRequest(format!("token {api_key} rejected"))` has put a secret in
//! an HTTP response.
//!
//! `clwd secrets` made `expose()` a named method precisely so plaintext has a
//! greppable census. That census stops at the crate boundary: nothing in the
//! CLI can see what a guest interpolates. So the guest side is closed by
//! construction instead — **a variant has no room for a value, so there is
//! nothing to interpolate.**
//!
//! Detail that helps the app author still has somewhere to go: `println!`
//! reaches the host's log, which `clwd logs` shows and which never reaches the
//! caller.
//!
//! ```ignore
//! println!("rejecting upload: {} bytes over limit", n - LIMIT);
//! return Err(Error::PayloadTooLarge);
//! ```

/// Declares the error type and, from the same input, its status mapping.
///
/// One invocation is the single source for the variant, its status, and its
/// body text — so a new error cannot be added without deciding all three, and
/// they cannot drift apart.
macro_rules! define_error {
    ($( $(#[$doc:meta])* $variant:ident => $status:literal, $message:literal; )*) => {
        /// Something an app could not do, and what the caller should be told.
        ///
        /// Deliberately not `#[non_exhaustive]`: an app that maps every variant
        /// today should be told at compile time when a new one appears, rather
        /// than having it fall into a wildcard and be reported as the wrong
        /// status.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
        pub enum Error {
            $( $(#[$doc])* #[error($message)] $variant, )*
        }

        impl Error {
            /// The HTTP status this error becomes.
            #[must_use]
            pub fn status(&self) -> u16 {
                match self { $( Self::$variant => $status, )* }
            }

            /// The text sent to the caller.
            ///
            /// `&'static str` by construction — see the module docs.
            #[must_use]
            pub fn message(&self) -> &'static str {
                match self { $( Self::$variant => $message, )* }
            }
        }

        /// Every variant, for a test that wants to check the whole table.
        pub const ALL: &[Error] = &[ $( Error::$variant, )* ];
    };
}

define_error! {
    /// The request was malformed: a missing field, an unparseable body, a
    /// path parameter that is not what it claims to be.
    BadRequest => 400, "bad request";

    /// No identity was established, and this route needs one.
    ///
    /// The right answer for a [`Caller::Anonymous`](crate::Caller::Anonymous)
    /// reaching something that requires a person.
    Unauthenticated => 401, "sign in to continue";

    /// The caller is known and is not allowed to do this.
    ///
    /// Distinct from [`Unauthenticated`](Self::Unauthenticated): signing in
    /// again will not help.
    Forbidden => 403, "not allowed";

    /// The thing named by the request does not exist.
    NotFound => 404, "not found";

    /// The request conflicts with the current state.
    Conflict => 409, "conflict";

    /// The request body is larger than this app accepts.
    PayloadTooLarge => 413, "payload too large";

    /// The app is being called faster than it will serve.
    TooManyRequests => 429, "too many requests";

    /// The app failed in a way the caller cannot do anything about.
    ///
    /// The default for anything unexpected. Put the detail in `println!`, where
    /// `clwd logs` will show it and the caller will not.
    Internal => 500, "internal error";

    /// A dependency this app needs is not answering.
    Unavailable => 503, "temporarily unavailable";
}

impl Error {
    /// This error as a response, ready to send.
    ///
    /// ```ignore
    /// let Some(item) = lookup(id) else {
    ///     return Error::NotFound.respond(out);
    /// };
    /// ```
    pub fn respond(self, out: crate::http::ResponseOutparam) {
        crate::http::Response::new(self.status()).text(out, self.message());
    }
}
