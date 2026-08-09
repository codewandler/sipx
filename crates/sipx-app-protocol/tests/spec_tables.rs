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
    AudioDirection, DialOutcome, DspBypassCause, DspRefusal, DspTeardownCause, EndCause, EventKind,
    Failure, GatherReason, OnFailure, Policy, TransferState, Verb, VoiceEndCause,
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
        } else if (started && !line.is_empty()) || line.starts_with('#') {
            // Past the table, or into the next section: either way the table is complete.
            break;
        }
    }
    assert!(!rows.is_empty(), "no table after {heading:?}");
    // Drop the header row.
    rows.remove(0);
    rows
}

/// The first fenced ```` ```json ```` block that follows a heading whose text contains `heading`.
///
/// The spec's examples are normative by being examples: §5.2 shows the snapshot's members, and a
/// member the code writes that the section does not show is a wire field nobody agreed to.
fn json_after(heading: &str) -> sipx_app_protocol::json::Json {
    let mut lines = SPEC
        .lines()
        .skip_while(|line| !(line.starts_with('#') && line.contains(heading)));
    assert!(lines.next().is_some(), "no heading containing {heading:?}");

    let mut body = String::new();
    let mut inside = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            if inside {
                break;
            }
            assert!(
                line.contains("json"),
                "the first block after {heading:?} is not JSON"
            );
            inside = true;
            continue;
        }
        if inside {
            body.push_str(line);
            body.push('\n');
        }
    }
    assert!(inside, "no fenced block after {heading:?}");
    // The snapshot example writes its id as `"b7c1…"`, which is a perfectly good JSON string.
    sipx_app_protocol::json::Json::parse(&body)
        .unwrap_or_else(|e| panic!("the example after {heading:?} is not JSON: {e}"))
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
        assert!(!row[1].is_empty(), "§3 row {:?} names no operation", row[0]);
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

