//! No silent discards.
//!
//! `docs/specs/sip-transport.md` §12.1: every place the signalling path throws something away has
//! either a counter or a written reason, and *this test is what keeps that true as the code changes*.
//! Without it the rule is a sentence in a spec, and the next `let _ = …` added under time pressure
//! is invisible again.
//!
//! # Why a source scan rather than a runtime assertion
//!
//! The failure this guards against is a discard nobody thought about. A runtime test can only
//! exercise the paths its author already knows are there, which is precisely the wrong shape: the
//! dangerous discard is the one added later, in a path this file's author never saw. So the check
//! reads the crate's own source, the way `check-audio-claims.py` and `check-pool-key.py` read theirs.
//!
//! # Which crates, and why the list is here rather than one per crate
//!
//! **The signalling path is `sipx-transport` and `sipx-call`** — see [`CRATES`] and §12.3. It ran
//! over this crate alone until `X-54`, which is how the dialog layer came to have seven discards
//! that nothing enumerated: `X-51` found them by hand, which is precisely the method this test
//! exists to replace.
//!
//! The list lives here, next to the one copy of the detector, rather than as a copy of this file in
//! each crate. Two copies of [`LOSS_WORDS`] is `X-24`'s pool-key failure again — a rule written in
//! two places was wrong in one of them through two changes, and nobody was told, because nothing
//! connected the sentence to the field. A test that reads a sibling crate's `src` is the smaller
//! price: it adds no dependency in either direction and nothing is compiled that would not be.
//!
//! # What to do when this test fails
//!
//! It will name a `path:line`. Go there and pick one:
//!
//! - **It loses something an operator would want counted.** Add a counter in
//!   `crate::counters::Meters` and increment it at the site. That is the preferred answer.
//! - **It cannot lose anything.** Say so in a `// discard: …` comment directly above it, giving the
//!   reason — not the restatement. "the caller stopped waiting, so nobody is listening for this
//!   answer" is a reason; "ignore the error" is not.
//!
//! Deleting the site or widening this test to exclude it are both wrong, and the second is why the
//! allowance is a *comment* rather than an entry in a list here: a reason that lives next to the code
//! is read by whoever changes the code.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// How many lines above a discard the excuse may live.
///
/// Small on purpose: a reason five lines up is about the code it sits above. A reason thirty lines up
/// is about something else and has been left behind by an edit.
const LOOKBACK: usize = 10;

/// The marker that says a discard has been thought about (§12.1).
const MARKER: &str = "// discard:";

/// Words in a log message that mean something was thrown away.
///
/// **This list is the limit of the check, and it is a real one.** A discarded *result* is found
/// structurally — `let _ =` is unambiguous — but a `tracing` line that reports a loss can only be
/// recognised by what it says, and there is no closed vocabulary for that. The first version of this
/// list held three words and missed two live sites: `tcp.rs`'s "closing connection on framing error"
/// and `ws.rs`'s "closing on a malformed websocket message", each of which drops a connection and
/// everything in flight on it.
///
/// So: adding a word here is cheap and the failure mode is a false positive, which costs one
/// `// discard:` comment. Leaving one out is a silent hole. When in doubt, add the word.
const LOSS_WORDS: &[&str] = &[
    "dropping",
    "dropped",
    "discard",
    "ignoring",
    "ignored",
    "malformed",
    "abandon",
    "refus",
    "unmatched",
    "framing error",
    "not keeping up",
    "never answered",
    "lost",
];

/// The crates the clause's words cover: **the signalling path**, which does not stop at the socket.
///
/// `sipx-transport` owns the sockets, the transactions and the capture; `sipx-call` owns the
/// dialogs and the dispatcher, and a request that is dropped there is dropped from the same path by
/// any reading of the phrase — an ACK that reaches no call is lost exactly as thoroughly as one
/// that reached no socket, and it is the loss that leaks calls.
///
/// **What is deliberately not here.** `sipx-sip` and `sipx-sdp` are the sans-IO core: they discard
/// nothing, because nothing reaches them that they did not ask for and they hand every failure back
/// as a typed error (`AGENTS.md` §2, §3). `sipx-media` and `sipx-rtp` are the *media* path, and
/// M12's clause says the signalling one — those counters are `M-32`, which extends this list rather
/// than editing what is in it. `sipx-ua` and `sipx-cli` compose the two crates below without
/// terminating anything themselves.
const CRATES: &[&str] = &["sipx-transport", "sipx-call"];

