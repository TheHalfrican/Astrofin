//! `cargo xtask test-ratio` — the "one test per public function" measurement
//! described in `docs/test-plan.md` §2.
//!
//! # What is walked
//!
//! * Every workspace member listed in `src/Cargo.toml`: `<member>/src/**/*.rs`,
//!   `<member>/build.rs` and `<member>/tests/*.rs`. Nothing outside a member
//!   directory is entered, so `src/target` and `third_party/` are never seen.
//! * `src/web/*.js` (minus `*.test.js`) and `dev/tools/**/*.mjs` (minus
//!   `*.test.mjs`). They are reported as the pseudo-crates `web` and
//!   `dev-tools`.
//!
//! # Rust counting rules
//!
//! * A **public function** is any `fn` item whose visibility token is `pub` or
//!   `pub(..)`, at any nesting depth (free functions, functions in nested
//!   modules, inherent `impl` methods), plus **every** `fn` inside an
//!   `impl <Trait> for <Type>` block — a trait impl is public surface even
//!   though its methods carry no visibility token.
//! * Trait *declarations* (`trait T { fn f(); }`) are not counted; the
//!   testable surface is the impl that provides the body.
//! * Nothing inside a `#[cfg(test)]` module, inside a `#[test]` function or
//!   carrying `#[cfg(test)]` itself counts as public, and `tests/*.rs` files
//!   never contribute public functions.
//! * A **test function** is any `fn` carrying an attribute whose path ends in
//!   `test` (`#[test]`, `#[tokio::test]`, `#[async_std::test]`), anywhere in
//!   the file including inside `#[cfg(test)]` modules.
//! * **Integration-test attribution:** the tests in `<member>/tests/foo.rs`
//!   are added to `<member>/src/**/foo.rs` when exactly one source file with
//!   that stem exists. Otherwise the integration file keeps a row of its own
//!   (0 public functions, N tests) so its tests still reach the total.
//!
//! Parsing is done with `syn` — never with a grep — so `fn` inside strings,
//! comments or `macro_rules!` bodies cannot be miscounted.
//!
//! # JS counting rules and their limits
//!
//! JS is scanned with a hand-written line tokenizer, not a parser. The
//! **exported names** of a file are the union of
//!
//! * the keys of the object literal assigned to `module.exports` (directly, or
//!   through a `var`/`let`/`const` binding defined in the same file — the
//!   `var api = { .. }; module.exports = api;` shape every tested module uses),
//!   plus any `module.exports.<name> =` / `exports.<name> =` assignment;
//! * ESM `export function|class|const|let|var <name>` and the names in
//!   `export { a, b as c }`;
//! * column-0 `function <name>(` / `async function <name>(` declarations;
//! * `window.<name> = <rhs>` where `<rhs>` is a function expression, an arrow
//!   function, an IIFE, an object literal, or a bare identifier that the file
//!   declares as a `function` or `class`.
//!
//! Known limits: names that appear inside comments or string literals in one
//! of those shapes are counted; every key of an exported object literal counts
//! whether or not its value is callable; and the methods of an object assigned
//! to `window` count as the single name of that object, not one apiece. Test
//! cases are `test(` calls in the sibling `<name>.test.js`, counted the same
//! crude way.
//!
//! # Exemptions
//!
//! `dev/test-exempt.txt` lists platform/FFI glue that cannot run without a
//! display server, a GPU or a live CEF/mpv process: one repo-relative path or
//! glob per line, `#` comments, an optional `| reason` suffix, and `!` to
//! negate (a negated pattern wins over every positive one, whatever the
//! order). Exempt files are reported in their own section and excluded from
//! the totals.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::TestRatioArgs;
use crate::paths;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// Public-function and test-function counts for one file or one crate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub public_fns: usize,
    pub tests: usize,
}

/// One walked source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// Repo-relative, forward slashes.
    pub path: String,
    /// Workspace member name, or `web` / `dev-tools`.
    pub krate: String,
    pub counts: Counts,
    /// `Some(reason)` when `dev/test-exempt.txt` matched this path.
    pub exempt: Option<String>,
}

/// One row of the per-crate summary (exempt files already removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateStat {
    pub name: String,
    pub files: usize,
    pub counts: Counts,
}

/// The whole measurement.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub files: Vec<FileStat>,
}

impl Report {
    /// Per-crate rows for the non-exempt files, in the order the crates were
    /// first walked.
    pub fn crates(&self) -> Vec<CrateStat> {
        let mut out: Vec<CrateStat> = Vec::new();
        for file in self.files.iter().filter(|f| f.exempt.is_none()) {
            if let Some(row) = out.iter_mut().find(|c| c.name == file.krate) {
                row.files += 1;
                row.counts.public_fns += file.counts.public_fns;
                row.counts.tests += file.counts.tests;
            } else {
                out.push(CrateStat {
                    name: file.krate.clone(),
                    files: 1,
                    counts: file.counts,
                });
            }
        }
        out
    }

    /// Totals over the non-exempt files.
    pub fn totals(&self) -> Counts {
        let mut total = Counts::default();
        for file in self.files.iter().filter(|f| f.exempt.is_none()) {
            total.public_fns += file.counts.public_fns;
            total.tests += file.counts.tests;
        }
        total
    }

