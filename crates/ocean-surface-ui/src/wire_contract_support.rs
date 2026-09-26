//! Shared toolkit for the `*_wire_contract_tests.rs` modules that hold Surface
//! inside ocean-os's vendored wire contracts (session, voice, component,
//! observatory; room-wire is #225's `room_wire_contract_tests.rs`).
//!
//! The artifacts are byte-for-byte copies under
//! `tests/fixtures/ocean-os-<name>/`, refreshed only by
//! `node scripts/vendor-ocean-os-wire-contracts.mjs`. This module reads them,
//! and gives the tests three levers the binary crate otherwise lacks:
//!
//! - [`field_names`] / [`variant_names`]: a serde NAME PROBE. It asks a type's
//!   own `Deserialize` derive which wire names it knows, so a `rename` changes
//!   what the test sees, and a new field is checked without anyone listing it.
//! - [`Source`]: a comment- and literal-aware view of a Rust source file, for
//!   the facts that live inside a wasm-only async fn a host test cannot call
//!   (an inline `json!` body, a `match name { "x" => … }` dispatch).
//! - route helpers that compare a call site's path with a published
//!   `"METHOD /path/{id}"` route.
//!
//! The proxy crate includes this same file by `#[path]` for its route checks,
//! which is why it depends on nothing but `serde` and `serde_json`, and why it
//! is `allow(dead_code)`: each includer uses a different subset.
#![allow(dead_code)]

use serde::de::{self, DeserializeOwned, Visitor};
use serde_json::Value;
use std::collections::BTreeSet;

pub const SESSION_WIRE: &str =
    include_str!("../tests/fixtures/ocean-os-session-wire/session-wire.json");
pub const VOICE_WIRE: &str = include_str!("../tests/fixtures/ocean-os-voice-wire/voice-wire.json");
pub const COMPONENT_WIRE: &str =
    include_str!("../tests/fixtures/ocean-os-component-wire/component-wire.json");
pub const OBSERVATORY_WIRE: &str =
    include_str!("../tests/fixtures/ocean-os-observatory-wire/observatory-wire.json");

pub const REVENDOR: &str = "re-vendored with `node scripts/vendor-ocean-os-wire-contracts.mjs`";

pub fn session_wire() -> Value {
    parse("session-wire.json", SESSION_WIRE)
}
pub fn voice_wire() -> Value {
    parse("voice-wire.json", VOICE_WIRE)
}
pub fn component_wire() -> Value {
    parse("component-wire.json", COMPONENT_WIRE)
}
pub fn observatory_wire() -> Value {
    parse("observatory-wire.json", OBSERVATORY_WIRE)
}

fn parse(name: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|err| panic!("vendored {name} is not JSON: {err}"))
}

/// The value at a `/`-separated JSON pointer. Missing is a failure naming the
/// pointer: a contract key the vendored file stopped carrying must not make a
/// check pass vacuously.
pub fn at<'a>(contract: &'a Value, pointer: &str) -> &'a Value {
    contract
        .pointer(pointer)
        .unwrap_or_else(|| panic!("contract has no `{pointer}` ({REVENDOR}?)"))
}

/// A published string.
pub fn string(contract: &Value, pointer: &str) -> String {
    at(contract, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("contract `{pointer}` is not a string"))
        .to_string()
}

/// A published, non-empty list of strings.
pub fn strings(contract: &Value, pointer: &str) -> BTreeSet<String> {
    let values: BTreeSet<String> = at(contract, pointer)
        .as_array()
        .unwrap_or_else(|| panic!("contract `{pointer}` is not an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("contract `{pointer}` holds a non-string: {value}"))
                .to_string()
        })
        .collect();
    assert!(!values.is_empty(), "contract `{pointer}` is empty");
    values
}

