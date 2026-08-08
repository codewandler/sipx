//! Refuse a `target/debug/sipx` that was not built with this test run's features (`X-121`).
//!
//! Cargo *uplifts* the executable it builds — it links `target/<profile>/deps/sipx-<hash>` to
//! `target/<profile>/sipx`, which is the path `CARGO_BIN_EXE_sipx` names and the only one these
//! tests can spawn. That path holds one binary while a build directory holds one per feature set,
//! so whichever command wrote it last wins: `cargo build -p sipx-cli` leaves a binary there with no
//! `device-audio`, `dtls` or `opus` in it, and a test compiled with all three then spawns it.
//!
//! What follows is the reason this module exists rather than a comment somewhere. The spawned
//! binary does not crash and does not say anything about its build — it *refuses* the feature, at
//! the boundary, in the ordinary way. `sipx devices` answers "audio devices require a build with
//! the `device-audio` feature" on stderr and exits 1; an answerer asked for a browser-audio profile
//! declines to start and never prints the address line the harness waits for. The assertions
//! downstream then report a missing recording, a wrong exit code, or a timeout — "heard no audio at
//! all", which is the exact shape of a real media regression and was read as one twice.
//!
//! So the mismatch is named here, before any assertion about behaviour, and the reader is told the
//! two feature sets and the one command that repairs it.

// A guard that cannot fail loudly is not a guard. Annotated on the module, per `AGENTS.md`, because
// the four test binaries that include this file do not all opt out at crate level.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use std::process::Command;
use std::sync::OnceLock;

/// The optional features *this test binary* was compiled with, sorted to match the binary's report.
///
/// Cargo compiles an integration test with its package's selected features, so this is exactly the
/// set the executable under test should have been built with — not an approximation of it.
fn required() -> Vec<&'static str> {
    let compiled = [
        ("device-audio", cfg!(feature = "device-audio")),
        ("dtls", cfg!(feature = "dtls")),
        ("opus", cfg!(feature = "opus")),
    ];
    compiled
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect()
}

/// Read the binary's own account of its build.
///
/// `sipx version --json` is one short-lived process that binds nothing and reads no configuration,
/// which is what makes this affordable at all: the alternative — asking cargo to rebuild — is a
/// dependency graph per test run.
fn reported(binary: &str) -> Result<Vec<String>, String> {
    let output = Command::new(binary)
        .args(["version", "--json"])
        .output()
        .map_err(|error| format!("{binary} could not be run at all: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{binary} exited {:?} for `version --json`",
            output.status.code()
        ));
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{binary} did not report one JSON object: {error}"))?;
    match report.get("features") {
        // A binary old enough to predate `X-121` reports no feature set at all. Saying so is more
        // use than reporting an empty one, which would look like a deliberate default build.
        None => Err(format!(
            "{binary} reports no `features` field, so it was built before this check existed"
        )),
        Some(serde_json::Value::Array(names)) => names
            .iter()
            .map(|name| {
                name.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{binary} reported a non-string feature name: {name}"))
            })
            .collect(),
        Some(other) => Err(format!(
            "{binary} reported `features` as {other}, not an array"
        )),
    }
}

/// Describe the mismatch in the terms a reader needs, and never in terms of audio.
fn complaint(binary: &str, required: &[&str], found: &[String]) -> String {
    let name_set = |names: &[String]| {
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    let required: Vec<String> = required.iter().map(|name| (*name).to_owned()).collect();
    format!(
        "the sipx binary these tests spawn was built with different features than the tests.\n\
         \n\
         \x20   binary:      {binary}\n\
         \x20   tests need:  {}\n\
         \x20   binary has:  {}\n\
         \n\
         This is a stale uplifted binary, not a media or device defect: cargo keeps one executable \
         per feature set under `deps/` but only one at the path above, so a `cargo build -p \
         sipx-cli` or a `./scripts/check-cli-reference.py` run leaves a binary there that a later \
         `--all-features` test run may spawn unchanged. Nothing about the audio path is implicated \
         by this failure.\n\
         \n\
         Rebuild the binary for these features, or remove it and run the same command again:\n\
         \n\
         \x20   rm -f {binary}\n",
        name_set(&required),
        name_set(found),
    )
}

/// Fail now, naming the mismatch, rather than later as though a feature were broken.
///
/// Every process test calls this before it asserts on behaviour. The verdict is computed once per
/// test binary — these files spawn `sipx` hundreds of times, and the check is a process spawn.
pub(crate) fn assert_binary_matches_this_build() {
    static VERDICT: OnceLock<Result<(), String>> = OnceLock::new();
    let verdict = VERDICT.get_or_init(|| {
        let binary = env!("CARGO_BIN_EXE_sipx");
        let required = required();
        match reported(binary) {
            Ok(found) if found == required => Ok(()),
            Ok(found) => Err(complaint(binary, &required, &found)),
            // An unreadable report is the same finding: what is at that path is not the binary this
            // run built. Reporting it as a mismatch keeps one explanation for one symptom.
            Err(why) => Err(format!(
                "{}\n\x20   the report could not be read: {why}\n",
                complaint(binary, &required, &[])
            )),
        }
    });
    if let Err(complaint) = verdict {
        panic!("{complaint}");
    }
}

#[test]
fn a_matching_build_is_not_refused() {
    // The real one: whatever this test binary was compiled with, the spawned binary reports the
    // same. A green run of this file is the assertion that the guard does not cry wolf.
    assert_binary_matches_this_build();
}

#[test]
fn a_mismatch_names_both_feature_sets_and_blames_the_build() {
    let text = complaint(
        "/w/target/debug/sipx",
        &["device-audio", "dtls", "opus"],
        &[],
    );
    assert!(
        text.contains("tests need:  device-audio, dtls, opus"),
        "{text}"
    );
    assert!(text.contains("binary has:  none"), "{text}");
    assert!(text.contains("stale uplifted binary"), "{text}");
    assert!(text.contains("rm -f /w/target/debug/sipx"), "{text}");
    // The whole point: a reader must not leave this message looking for a media defect.
    assert!(
        text.contains("not a media or device defect"),
        "the message has to rule audio out by name: {text}"
    );
}

#[test]
fn a_binary_ahead_of_the_tests_is_a_mismatch_too() {
    // The trap runs both ways. A default-feature `cargo test` after an all-features build spawns a
    // binary that *accepts* what the test expects it to refuse, and the assertion that fails is
    // about an exit code.
    let text = complaint(
        "/w/target/debug/sipx",
        &[],
        &["device-audio".to_owned(), "opus".to_owned()],
    );
    assert!(text.contains("tests need:  none"), "{text}");
    assert!(text.contains("binary has:  device-audio, opus"), "{text}");
}
