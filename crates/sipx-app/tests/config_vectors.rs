//! The host configuration's vector set (story `A-1`).
//!
//! [`specs/host-config.md`](../../../docs/specs/host-config.md) numbers its normative points `N1`
//! … `N12` and lists a vector for each. This file is where those vectors run: a document is a
//! `&str`, an outcome is data, and every point is covered *mechanically* — a point added to the
//! spec with no vector fails [`the_vector_set_covers_every_normative_point`] rather than going
//! quietly unproven.
//!
//! Two of the cases run the `A-7` harness rather than only comparing structs, because the claim
//! they make is about a **call**: a declared `on_5xx` has to end one, and a reload has to leave a
//! live one alone. Nothing here touches a socket, a runtime or a clock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use sipx_app::config::vectors;
use sipx_app::config::{Admission, ConfigError, Grants, HostConfig, NORMATIVE_POINTS, Running};
use sipx_app::harness::policy::{Failure, FailurePolicy, OnFailure};

/// Every vector in the spec. Each carries its own expectation, so a failure names itself.
#[test]
fn every_host_config_vector_holds() {
    for vector in vectors::all() {
        if let Err(mismatch) = vector.check() {
            panic!("{mismatch}");
        }
    }
}

/// Acceptance point 1, mechanically: *every* normative point has at least one vector. Adding `N13`
/// to the spec without writing a vector for it fails here.
#[test]
fn the_vector_set_covers_every_normative_point() {
    let vectors = vectors::all();
    for (id, what) in NORMATIVE_POINTS {
        assert!(
            vectors.iter().any(|vector| vector.points.contains(&id)),
            "no vector pins {id} ({what})"
        );
    }
}

/// And no vector claims a point the spec does not have — a typo in a tag would otherwise make the
/// coverage test above pass by covering nothing.
#[test]
fn no_vector_claims_a_point_the_spec_does_not_state() {
    for vector in vectors::all() {
        for point in vector.points {
            assert!(
                NORMATIVE_POINTS.iter().any(|(id, _)| id == point),
                "{} claims {point}, which is not a normative point",
                vector.name
            );
        }
    }
}

/// Acceptance point 2: the `on_failure` table's keys are the contract's §9.2 knobs **by name**, and
/// the file accepts no other.
///
/// Two assertions, deliberately from opposite directions. The first holds the derived list against
/// §9.2's names written out — the literal side, which is what "byte-identical" means. The second
/// holds it against the harness's own enumeration, so a fifth knob added to §9.2 fails here until
/// the document can set it.
#[test]
fn the_failure_keys_are_the_contracts_own() {
    assert_eq!(
        sipx_app::config::failure_keys(),
        vec![
            "timeout_ms",
            "on_timeout",
            "on_5xx",
            "on_unreachable",
            "on_4xx"
        ],
        "the on_failure table's keys are §9.2's, verbatim"
    );

    for failure in Failure::all() {
        assert!(
            sipx_app::config::failure_keys().contains(&failure.knob()),
            "§9.2 declares {}, and the document cannot set it",
            failure.knob()
        );
    }
}

/// Acceptance point 2, the other half: an app that declares nothing gets §9.2's defaults exactly.
#[test]
fn an_undeclared_app_gets_the_contracts_defaults() {
    let config = HostConfig::parse(vectors::REFERENCE).unwrap();
    let app = config.app("notifier").unwrap();

    assert_eq!(app.failure, FailurePolicy::declared());
}

/// Acceptance point 3: the reference document references secrets **by name** and contains no
/// material. The second assertion is the interesting one — it is over the document's own bytes.
#[test]
fn the_reference_document_is_committable() {
    let config = HostConfig::parse(vectors::REFERENCE).unwrap();

    assert_eq!(
        config.secret_names(),
        vec![
            "notifier-bearer",
            "reception-hook-2026-04",
            "reception-hook-2026-07"
        ],
        "every secret is a name"
    );

    for marker in [
        "-----BEGIN",
        "PRIVATE KEY",
        "Bearer ",
        "password",
        "Authorization",
    ] {
        assert!(
            !vectors::REFERENCE.contains(marker),
            "the reference document must be committable, found {marker:?}"
        );
    }
}

/// A pasted key does not fit through the name grammar. This is the property that makes the claim
/// above structural rather than a matter of the author's care.
#[test]
fn a_secret_value_cannot_be_written_where_a_secret_name_goes() {
    let document = vectors::REFERENCE.replace(
        r#"signing_secrets = ["reception-hook-2026-07", "reception-hook-2026-04"]"#,
        r#"signing_secrets = ["MIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEA1+/="]"#,
    );

    let error = HostConfig::parse(&document).unwrap_err();
    assert_eq!(error.code(), "not-a-secret-name", "{error}");
}

/// Acceptance point 1's reload half, as its own test rather than only as a vector: a refused reload
/// changes **nothing**. The running configuration is compared before and after.
#[test]
fn a_refused_reload_leaves_the_running_configuration_untouched() {
    let mut running = Running::start(HostConfig::parse(vectors::REFERENCE).unwrap());
    let before = running.current().clone();

    let error = running
        .reload("[listener.pbx]\nbind = nonsense\n")
        .unwrap_err();
    assert_eq!(error.code(), "syntax", "{error}");
    assert_eq!(running.current(), &before);
}