    /// The exempt files, in walk order.
    pub fn exempt(&self) -> Vec<&FileStat> {
        self.files.iter().filter(|f| f.exempt.is_some()).collect()
    }
}

/// `tests / public_fns`, defined as `0.0` when there are no public functions.
pub fn ratio(tests: usize, public_fns: usize) -> f64 {
    if public_fns == 0 {
        0.0
    } else {
        tests as f64 / public_fns as f64
    }
}

// ---------------------------------------------------------------------------
// Exemption list
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Matcher {
    /// `some/dir/**` — matches everything under `some/dir/`.
    Prefix(String),
    Glob(glob::Pattern),
}

#[derive(Debug)]
struct ExemptRule {
    matcher: Matcher,
    negate: bool,
    reason: String,
}

/// Parsed `dev/test-exempt.txt`.
#[derive(Debug, Default)]
pub struct ExemptList {
    rules: Vec<ExemptRule>,
}

const GLOB_OPTS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

impl ExemptList {
    /// The reason this repo-relative path is exempt, if it is. A `!` rule
    /// beats every positive rule regardless of order.
    pub fn reason(&self, path: &str) -> Option<&str> {
        let mut hit: Option<&str> = None;
        for rule in &self.rules {
            let matched = match &rule.matcher {
                Matcher::Prefix(prefix) => path.starts_with(prefix.as_str()),
                Matcher::Glob(pattern) => pattern.matches_with(path, GLOB_OPTS),
            };
            if !matched {
                continue;
            }
            if rule.negate {
                return None;
            }
            if hit.is_none() {
                hit = Some(rule.reason.as_str());
            }
        }
        hit
    }
}

/// Parse the `dev/test-exempt.txt` format.
pub fn parse_exempt_list(text: &str) -> Result<ExemptList> {
    let mut rules = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (pattern, reason) = match line.split_once('|') {
            Some((p, r)) => (p.trim(), r.trim().to_owned()),
            None => (line, String::new()),
        };
        let (negate, pattern) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, pattern),
        };
        if pattern.is_empty() {
            continue;
        }
        let matcher = match pattern.strip_suffix("/**") {
            Some(prefix) => Matcher::Prefix(format!("{prefix}/")),
            None => Matcher::Glob(
                glob::Pattern::new(pattern)
                    .with_context(|| format!("bad exemption glob `{pattern}`"))?,
            ),
        };
        rules.push(ExemptRule {
            matcher,
            negate,
            reason,
        });
    }
    Ok(ExemptList { rules })
}

/// Parse `dev/test-ratio-floor.txt`: a single decimal, `#` comments allowed.
pub fn parse_floor(text: &str) -> Result<f64> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return line
            .parse::<f64>()
            .with_context(|| format!("floor `{line}` is not a decimal"));
    }
    Err(anyhow!("no floor value found"))
}

// ---------------------------------------------------------------------------
// Workspace manifest
// ---------------------------------------------------------------------------

/// The `members = [..]` entries of a workspace `Cargo.toml`.
///
/// Deliberately a tiny scanner rather than a TOML dependency: it reads the
/// first `members` array of the `[workspace]` table and takes every quoted
/// string in it.
pub fn parse_workspace_members(manifest: &str) -> Vec<String> {
    let mut in_workspace = false;
    let mut collecting = false;
    let mut members = Vec::new();
    for raw in manifest.lines() {
        let line = raw.trim();
        if !collecting {
            if line.starts_with('[') {
                in_workspace = line == "[workspace]";
                continue;
            }
            if !in_workspace {
                continue;
            }
            let Some(rest) = line.strip_prefix("members") else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            collecting = true;
            push_quoted(rest, &mut members);
            if rest.contains(']') {
                break;
            }
            continue;
        }
        push_quoted(line, &mut members);
        if line.contains(']') {
            break;
        }
    }
    members
}

fn push_quoted(line: &str, out: &mut Vec<String>) {
    let mut rest = line;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
}

// ---------------------------------------------------------------------------
// Rust counting
// ---------------------------------------------------------------------------

struct Counter {
    counts: Counts,
    /// Inside `#[cfg(test)]` or inside a test function.
    in_test_cfg: bool,
    /// Inside `impl <Trait> for <Type>`.
    in_trait_impl: bool,
    /// The file is a `tests/*.rs` integration test: no public surface.
    integration: bool,
}

impl Counter {
    fn record(&mut self, attrs: &[syn::Attribute], public: bool) -> bool {
        if is_test_attr(attrs) {
            self.counts.tests += 1;
            return true;
        }
        if public && !self.integration && !self.in_test_cfg && !has_cfg_test(attrs) {
            self.counts.public_fns += 1;
        }
        false
    }
}

