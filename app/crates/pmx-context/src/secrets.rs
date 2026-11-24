//! Secret scanner. Replaces keys, tokens and passwords with `[secret removed]`
//! before any text leaves the Mac (SPEC section 9).

use regex::{Captures, Regex};
use std::sync::OnceLock;

/// What a removed secret is replaced with.
pub const SECRET_MARK: &str = "[secret removed]";

struct Rules {
    shapes: Vec<Regex>,
    url_password: Regex,
    quoted_assign: Regex,
    line_assign: Regex,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let shapes = [
            // Private key blocks (PEM, OpenSSH, PGP).
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----",
            // Anthropic.
            r"\bsk-ant-[A-Za-z0-9_\-]{16,}",
            // OpenAI (sk-..., sk-proj-..., sk-svcacct-..., sk-admin-...).
            r"\bsk-(?:proj-|svcacct-|admin-|None-)?[A-Za-z0-9_\-]{20,}",
            // AWS access key ids.
            r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}\b",
            // GitHub tokens.
            r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}\b",
            r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
            // GitLab.
            r"\bglpat-[A-Za-z0-9_\-]{20,}\b",
            // Slack tokens and webhooks.
            r"\bxox[abposre]-[A-Za-z0-9\-]{10,}",
            r"\bxapp-\d-[A-Za-z0-9\-]{10,}",
            r"https://hooks\.slack\.com/services/[A-Za-z0-9/_\-]+",
            // Stripe.
            r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b",
            r"\bwhsec_[A-Za-z0-9]{20,}\b",
            // Google API keys, OAuth tokens and client secrets.
            r"\bAIza[0-9A-Za-z_\-]{35}",
            r"\bya29\.[0-9A-Za-z_\-]{20,}",
            r"\bGOCSPX-[0-9A-Za-z_\-]{20,}",
            // Hugging Face, npm, PyPI, SendGrid, Twilio-style.
            r"\bhf_[A-Za-z0-9]{30,}\b",
            r"\bnpm_[A-Za-z0-9]{36}\b",
            r"\bpypi-[A-Za-z0-9_\-]{50,}",
            r"\bSG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}",
            // JWTs (header.payload.signature, header and payload are base64 JSON).
            r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
            // Bearer tokens in pasted headers.
            r"(?i)\bbearer\s+[A-Za-z0-9\-._~+/]{20,}=*",
            // Basic auth in pasted headers.
            r"(?i)\bbasic\s+[A-Za-z0-9+/]{12,}=*",
            // ClickUp personal tokens.
            r"\bpk_\d+_[A-Za-z0-9]{20,}\b",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("secret regex"))
        .collect();
        Rules {
            shapes,
            // scheme://user:password@host
            url_password: Regex::new(r"(?P<pre>\b[a-zA-Z][a-zA-Z0-9+.\-]*://[^/\s:@]+:)(?P<pw>[^@\s/]+)@").unwrap(),
            // "token": "value"   api_key = 'value'   password: "value"
            quoted_assign: Regex::new(
                r#"(?P<pre>["']?(?P<name>[A-Za-z_][A-Za-z0-9_.\-]*)["']?[ \t]*(?:=|:|:=|=>)[ \t]*(?P<q>["'`]))(?P<val>[^"'`\r\n]*)(?P<q2>["'`])"#,
            )
            .unwrap(),
            // KEY=value   export TOKEN=value   password: value   - secret: value
            line_assign: Regex::new(
                r#"(?m)^(?P<pre>[ \t]*(?:[-*][ \t]+)?(?:export[ \t]+)?[`*]*(?P<name>[A-Za-z_][A-Za-z0-9_.\-]*)[`*]*[ \t]*(?P<op>=|:)[`*]*[ \t]*)(?P<val>[^\s"'`][^\r\n]*?)[ \t]*$"#,
            )
            .unwrap(),
        }
    })
}

/// True when a variable or field name says it holds a secret.
fn secret_name(name: &str) -> bool {
    let low = name.to_lowercase();
    if ["secret", "password", "passwd", "passphrase", "apikey", "api_key", "api-key", "privatekey", "private_key", "credential"]
        .iter()
        .any(|w| low.contains(w))
    {
        return true;
    }
    // Word-level: "key" and "token" as their own word (api_key, accessToken, GITHUB_TOKEN),
    // not inside another word (monkey, max_tokens, keyboard).
    words(name).iter().any(|w| matches!(w.as_str(), "key" | "token" | "pwd" | "pat"))
}

/// Split a name into lowercase words on `_ . -` and camelCase.
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '.' || c == '-' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// True when a name says it holds a password (not just a key or token).
fn password_name(name: &str) -> bool {
    let low = name.to_lowercase();
    ["password", "passwd", "passphrase"].iter().any(|w| low.contains(w)) || words(name).iter().any(|w| w == "pwd" || w == "pin")
}