/// Every name in `used` must be in `published`; the message names the strays
/// and the Surface site that sends or reads them.
pub fn assert_subset<'a>(
    what: &str,
    used: impl IntoIterator<Item = &'a str>,
    published: &BTreeSet<String>,
) {
    let strays: Vec<&str> = used
        .into_iter()
        .filter(|name| !published.contains(*name))
        .collect();
    assert!(
        strays.is_empty(),
        "{what} uses {strays:?}, which the vendored contract does not publish \
         (published: {published:?})"
    );
}

/// The top-level keys of a JSON object.
pub fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {value}"))
        .keys()
        .cloned()
        .collect()
}

/// A JSON object carrying exactly `names`, each set to `null`. With every
/// Surface field `Option`/`default`, a body of only published keys must decode.
pub fn object_with(names: &BTreeSet<String>, sample: impl Fn(&str) -> Value) -> Value {
    Value::Object(
        names
            .iter()
            .map(|name| (name.clone(), sample(name)))
            .collect(),
    )
}

// ---- serde name probe -------------------------------------------------------

/// The wire field names a struct's `Deserialize` derive knows, renames applied.
/// Panics for a type that is not a plain struct (a `flatten` makes serde ask
/// for a map, which hides the names — the ocean-os probe has the same limit).
pub fn field_names<T: DeserializeOwned>() -> BTreeSet<String> {
    let mut seen = None;
    let _ = T::deserialize(Probe { seen: &mut seen });
    match seen {
        Some(Probed::Struct(fields)) => fields.iter().map(|f| f.to_string()).collect(),
        other => panic!(
            "{} is not a plain struct to the probe ({other:?})",
            std::any::type_name::<T>()
        ),
    }
}

/// The wire variant names of an externally tagged or unit enum, renames applied.
pub fn variant_names<T: DeserializeOwned>() -> BTreeSet<String> {
    let mut seen = None;
    let _ = T::deserialize(Probe { seen: &mut seen });
    match seen {
        Some(Probed::Enum(variants)) => variants.iter().map(|v| v.to_string()).collect(),
        other => panic!(
            "{} is not an externally tagged enum to the probe ({other:?})",
            std::any::type_name::<T>()
        ),
    }
}

/// The variant names an internally or adjacently tagged enum knows, read off
/// the error its derive gives an unknown tag (the same move the ocean-os
/// contract tests make). `body` builds the probe object from a tag value.
pub fn tagged_variant_names<T: DeserializeOwned>(body: impl Fn(&str) -> Value) -> BTreeSet<String> {
    const TAG: &str = "__wire_contract_probe__";
    let err = match serde_json::from_value::<T>(body(TAG)) {
        Ok(_) => panic!(
            "{} accepted an unknown tag; it has a catch-all, so its names cannot be probed",
            std::any::type_name::<T>()
        ),
        Err(err) => err.to_string(),
    };
    let expected = err
        .split("expected one of ")
        .nth(1)
        .unwrap_or_else(|| panic!("unexpected probe error: {err}"));
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[derive(Debug)]
enum Probed {
    Struct(&'static [&'static str]),
    Enum(&'static [&'static str]),
}

struct Probe<'a> {
    seen: &'a mut Option<Probed>,
}

impl<'de> de::Deserializer<'de> for Probe<'_> {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom("probe"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        *self.seen = Some(Probed::Struct(fields));
        Err(de::Error::custom("probe"))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        *self.seen = Some(Probed::Enum(variants));
        Err(de::Error::custom("probe"))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map identifier ignored_any
    }
}

// ---- source scanning --------------------------------------------------------

/// A Rust source file with comments blanked and string/char literal contents
/// masked, so braces and `=>` inside prose or a format string cannot confuse a
/// scan. Offsets are shared with the original text, and every plain string
/// literal's span is recorded so its contents can be read back.
pub struct Source<'a> {
    pub text: &'a str,
    masked: Vec<u8>,
    /// (start, end) of each string literal's CONTENTS, quotes excluded.
    literals: Vec<(usize, usize)>,
}