impl<'ast> syn::visit::Visit<'ast> for Counter {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let prev = self.in_test_cfg;
        self.in_test_cfg |= has_cfg_test(&node.attrs);
        syn::visit::visit_item_mod(self, node);
        self.in_test_cfg = prev;
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let prev_cfg = self.in_test_cfg;
        let prev_impl = self.in_trait_impl;
        self.in_test_cfg |= has_cfg_test(&node.attrs);
        self.in_trait_impl = node.trait_.is_some();
        syn::visit::visit_item_impl(self, node);
        self.in_test_cfg = prev_cfg;
        self.in_trait_impl = prev_impl;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let is_test = self.record(&node.attrs, is_public(&node.vis));
        let prev_cfg = self.in_test_cfg;
        let prev_impl = self.in_trait_impl;
        self.in_test_cfg |= is_test || has_cfg_test(&node.attrs);
        self.in_trait_impl = false;
        syn::visit::visit_item_fn(self, node);
        self.in_test_cfg = prev_cfg;
        self.in_trait_impl = prev_impl;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let public = self.in_trait_impl || is_public(&node.vis);
        let is_test = self.record(&node.attrs, public);
        let prev_cfg = self.in_test_cfg;
        let prev_impl = self.in_trait_impl;
        self.in_test_cfg |= is_test || has_cfg_test(&node.attrs);
        self.in_trait_impl = false;
        syn::visit::visit_impl_item_fn(self, node);
        self.in_test_cfg = prev_cfg;
        self.in_trait_impl = prev_impl;
    }
}

fn is_public(vis: &syn::Visibility) -> bool {
    matches!(
        vis,
        syn::Visibility::Public(_) | syn::Visibility::Restricted(_)
    )
}

/// Any attribute whose path ends in `test`: `#[test]`, `#[tokio::test]`, ...
fn is_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "test")
    })
}

fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let syn::Meta::List(list) = &attr.meta else {
            return false;
        };
        list.tokens
            .to_string()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == "test")
    })
}

/// Count the public and test functions of one Rust file.
///
/// `integration` marks a `tests/*.rs` file, which contributes tests but never
/// public functions.
pub fn count_rust(source: &str, integration: bool) -> Result<Counts> {
    let file = syn::parse_file(source).map_err(|e| anyhow!("{e}"))?;
    let mut counter = Counter {
        counts: Counts::default(),
        in_test_cfg: false,
        in_trait_impl: false,
        integration,
    };
    syn::visit::Visit::visit_file(&mut counter, &file);
    Ok(counter.counts)
}

// ---------------------------------------------------------------------------
// JS counting
// ---------------------------------------------------------------------------

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Read the identifier starting at `at`, returning it and the index after it.
fn read_ident(bytes: &[u8], at: usize) -> Option<(String, usize)> {
    if at >= bytes.len() || !is_ident_start(bytes[at]) {
        return None;
    }
    let mut end = at;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    String::from_utf8(bytes[at..end].to_vec())
        .ok()
        .map(|s| (s, end))
}

fn skip_spaces(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && (bytes[at] == b' ' || bytes[at] == b'\t') {
        at += 1;
    }
    at
}

/// `function <name>` / `class <name>` declarations anywhere in the file, used
/// to decide whether `window.x = y;` is exporting a callable.
fn js_callables(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        for kw in ["function", "class"] {
            let Some(rest) = trimmed.strip_prefix(kw) else {
                continue;
            };
            let rest = rest.trim_start().trim_start_matches('*').trim_start();
            if let Some((name, _)) = read_ident(rest.as_bytes(), 0) {
                out.insert(name);
            }
        }
    }
    out
}

fn fn_decl_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("async ").map_or(line, str::trim_start);
    let rest = rest.strip_prefix("function")?;
    let rest = rest.trim_start().trim_start_matches('*').trim_start();
    let (name, after) = read_ident(rest.as_bytes(), 0)?;
    // A declaration, not `function name` inside an expression: `(` must follow.
    let after = skip_spaces(rest.as_bytes(), after);
    (rest.as_bytes().get(after) == Some(&b'(')).then_some(name)
}

/// `window.<name> = <rhs>` on this line, as `(name, rhs)`.
fn window_assign(line: &str) -> Option<(String, &str)> {
    let bytes = line.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = line[from..].find("window.") {
        let at = from + rel;
        let before_ok = at == 0 || !is_ident_char(bytes[at - 1]);
        let name_at = at + "window.".len();
        if before_ok && let Some((name, after)) = read_ident(bytes, name_at) {
            let eq = skip_spaces(bytes, after);
            if bytes.get(eq) == Some(&b'=') && bytes.get(eq + 1) != Some(&b'=') {
                return Some((name, line[eq + 1..].trim()));
            }
        }
        from = at + "window.".len();
    }
    None
}

fn rhs_is_callable(rhs: &str, callables: &BTreeSet<String>) -> bool {
    let rhs = rhs.trim();
    if rhs.starts_with("function")
        || rhs.starts_with("async ")
        || rhs.starts_with('(')
        || rhs.starts_with('{')
        || rhs.starts_with("class")
    {
        return true;
    }
    let bare = rhs.trim_end_matches(';').trim();
    callables.contains(bare)
}