/// **§5.3** — every row a call can reach has an arm in the bridge (`M-98`).
///
/// The test above is the claim *the crate has the variant*. This is the different claim *a call can
/// produce it*, and only the first was ever checked: `M-59` shipped `call.signal.metrics` and
/// `call.signal.silence` — the rows, the variants, the wire round trip — and no arm in
/// [`sipx_app_protocol::event_from_call`], so the contract named two events no host could ever
/// emit and every derived test in this file passed.
///
/// So the section's rows are held against the bridge's own source. A row whose [`EventKind`]
/// variant `src/call.rs` never names is a row nothing reaches. The variant name is read out of the
/// fixture's `Debug` spelling rather than typed here, for the reason this file's header gives: a
/// list of variant names next to a list of variant names tests that a list equals itself.
///
/// `COMPOSED_BY_THE_DRIVER` is the other half of the answer, and each of its rows has to satisfy
/// **both** halves: absent from the bridge, *and* present in the source that composes it instead.
///
/// # Why the second half had to be added (`M-103`)
///
/// Until `M-103` this list held names and nothing else, and the only thing asserted about a name
/// on it was that `src/call.rs` does *not* produce it. That is a claim about where a row is not,
/// and it is satisfied by a row nothing produces anywhere — which is exactly the defect `M-98`,
/// `M-99` and `M-103` each closed once. `call.incoming` and `call.gather.finished` happened to have
/// real producers behind them; `call.dial.finished` sat in the same list with none, and this test
/// could not tell those two situations apart. So a row named here now has to point at the file that
/// composes it, and that file has to contain the construction.
#[test]
fn section_5_3_s_rows_are_reachable_through_the_bridge() {
    /// The bridge, as text. Read rather than called so that this runs with the `call` feature off:
    /// the question is which arms exist, and that is answered by the source either way.
    const BRIDGE: &str = include_str!("../src/call.rs");

    /// The shipped driver (`X-38`), read for the same reason and in the same way as the bridge.
    ///
    /// A test in this crate reading the application crate's source is deliberate. The claim being
    /// checked is *a driver composes this row*, and the only thing that can answer a claim about a
    /// driver is a driver — the alternative is the list of bare names this test used to hold. It is
    /// text, not a dependency: `sipx-app` already depends on this crate, and nothing here links it.
    const DRIVER: &str = include_str!("../../sipx-app/src/host.rs");

    /// The interpreter, which composes the one §5.3 row that is neither a call's fact nor a host's.
    const INTERPRETER: &str = include_str!("../src/interpreter.rs");

    /// The driver's DSP door (`M-67`), read for the same reason and in the same way as the bridge.
    ///
    /// A graph's transitions are `sipx-media`'s facts and not `sipx-call`'s, so no `CallEvent`
    /// carries one and the bridge cannot have an arm. The file that turns a `GraphTransition` into
    /// a §5.3 row is the producer, and this is that file.
    const HOST_DSP: &str = include_str!("../../sipx-app/src/dsp.rs");

    /// §5.3 rows no `sipx-call` event carries, each named beside **the source that composes it**.
    ///
    /// - `call.incoming` — a call's event stream begins after the INVITE matched an app; the
    ///   arrival is the host's fact, not one of the call's.
    /// - `call.gather.finished` — §6.2's `gather` is composed from `call.dtmf` by the interpreter
    ///   against the instruction's own bounds. No `CallEvent` says a gather resolved.
    /// - `call.dial.finished` — about the second leg the driver created, which is a different call
    ///   from the one whose stream this bridges, and whose outcome is not on a `CallEvent` at all:
    ///   `sipx-call` reports a refusal as the `Err` of the dial, never as an event (`M-103`).
    ///
    /// `call.bridged` and `call.unbridged` were here until `M-99` and are not any more. The reason
    /// given for them — §5.3 names the other `leg` and `C-6`'s events do not carry it — was a
    /// missing name rather than a missing event, and this list is for rows no call event reports at
    /// all. Nothing composed them either, so the entry was a claim about a producer that did not
    /// exist; they now have an arm, and this test's other branch is what keeps it.
    ///
    /// The five `call.dsp.*` rows join the list for the same reason as the first three and with the
    /// same obligation: a graph's transitions belong to `sipx-media`, no `CallEvent` carries one,
    /// and the file that composes them has to contain the construction (`M-67`).
    const COMPOSED_BY_THE_DRIVER: [(&str, &str, &str); 8] = [
        ("call.incoming", "crates/sipx-app/src/host.rs", DRIVER),
        (
            "call.gather.finished",
            "crates/sipx-app-protocol/src/interpreter.rs",
            INTERPRETER,
        ),
        ("call.dial.finished", "crates/sipx-app/src/host.rs", DRIVER),
        ("call.dsp.activated", "crates/sipx-app/src/dsp.rs", HOST_DSP),
        (
            "call.dsp.configured",
            "crates/sipx-app/src/dsp.rs",
            HOST_DSP,
        ),
        ("call.dsp.bypassed", "crates/sipx-app/src/dsp.rs", HOST_DSP),
        ("call.dsp.removed", "crates/sipx-app/src/dsp.rs", HOST_DSP),
        ("call.dsp.refused", "crates/sipx-app/src/dsp.rs", HOST_DSP),
    ];

    let rows: BTreeSet<String> = table_after("5.3 Event types")
        .iter()
        .flat_map(|row| backticked(&row[0]))
        .collect();
    assert!(rows.len() > COMPOSED_BY_THE_DRIVER.len(), "§5.3 lost rows");

    for kind in sipx_app_protocol::testing::one_of_every_event() {
        let type_name = kind.type_name().to_owned();
        assert!(rows.contains(&type_name), "{type_name} is not a §5.3 row");

        // `Ringing { reliable: true }` names its variant first; everything after the identifier is
        // the fixture's payload and not part of the spelling `src/call.rs` writes.
        let variant: String = format!("{kind:?}")
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let arm = format!("EventKind::{variant}");

        let composed = COMPOSED_BY_THE_DRIVER
            .iter()
            .find(|(row, _, _)| *row == type_name);
        if let Some((_, path, source)) = composed {
            assert!(
                !BRIDGE.contains(&arm),
                "{type_name} is listed as the driver's to compose and `src/call.rs` produces \
                 `{arm}` — say which it is in one place"
            );
            assert!(
                source.contains(&arm),
                "{type_name} is listed as composed by `{path}` and that file never writes \
                 `{arm}`, so nothing produces the row at all — this list says *where* a row comes \
                 from, and a name on it with no producer behind it is the defect `M-98`, `M-99` \
                 and `M-103` each closed once"
            );
        } else {
            assert!(
                BRIDGE.contains(&arm),
                "§5.3 lists {type_name} and the bridge has no arm producing `{arm}`, so no call \
                 can reach it — add the arm, or name the row in `COMPOSED_BY_THE_DRIVER` with the \
                 source that composes it instead"
            );
        }
    }
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
    assert_eq!(
        lists.len(),
        9,
        "§5.3 should carry nine inline lists: {lists:?}"
    );

    for (field, values) in lists {
        let implemented: Vec<String> = match field.as_str() {
            // `M-58`'s two rows carry one list each, which is the shape this test can check: two
            // lists on one row would both key on that row's type and only one of them could match.
            "call.voice.started" => [AudioDirection::Inbound, AudioDirection::Outbound]
                .iter()
                .map(|d| d.as_str().to_owned())
                .collect(),
            "call.voice.ended" => [VoiceEndCause::Hangover, VoiceEndCause::Cut]
                .iter()
                .map(|c| tag_of(&c.to_json()))
                .collect(),
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
            "call.dsp.bypassed" => DspBypassCause::all()
                .iter()
                .map(|c| c.as_str().to_owned())
                .collect(),
            "call.dsp.removed" => DspTeardownCause::all()
                .iter()
                .map(|c| c.as_str().to_owned())
                .collect(),
            "call.dsp.refused" => DspRefusal::all()
                .iter()
                .map(|r| r.as_str().to_owned())
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
        .or_else(|| {
            value
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_owned)
        })
        .expect("a tagged value is a name or an object with one")
}

/// **§5.2.1** — the `voice` member is exactly the record the crate writes (`M-84`).
///
/// Read from two places in the section that a change would have to move together: the snapshot
/// example, which is what a reader copies, and §5.2.1's table, which is what a reader looks a member
/// up in. A field added to [`VoiceThresholds`] and to neither fails here, which is what makes
/// widening the threshold surface a reviewable diff against this specification rather than a quiet
/// one.
#[test]
fn section_5_2_1_documents_exactly_the_voice_members_the_crate_writes() {
    let written: BTreeSet<String> = sipx_app_protocol::testing::reference_thresholds()
        .to_json()
        .as_object()
        .expect("the record is an object")
        .keys()
        .cloned()
        .collect();

    let example = json_after("5.2 The call snapshot");
    let shown: BTreeSet<String> = example
        .get("voice")
        .and_then(sipx_app_protocol::json::Json::as_object)
        .expect("§5.2's example shows the `voice` member")
        .keys()
        .cloned()
        .collect();
    assert_eq!(shown, written, "§5.2's example and the record differ");

    let mut documented = BTreeSet::new();
    for row in table_after("5.2.1") {
        for member in backticked(&row[0]) {
            documented.insert(member);
        }
        assert!(!row[1].is_empty(), "§5.2.1 row {:?} says nothing", row[0]);
    }
    assert_eq!(documented, written, "§5.2.1's table and the record differ");
}

/// **§5.2.1** — the threshold surface carries no audio and no field audio could be rebuilt from.
///
/// This is the check that section names, and it is deliberately a property of the *serialization*
/// rather than of the Rust type: what an application receives is the JSON, so the rule is stated
/// over the JSON. Every member is a number, a `null`, or one of the two words
/// [`AudioDirection`] spells. Samples could only arrive as an array of numbers or as text — the
/// first is refused because no member may be an array, the second because no string outside that
/// closed pair is admitted — so a member carrying audio cannot be added without failing here.
///
/// [call-audio-processing.md](../../../docs/specs/call-audio-processing.md) §3.3 and §8.1 mean
/// there is no retained audio upstream to send in the first place. This is the guard for the case
/// where that stops being true.
#[test]
fn the_voice_member_carries_only_counts_and_amplitudes() {
    let directions: BTreeSet<&str> = [AudioDirection::Inbound, AudioDirection::Outbound]
        .iter()
        .map(|direction| direction.as_str())
        .collect();

    // Both shapes the record appears in, because the wire surface is both of them: §5.2's read and
    // §5.3's announcement carry the same object and it has to be the same object in both.
    let mut objects = vec![sipx_app_protocol::testing::reference_thresholds().to_json()];
    let announcement = EventKind::VoiceThresholds {
        sample_time: 1_600,
        thresholds: sipx_app_protocol::testing::reference_thresholds(),
    }
    .to_json();
    assert!(
        matches!(
            announcement.get("sample_time"),
            Some(sipx_app_protocol::json::Json::Int(_))
        ),
        "the position the value took effect at is a sample count"
    );
    objects.push(
        announcement
            .get("thresholds")
            .expect("the announcement carries the record")
            .clone(),
    );

    for object in objects {
        let members = object.as_object().expect("the record is an object");
        assert!(!members.is_empty(), "an empty record proves nothing");
        for (name, value) in members {
            match value {
                sipx_app_protocol::json::Json::Int(_) | sipx_app_protocol::json::Json::Null => {}
                sipx_app_protocol::json::Json::Str(text) => assert!(
                    directions.contains(text.as_str()),
                    "`{name}` is a string that is not a direction: {text:?} — the threshold \
                     surface carries counts and amplitudes, and nothing audio could be rebuilt from"
                ),
                other => panic!(
                    "`{name}` is neither a count, a null, nor a direction: {}",
                    other.to_text()
                ),
            }
        }
    }
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
    let implemented: BTreeSet<String> = [
        "timeout_ms",
        "on_timeout",
        "on_5xx",
        "on_unreachable",
        "on_4xx",
    ]
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
    assert_eq!(
        policy.on(Failure::ServerError),
        OnFailure::Reject { status: 500 }
    );
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
        assert!(!row[2].is_empty(), "§11 row {id} asserts nothing");
    }
}

/// The spec's wire line remains experimental after the Rust crate's first real caller graduates
/// its API. The two claims are deliberately different and both must remain visible.
#[test]
fn the_spec_and_the_crate_agree_that_the_wire_is_experimental() {
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
        lib.contains("wire line remains") && lib.contains("Experimental"),
        "the supported crate docs must retain the wire's experimental qualification"
    );
}