/// Acceptance point 1's last item: a live call keeps the policy it was admitted with, and the
/// proof is a call — the same 5xx ends the fresh call and does not end the live one.
#[test]
fn a_reload_never_changes_a_live_calls_policy() {
    let mut running = Running::start(HostConfig::parse(vectors::REFERENCE).unwrap());

    let Admission::App(live) = running.admit("live-1", "pbx") else {
        panic!("the pbx listener routes to an app");
    };
    assert_eq!(live.failure.on_5xx, OnFailure::Continue);

    running.reload(vectors::REFERENCE_ON_5XX_HANGUP).unwrap();

    assert_eq!(
        running.policy_of("live-1").unwrap().failure.on_5xx,
        OnFailure::Continue,
        "the live call kept what it started with"
    );

    let Admission::App(fresh) = running.admit("live-2", "pbx") else {
        panic!("the pbx listener still routes to an app");
    };
    assert_eq!(
        fresh.failure.on_5xx,
        OnFailure::Hangup,
        "the new call did not"
    );
}

/// A listener with no app refuses with the status it declares. "Silence is not an option" is a
/// total function, so this is checked rather than assumed.
#[test]
fn a_listener_with_no_app_refuses_with_the_declared_status() {
    let mut running = Running::start(HostConfig::parse(vectors::REFERENCE).unwrap());

    assert_eq!(running.admit("c-1", "trunk"), Admission::Refuse(503));
    assert_eq!(running.admit("c-2", "nowhere"), Admission::NoSuchListener);
}

/// Deny-by-default, as a value: an app that declares no grants may use inline audio only, may set
/// no `dial` header and may not originate.
#[test]
fn an_app_that_grants_nothing_is_granted_nothing() {
    let config = HostConfig::parse(vectors::REFERENCE).unwrap();

    assert_eq!(config.app("notifier").unwrap().grants, Grants::denied());
    assert!(Grants::denied().play_roots.is_empty());
    assert!(Grants::denied().dial_headers.is_empty());
    assert!(!Grants::denied().originate);
}

/// Acceptance point 4: the schema does not decide the open question. One app per document and many
/// apps in one document are both valid, and the shared app means the same in each — which is what
/// phase 4 needs preserved whichever way the stance lands.
#[test]
fn a_document_means_the_same_thing_with_one_app_and_with_many() {
    let one = HostConfig::parse(vectors::ONE_APP).unwrap();
    let many = HostConfig::parse(vectors::MANY_APPS).unwrap();

    assert_eq!(one.apps().count(), 1);
    assert!(many.apps().count() > 1);
    assert_eq!(one.app("reception"), many.app("reception"));
    assert_eq!(one.listener("pbx"), many.listener("pbx"));
}

/// The syntax refuses what it does not define, and says where. A reason with no line is a reason an
/// operator cannot act on.
#[test]
fn a_syntax_error_names_the_line_that_caused_it() {
    let error =
        HostConfig::parse("[listener.pbx]\nprotocol = \"sip\"\nbind = 0.0.0.0:5060\n").unwrap_err();

    assert_eq!(error.code(), "syntax");
    assert_eq!(error.line(), Some(3), "{error}");
}

/// Every rejection code a vector names is one the error type can actually produce, and every code
/// the error type produces has a vector. The codes are the operator-facing half of "rejected wholly
/// with the reason", so they are held to the same enumerability as the knobs.
#[test]
fn every_rejection_code_is_exercised_by_a_vector() {
    let claimed: Vec<&str> = vectors::all()
        .iter()
        .filter_map(vectors::Vector::code)
        .collect();

    for code in ConfigError::CODES {
        assert!(
            claimed.contains(&code),
            "no vector produces {code}: {claimed:?}"
        );
    }
}

/// The harness is not decoration here: a policy that came out of a document has to change what
/// happens to a call. Same app, same 5xx, opposite outcomes, one word of configuration apart.
#[test]
fn a_declared_knob_read_from_a_document_changes_what_a_call_does() {
    use sipx_app::harness::binding::{Outcome, Reply};
    use sipx_app::harness::{Conclusion, EndCause, EventKind, Scenario, Step};

    let run_with = |policy: FailurePolicy| {
        Scenario::new("a 5xx under a declared policy")
            .policy(policy)
            .then(Reply::failing(
                Duration::ZERO,
                Outcome::ServerError { status: 500 },
            ))
            .steps(vec![Step::event(0, EventKind::Incoming)])
            .until(5000)
            .run()
            .conclusion
    };

    let default = HostConfig::parse(vectors::REFERENCE).unwrap();
    let hangup = HostConfig::parse(vectors::REFERENCE_ON_5XX_HANGUP).unwrap();

    assert_eq!(
        run_with(default.app("reception").unwrap().failure.clone()),
        Conclusion::Live
    );
    assert_eq!(
        run_with(hangup.app("reception").unwrap().failure.clone()),
        Conclusion::Ended(EndCause::Hangup)
    );
}
