//! §3.2 of `docs/specs/sip-tls.md` is a list of knobs; this is what holds it to the code.
//!
//! The list said the minimum protocol version was configurable "at or above the floor in §3.5".
//! It never was: neither `ClientTls` nor `ServerTls` takes a version, and nothing above them names
//! one (`X-46`). Nothing caught it because a sentence in a spec cannot stop being true on its own —
//! which is the reasoning that made `X-43` require an `implemented` registry row to cite code,
//! applied one level down, inside a spec.
//!
//! So every entry in §3.2 names the API that provides it, and every name it gives must be a public
//! item of `src/tls.rs`. An entry that names nothing, or names something that has since been
//! renamed, fails here rather than being read as true for another two releases.
//!
//! **A text check on purpose.** It is the documentation that goes stale, and it must be checked in
//! every feature configuration — including the ones where `tls` is off and there is no API to
//! reference at all. Reading the source as text is also what lets the check say *which* entry is
//! unmoored, which is the half a compile error could not.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// The names `src/tls.rs` actually offers.
#[derive(Debug, Default)]
struct Api {
    /// `pub struct` and `pub enum`.
    types: Vec<String>,
    /// `pub fn` outside any `impl`.
    functions: Vec<String>,
    /// `pub fn` inside an inherent `impl`, as (type, function).
    methods: Vec<(String, String)>,
}

impl Api {
    /// Whether a name written in the spec resolves to something in this file.
    ///
    /// `Type::function` is checked as a pair rather than as two independent names: `new` exists on
    /// three types here, and a spec entry pointing the reader at the wrong one is exactly the kind
    /// of near-miss this file is for.
    fn resolves(&self, name: &str) -> bool {
        match name.split_once("::") {
            Some((ty, function)) => self.methods.iter().any(|(t, f)| t == ty && f == function),
            None => {
                self.types.iter().any(|t| t == name) || self.functions.iter().any(|f| f == name)
            }
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {relative}: {error}"))
}

/// The identifier at the start of a declaration, e.g. `new(anchors: &TrustAnchors)` -> `new`.
fn identifier(rest: &str) -> Option<String> {
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The public surface of `src/tls.rs`, read as text.
///
/// The file declares no generics and no trait `impl` with a `pub fn` in it, so a line-oriented
/// reading is honest here; anything it cannot classify it leaves out, and an entry naming that
/// item then fails loudly rather than passing on a guess.
fn public_api(source: &str) -> Api {
    let mut api = Api::default();
    let mut current_impl: Option<String> = None;

    for line in source.lines() {
        let trimmed = line.trim();

        if line.starts_with("impl ") {
            // `impl Type {` and `impl Trait for Type {`. Only the inherent ones carry the API a
            // spec entry can name, so a trait `impl` sets no current type.
            let head = trimmed.trim_end_matches('{').trim();
            current_impl = match head.rsplit_once(" for ") {
                Some(_) => None,
                None => head.strip_prefix("impl ").and_then(identifier),
            };
        } else if line == "}" {
            current_impl = None;
        }

        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            api.types.extend(identifier(rest));
        } else if let Some(rest) = trimmed.strip_prefix("pub enum ") {
            api.types.extend(identifier(rest));
        } else if let Some(rest) = trimmed.strip_prefix("pub fn ") {
            match &current_impl {
                Some(ty) => api
                    .methods
                    .extend(identifier(rest).map(|f| (ty.clone(), f))),
                None => api.functions.extend(identifier(rest)),
            }
        }
    }

    api
}

/// The top-level bullets of a `### ` section, each as one string.
fn bullets(spec: &str, heading: &str) -> Vec<String> {
    let mut bullets: Vec<String> = Vec::new();
    let mut inside = false;

    for line in spec.lines() {
        if line.starts_with("### ") {
            if inside {
                break;
            }
            inside = line.starts_with(heading);
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            bullets.push(rest.to_owned());
        } else if line.starts_with("  ") {
            // A wrapped continuation of the bullet above, which may be where the API is named.
            if let Some(last) = bullets.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else if !line.trim().is_empty() {
            // Prose after the list: the section continues, the list does not.
            if !bullets.is_empty() {
                break;
            }
        }
    }

    bullets
}

/// Every backticked span that could be a Rust path — `TrustAnchors`, `ClientTls::new`.
fn code_spans(bullet: &str) -> Vec<String> {
    bullet
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| {
            !span.is_empty()
                && span
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        })
        .map(str::to_owned)
        .collect()
}

/// Every knob §3.2 lists names an API, and every API it names exists.
///
/// This is the guard `X-46` asks for. At the merge base all three entries failed it: the list
/// named no code at all, so the false one was indistinguishable from the true ones.
#[test]
fn every_configurable_entry_names_a_real_api() {
    let spec = read("docs/specs/sip-tls.md");
    let api = public_api(&read("crates/sipx-transport/src/tls.rs"));

    let entries = bullets(&spec, "### 3.2");
    assert!(
        !entries.is_empty(),
        "§3.2 of docs/specs/sip-tls.md lists nothing — an empty list must not pass this check \
         by having nothing to check"
    );

    let mut unmoored: Vec<String> = Vec::new();
    for entry in &entries {
        let named = code_spans(entry);
        if !named.iter().any(|name| api.resolves(name)) {
            unmoored.push(format!("  - {entry}\n    names: {named:?}"));
        }
    }

    assert!(
        unmoored.is_empty(),
        "every entry in §3.2 of docs/specs/sip-tls.md must name a public item of \
         crates/sipx-transport/src/tls.rs that provides it. These do not:\n{}\n\
         Known API: types {:?}, functions {:?}, methods {:?}",
        unmoored.join("\n"),
        api.types,
        api.functions,
        api.methods,
    );
}

/// The floor belongs to the TLS library, and sipx names no version anywhere.
///
/// This is the fact §3.2 now points at instead of claiming a knob, and it is restated wherever a
/// reader might rely on it — `docs/specs/sip-tls.md` §3.2 and §3.5, `src/tls.rs`'s module
/// documentation, `tests/tls_versions.rs`, and RFC 8996's registry row — so that a backend swap
/// cannot move it in one place only. If sipx ever does select a version itself, this is the
/// tripwire that says every one of them has to move with it.
#[test]
fn no_tls_version_is_named_in_the_crate() {
    let source = read("crates/sipx-transport/src/tls.rs");
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    for naming in [
        "with_protocol_versions",
        "ProtocolVersion",
        "TLS12",
        "TLS13",
        "ALL_VERSIONS",
    ] {
        assert!(
            !code.contains(naming),
            "src/tls.rs names `{naming}`, so sipx now selects TLS versions itself. That is a \
             change of what the stack claims: §3.2 and §3.5 of docs/specs/sip-tls.md, this \
             file's module documentation and RFC 8996's row in docs/rfc/registry.toml all say \
             the floor is the library's, and all of them have to move with it."
        );
    }
}