/// The keys of the object literal whose `{` is at `open`, at nesting depth 1.
fn object_literal_keys(source: &str, open: usize) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0i32;
    let mut expect_key = false;
    let mut i = open;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'{' | b'[' | b'(' => {
                depth += 1;
                expect_key = c == b'{' && depth == 1;
                i += 1;
            }
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                i += 1;
            }
            b',' => {
                expect_key = depth == 1;
                i += 1;
            }
            b'\'' | b'"' | b'`' => {
                let (text, next) = read_js_string(bytes, i);
                if expect_key && depth == 1 {
                    keys.push(text);
                    expect_key = false;
                }
                i = next;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = source[i..].find('\n').map_or(bytes.len(), |n| i + n + 1);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = source[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
            }
            _ if is_ident_start(c) => {
                let Some((name, next)) = read_ident(bytes, i) else {
                    i += 1;
                    continue;
                };
                if expect_key && depth == 1 {
                    keys.push(name);
                    expect_key = false;
                }
                i = next;
            }
            _ => i += 1,
        }
    }
    keys
}

/// Read the string literal starting at the quote `at`; returns its contents
/// and the index after the closing quote.
fn read_js_string(bytes: &[u8], at: usize) -> (String, usize) {
    let quote = bytes[at];
    let mut i = at + 1;
    let start = i;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            break;
        }
        i += 1;
    }
    let end = i.min(bytes.len());
    let text = String::from_utf8(bytes[start..end].to_vec()).unwrap_or_default();
    (text, (end + 1).min(bytes.len()))
}

/// The exported names of a JS/MJS module. See the module docs for the rules
/// and their limits.
pub fn js_exported_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let callables = js_callables(source);

    for line in source.lines() {
        let trimmed = line.trim_start();

        if let Some(rest) = trimmed.strip_prefix("export ") {
            let rest = rest.trim_start();
            if let Some(name) = fn_decl_name(rest) {
                names.insert(name);
            } else if let Some(list) = rest.strip_prefix('{').and_then(|r| r.split('}').next()) {
                for part in list.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let last = part.split_whitespace().last().unwrap_or(part);
                    names.insert(last.to_owned());
                }
            } else if rest.starts_with("default") {
                names.insert("default".to_owned());
            } else {
                for kw in ["const ", "let ", "var ", "class ", "function "] {
                    if let Some(tail) = rest.strip_prefix(kw)
                        && let Some((name, _)) = read_ident(tail.trim_start().as_bytes(), 0)
                    {
                        names.insert(name);
                        break;
                    }
                }
            }
        }

        // Column-0 declarations only: anything indented lives inside an IIFE
        // or a block and is module-private.
        if !line.starts_with([' ', '\t'])
            && let Some(name) = fn_decl_name(line)
        {
            names.insert(name);
        }

        if let Some((name, rhs)) = window_assign(line)
            && rhs_is_callable(rhs, &callables)
        {
            names.insert(name);
        }
    }

    names.extend(module_exports_names(source));
    names
}

fn module_exports_names(source: &str) -> BTreeSet<String> {
    let bytes = source.as_bytes();
    let mut names = BTreeSet::new();
    for prefix in ["module.exports", "exports"] {
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(prefix) {
            let at = from + rel;
            from = at + prefix.len();
            if at > 0 && (is_ident_char(bytes[at - 1]) || bytes[at - 1] == b'.') {
                continue;
            }
            let mut i = skip_spaces(bytes, at + prefix.len());
            if bytes.get(i) == Some(&b'.') {
                let Some((name, after)) = read_ident(bytes, i + 1) else {
                    continue;
                };
                let eq = skip_spaces(bytes, after);
                if bytes.get(eq) == Some(&b'=') && bytes.get(eq + 1) != Some(&b'=') {
                    names.insert(name);
                }
                continue;
            }
            if bytes.get(i) != Some(&b'=') || bytes.get(i + 1) == Some(&b'=') {
                continue;
            }
            i = skip_spaces(bytes, i + 1);
            match bytes.get(i) {
                Some(&b'{') => names.extend(object_literal_keys(source, i)),
                Some(&c) if is_ident_start(c) => {
                    if let Some((binding, _)) = read_ident(bytes, i) {
                        names.extend(binding_object_keys(source, &binding));
                    }
                }
                _ => {}
            }
        }
    }
    names
}

/// Keys of `var <binding> = { .. }` / `const` / `let` defined in this file.
fn binding_object_keys(source: &str, binding: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    for kw in ["var ", "const ", "let "] {
        let needle = format!("{kw}{binding}");
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(&needle) {
            let at = from + rel;
            from = at + needle.len();
            let after = at + needle.len();
            if bytes.get(after).is_some_and(|c| is_ident_char(*c)) {
                continue;
            }
            let eq = skip_spaces(bytes, after);
            if bytes.get(eq) != Some(&b'=') {
                continue;
            }
            let brace = skip_spaces(bytes, eq + 1);
            if bytes.get(brace) == Some(&b'{') {
                return object_literal_keys(source, brace);
            }
        }
    }
    Vec::new()
}