/// True when the value is clearly not a literal secret: empty, a number, a bool,
/// a reference to another variable, a placeholder, or prose.
fn harmless_value(v: &str) -> bool {
    let v = v.trim().trim_end_matches([',', ';']).trim();
    let unq = v.trim_matches(|c| c == '"' || c == '\'' || c == '`');
    if unq.is_empty() || unq == SECRET_MARK {
        return true;
    }
    let low = unq.to_lowercase();
    if matches!(low.as_str(), "true" | "false" | "null" | "none" | "nil" | "undefined" | "string" | "str" | "number" | "int" | "bool" | "boolean" | "optional" | "required" | "yes" | "no" | "on" | "off") {
        return true;
    }
    if unq.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '_') {
        return true;
    }
    if unq.starts_with('$') || unq.starts_with('<') || unq.starts_with('{') || unq.starts_with('[') || unq.starts_with('(') || unq.starts_with('%') {
        return true;
    }
    if low.starts_with("process.env") || low.starts_with("os.environ") || low.starts_with("os.getenv") || low.starts_with("env(") || low.starts_with("env::var") || low.starts_with("std::env") || low.starts_with("getenv") {
        return true;
    }
    if unq.chars().all(|c| c == '*' || c == 'x' || c == 'X' || c == '.' || c == '-') {
        return true;
    }
    if low.contains("your") && (low.contains("key") || low.contains("token") || low.contains("secret") || low.contains("password")) {
        return true;
    }
    // Prose ("see 1Password", "look in the Keychain") is not a secret.
    unq.split_whitespace().count() > 1
}

