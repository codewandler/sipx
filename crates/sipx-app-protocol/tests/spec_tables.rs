//! Every table in the contract spec, read out of the spec and held against the code.
//!
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) has five tables, and each
//! one is a claim about this crate: §3 says every verb has a call-framework operation, §5.3 lists
//! the event types, §6.2 lists the verbs and their fields, §9.2 lists the failure knobs, and §11
//! lists the vectors. Each has a test here.
//!
//! **These tests parse the specification.** That is the whole point of them, and it is the
//! difference between a derived test and a transcribed one. A test that hard-coded the same
//! fifteen event names the implementation hard-codes would agree with the implementation forever,
//! including when both had drifted from the document they are supposed to implement — it would be
//! testing that a list equals itself. Reading the markdown means a row added to the spec fails the
//! build until somebody adds the variant, and a variant added to the code fails it until somebody
//! writes the row.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;

use sipx_app_protocol::{
    DialOutcome, EndCause, EventKind, Failure, GatherReason, OnFailure, Policy, TransferState,
    Verb,
};

const SPEC: &str = include_str!("../../../docs/specs/app-contract.md");
const VECTOR_TESTS: &str = include_str!("vectors.rs");

/// The rows of the first markdown table that follows a heading whose text contains `heading`.
///
/// A row is its cells, already trimmed. The header row and the `|---|` rule are dropped.
fn table_after(heading: &str) -> Vec<Vec<String>> {
    let mut lines = SPEC
        .lines()
        .skip_while(|line| !(line.starts_with('#') && line.contains(heading)));
    assert!(lines.next().is_some(), "no heading containing {heading:?}");

    let mut rows = Vec::new();
    let mut started = false;
    for line in lines {
        let line = line.trim();
        if line.starts_with('|') {
            started = true;
            // The `|---|---|` rule under the header carries no cells worth having.
            if line.chars().all(|c| "|-: ".contains(c)) {
                continue;
            }
            let cells: Vec<String> = line
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect();
            rows.push(cells);
        } else if started && !line.is_empty() {
            break;
        } else if line.starts_with('#') {
            break;
        }
    }
    assert!(!rows.is_empty(), "no table after {heading:?}");
    // Drop the header row.
    rows.remove(0);
    rows
}

/// Every `` `backticked` `` token in a cell, in order.
fn backticked(cell: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = cell;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        found.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    found
}

/// **§3** — every verb the contract names resolves to a call-framework operation.
///
/// The section's own rule: *the contract may not name a verb that has no operation*. So the test
/// is set equality between §3's first column and the verbs this crate implements — a verb in the
/// table with no [`Verb`] variant is a promise the code does not keep, and one in the code with no
/// row is an operation nobody has justified.
#[test]
fn section_3_maps_every_verb_to_an_operation() {
    let mut in_table = BTreeSet::new();
    for row in table_after("3. Effects") {
        for verb in backticked(&row[0]) {
            in_table.insert(verb);
        }
        assert!(
            !row[1].is_empty(),
            "§3 row {:?} names no operation",
            row[0]
        );
    }
    let implemented: BTreeSet<String> = Verb::names().iter().map(|n| (*n).to_owned()).collect();
    assert_eq!(
        in_table, implemented,
        "§3's verbs and the crate's verbs differ"
    );
}

/// **§5.3** — the event-type table is exactly what [`EventKind`] spells.
#[test]
fn section_5_3_lists_exactly_the_event_types_the_crate_has() {
    let mut in_table = BTreeSet::new();
    for row in table_after("5.3 Event types") {
        for name in backticked(&row[0]) {
            in_table.insert(name);
        }
    }
    let implemented: BTreeSet<String> = EventKind::type_names()
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    assert_eq!(
        in_table, implemented,
        "§5.3's event types and the crate's differ"
    );

    // And the ordered fixture covers each of them exactly once, which is what lets the wire
    // round-trip test in `event.rs` claim to have covered the table.
    let fixture: BTreeSet<String> = sipx_app_protocol::testing::one_of_every_event()
        .iter()
        .map(|kind| kind.type_name().to_owned())
        .collect();
    assert_eq!(fixture, implemented, "the fixture misses an event type");
}

/// **§5.3**, the enumerations inside it: `reason`, `outcome`, `state` and `cause` are each written
/// as a `·`-separated list in the table, and each has a Rust enum behind it.
///
/// These are the values [`sipx_app_protocol`]'s `tagged` encoding exists to keep from drifting, so
/// they are read out of the same row that documents them rather than copied next to the enum.
#[test]
fn section_5_3_s_inline_enumerations_match_their_types() {
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    for row in table_after("5.3 Event types") {
        // The shape is: `field` (`a · b · c{x}`). The bracketed list is the last backticked token
        // in the cell whenever it contains a `·`.
        for token in backticked(&row[1]) {
            if token.contains('·') {
                lists.push((
                    row[0].trim_matches('`').to_owned(),
                    token
                        .split('·')
                        .map(|value| {
                            value
                                .trim()
                                // `rejected{status}` names the value `rejected`; the brace is the
                                // field it carries, which the tagged encoding writes separately.
                                .split('{')
                                .next()
                                .unwrap_or_default()
                                .to_owned()
                        })
                        .collect(),
                ));
            }
        }
    }
    assert_eq!(lists.len(), 4, "§5.3 should carry four inline lists: {lists:?}");

    for (field, values) in lists {
        let implemented: Vec<String> = match field.as_str() {
            "call.gather.finished" => [
                GatherReason::Terminator,
                GatherReason::Max,
                GatherReason::Timeout,
            ]
            .iter()
            .map(|r| r.as_str().to_owned())
            .collect(),
            "call.dial.finished" => [
                DialOutcome::Answered,
                DialOutcome::Busy,
                DialOutcome::Rejected { status: 486 },
                DialOutcome::Timeout,
            ]
            .iter()
            .map(|o| tag_of(&o.to_json()))
            .collect(),
            "call.transfer.progress" => [
                TransferState::Trying,
                TransferState::Ringing,
                TransferState::Succeeded,
                TransferState::Failed { status: 480 },
            ]
            .iter()
            .map(|s| tag_of(&s.to_json()))
            .collect(),
            "call.ended" => [
                EndCause::Hangup,
                EndCause::Remote,
                EndCause::Rejected { status: 486 },
                EndCause::Timeout,
                EndCause::Error,
            ]
            .iter()
            .map(|c| tag_of(&c.to_json()))
            .collect(),
            other => panic!("§5.3 grew an inline list on {other}, with no type behind it"),
        };
        assert_eq!(values, implemented, "the values of {field} differ");
    }
}

