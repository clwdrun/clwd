//! The template rules that can be read off the source, checked on every
//! template.
//!
//! The behaviour is tested by serving each template (the dev server's
//! `templates` tests). These are the rules a reader — often an agent copying a
//! template — would pick up from the code itself: no auth code, identity only
//! from `caller()`, every response through `html` or `text`, per-caller
//! storage, nothing granted.
//!
//! Comments are excluded from the code checks: a template explains why there is
//! no cookie handling, and saying "cookie" in that explanation is the point.

use std::path::{Path, PathBuf};

fn templates() -> Vec<(String, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates");
    let mut found: Vec<_> = std::fs::read_dir(&root)
        .expect("templates/ is committed")
        .flatten()
        .filter(|e| e.path().join("clwd.toml").is_file())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    found.sort();
    assert!(!found.is_empty());
    found
}

fn source(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("src/lib.rs")).expect("every template has src/lib.rs")
}

/// One piece of code with its comments removed, and the string literals in it.
struct Code {
    text: String,
    literals: Vec<String>,
}

/// Strip `//` comments, tracking string and char literals so a `//` inside
/// `"http://..."` is not mistaken for one. Templates use no block comments;
/// one would be a failure here rather than a blind spot.
fn code(src: &str) -> Code {
    assert!(!src.contains("/*"), "templates use line comments only");
    let mut text = String::new();
    let mut literals = Vec::new();
    for line in src.lines() {
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => break,
                '"' => {
                    let mut literal = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                if let Some(next) = chars.next() {
                                    literal.push(next);
                                }
                            }
                            '"' => break,
                            c => literal.push(c),
                        }
                    }
                    text.push('"');
                    text.push_str(&literal);
                    text.push('"');
                    literals.push(literal);
                }
                '\'' => {
                    // A char literal like '"' must not open a string.
                    text.push(c);
                    let rest: String = chars.clone().take(3).collect();
                    if rest.starts_with("\\") || rest.chars().nth(1) == Some('\'') {
                        for _ in 0..if rest.starts_with('\\') { 3 } else { 2 } {
                            if let Some(c) = chars.next() {
                                text.push(c);
                            }
                        }
                    }
                }
                c => text.push(c),
            }
        }
        text.push('\n');
    }
    Code { text, literals }
}

/// No authentication code: the host decides who is calling before the app
/// runs. A template that handles cookies, tokens or sessions teaches every app
/// copied from it to do the same.
#[test]
fn no_template_contains_auth_code() {
    const FORBIDDEN: &[&str] = &[
        "cookie",
        "authorization",
        "bearer",
        "session",
        "login",
        "password",
        "jwt",
    ];
    for (name, dir) in templates() {
        let code = code(&source(&dir)).text.to_ascii_lowercase();
        for word in FORBIDDEN {
            assert!(
                !code.contains(word),
                "{name}: `{word}` appears in code; templates contain no auth code"
            );
        }
    }
}

/// Identity comes only from `caller()`. A form field, query parameter or
/// header naming a user is whatever the client chose to send.
#[test]
fn no_template_reads_identity_from_the_request() {
    const NAMES: &[&str] = &[
        "user", "user_id", "userid", "uid", "email", "subject", "sub", "owner",
    ];
    for (name, dir) in templates() {
        for literal in code(&source(&dir)).literals {
            let key = literal
                .to_ascii_lowercase()
                .trim_start_matches(['?', '&'])
                .trim_end_matches('=')
                .trim_start_matches("x-")
                .to_owned();
            assert!(
                !NAMES.contains(&key.as_str()),
                "{name}: the literal {literal:?} names an identity field to read from the \
                 request; identity comes only from caller()"
            );
        }
    }
}

/// Every response with a body goes through `Response::html` or
/// `Response::text`, which set the content type and `nosniff`. `send` is only
/// for an empty body, and no template sets `text/html` by hand.
#[test]
fn every_body_goes_through_html_or_text() {
    for (name, dir) in templates() {
        let code = code(&source(&dir));
        for (n, line) in code.text.lines().enumerate() {
            if let Some(at) = line.find(".send(") {
                assert!(
                    line[at..].contains("b\"\")"),
                    "{name} line {}: `send` with a body; use Response::html or Response::text",
                    n + 1
                );
            }
        }
        assert!(
            !code.literals.iter().any(|l| l.contains("text/html")),
            "{name} sets text/html by hand; Response::html does that"
        );
        assert!(
            !code.text.contains("format!(\"<"),
            "{name} formats markup into a string; build it with Html"
        );
    }
}

/// Per-caller data is keyed by the caller's subject through
/// `data::for_caller`, and a template that does so says the subject is
/// pairwise. `data::shared()` is one namespace for every caller.
#[test]
fn storage_is_per_caller_and_says_why() {
    for (name, dir) in templates() {
        let src = source(&dir);
        let code = code(&src).text;
        assert!(
            !code.contains("data::shared"),
            "{name} uses data::shared(); a template's data belongs to its caller"
        );
        if code.contains("for_caller") {
            assert!(
                src.contains("pairwise"),
                "{name} keys data by subject without saying the subject is pairwise"
            );
        }
    }
}

/// Nothing granted: no outbound network access and no secrets. A deployed app
/// can use neither today, and a template that needs one does not work deployed.
#[test]
fn no_template_is_granted_egress_or_secrets() {
    for (name, dir) in templates() {
        let manifest = std::fs::read_to_string(dir.join("clwd.toml")).expect("clwd.toml");
        let source = clwd_cli::build::Source::parse(&manifest)
            .unwrap_or_else(|e| panic!("{name}'s clwd.toml does not parse: {e}"));
        let caps = source.capabilities().expect("valid capabilities");
        assert!(caps.egress.is_empty(), "{name} is granted egress");
        assert!(caps.secrets.is_empty(), "{name} is granted secrets");
        assert_eq!(source.host_label(), Some(name.as_str()), "{name}");
    }
}

/// Every README gives the commands an author runs next: `clwd dev`, then
/// `clwd deploy`. Not `clwd run app.clwd`, which names a file `clwd build` does
/// not write there.
#[test]
fn every_readme_runs_with_dev_and_deploys_with_deploy() {
    for (name, dir) in templates() {
        let readme = std::fs::read_to_string(dir.join("README.md")).expect("README.md");
        let dev = readme.find("clwd dev");
        let deploy = readme.find("clwd deploy");
        assert!(
            dev.is_some() && deploy.is_some() && dev < deploy,
            "{name}'s README must say clwd dev, then clwd deploy"
        );
        assert!(
            !readme.contains("app.clwd"),
            "{name}'s README names app.clwd"
        );
    }
}

/// The scanner itself: comments are excluded, string contents are seen, and a
/// `//` inside a string is not a comment.
#[test]
fn the_scanner_strips_comments_but_not_strings() {
    let c =
        code("let a = \"http://x\"; // cookie here\n// session\nlet b = '\"'; let c = \"user\";\n");
    assert!(!c.text.contains("cookie") && !c.text.contains("session"));
    assert!(c.text.contains("http://x"));
    assert_eq!(c.literals, ["http://x", "user"]);
}
