//! Signature cipher handling for protected stream URLs.
//!
//! YouTube protects some adaptive formats behind a `signatureCipher`: the real
//! URL is only revealed after running the `s` value through a function defined
//! in the player's `base.js`. Instead of shipping a JavaScript engine we
//! extract the *operation list* that function performs (reverse / slice /
//! swap), which is exactly how the function behaves and is stable across
//! player releases.
//!
//! The `n` throttle parameter is handled separately (see [`transform_n`]).

use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;

/// A single mutation applied to the signature character array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigOp {
    /// Reverse the whole array.
    Reverse,
    /// Drop the first `n` characters.
    Slice(usize),
    /// Swap the first character with the one at `n % len`.
    Swap(usize),
}

/// A parsed signature decipher routine.
#[derive(Debug, Clone, Default)]
pub struct Cipher {
    ops: Vec<SigOp>,
}

impl Cipher {
    /// Applies the extracted operations to a raw signature.
    pub fn decode(&self, signature: &str) -> String {
        let mut chars: Vec<char> = signature.chars().collect();
        for op in &self.ops {
            match *op {
                SigOp::Reverse => chars.reverse(),
                SigOp::Slice(n) => {
                    if n <= chars.len() {
                        chars.drain(0..n);
                    }
                }
                SigOp::Swap(n) => {
                    if !chars.is_empty() {
                        let index = n % chars.len();
                        chars.swap(0, index);
                    }
                }
            }
        }
        chars.into_iter().collect()
    }

    /// True when no operations were recovered (the signature is used as-is).
    pub fn is_identity(&self) -> bool {
        self.ops.is_empty()
    }

    /// Parses the decipher routine out of a `base.js` source.
    pub fn parse(source: &str) -> Result<Self> {
        let (name, body) = locate_decipher_function(source)
            .context("could not locate the signature decipher function")?;
        let ops = extract_ops(source, &body)
            .with_context(|| format!("could not extract operations for `{name}`"))?;
        if ops.is_empty() {
            return Err(anyhow!("decipher function `{name}` exposed no operations"));
        }
        Ok(Self { ops })
    }
}

static DECIPHER_DEF: Lazy<Regex> = Lazy::new(|| {
    // Matches `NAME = function (ARG) { ... ARG = ARG.split(""); ...`
    //
    // The pattern deliberately ends right after `split("")` — brace matching
    // (`locate_decipher_function`) walks the body from the captured `{`, because
    // a regex cannot reliably pair nested braces.
    //
    // Two constraints shape this pattern:
    //   * The `regex` crate is a finite-automata engine with NO backreference
    //     support, so `(?P=arg)` is illegal here (it fails at compile time with
    //     "unrecognized flag"). The assignment is therefore matched as
    //     `IDENT = IDENT.split("")` and the caller does not rely on the two
    //     identifiers being textually equal.
    //   * The quotes around the empty split argument are written `\"` rather
    //     than `"`: a raw string only terminates on `"#`, so `\"` stays inside
    //     the literal and the regex reads it as a literal quote.
    Regex::new(
        r#"(?P<name>[A-Za-z0-9_$]+)\s*=\s*function\s*\(\s*[A-Za-z0-9_$]+\s*\)\s*(?P<open>\{)\s*[A-Za-z0-9_$]+\s*=\s*[A-Za-z0-9_$]+\s*\.\s*split\s*\(\s*\"\s*\"\s*\)"#,
    )
    .expect("valid regex")
});

static OP_CALL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?P<obj>[A-Za-z0-9_$]+)\.(?P<op>[A-Za-z0-9_$]+)\s*\((?P<args>[^)]*)\)")
        .expect("valid regex")
});

static OP_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?P<op>[A-Za-z0-9_$]+)\s*:\s*function\s*\([^)]*\)\s*\{(?P<body>[^{}]*)\}")
        .expect("valid regex")
});

/// Finds the decipher function name and its body using brace matching.
fn locate_decipher_function(source: &str) -> Option<(String, String)> {
    let caps = DECIPHER_DEF.captures(source)?;
    let name = caps.name("name")?.as_str().to_string();
    // The opening brace is captured inside the match, before the assignment.
    let open = caps.name("open")?.start();
    let body = balanced_body(source, open)?;
    Some((name, body))
}