/// A discard the scan found.
#[derive(Debug)]
struct Site {
    file: PathBuf,
    line: usize,
    text: String,
}

/// Where a sibling crate's sources live, from this crate's own manifest directory.
fn source_dir(crate_name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the workspace's crates directory")
        .join(crate_name)
        .join("src")
}

/// Every `.rs` file in one crate's `src`, sorted so a failure names them in a stable order.
///
/// `None` when the crate is not on disk at all, which happens in exactly one situation: this crate
/// unpacked from a published `.crate` archive, where the sibling is a registry dependency rather
/// than a directory. Panicking there would fail a packaged test run over the absence of a file the
/// package was never going to contain. It is `None` rather than an empty list so that the caller
/// can tell "not present" from "present and empty" — the second is a broken scan and still fails.
fn sources_of(crate_name: &str) -> Option<Vec<PathBuf>> {
    let directory = source_dir(crate_name);
    if !directory.is_dir() {
        return None;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{crate_name} has a src directory: {error}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    assert!(
        files.len() > 5,
        "the scan found almost no source files in {crate_name}, so it is looking in the wrong \
         place: {files:?}"
    );
    Some(files)
}

/// Every source file the scan covers, across every crate in [`CRATES`] that is on disk.
///
/// **This crate's own `src` is never optional.** If the scan cannot find the tree it is compiled
/// from, it is not narrowed, it is broken — and a guard that silently scans nothing is
/// indistinguishable from a codebase with nothing to find, which is `X-29`'s lesson and the reason
/// [`the_scan_detects_an_unexplained_discard`] exists.
fn sources() -> Vec<PathBuf> {
    let own = env!("CARGO_PKG_NAME");
    assert!(
        CRATES.contains(&own),
        "the scan must cover the crate it is compiled from, and {own} is not in CRATES"
    );
    let files: Vec<PathBuf> = CRATES
        .iter()
        .filter_map(|name| sources_of(name))
        .flatten()
        .collect();
    assert!(
        sources_of(own).is_some(),
        "this crate's own src is missing, so the scan is broken rather than narrowed"
    );
    files
}

/// Where a file's test module starts, so the scan stops there.
///
/// Test code is allowed to discard freely — a test that cannot read its own fixture should fail
/// loudly (`AGENTS.md` §4) — and holding it to the production rule would only teach people to write
/// the marker without meaning it.
fn production_lines(body: &str) -> Vec<&str> {
    let lines: Vec<&str> = body.lines().collect();
    let end = lines
        .iter()
        .position(|line| line.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    lines[..end].to_vec()
}

/// Discards in one file that carry neither a counter nor a reason.
fn unexplained(path: &Path) -> Vec<Site> {
    let body = std::fs::read_to_string(path).expect("a source file the scan just listed");
    let lines = production_lines(&body);
    let mut found = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // A discarded result, in either of the two spellings the crate uses. `_ = …` inside a
        // `select!` arm is not a discard — it is a pattern — and is excluded by requiring `let`.
        let discards = trimmed.starts_with("let _ =") || trimmed.starts_with("let _x =");
        // A `tracing` line whose own message says something was thrown away. §12.1 is explicit that
        // logging a discard is not counting it: logs rotate, and "how often" should not be answered
        // with `grep | wc -l`.
        let reports_a_drop =
            trimmed.contains("tracing::") && LOSS_WORDS.iter().any(|word| trimmed.contains(word));
        if !discards && !reports_a_drop {
            continue;
        }

        let from = index.saturating_sub(LOOKBACK);
        let context = lines.get(from..=index).unwrap_or(&[]);
        let explained = context.iter().any(|near| {
            // A counter next to it, or a stated reason. Either satisfies §12.1; a counter is better,
            // because it answers "how often" as well as "why".
            //
            // Each crate spells "a counter was incremented here" its own way, and the spellings are
            // listed rather than guessed at: `sipx-transport` reaches its `Meters` through a field
            // called `meters`, `sipx-call` counts a request it did not deliver through
            // `Calls::counted` and a call event it dropped through the sink's `drops`.
            near.contains(MARKER)
                || near.contains("meters.")
                || near.contains("self.meters")
                || near.contains(".counted(")
                || near.contains("drops.")
        });
        if !explained {
            found.push(Site {
                file: path.to_path_buf(),
                line: index + 1,
                text: trimmed.to_owned(),
            });
        }
    }
    found
}

/// `<crate>/src/<file>.rs`, which is what a reader needs to go and open it.
fn short(path: &Path) -> String {
    let mut parts: Vec<String> = path
        .components()
        .rev()
        .take(3)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.reverse();
    parts.join("/")
}

/// **§12.1's guard.** Every discard in the signalling path is counted or explained.
#[test]
fn no_discard_in_the_signalling_path_is_silent() {
    let unexplained: Vec<Site> = sources()
        .iter()
        .flat_map(|path| unexplained(path))
        .collect();

    if !unexplained.is_empty() {
        let listing = unexplained
            .iter()
            .map(|site| {
                // Crate and file, because two crates are scanned now and each has a `lib.rs`.
                format!("  {}:{}\n      {}", short(&site.file), site.line, site.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} discard site(s) have neither a counter nor a reason (spec §12.1):\n{listing}\n\n\
             Add a counter in `counters::Meters` and increment it at the site, or put a\n\
             `{MARKER} <reason>` comment directly above it. See this file's module docs.",
            unexplained.len()
        );
    }
}

/// The scan has to be able to fail, or it proves nothing.
///
/// `X-29`'s lesson in a different costume: a guard whose detector is broken is indistinguishable from
/// a codebase with nothing to find, and the second is what everyone assumes. This asserts the
/// detector fires on a discard with no excuse and stays quiet on the two forms of excuse.
#[test]
fn the_scan_detects_an_unexplained_discard() {
    let directory = std::env::temp_dir().join(format!("sipx-discards-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a scratch directory");

    let cases: [(&str, &str, bool); 5] = [
        ("bare.rs", "fn f() {\n    let _ = send();\n}\n", true),
        (
            "reasoned.rs",
            "fn f() {\n    // discard: nobody is waiting for this.\n    let _ = send();\n}\n",
            false,
        ),
        (
            "counted.rs",
            "fn f() {\n    self.meters.capture_drop();\n    let _ = send();\n}\n",
            false,
        ),
        (
            "logged_drop.rs",
            "fn f() {\n    tracing::debug!(\"dropping a datagram\");\n}\n",
            true,
        ),
        (
            // Test code is out of scope, and the scan must actually stop at the boundary.
            "in_tests.rs",
            "fn f() {}\n#[cfg(test)]\nmod tests {\n    let _ = send();\n}\n",
            false,
        ),
    ];

    for (name, body, should_fire) in cases {
        let path = directory.join(name);
        std::fs::write(&path, body).expect("writes the fixture");
        let found = unexplained(&path);
        assert_eq!(
            !found.is_empty(),
            should_fire,
            "the detector is wrong about {name}: found {found:?}"
        );
    }

    std::fs::remove_dir_all(&directory).ok();
}

/// A discard is only explained by a reason *above* it, not one below.
///
/// Without this the lookback could be made to pass by writing the marker anywhere in the file, and a
/// reason that is not next to its code is a reason nobody will read when they change that code.
#[test]
fn a_reason_below_a_discard_does_not_explain_it() {
    let directory =
        std::env::temp_dir().join(format!("sipx-discards-order-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let path = directory.join("below.rs");
    std::fs::write(
        &path,
        "fn f() {\n    let _ = send();\n    // discard: too late to count.\n}\n",
    )
    .expect("writes the fixture");

    assert!(
        !unexplained(&path).is_empty(),
        "a marker below the discard must not satisfy the scan"
    );
    std::fs::remove_dir_all(&directory).ok();
}