/// Replace secrets in `text` with `[secret removed]`.
pub fn scrub_secrets(text: &str) -> String {
    let r = rules();
    let mut out = text.to_string();
    for re in &r.shapes {
        if re.is_match(&out) {
            out = re.replace_all(&out, SECRET_MARK).into_owned();
        }
    }
    out = r
        .url_password
        .replace_all(&out, |c: &Captures| {
            let pw = &c["pw"];
            if pw.starts_with('$') || pw.starts_with('<') || pw == SECRET_MARK {
                c[0].to_string()
            } else {
                format!("{}{}@", &c["pre"], SECRET_MARK)
            }
        })
        .into_owned();
    out = r
        .quoted_assign
        .replace_all(&out, |c: &Captures| {
            let (name, val) = (&c["name"], &c["val"]);
            if secret_name(name) && !harmless_value(val) {
                format!("{}{}{}", &c["pre"], SECRET_MARK, &c["q2"])
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    out = r
        .line_assign
        .replace_all(&out, |c: &Captures| {
            let (name, val) = (&c["name"], &c["val"]);
            let v = val.trim();
            // Passwords can be all digits, and `NAME=a few words` in an env file is a passphrase, not prose.
            let pw_override = password_name(name)
                && v != SECRET_MARK
                && (v.chars().all(|ch| ch.is_ascii_digit()) && v.len() >= 4
                    || (&c["op"] == "=" && !c["pre"].ends_with(' ') && v.split_whitespace().count() > 1));
            if pw_override || (secret_name(name) && !harmless_value(val)) {
                format!("{}{}", &c["pre"], SECRET_MARK)
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // All values below are fake and built at run time so no real-looking key sits in the source.
    fn rep(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    fn gone(input: &str, secret: &str) {
        let out = scrub_secrets(input);
        assert!(!out.contains(secret), "secret survived in: {out}");
        assert!(out.contains(SECRET_MARK), "no mark in: {out}");
    }

    #[test]
    fn common_forms_from_notes_files() {
        let k = format!("k{}", rep('q', 30));
        gone(&format!("- `OPENAI_API_KEY`: {k}"), &k);
        gone(&format!("**Password:** Tr0ub{}", rep('x', 8)), &format!("Tr0ub{}", rep('x', 8)));
        gone("DB_PASSWORD=84726193", "84726193");
        gone("password: 20261122", "20261122");
        gone("MINI_PASSWORD=correct horse battery staple", "correct horse");
        let basic = format!("Authorization: Basic {}==", rep('Q', 24));
        gone(&basic, &rep('Q', 24));
        let pk = format!("pk_12345678_{}", rep('A', 32));
        gone(&format!("use {pk}"), &pk);
        // Prose about where a password lives stays.
        assert_eq!(scrub_secrets("password: see 1Password"), "password: see 1Password");
        assert_eq!(scrub_secrets("max_tokens: 4096"), "max_tokens: 4096");
    }

    #[test]
    fn anthropic_and_openai_keys() {
        let ant = format!("sk-ant-api03-{}-{}AA", rep('a', 40), rep('B', 20));
        gone(&format!("use {ant} here"), &ant);
        let oai = format!("sk-proj-{}", rep('Z', 48));
        gone(&format!("key: {oai}"), &oai);
        let old = format!("sk-{}", rep('9', 48));
        gone(&old, &old);
        // Ordinary words with sk- stay.
        assert_eq!(scrub_secrets("pip install scikit-learn sk-learn"), "pip install scikit-learn sk-learn");
    }

    #[test]
    fn aws_keys() {
        let id = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
        gone(&format!("aws id {id} ok"), &id);
        let secret = format!("{}{}", rep('w', 20), rep('Q', 20));
        gone(&format!("aws_secret_access_key = {secret}"), &secret);
    }

    #[test]
    fn github_and_gitlab_tokens() {
        let ghp = format!("ghp_{}", rep('x', 36));
        gone(&format!("token {ghp}"), &ghp);
        let pat = format!("github_pat_{}_{}", rep('A', 22), rep('b', 59));
        gone(&pat, &pat);
        let gl = format!("glpat-{}", rep('c', 20));
        gone(&gl, &gl);
    }

    #[test]
    fn slack_tokens() {
        let bot = format!("xoxb-{}-{}-{}", "1234567890", "1234567890123", rep('a', 24));
        gone(&format!("SLACK={bot}"), &bot);
        let hook = format!("https://hooks.slack.com/services/T000/B000/{}", rep('X', 24));
        gone(&hook, &hook);
    }

    #[test]
    fn stripe_keys() {
        let sk = format!("sk_live_{}", rep('4', 24));
        gone(&sk, &sk);
        let rk = format!("rk_live_{}", rep('r', 24));
        gone(&rk, &rk);
        let test = format!("sk_test_{}", rep('t', 24));
        gone(&test, &test);
    }

    #[test]
    fn google_keys() {
        let k = format!("AIza{}", rep('S', 35));
        gone(&format!("key={k}"), &k);
        let oauth = format!("ya29.{}", rep('g', 40));
        gone(&oauth, &oauth);
    }

    #[test]
    fn jwt() {
        let jwt = format!("eyJ{}.eyJ{}.{}", rep('h', 20), rep('p', 30), rep('s', 43));
        gone(&format!("Authorization: {jwt}"), &jwt);
    }

    #[test]
    fn bearer_header() {
        let tok = rep('m', 32);
        gone(&format!("curl -H 'Authorization: Bearer {tok}'"), &tok);
    }

    #[test]
    fn private_key_block() {
        let body = rep('K', 64);
        // Markers built at run time, so secret scanners do not flag this fake key in the source.
        let kind = ["PRIVATE", "KEY"].join(" ");
        let pem = format!("before\n-----BEGIN RSA {kind}-----\n{body}\n{body}\n-----END RSA {kind}-----\nafter");
        let out = scrub_secrets(&pem);
        assert!(!out.contains(&body));
        assert_eq!(out, format!("before\n{SECRET_MARK}\nafter"));
        let ssh = format!("-----BEGIN OPENSSH {kind}-----\n{body}\n-----END OPENSSH {kind}-----");
        gone(&ssh, &body);
    }

    #[test]
    fn env_lines() {
        let v = format!("fake{}", rep('v', 12));
        for line in [
            format!("API_KEY={v}"),
            format!("export GITHUB_TOKEN={v}"),
            format!("DB_PASSWORD={v}"),
            format!("client_secret: {v}"),
            format!("  - password: {v}"),
            format!("OPENAI_API_KEY=\"{v}\""),
            format!("accessToken = '{v}'"),
            format!("SECRET_KEY_BASE={v}"),
        ] {
            let out = scrub_secrets(&line);
            assert!(!out.contains(&v), "{line} -> {out}");
            assert!(out.contains(SECRET_MARK), "{line} -> {out}");
        }
        // Name and separator stay so the reader still sees which setting it was.
        assert_eq!(scrub_secrets(&format!("API_KEY={v}")), format!("API_KEY={SECRET_MARK}"));
    }

    #[test]
    fn json_fields() {
        let v = rep('j', 20);
        let out = scrub_secrets(&format!(r#"{{"token": "{v}", "name": "shop", "apiKey":"{v}"}}"#));
        assert!(!out.contains(&v), "{out}");
        assert!(out.contains(r#""name": "shop""#));
        assert_eq!(out.matches(SECRET_MARK).count(), 2);
    }

    #[test]
    fn url_password() {
        let pw = rep('p', 12);
        let out = scrub_secrets(&format!("DATABASE=postgres://app:{pw}@db.local:5432/shop"));
        assert!(!out.contains(&pw));
        assert!(out.contains("postgres://app:"));
        assert!(out.contains("@db.local:5432/shop"));
    }

    #[test]
    fn harmless_text_is_kept() {
        let keep = [
            "- Keys: read them from the project `.env` file.",
            "max_tokens=4000",
            "API_KEY=${API_KEY}",
            "api_key = os.environ[\"OPENAI_API_KEY\"]",
            "token: string",
            "password: <your password>",
            "monkey=banana",
            "keyboard_layout=us",
            "The secret is to keep tests small.",
            "Never repeat a key in chat, logs or Git.",
            "Run `cargo test` in app/crates/pmx-context.",
            "\"tokens\": 1200",
            "SECRET_FILE=",
        ];
        for k in keep {
            assert_eq!(scrub_secrets(k), k, "changed: {k}");
        }
    }

    #[test]
    fn name_words() {
        assert_eq!(words("accessToken"), vec!["access", "token"]);
        assert_eq!(words("GITHUB_TOKEN"), vec!["github", "token"]);
        assert!(secret_name("apiKey"));
        assert!(secret_name("x-api-key"));
        assert!(!secret_name("max_tokens"));
        assert!(!secret_name("monkey"));
    }
}