/// `test(` calls in a `node:test` file.
pub fn count_js_tests(source: &str) -> usize {
    let bytes = source.as_bytes();
    let mut count = 0usize;
    let mut from = 0usize;
    while let Some(rel) = source[from..].find("test(") {
        let at = from + rel;
        from = at + "test(".len();
        let before_ok = at == 0 || (!is_ident_char(bytes[at - 1]) && bytes[at - 1] != b'.');
        if before_ok {
            count += 1;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn files_with_ext(dir: &Path, ext: &str, recursive: bool) -> Vec<PathBuf> {
    let mut walk = walkdir::WalkDir::new(dir).sort_by_file_name();
    if !recursive {
        walk = walk.max_depth(1);
    }
    walk.into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect()
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Walk the repo at `root` and count everything.
pub fn collect(root: &Path, exempt: &ExemptList) -> Result<Report> {
    let src_root = root.join("src");
    let manifest = read(&src_root.join("Cargo.toml"))?;
    let mut files: Vec<FileStat> = Vec::new();

    for member in parse_workspace_members(&manifest) {
        let crate_dir = src_root.join(&member);
        if !crate_dir.is_dir() {
            continue;
        }
        let mut rust: Vec<PathBuf> = files_with_ext(&crate_dir.join("src"), "rs", true);
        let build_rs = crate_dir.join("build.rs");
        if build_rs.is_file() {
            rust.push(build_rs);
        }

        let mut stats: Vec<FileStat> = Vec::new();
        for path in &rust {
            let text = read(path)?;
            let counts =
                count_rust(&text, false).with_context(|| format!("parse {}", rel(root, path)))?;
            stats.push(FileStat {
                path: rel(root, path),
                krate: member.clone(),
                counts,
                exempt: None,
            });
        }

        for path in files_with_ext(&crate_dir.join("tests"), "rs", false) {
            let text = read(&path)?;
            let counts =
                count_rust(&text, true).with_context(|| format!("parse {}", rel(root, &path)))?;
            let suffix = format!("/{}.rs", stem_of(&path));
            let hits: Vec<usize> = stats
                .iter()
                .enumerate()
                .filter(|(_, s)| s.path.ends_with(&suffix))
                .map(|(i, _)| i)
                .collect();
            if let [only] = hits[..] {
                stats[only].counts.tests += counts.tests;
            } else {
                stats.push(FileStat {
                    path: rel(root, &path),
                    krate: member.clone(),
                    counts,
                    exempt: None,
                });
            }
        }
        files.extend(stats);
    }

    files.extend(collect_js(root, &src_root.join("web"), "web", "js", false)?);
    files.extend(collect_js(
        root,
        &root.join("dev").join("tools"),
        "dev-tools",
        "mjs",
        true,
    )?);

    for file in &mut files {
        file.exempt = exempt.reason(&file.path).map(str::to_owned);
    }
    Ok(Report { files })
}

fn collect_js(
    root: &Path,
    dir: &Path,
    krate: &str,
    ext: &str,
    recursive: bool,
) -> Result<Vec<FileStat>> {
    let test_suffix = format!(".test.{ext}");
    let mut out = Vec::new();
    for path in files_with_ext(dir, ext, recursive) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(&test_suffix) {
            continue;
        }
        let text = read(&path)?;
        let test_path = path.with_file_name(format!("{}{test_suffix}", stem_of(&path)));
        let tests = if test_path.is_file() {
            count_js_tests(&read(&test_path)?)
        } else {
            0
        };
        out.push(FileStat {
            path: rel(root, &path),
            krate: krate.to_owned(),
            counts: Counts {
                public_fns: js_exported_names(&text).len(),
                tests,
            },
            exempt: None,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn fmt_ratio(counts: Counts) -> String {
    if counts.public_fns == 0 {
        "-".to_owned()
    } else {
        format!("{:.2}", ratio(counts.tests, counts.public_fns))
    }
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.len());
            }
        }
    }
    let mut out = String::new();
    let render = |out: &mut String, cells: &[String]| {
        for (i, cell) in cells.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(0);
            if i == 0 {
                out.push_str(&format!("{cell:<w$}"));
            } else {
                out.push_str(&format!("  {cell:>w$}"));
            }
        }
        out.push('\n');
    };
    let head: Vec<String> = headers.iter().map(|h| (*h).to_owned()).collect();
    render(&mut out, &head);
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    render(&mut out, &rule);
    for row in rows {
        render(&mut out, row);
    }
    out
}

/// The human-readable report.
pub fn render_text(report: &Report, show_files: bool) -> String {
    let mut out = String::new();

    if show_files {
        let rows: Vec<Vec<String>> = report
            .files
            .iter()
            .filter(|f| f.exempt.is_none())
            .map(|f| {
                vec![
                    f.path.clone(),
                    f.counts.public_fns.to_string(),
                    f.counts.tests.to_string(),
                    fmt_ratio(f.counts),
                ]
            })
            .collect();
        out.push_str(&table(&["file", "pub fns", "tests", "ratio"], &rows));
        out.push('\n');
    }

    let crates = report.crates();
    let mut rows: Vec<Vec<String>> = crates
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.files.to_string(),
                c.counts.public_fns.to_string(),
                c.counts.tests.to_string(),
                fmt_ratio(c.counts),
            ]
        })
        .collect();
    let totals = report.totals();
    let total_files: usize = crates.iter().map(|c| c.files).sum();
    rows.push(vec![
        "TOTAL".to_owned(),
        total_files.to_string(),
        totals.public_fns.to_string(),
        totals.tests.to_string(),
        fmt_ratio(totals),
    ]);
    out.push_str(&table(
        &["crate", "files", "pub fns", "tests", "ratio"],
        &rows,
    ));

    let exempt = report.exempt();
    out.push_str(&format!(
        "\nexempt ({} files, excluded from the totals)\n",
        exempt.len()
    ));
    for file in &exempt {
        out.push_str(&format!(
            "  {}  {}\n",
            file.path,
            file.exempt.as_deref().unwrap_or("")
        ));
    }

    out.push_str(&format!(
        "\ntotal: {}/{} = {:.2}\n",
        totals.tests,
        totals.public_fns,
        ratio(totals.tests, totals.public_fns)
    ));
    out
}