/// The name a tagged value writes, whether it is a bare string or an object with a `name`.
fn tag_of(value: &sipx_app_protocol::json::Json) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.get("name").and_then(|n| n.as_str()).map(str::to_owned))
        .expect("a tagged value is a name or an object with one")
}

/// **§6.2** — the verb table is exactly what [`Verb`] spells, and every row says what completes it.
#[test]
fn section_6_2_lists_exactly_the_verbs_the_crate_has() {
    let mut in_table = BTreeSet::new();
    for row in table_after("6.2 Verbs") {
        for verb in backticked(&row[0]) {
            in_table.insert(verb);
        }
        assert!(
            !row[2].is_empty(),
            "§6.2 row {:?} does not say what completes it",
            row[0]
        );
    }
    let implemented: BTreeSet<String> = Verb::names().iter().map(|n| (*n).to_owned()).collect();
    assert_eq!(
        in_table, implemented,
        "§6.2's verbs and the crate's verbs differ"
    );

    // And the fixture has one of each, so the document round trip covers the table.
    let fixture: BTreeSet<String> = sipx_app_protocol::testing::one_of_every_verb()
        .iter()
        .map(|instruction| instruction.verb.name().to_owned())
        .collect();
    assert_eq!(fixture, implemented, "the fixture misses a verb");
}

/// **§9.2** — the knobs table is exactly [`Policy`]'s fields, and its values are [`OnFailure`]'s
/// variants.
#[test]
fn section_9_2_lists_exactly_the_knobs_the_policy_has() {
    let mut knobs = BTreeSet::new();
    let mut values = BTreeSet::new();
    for row in table_after("9.2 Declared failure semantics") {
        for knob in backticked(&row[0]) {
            knobs.insert(knob);
        }
        for value in backticked(&row[1]) {
            // `same values` is prose, and `duration` is a type rather than a value.
            values.insert(
                value
                    .split('{')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
            );
        }
    }
    let implemented: BTreeSet<String> = ["timeout_ms", "on_timeout", "on_5xx", "on_unreachable", "on_4xx"]
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    assert_eq!(knobs, implemented, "§9.2's knobs and the policy's differ");

    let declared: BTreeSet<String> = ["continue", "hangup", "reject"]
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    assert_eq!(values, declared, "§9.2's values and `OnFailure`'s differ");

    // Each knob is reachable, and each failure maps to one of them. §9.2's own table, as code.
    let policy = Policy {
        timeout_ms: 1,
        on_timeout: OnFailure::Hangup,
        on_5xx: OnFailure::Reject { status: 500 },
        on_unreachable: OnFailure::Continue,
        on_4xx: OnFailure::Hangup,
        dial_headers: Vec::new(),
    };
    assert_eq!(policy.on(Failure::Timeout), OnFailure::Hangup);
    assert_eq!(policy.on(Failure::ServerError), OnFailure::Reject { status: 500 });
    assert_eq!(policy.on(Failure::Unreachable), OnFailure::Continue);
    assert_eq!(policy.on(Failure::ClientError), OnFailure::Hangup);
}

/// **§11** — every vector row has a test in `tests/vectors.rs`.
///
/// §11's own sentence is "each row is a test in `sipx-app-protocol`", so this is that sentence
/// checked. It reads both the spec and the test file, which is why a vector added to the section
/// is a red build rather than a quiet omission nobody notices for two releases.
#[test]
fn section_11_has_a_test_for_every_vector() {
    let rows = table_after("11. Vectors");
    assert!(rows.len() >= 9, "§11 lost rows: {}", rows.len());
    for row in rows {
        let id = row[0].trim();
        assert!(id.starts_with("AC-"), "§11 row is not a vector: {id}");
        let expected = format!("fn {}_", id.to_lowercase().replace('-', "_"));
        assert!(
            VECTOR_TESTS.contains(&expected),
            "§11 lists {id} and tests/vectors.rs has no `{expected}…` test"
        );
        assert!(
            !row[2].is_empty(),
            "§11 row {id} asserts nothing"
        );
    }
}

/// The spec's status line says experimental, and so must the crate — Acceptance asks for the two
/// to match, and a stability claim that is true in one place and not the other is worse than
/// either alone.
#[test]
fn the_spec_and_the_crate_agree_that_this_is_experimental() {
    assert!(
        SPEC.contains("**experimental**"),
        "the spec no longer says experimental; the crate docs and README must change with it"
    );
    let readme = include_str!("../README.md");
    assert!(
        readme.to_lowercase().contains("experimental"),
        "the README must say what the spec says"
    );
    let lib = include_str!("../src/lib.rs");
    assert!(
        lib.contains("# Experimental"),
        "the crate docs must say what the spec says"
    );
}