impl<'a> Source<'a> {
    /// The half of a module a release build compiles: everything before the
    /// first top-level `#[cfg(test)]`. A test quoting a literal must not be
    /// what satisfies a scan.
    pub fn production(text: &'a str) -> Self {
        let cut = text.find("\n#[cfg(test)]").unwrap_or(text.len());
        Self::new(&text[..cut])
    }

    pub fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut masked = bytes.to_vec();
        let mut literals = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        masked[i] = b' ';
                        i += 1;
                    }
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    let end = text[i + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |e| i + 2 + e + 2);
                    masked[i..end]
                        .iter_mut()
                        .for_each(|b| *b = if *b == b'\n' { b'\n' } else { b' ' });
                    i = end;
                }
                b'r' if matches!(bytes.get(i + 1), Some(b'#') | Some(b'"'))
                    && (i == 0 || !is_ident(bytes[i - 1])) =>
                {
                    let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
                    if bytes.get(i + 1 + hashes) != Some(&b'"') {
                        i += 1;
                        continue;
                    }
                    let open = i + 1 + hashes + 1;
                    let close_pat = format!("\"{}", "#".repeat(hashes));
                    let close = text[open..]
                        .find(&close_pat)
                        .map_or(bytes.len(), |e| open + e);
                    masked[open..close].iter_mut().for_each(|b| *b = b'x');
                    i = close + close_pat.len();
                }
                b'"' => {
                    let open = i + 1;
                    let mut j = open;
                    while j < bytes.len() && bytes[j] != b'"' {
                        j += if bytes[j] == b'\\' { 2 } else { 1 };
                    }
                    let close = j.min(bytes.len());
                    masked[open..close].iter_mut().for_each(|b| *b = b'x');
                    literals.push((open, close));
                    i = close + 1;
                }
                b'\'' => {
                    // A char literal ('x', '\n', '{') or a lifetime ('a).
                    if bytes.get(i + 1) == Some(&b'\\') {
                        // Skip the escaped char itself, so `'\''` closes right.
                        let close = text[i + 3..].find('\'').map_or(bytes.len(), |e| i + 3 + e);
                        masked[i + 1..close].iter_mut().for_each(|b| *b = b'x');
                        i = close + 1;
                    } else if bytes.get(i + 2) == Some(&b'\'') {
                        masked[i + 1] = b'x';
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        Self {
            text,
            masked,
            literals,
        }
    }

    fn masked_str(&self) -> &str {
        std::str::from_utf8(&self.masked).expect("masking keeps UTF-8 boundaries")
    }

    /// Byte offset of the first `needle` that is not inside a comment. The
    /// needle may span a string literal (`"{}/v1/model"`, quotes included).
    pub fn find_code(&self, needle: &str) -> Option<usize> {
        self.text
            .match_indices(needle)
            .map(|(at, _)| at)
            .find(|&at| {
                self.text.as_bytes()[at..at + needle.len()]
                    .iter()
                    .zip(&self.masked[at..at + needle.len()])
                    .all(|(orig, masked)| orig == masked || *masked == b'x')
            })
    }

    /// The `{ … }` block that opens at the first `{` after `needle` in code,
    /// as (start, end) offsets of its contents.
    pub fn block_after(&self, needle: &str) -> (usize, usize) {
        let at = self
            .find_code(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found in production source"));
        let open = self.masked[at..]
            .iter()
            .position(|b| *b == b'{')
            .map(|p| at + p)
            .unwrap_or_else(|| panic!("no `{{` after `{needle}`"));
        let mut depth = 0usize;
        for (offset, byte) in self.masked[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (open + 1, open + offset);
                    }
                }
                _ => {}
            }
        }
        panic!("`{needle}`'s block is unterminated")
    }

    /// The original text of that block.
    pub fn body_after(&self, needle: &str) -> &'a str {
        let (start, end) = self.block_after(needle);
        &self.text[start..end]
    }

    fn literals_in(&self, span: (usize, usize)) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.literals
            .iter()
            .copied()
            .filter(move |(s, e)| *s >= span.0 && *e <= span.1)
    }

    fn next_code(&self, from: usize) -> &str {
        self.masked_str()[from..].trim_start()
    }

    /// The string literals that are MATCH-ARM patterns in the block after
    /// `needle`: a literal followed by `=>`, or by a single `|` of an
    /// alternation.
    pub fn match_arm_literals(&self, needle: &str) -> BTreeSet<String> {
        let span = self.block_after(needle);
        self.literals_in(span)
            .filter(|(_, end)| {
                let rest = self.next_code(end + 1);
                rest.starts_with("=>") || (rest.starts_with('|') && !rest.starts_with("||"))
            })
            .map(|(s, e)| self.text[s..e].to_string())
            .collect()
    }

    /// The top-level keys of the first `json!({ … })` after `needle`.
    pub fn json_body_keys_after(&self, needle: &str) -> BTreeSet<String> {
        let at = self
            .find_code(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found in production source"));
        let json_at = self.masked_str()[at..]
            .find("json!(")
            .map(|p| at + p)
            .unwrap_or_else(|| panic!("no json!( body after `{needle}`"));
        let span = self.block_after_offset(json_at);
        let mut keys = BTreeSet::new();
        let mut depth = 0usize;
        let mut literals = self.literals_in(span).peekable();
        let mut i = span.0;
        while i < span.1 {
            if let Some(&(s, e)) = literals.peek() {
                if s == i + 1 && self.masked[i] == b'"' {
                    if depth == 0 && self.next_code(e + 1).starts_with(':') {
                        keys.insert(self.text[s..e].to_string());
                    }
                    literals.next();
                    i = e + 1;
                    continue;
                }
            }
            match self.masked[i] {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            i += 1;
        }
        assert!(!keys.is_empty(), "json! body after `{needle}` has no keys");
        keys
    }

    fn block_after_offset(&self, at: usize) -> (usize, usize) {
        let needle_end = at + self.masked_str()[at..].find('{').expect("a `{`");
        let mut depth = 0usize;
        for (offset, byte) in self.masked[needle_end..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (needle_end + 1, needle_end + offset);
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated block at byte {at}")
    }

    /// The wire names of a struct declared in this source (`needle` is e.g.
    /// `"struct ModelsResponse"`): each field's identifier, or its
    /// `#[serde(rename = "…")]`. For the local structs an async fn declares,
    /// which a test cannot name.
    pub fn struct_fields(&self, needle: &str) -> BTreeSet<String> {
        let (start, end) = self.block_after(needle);
        let masked = &self.masked_str()[start..end];
        let original = &self.text[start..end];
        let mut fields = BTreeSet::new();
        let mut depth = 0i32;
        let mut piece_start = 0usize;
        let mut pieces = Vec::new();
        for (i, b) in masked.bytes().enumerate() {
            match b {
                b'<' | b'(' | b'[' | b'{' => depth += 1,
                b'>' | b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => {
                    pieces.push((piece_start, i));
                    piece_start = i + 1;
                }
                _ => {}
            }
        }
        pieces.push((piece_start, masked.len()));
        for (s, e) in pieces {
            let code = masked[s..e].trim();
            if code.is_empty() {
                continue;
            }
            let rename = original[s..e]
                .split("rename = \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_string);
            // Drop attributes, then visibility, then read `ident:`.
            let mut rest = code;
            while let Some(stripped) = rest.strip_prefix("#[") {
                let close = stripped.find(']').expect("attribute closes");
                rest = stripped[close + 1..].trim_start();
            }
            for vis in ["pub(crate) ", "pub "] {
                rest = rest.strip_prefix(vis).unwrap_or(rest).trim_start();
            }
            let ident: String = rest.chars().take_while(|c| is_ident(*c as u8)).collect();
            assert!(!ident.is_empty(), "cannot read a field out of `{code}`");
            fields.insert(rename.unwrap_or(ident));
        }
        fields
    }

    /// Every axum `.route("/path", get(…).post(…))` registration in the block
    /// after `needle`, as (METHOD, path) pairs in source order.
    pub fn axum_routes(&self, needle: &str) -> Vec<(String, String)> {
        let (start, end) = self.block_after(needle);
        let masked = self.masked_str();
        let mut routes = Vec::new();
        let mut from = start;
        while let Some(found) = masked[from..end].find(".route(") {
            let open = from + found + ".route".len();
            let mut depth = 0usize;
            let mut close = open;
            for (offset, byte) in self.masked[open..end].iter().enumerate() {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let (s, e) = self
                .literals_in((open, close))
                .next()
                .unwrap_or_else(|| panic!("a .route( with no path literal at byte {open}"));
            let path = self.text[s..e].to_string();
            let args = &masked[e..close];
            for (verb, method) in [
                ("get(", "GET"),
                ("post(", "POST"),
                ("put(", "PUT"),
                ("patch(", "PATCH"),
                ("delete(", "DELETE"),
            ] {
                for (at, _) in args.match_indices(verb) {
                    let before = args[..at].bytes().last();
                    if !before.is_some_and(is_ident) {
                        routes.push((method.to_string(), path.clone()));
                    }
                }
            }
            from = close;
        }
        routes
    }

    /// Every string literal inside the block after `needle`.
    pub fn literals_after(&self, needle: &str) -> BTreeSet<String> {
        let span = self.block_after(needle);
        self.literals_in(span)
            .map(|(s, e)| self.text[s..e].to_string())
            .collect()
    }

    /// Every string literal in the file that contains `/v1/`, with its offset.
    pub fn v1_literals(&self) -> Vec<(usize, String)> {
        self.literals
            .iter()
            .filter_map(|&(s, e)| {
                let lit = &self.text[s..e];
                lit.contains("/v1/").then(|| (s, lit.to_string()))
            })
            .collect()
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ---- routes -----------------------------------------------------------------

/// A published `"METHOD /path"` route, split.
pub fn split_route(route: &str) -> (String, String) {
    let (method, path) = route
        .split_once(' ')
        .unwrap_or_else(|| panic!("route `{route}` is not `METHOD /path`"));
    (method.to_string(), path.to_string())
}

/// A path with every `{…}` segment collapsed to `{}` and any query dropped, so
/// a call site's `format!("{}/v1/sessions/{}", …)` and a published
/// `/v1/sessions/{id}` compare equal.
pub fn path_shape(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    // A call site's leading `{}` / `{base}` is the daemon origin, not a segment.
    let path = match path.find("/v1/").or_else(|| path.find("/health")) {
        Some(start) => &path[start..],
        None => path,
    };
    path.split('/')
        .map(|segment| if segment.contains('{') { "{}" } else { segment })
        .collect::<Vec<_>>()
        .join("/")
}

/// Every published route in the four vendored contracts, as (METHOD, shape).
pub fn published_routes() -> BTreeSet<(String, String)> {
    let mut routes = BTreeSet::new();
    let mut add = |route: &str| {
        let (method, path) = split_route(route);
        routes.insert((method, path_shape(&path)));
    };
    for contract in [session_wire(), voice_wire(), observatory_wire()] {
        collect_routes(&contract, &mut add);
    }
    routes
}

/// Walks a contract for every string that reads as `"METHOD /…"`: the value of
/// a `route`/`*_route` key, a `routes` list entry, or a `statuses` map key.
fn collect_routes(value: &Value, add: &mut impl FnMut(&str)) {
    let is_route = |s: &str| {
        ["GET /", "POST /", "PATCH /", "PUT /", "DELETE /"]
            .iter()
            .any(|prefix| s.starts_with(prefix))
    };
    match value {
        Value::String(s) if is_route(s) => add(s),
        Value::Array(items) => items.iter().for_each(|item| collect_routes(item, add)),
        Value::Object(map) => {
            for (key, item) in map {
                if is_route(key) {
                    add(key);
                }
                collect_routes(item, add);
            }
        }
        _ => {}
    }
}