/// The same report as JSON.
pub fn render_json(report: &Report) -> Result<String> {
    let totals = report.totals();
    let value = serde_json::json!({
        "crates": report.crates().iter().map(|c| serde_json::json!({
            "crate": c.name,
            "files": c.files,
            "public_fns": c.counts.public_fns,
            "tests": c.counts.tests,
            "ratio": ratio(c.counts.tests, c.counts.public_fns),
        })).collect::<Vec<_>>(),
        "files": report.files.iter().filter(|f| f.exempt.is_none()).map(|f| serde_json::json!({
            "path": f.path,
            "crate": f.krate,
            "public_fns": f.counts.public_fns,
            "tests": f.counts.tests,
        })).collect::<Vec<_>>(),
        "exempt": report.exempt().iter().map(|f| serde_json::json!({
            "path": f.path,
            "crate": f.krate,
            "reason": f.exempt,
            "public_fns": f.counts.public_fns,
            "tests": f.counts.tests,
        })).collect::<Vec<_>>(),
        "total": {
            "files": report.files.iter().filter(|f| f.exempt.is_none()).count(),
            "public_fns": totals.public_fns,
            "tests": totals.tests,
            "ratio": ratio(totals.tests, totals.public_fns),
        },
    });
    serde_json::to_string_pretty(&value).map_err(|e| anyhow!("serialize report: {e}"))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// `cargo xtask test-ratio`.
pub fn run(args: &TestRatioArgs) -> Result<()> {
    let root = paths::repo_root().as_path();
    let exempt_path = root.join("dev").join("test-exempt.txt");
    let exempt = if exempt_path.is_file() {
        parse_exempt_list(&read(&exempt_path)?)?
    } else {
        ExemptList::default()
    };

    let report = collect(root, &exempt)?;
    if args.json {
        println!("{}", render_json(&report)?);
    } else {
        print!("{}", render_text(&report, args.files));
    }

    if args.check {
        let floor_path = root.join("dev").join("test-ratio-floor.txt");
        let floor = parse_floor(&read(&floor_path)?)?;
        let totals = report.totals();
        let actual = ratio(totals.tests, totals.public_fns);
        if actual + 1e-9 < floor {
            return Err(anyhow!(
                "test ratio {actual:.2} is below the floor {floor:.2} in {}",
                floor_path.display()
            ));
        }
        if !args.json {
            println!("check: {actual:.2} >= floor {floor:.2}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn stat(path: &str, krate: &str, public_fns: usize, tests: usize) -> FileStat {
        FileStat {
            path: path.to_owned(),
            krate: krate.to_owned(),
            counts: Counts { public_fns, tests },
            exempt: None,
        }
    }

    fn sample_report() -> Report {
        let mut exempted = stat("src/windows/src/lib.rs", "windows", 9, 0);
        exempted.exempt = Some("win32 glue".to_owned());
        Report {
            files: vec![
                stat("src/color/src/lib.rs", "color", 4, 8),
                stat("src/color/src/hsl.rs", "color", 2, 0),
                stat("src/web/ab-loop.js", "web", 3, 36),
                exempted,
            ],
        }
    }

    #[test]
    fn ratio_is_zero_when_nothing_is_public() {
        assert!((ratio(0, 0) - 0.0).abs() < f64::EPSILON);
        assert!((ratio(7, 0) - 0.0).abs() < f64::EPSILON);
        assert!((ratio(3, 6) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn crates_group_non_exempt_files_in_walk_order() {
        let crates = sample_report().crates();
        assert_eq!(crates.len(), 2, "the exempt windows file must not appear");
        assert_eq!(crates[0].name, "color");
        assert_eq!(crates[0].files, 2);
        assert_eq!(
            crates[0].counts,
            Counts {
                public_fns: 6,
                tests: 8
            }
        );
        assert_eq!(crates[1].name, "web");
    }

    #[test]
    fn totals_exclude_exempt_files() {
        assert_eq!(
            sample_report().totals(),
            Counts {
                public_fns: 9,
                tests: 44
            }
        );
    }

    #[test]
    fn exempt_lists_only_the_exempt_files() {
        let report = sample_report();
        let exempt = report.exempt();
        assert_eq!(exempt.len(), 1);
        assert_eq!(exempt[0].path, "src/windows/src/lib.rs");
    }

    #[test]
    fn workspace_members_are_read_from_the_array() {
        let manifest = "\
[workspace]\n\
resolver = \"3\"\n\
members = [\n    \"color\",\n    \"mpv\", # trailing comment\n]\n\
\n[workspace.package]\nversion = \"0.1.0\"\nmembers = [\"not-this\"]\n";
        assert_eq!(parse_workspace_members(manifest), vec!["color", "mpv"]);
    }

    #[test]
    fn workspace_members_on_one_line_are_read_too() {
        let manifest = "[workspace]\nmembers = [\"a\", \"b\"]\n";
        assert_eq!(parse_workspace_members(manifest), vec!["a", "b"]);
    }

    #[test]
    fn exempt_list_parses_globs_reasons_and_negations() {
        let list = parse_exempt_list(
            "# comment\n\
             \n\
             src/wayland/** | needs a compositor\n\
             src/windows/** | win32 glue\n\
             !src/windows/src/file_dialog.rs | has unit tests\n\
             **/build.rs | build script\n\
             src/xtask/src/platform_*.rs | shells out\n",
        )
        .unwrap();
        assert_eq!(
            list.reason("src/wayland/src/scene/sink.rs"),
            Some("needs a compositor")
        );
        assert_eq!(list.reason("src/windows/src/input.rs"), Some("win32 glue"));
        assert_eq!(list.reason("src/windows/src/file_dialog.rs"), None);
        assert_eq!(list.reason("src/mpv/build.rs"), Some("build script"));
        assert_eq!(
            list.reason("src/xtask/src/platform_windows.rs"),
            Some("shells out")
        );
        assert_eq!(list.reason("src/color/src/lib.rs"), None);
        assert!(parse_exempt_list("src/[bad | oops\n").is_err());
    }

    #[test]
    fn reason_returns_the_first_matching_positive_rule() {
        let list = parse_exempt_list("a/** | first\na/b.rs | second\n").unwrap();
        assert_eq!(list.reason("a/b.rs"), Some("first"));
        assert_eq!(list.reason("c/b.rs"), None);
    }

    #[test]
    fn floor_parsing_skips_comments_and_rejects_junk() {
        assert!((parse_floor("# baseline\n0.41\n").unwrap() - 0.41).abs() < 1e-9);
        assert!(parse_floor("# only a comment\n").is_err());
        assert!(parse_floor("nope\n").is_err());
    }

    #[test]
    fn rust_counts_pub_fns_at_every_nesting_level() {
        let counts = count_rust(
            "\
pub fn free() {}
fn private() {}
pub(crate) fn restricted() {}
pub mod inner {
    pub fn nested() {}
    fn hidden() {}
}
pub struct S;
impl S {
    pub fn method(&self) {}
    fn helper(&self) {}
}
trait T { fn declared(&self); }
impl T for S {
    fn declared(&self) {}
}
",
            false,
        )
        .unwrap();
        // free, restricted, nested, method, trait-impl declared
        assert_eq!(
            counts,
            Counts {
                public_fns: 5,
                tests: 0
            }
        );
    }

    #[test]
    fn rust_test_fns_are_counted_and_cfg_test_is_not_public() {
        let counts = count_rust(
            "\
pub fn real() {}
#[cfg(test)]
mod tests {
    pub fn helper() {}
    #[test]
    fn a_behaviour() {}
    #[tokio::test]
    async fn an_async_behaviour() {}
}
#[cfg(test)]
pub fn only_for_tests() {}
",
            false,
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                public_fns: 1,
                tests: 2
            }
        );
    }

    #[test]
    fn integration_files_contribute_tests_only() {
        let source = "pub fn helper() {}\n#[test]\nfn behaviour() {}\n";
        assert_eq!(
            count_rust(source, true).unwrap(),
            Counts {
                public_fns: 0,
                tests: 1
            }
        );
        assert!(count_rust("pub fn (", false).is_err());
    }

    #[test]
    fn js_exports_come_from_module_exports_window_and_top_level_fns() {
        let source = "\
function topLevel(a) { return a; }
(function (root) {
    function buildCategories(s) { return s; }
    function stop() {}
    var api = {
        buildCategories: buildCategories,
        stop: stop,
        IDLE_MS: 900
    };
    window._nativeUpdateStats = function (stats) {};
    window._isFullscreen = false;
    window._inputPlugin = inputPlugin;
    window.jmpInfo = {
        a: 1
    };
    function inputPlugin() {}
    if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(window);
";
        let names = js_exported_names(source);
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for n in [
            "topLevel",
            "buildCategories",
            "stop",
            "IDLE_MS",
            "_nativeUpdateStats",
            "_inputPlugin",
            "jmpInfo",
        ] {
            expected.insert(n.to_owned());
        }
        assert_eq!(names, expected);
    }

    #[test]
    fn js_exports_handle_esm_and_inline_object_exports() {
        let names = js_exported_names(
            "export function renderPng(size) {}\nexport const SIZES = [];\nexport { a, b as c };\n",
        );
        assert!(names.contains("renderPng"));
        assert!(names.contains("SIZES"));
        assert!(names.contains("a"));
        assert!(names.contains("c"));

        let inline =
            js_exported_names("module.exports = { alpha: 1, beta };\nexports.gamma = f;\n");
        assert!(inline.contains("alpha"));
        assert!(inline.contains("beta"));
        assert!(inline.contains("gamma"));
    }

    #[test]
    fn js_tests_are_counted_by_call_site() {
        let source = "\
const test = require('node:test');
test('one', () => {});
test('two', async () => {});
subtest('not counted', () => {});
t.test('method call not counted', () => {});
";
        assert_eq!(count_js_tests(source), 2);
    }

    #[test]
    fn text_report_ends_with_the_total_line() {
        let report = sample_report();
        let text = render_text(&report, true);
        assert!(text.contains("src/color/src/lib.rs"), "--files table");
        assert!(text.contains("TOTAL"));
        assert!(text.contains("exempt (1 files, excluded from the totals)"));
        assert!(
            text.trim_end().ends_with("total: 44/9 = 4.89"),
            "unexpected tail: {text}"
        );
        assert!(!render_text(&report, false).contains("src/color/src/hsl.rs"));
    }

    #[test]
    fn json_report_carries_the_same_totals() {
        let json = render_json(&sample_report()).unwrap();
        assert!(json.contains("\"public_fns\": 9"), "{json}");
        assert!(json.contains("\"tests\": 44"), "{json}");
        assert!(json.contains("\"reason\": \"win32 glue\""), "{json}");
    }

    #[test]
    fn collect_walks_a_synthetic_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/demo/src")).unwrap();
        std::fs::create_dir_all(root.join("src/demo/tests")).unwrap();
        std::fs::create_dir_all(root.join("src/web")).unwrap();
        std::fs::write(
            root.join("src/Cargo.toml"),
            "[workspace]\nmembers = [\"demo\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/demo/src/lib.rs"),
            "pub fn a() {}\npub fn b() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/demo/src/glue.rs"), "pub fn c() {}\n").unwrap();
        std::fs::write(root.join("src/demo/build.rs"), "pub fn main() {}\n").unwrap();
        std::fs::write(
            root.join("src/demo/tests/lib.rs"),
            "#[test]\nfn one() {}\n#[test]\nfn two() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/web/mod.js"), "module.exports = { x: 1 };\n").unwrap();
        std::fs::write(root.join("src/web/mod.test.js"), "test('a', () => {});\n").unwrap();

        let exempt =
            parse_exempt_list("src/demo/src/glue.rs | ffi\n**/build.rs | build\n").unwrap();
        let report = collect(root, &exempt).unwrap();

        let lib = report
            .files
            .iter()
            .find(|f| f.path == "src/demo/src/lib.rs")
            .unwrap();
        assert_eq!(
            lib.counts,
            Counts {
                public_fns: 2,
                tests: 2
            },
            "tests/lib.rs must be attributed to src/lib.rs"
        );
        assert!(
            !report
                .files
                .iter()
                .any(|f| f.path == "src/demo/tests/lib.rs"),
            "the attributed integration file must not get its own row"
        );
        assert_eq!(report.exempt().len(), 2);
        assert_eq!(
            report.totals(),
            Counts {
                public_fns: 3,
                tests: 3
            }
        );
        assert!(report.crates().iter().any(|c| c.name == "web"));
    }

    #[test]
    fn run_measures_this_repository() {
        let args = TestRatioArgs::default();
        run(&args).expect("test-ratio must succeed on the checked-out repo");
    }

    #[test]
    fn cfg_test_module_does_not_hide_later_public_functions() {
        let counts = count_rust(
            "#[cfg(test)]
mod tests {
    pub fn helper() {}
}
pub fn after() {}
",
            false,
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                public_fns: 1,
                tests: 0
            }
        );
    }

    #[test]
    fn trait_impl_methods_count_as_public_without_a_pub_keyword() {
        let trait_impl = count_rust(
            "pub struct S;
trait T { fn f(&self); }
impl T for S {
    fn f(&self) {}
}
",
            false,
        )
        .unwrap();
        assert_eq!(trait_impl.public_fns, 1);

        let inherent = count_rust(
            "pub struct S;
impl S {
    fn f(&self) {}
}
",
            false,
        )
        .unwrap();
        assert_eq!(inherent.public_fns, 0);
    }

    #[test]
    fn helpers_nested_inside_a_test_function_are_not_public() {
        let counts = count_rust(
            "#[test]
fn behaviour() {
    pub fn helper() {}
    struct S;
    impl S { pub fn m(&self) {} }
}
",
            false,
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                public_fns: 0,
                tests: 1
            }
        );
    }

    #[test]
    fn impl_blocks_inside_a_cfg_test_module_add_no_public_surface() {
        let counts = count_rust(
            "#[cfg(test)]
mod tests {
    pub struct Fixture;
    impl Fixture {
        pub fn build() -> Self { Fixture }
    }
    impl Default for Fixture {
        fn default() -> Self { Fixture }
    }
}
",
            false,
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                public_fns: 0,
                tests: 0
            }
        );
    }
}