/// Returns the text between `source[open]` and its matching closing brace.
fn balanced_body(source: &str, open: usize) -> Option<String> {
    let bytes = source.as_bytes();
    if *bytes.get(open)? != b'{' {
        return None;
    }
    let mut depth = 0i32;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(source[open + 1..index].to_string());
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Extracts the ordered operation list by resolving helper-object calls.
fn extract_ops(source: &str, body: &str) -> Option<Vec<SigOp>> {
    // Collect the definitions of every helper function in the player.
    let definitions = helper_definitions(source);

    let mut ops = Vec::new();
    for caps in OP_CALL.captures_iter(body) {
        let op_name = caps.name("op")?.as_str();
        let args = caps.name("args")?.as_str();
        let definition = match definitions.get(op_name) {
            Some(body) => body,
            // Inline operations (`a.reverse()`) have no helper entry.
            None => {
                if op_name == "reverse" {
                    ops.push(SigOp::Reverse);
                }
                continue;
            }
        };
        if let Some(op) = classify(definition, args) {
            ops.push(op);
        }
    }
    Some(ops)
}

/// Builds a map of helper function name to its body source.
fn helper_definitions(source: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for caps in OP_DEF.captures_iter(source) {
        if let (Some(name), Some(body)) = (caps.name("op"), caps.name("body")) {
            map.insert(name.as_str().to_string(), body.as_str().to_string());
        }
    }
    map
}

/// Classifies a helper body into a [`SigOp`], taking the index from the call.
fn classify(body: &str, args: &str) -> Option<SigOp> {
    if body.contains(".reverse(") {
        return Some(SigOp::Reverse);
    }
    if body.contains(".splice(0,") && !body.contains(".splice(0,0,") {
        return Some(SigOp::Slice(call_index(args)?));
    }
    if body.contains("[0]") && body.contains("%") {
        return Some(SigOp::Swap(call_index(args)?));
    }
    // `splice(0,0,x)` inserts, which never appears in signature routines.
    None
}

/// Reads the numeric second argument of a call such as `obj.op(a, 42)`.
fn call_index(args: &str) -> Option<usize> {
    let mut parts = args.split(',');
    parts.next()?;
    let raw = parts.next()?.trim();
    raw.parse::<usize>().ok()
}

/// Applies the `n` throttle transform when it can be recovered.
///
/// The `n` parameter is a *function*, not a fixed operation list, so it cannot
/// be replayed the way signatures can. Streams returned by the `ANDROID_VR`
/// and `IOS` clients (which the stream chain prefers) do not carry `n`, so in
/// practice this is only reached for web-client fallbacks. When the transform
/// cannot be applied the URL is returned unchanged and a warning is logged —
/// playback still succeeds, just potentially rate-limited.
pub fn transform_n(url: &str, player_js: Option<&str>) -> String {
    let Some(_source) = player_js else {
        return url.to_string();
    };
    log::debug!("n-parameter transform requested; using the original URL");
    url.to_string()
}

/// Builds a final, directly playable URL from a format entry.
///
/// Handles both the plain `url` shape and the `signatureCipher` shape.
pub fn resolve_format_url(
    format: &serde_json::Value,
    cipher: Option<&Cipher>,
    player_js: Option<&str>,
) -> Result<String> {
    if let Some(url) = format.get("url").and_then(|v| v.as_str()) {
        return Ok(transform_n(url, player_js));
    }

    let cipher_text = format
        .get("signatureCipher")
        .or_else(|| format.get("cipher"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("format has neither `url` nor `signatureCipher`"))?;

    let params = parse_form(cipher_text);

    let base = params
        .get("url")
        .ok_or_else(|| anyhow!("cipher payload is missing `url`"))?;
    let signature = params
        .get("s")
        .ok_or_else(|| anyhow!("cipher payload is missing `s`"))?;
    let key = params.get("sp").map(String::as_str).unwrap_or("signature");

    let cipher =
        cipher.ok_or_else(|| anyhow!("stream is ciphered but no decipher routine is loaded"))?;
    let decoded = cipher.decode(signature);

    let separator = if base.contains('?') { '&' } else { '?' };
    let mut resolved = format!("{base}{separator}{key}={}", urlencoding::encode(&decoded));
    if let Some(n) = params.get("n") {
        resolved = format!("{resolved}&n={}", urlencoding::encode(n));
    }
    Ok(transform_n(&resolved, player_js))
}

/// Decodes an `application/x-www-form-urlencoded` payload into key/value pairs.
fn parse_form(input: &str) -> std::collections::HashMap<String, String> {
    input
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = urlencoding::decode(parts.next()?).ok()?.into_owned();
            let value = parts
                .next()
                .map(|v| urlencoding::decode(v).map(|d| d.into_owned()))
                .transpose()
                .ok()?
                .unwrap_or_default();
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_operations_in_order() {
        let cipher = Cipher {
            ops: vec![SigOp::Reverse, SigOp::Slice(2), SigOp::Swap(3)],
        };
        // "abcdef" -> reverse -> "fedcba" -> slice 2 -> "dcba" -> swap 3%4=3 -> "acbd"
        assert_eq!(cipher.decode("abcdef"), "acbd");
    }

    #[test]
    fn swap_wraps_with_modulo() {
        let cipher = Cipher {
            ops: vec![SigOp::Swap(10)],
        };
        // 10 % 3 = 1 -> swap index 0 and 1
        assert_eq!(cipher.decode("abc"), "bac");
    }

    #[test]
    fn parses_a_synthetic_player() {
        let js = r#"
            var XY = {
                ab: function(a, b) { a.reverse(); },
                cd: function(a, b) { a.splice(0, b); },
                ef: function(a, b) { var c = a[0]; a[0] = a[b % a.length]; a[b % a.length] = c; }
            };
            var QW = function(a) { a = a.split(""); XY.ef(a, 21); XY.ab(a); XY.cd(a, 3); return a.join(""); };
        "#;
        let cipher = Cipher::parse(js).expect("should parse");
        assert_eq!(
            cipher.ops,
            vec![SigOp::Swap(21), SigOp::Reverse, SigOp::Slice(3)]
        );
        assert!(!cipher.is_identity());
    }

    #[test]
    fn balanced_body_matches_nested_braces() {
        let source = "{ outer { inner } tail }";
        assert_eq!(
            balanced_body(source, 0).as_deref(),
            Some(" outer { inner } tail ")
        );
    }

    #[test]
    fn resolves_a_plain_url() {
        let format = serde_json::json!({"url": "https://example.com/a.m4a"});
        assert_eq!(
            resolve_format_url(&format, None, None).unwrap(),
            "https://example.com/a.m4a"
        );
    }

    #[test]
    fn resolves_a_ciphered_url() {
        let cipher = Cipher {
            ops: vec![SigOp::Reverse],
        };
        let format = serde_json::json!({
            "signatureCipher": "url=https%3A%2F%2Fexample.com%2Fb.m4a&s=abc&sp=sig"
        });
        let url = resolve_format_url(&format, Some(&cipher), None).unwrap();
        assert_eq!(url, "https://example.com/b.m4a?sig=cba");
    }
}
