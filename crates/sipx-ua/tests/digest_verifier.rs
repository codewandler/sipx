//! Server Digest boundary regressions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use sipx_ua::auth::{Algorithm, Challenge, Credentials, respond};
use sipx_ua::challenge::{Authenticator, Presented, Reason, Verdict};
use std::time::Duration;

fn answer(server: &Authenticator, user: &str, cnonce: &str, count: u32, at: u64) -> Presented {
    let challenge = Challenge::parse(server.challenge_at(false, at).as_bytes(), false).unwrap();
    Presented::parse(
        respond(
            &challenge,
            &Credentials::new(user, "password"),
            "INVITE",
            "sip:example.com",
            count,
            cnonce,
        )
        .as_bytes(),
    )
    .unwrap()
}

#[test]
fn declared_realm_is_bound() {
    let mut server = Authenticator::new("example.com", [7; 32]);
    let mut presented = answer(&server, "alice", "one", 1, 100);
    presented.realm = "other.example.com".into();
    assert_eq!(
        server.verify_at(&presented, "INVITE", "password", 100),
        Verdict::Rejected(Reason::Mismatch)
    );
}

#[test]
fn unknown_named_algorithm_is_not_md5() {
    assert!(Presented::parse(br#"Digest username="alice", realm="example.com", nonce="n", uri="sip:example.com", response="r", algorithm=UNKNOWN"#).is_none());
}

#[test]
fn users_and_client_nonces_have_independent_counters() {
    let mut server = Authenticator::new("example.com", [7; 32]);
    for (user, cnonce) in [("alice", "one"), ("bob", "one"), ("alice", "two")] {
        let presented = answer(&server, user, cnonce, 1, 100);
        assert_eq!(
            server.verify_at(&presented, "INVITE", "password", 100),
            Verdict::Authenticated
        );
    }
}

#[test]
fn capacity_never_reopens_an_unexpired_replay() {
    let mut server =
        Authenticator::new("example.com", [7; 32]).with_lifetime(Duration::from_secs(10_000));
    let original = answer(&server, "alice", "one", 1, 100);
    let advanced = answer(&server, "alice", "one", 2, 100);
    assert_eq!(
        server.verify_at(&original, "INVITE", "password", 100),
        Verdict::Authenticated
    );
    assert_eq!(
        server.verify_at(&advanced, "INVITE", "password", 100),
        Verdict::Authenticated
    );
    for at in 101..=4196 {
        let presented = answer(&server, "alice", "one", 1, at);
        let _ = server.verify_at(&presented, "INVITE", "password", at);
    }
    assert_eq!(
        server.verify_at(&original, "INVITE", "password", 4196),
        Verdict::Rejected(Reason::Replay)
    );
}

use sipx_ua::auth::{DigestVerifier, VerifierError};

#[test]
fn published_rfc2617_verifier_vector() {
    let verifier = DigestVerifier::from_ha1(
        "Mufasa",
        "testrealm@host.com",
        Algorithm::Md5,
        "939e7578ed9e3c518a452acee763bce9",
    )
    .unwrap();
    let challenge = Challenge::parse(br#"Digest realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", algorithm=MD5, qop="auth""#, false).unwrap();
    let header = verifier
        .respond(&challenge, "GET", "/dir/index.html", 1, "0a4f113b")
        .unwrap();
    assert_eq!(
        Presented::parse(header.as_bytes()).unwrap().response,
        "6629fae49393a05397450978507c4ef1"
    );
}

fn algorithms() -> [Algorithm; 6] {
    [
        Algorithm::Md5,
        Algorithm::Md5Sess,
        Algorithm::Sha256,
        Algorithm::Sha256Sess,
        Algorithm::Sha512_256,
        Algorithm::Sha512_256Sess,
    ]
}

#[test]
fn all_algorithms_derive_import_and_authenticate_without_password() {
    use sha2::Digest as _;
    for algorithm in algorithms() {
        let credentials = Credentials::new("alice", "password");
        let verifier = DigestVerifier::derive(&credentials, "example.com", algorithm).unwrap();
        let ha1 = match algorithm {
            Algorithm::Md5 | Algorithm::Md5Sess => {
                format!("{:x}", md5::Md5::digest(b"alice:example.com:password"))
            }
            Algorithm::Sha256 | Algorithm::Sha256Sess => {
                format!("{:x}", sha2::Sha256::digest(b"alice:example.com:password"))
            }
            _ => format!(
                "{:x}",
                sha2::Sha512_256::digest(b"alice:example.com:password")
            ),
        };
        let imported =
            DigestVerifier::from_ha1("alice", "example.com", algorithm, &ha1.to_uppercase())
                .unwrap();
        for verifier in [verifier, imported] {
            let mut server = Authenticator::new("example.com", [7; 32]).with_algorithm(algorithm);
            let presented = answer(&server, "alice", "one", 1, 100);
            assert_eq!(
                server.verify_with_verifier_at(&presented, "INVITE", &verifier, 100),
                Verdict::Authenticated
            );
            assert_eq!(verifier.username(), "alice");
            assert_eq!(verifier.realm(), "example.com");
            assert_eq!(verifier.algorithm(), algorithm);
            let diagnostic = format!("{verifier:?}");
            assert!(!diagnostic.contains(&ha1));
            assert!(!diagnostic.contains("password"));
        }
    }
}

#[test]
fn malformed_verifiers_have_typed_redacted_errors() {
    for algorithm in algorithms() {
        for bad in [
            "secret material",
            "",
            &"0".repeat(31),
            &"0".repeat(33),
            &"g".repeat(64),
        ] {
            let error =
                DigestVerifier::from_ha1("alice", "example.com", algorithm, bad).unwrap_err();
            assert_eq!(error, VerifierError::InvalidHa1);
            assert!(!format!("{error:?} {error}").contains("secret material"));
        }
        for (user, realm) in [
            ("", "example.com"),
            ("al:ice", "example.com"),
            ("a\n", "example.com"),
            ("alice", "a\r"),
        ] {
            assert_eq!(
                DigestVerifier::derive(&Credentials::new(user, "password"), realm, algorithm)
                    .unwrap_err(),
                VerifierError::InvalidMetadata
            );
            assert_eq!(
                DigestVerifier::from_ha1(user, realm, algorithm, &"0".repeat(64)).unwrap_err(),
                VerifierError::InvalidMetadata
            );
        }
    }
}

#[test]
fn verifier_rejects_binding_method_credential_nonce_and_qop_changes() {
    let verifier = DigestVerifier::derive(
        &Credentials::new("alice", "password"),
        "example.com",
        Algorithm::Sha256,
    )
    .unwrap();
    let mut server = Authenticator::new("example.com", [7; 32]);
    let original = answer(&server, "alice", "one", 1, 100);
    for field in 0..8 {
        let mut request = original.clone();
        match field {
            0 => request.realm = "other.example.com".into(),
            1 => request.username = "bob".into(),
            2 => request.algorithm = Algorithm::Md5,
            3 => request.response = "0".repeat(64),
            4 => request.nonce.push('0'),
            5 => request.qop_auth = false,
            6 => request.nonce_count = Some(0),
            _ => request.cnonce = Some(String::new()),
        }
        assert!(matches!(
            server.verify_with_verifier_at(&request, "INVITE", &verifier, 100),
            Verdict::Rejected(_)
        ));
    }
    assert_eq!(
        server.verify_with_verifier_at(&original, "REGISTER", &verifier, 100),
        Verdict::Rejected(Reason::Mismatch)
    );
    let wrong = DigestVerifier::derive(
        &Credentials::new("alice", "wrong"),
        "example.com",
        Algorithm::Sha256,
    )
    .unwrap();
    assert_eq!(
        server.verify_with_verifier_at(&original, "INVITE", &wrong, 100),
        Verdict::Rejected(Reason::Mismatch)
    );
    let other = DigestVerifier::derive(
        &Credentials::new("alice", "password"),
        "other.example.com",
        Algorithm::Sha256,
    )
    .unwrap();
    assert_eq!(
        server.verify_with_verifier_at(&original, "INVITE", &other, 100),
        Verdict::Rejected(Reason::Mismatch)
    );
    let challenge = Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
    assert_eq!(
        other.respond(&challenge, "INVITE", "sip:example.com", 1, "one"),
        Err(VerifierError::BindingMismatch)
    );
}

#[test]
fn bounded_capacity_retains_counters_and_recovers_after_expiry_for_both_apis() {
    let verifier = DigestVerifier::derive(
        &Credentials::new("alice", "password"),
        "example.com",
        Algorithm::Sha256,
    )
    .unwrap();
    for use_verifier in [false, true] {
        let mut server =
            Authenticator::new("example.com", [7; 32]).with_lifetime(Duration::from_secs(10));
        let verify = |server: &mut Authenticator, p: &Presented, now| {
            if use_verifier {
                server.verify_with_verifier_at(p, "INVITE", &verifier, now)
            } else {
                server.verify_at(p, "INVITE", "password", now)
            }
        };
        for index in 0..4096 {
            let request = answer(&server, "alice", &index.to_string(), 1, 100);
            assert_eq!(verify(&mut server, &request, 100), Verdict::Authenticated);
        }
        let extra = answer(&server, "alice", "extra", 1, 100);
        assert_eq!(
            verify(&mut server, &extra, 100),
            Verdict::Rejected(Reason::ReplayCapacity)
        );
        let retained = answer(&server, "alice", "0", 2, 100);
        assert_eq!(verify(&mut server, &retained, 100), Verdict::Authenticated);
        assert_eq!(verify(&mut server, &retained, 100), Verdict::Authenticated);
        let old = answer(&server, "alice", "0", 1, 100);
        assert_eq!(
            verify(&mut server, &old, 100),
            Verdict::Rejected(Reason::Replay)
        );
        assert_eq!(
            verify(&mut server, &extra, 110),
            Verdict::Rejected(Reason::ReplayCapacity)
        );
        assert_eq!(verify(&mut server, &extra, 111), Verdict::Stale);
        let mut wrong = extra.clone();
        wrong.response = "0".repeat(64);
        assert_eq!(
            verify(&mut server, &wrong, 111),
            Verdict::Rejected(Reason::Mismatch)
        );
        let fresh = answer(&server, "alice", "extra", 1, 111);
        assert_eq!(verify(&mut server, &fresh, 111), Verdict::Authenticated);
    }
}

#[test]
fn adversary_malformed_present_algorithm_cannot_default_to_md5() {
    let mut server = Authenticator::new("example.com", [7; 32]).with_algorithm(Algorithm::Md5);
    let challenge = Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
    let valid = respond(
        &challenge,
        &Credentials::new("alice", "password"),
        "INVITE",
        "sip:example.com",
        1,
        "one",
    );
    let absent = valid.strip_suffix(", algorithm=MD5").unwrap();
    let original = Presented::parse(absent.as_bytes()).unwrap();
    assert_eq!(
        server.verify_at(&original, "INVITE", "password", 100),
        Verdict::Authenticated
    );
    let malformed = format!("{absent}, algorithm=\"UNKNOWN");
    assert!(
        Presented::parse(malformed.as_bytes()).is_none(),
        "a present unterminated algorithm must not be treated as an absent MD5 default"
    );
}

#[test]
fn adversary_password_api_preserves_empty_realm_compatibility() {
    let mut server = Authenticator::new("", [7; 32]);
    let challenge = Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
    assert_eq!(challenge.realm, "");
    let header = respond(
        &challenge,
        &Credentials::new("alice", "password"),
        "INVITE",
        "sip:example.com",
        1,
        "one",
    );
    let presented = Presented::parse(header.as_bytes()).unwrap();
    assert_eq!(
        server.verify_at(&presented, "INVITE", "password", 100),
        Verdict::Authenticated
    );
}

#[test]
fn adversary_imported_hashes_match_independent_six_algorithm_vectors() {
    // RFC 7616 formulas evaluated independently with hashlib, using RFC 2617's input tuple.
    // Only the MD5 output is a published RFC vector; the other five are independently calculated.
    let cases = [
        (
            Algorithm::Md5,
            "939e7578ed9e3c518a452acee763bce9",
            "6629fae49393a05397450978507c4ef1",
        ),
        (
            Algorithm::Md5Sess,
            "939e7578ed9e3c518a452acee763bce9",
            "8e3825c57e897f5a0dec6c2d4e5059d0",
        ),
        (
            Algorithm::Sha256,
            "3ba6cd94661c5ef34598040c868f13b8775df29109986be50ad35ae537dd3aa4",
            "5abdd07184ba512a22c53f41470e5eea7dcaa3a93a59b630c13dfe0a5dc6e38b",
        ),
        (
            Algorithm::Sha256Sess,
            "3ba6cd94661c5ef34598040c868f13b8775df29109986be50ad35ae537dd3aa4",
            "b8822e12417cb7750f4e2b8515f0dcf25b7dd26993e80bee1426201446a7f59b",
        ),
        (
            Algorithm::Sha512_256,
            "4f89a1c293dd533bc27546c1da0608df9efcaa6bd1c350edca70a01c8a823360",
            "f23c08ec7334a881f8286e68450ddbd9f0cd91c41481f0e1433604da8113c6dc",
        ),
        (
            Algorithm::Sha512_256Sess,
            "4f89a1c293dd533bc27546c1da0608df9efcaa6bd1c350edca70a01c8a823360",
            "0d21f0db3ec5cda5b850c0afa3bc29b4a3c5a6191959ff1baf511d4b38eb6b1e",
        ),
    ];
    for (algorithm, ha1, expected) in cases {
        let verifier =
            DigestVerifier::from_ha1("Mufasa", "testrealm@host.com", algorithm, ha1).unwrap();
        let challenge = Challenge {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            opaque: None,
            algorithm,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let result = verifier
            .respond(&challenge, "GET", "/dir/index.html", 1, "0a4f113b")
            .unwrap();
        assert_eq!(
            Presented::parse(result.as_bytes()).unwrap().response,
            expected
        );
    }
}

#[test]
fn adversary_replay_identity_is_length_framed() {
    let mut server = Authenticator::new("example.com", [7; 32]);
    // Concatenating these username/client-nonce pairs without lengths would alias.
    for (user, cnonce) in [("ab", "c"), ("a", "bc")] {
        let verifier = DigestVerifier::derive(
            &Credentials::new(user, "password"),
            "example.com",
            Algorithm::Sha256,
        )
        .unwrap();
        let request = answer(&server, user, cnonce, 2, 100);
        assert_eq!(
            server.verify_with_verifier_at(&request, "INVITE", &verifier, 100),
            Verdict::Authenticated
        );
        let old = answer(&server, user, cnonce, 1, 100);
        assert_eq!(
            server.verify_with_verifier_at(&old, "INVITE", &verifier, 100),
            Verdict::Rejected(Reason::Replay)
        );
    }
}

#[test]
fn adversary_escaped_metadata_is_bound_without_secret_diagnostics() {
    let user = "alice\\\"";
    let realm = "example.com\\\"";
    let credentials = Credentials::new(user, "unique-secret-password");
    let verifier = DigestVerifier::derive(&credentials, realm, Algorithm::Sha256).unwrap();
    let mut server = Authenticator::new(realm, [7; 32]);
    let challenge = Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
    assert_eq!(challenge.realm, realm);
    let header = respond(
        &challenge,
        &credentials,
        "INVITE",
        "sip:example.com",
        1,
        "one",
    );
    let presented = Presented::parse(header.as_bytes()).unwrap();
    assert_eq!(presented.username, user);
    assert_eq!(
        server.verify_with_verifier_at(&presented, "INVITE", &verifier, 100),
        Verdict::Authenticated
    );
    assert!(!format!("{verifier:?}").contains("unique-secret-password"));
    let changed = Challenge {
        realm: "other.example.com".into(),
        ..challenge
    };
    let error = verifier
        .respond(&changed, "INVITE", "sip:example.com", 1, "one")
        .unwrap_err();
    assert_eq!(error, VerifierError::BindingMismatch);
    assert_eq!(error.to_string(), "Digest verifier binding mismatch");
}

#[test]
fn malformed_authorization_parameter_lists_are_never_absent_metadata() {
    let base = r#"Digest username="alice", realm="example.com", nonce="n", uri="sip:example.com", response="r""#;
    let malformed = [
        "algorithm=\"UNKNOWN",
        "algorithm=\"MD5\\",
        "algorithm",
        "algorithm=",
        "algorithm=\"\"",
        "algorithm=MD5 garbage",
        "algorithm=MD5=unknown",
        "algorithm=\"MD5\"garbage",
        "algorithm=MD5,",
        "algorithm=MD5,,qop=auth",
        "algorithm=MD5, ALGORITHM=MD5",
        "algorithm=MD5, algorithm=UNKNOWN",
        "algorithm=MD5, realm=\"other\"",
        "bad name=MD5",
        "=MD5",
        "extension=\"unfinished",
        "extension=MD5, algorithm",
        "algorithm=MD5\r\n",
    ];
    let accepted: Vec<_> = malformed
        .into_iter()
        .filter(|suffix| Presented::parse(format!("{base}, {suffix}").as_bytes()).is_some())
        .collect();
    assert!(
        accepted.is_empty(),
        "malformed values accepted: {accepted:?}"
    );
    assert!(Presented::parse(base.replace("Digest ", "Digestive ").as_bytes()).is_none());
    let mut invalid_utf8 = format!("{base}, algorithm=MD5, extension=\"").into_bytes();
    invalid_utf8.push(0xff);
    invalid_utf8.push(b'"');
    assert!(Presented::parse(&invalid_utf8).is_none());
}

#[test]
fn absent_algorithm_and_well_formed_extensions_remain_valid() {
    for scheme in ["Digest", "digest", "DIGEST"] {
        let wire = format!(
            "{scheme}\t username=\"alice\", realm=\"example.com\", nonce=\"n\", uri=\"sip:example.com\", response=\"r\", extension=\"a,b\\\"c\", token=value\t"
        );
        let parsed = Presented::parse(wire.as_bytes()).unwrap();
        assert_eq!(parsed.algorithm, Algorithm::Md5);
    }
}

#[test]
fn empty_realm_verifiers_preserve_exact_binding() {
    use sha2::Digest as _;
    let derived = DigestVerifier::derive(
        &Credentials::new("alice", "password"),
        "",
        Algorithm::Sha256,
    )
    .unwrap();
    let imported = DigestVerifier::from_ha1(
        "alice",
        "",
        Algorithm::Sha256,
        &format!("{:x}", sha2::Sha256::digest(b"alice::password")),
    )
    .unwrap();
    for verifier in [derived, imported] {
        let mut server = Authenticator::new("", [7; 32]);
        let request = answer(&server, "alice", "one", 1, 100);
        assert_eq!(
            server.verify_with_verifier_at(&request, "INVITE", &verifier, 100),
            Verdict::Authenticated
        );
        let mut changed = request.clone();
        changed.realm = "example.com".into();
        assert_eq!(
            server.verify_with_verifier_at(&changed, "INVITE", &verifier, 100),
            Verdict::Rejected(Reason::Mismatch)
        );
        assert_eq!(
            server.verify_at(&changed, "INVITE", "password", 100),
            Verdict::Rejected(Reason::Mismatch)
        );
    }
}

#[test]
fn explicit_export_matches_published_base_ha1_without_diagnostic_leak() {
    let verifier = DigestVerifier::derive(
        &Credentials::new("Mufasa", "Circle Of Life"),
        "testrealm@host.com",
        Algorithm::Md5,
    )
    .unwrap();
    assert_eq!(verifier.expose_ha1(), "939e7578ed9e3c518a452acee763bce9");
    assert!(!format!("{verifier:?}").contains(verifier.expose_ha1()));
}

#[test]
fn exported_bound_record_authenticates_after_original_credentials_are_dropped() {
    for algorithm in algorithms() {
        let mut server = Authenticator::new("example.com", [7; 32]).with_algorithm(algorithm);
        let (username, realm, stored_algorithm, secret, presented) = {
            let credentials = Credentials::new("alice", "password");
            let verifier = DigestVerifier::derive(&credentials, "example.com", algorithm).unwrap();
            let challenge =
                Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
            let presented = Presented::parse(
                respond(
                    &challenge,
                    &credentials,
                    "INVITE",
                    "sip:example.com",
                    1,
                    "one",
                )
                .as_bytes(),
            )
            .unwrap();
            (
                verifier.username().to_owned(),
                verifier.realm().to_owned(),
                verifier.algorithm(),
                verifier.expose_ha1().to_owned(),
                presented,
            )
        }; // Plaintext credentials and the original verifier are dropped here.
        let restored =
            DigestVerifier::from_ha1(&username, &realm, stored_algorithm, &secret).unwrap();
        assert_eq!(
            server.verify_with_verifier_at(&presented, "INVITE", &restored, 100),
            Verdict::Authenticated
        );
        assert!(!format!("{restored:?}").contains(&secret));
    }
}

#[test]
fn adversary_final_malformed_parameters_refuse_at_every_field_boundary() {
    let server = Authenticator::new("example.com", [7; 32]);
    let challenge = Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
    let header = respond(
        &challenge,
        &Credentials::new("alice", "password"),
        "INVITE",
        "sip:example.com",
        1,
        "one",
    );
    let fields: Vec<_> = header
        .strip_prefix("Digest ")
        .unwrap()
        .split(", ")
        .collect();
    let original = Presented::parse(header.as_bytes()).unwrap();
    assert_eq!(original.algorithm, Algorithm::Sha256);
    let malformed = [
        "extension",
        "extension=",
        "extension=\"unterminated",
        "extension=\"dangling\\",
        "extension=\"x\"junk",
        "extension=bad value",
        "extension=\"\u{7f}\"",
        "extension=\"\u{85}\"",
        "extension=\"\\\n\"",
        "extension=token,,other=value",
    ];
    for boundary in 0..=fields.len() {
        for bad in malformed {
            let mut changed = fields.clone();
            changed.insert(boundary, bad);
            let wire = format!("Digest {}", changed.join(", "));
            assert!(
                Presented::parse(wire.as_bytes()).is_none(),
                "accepted boundary {boundary}: {bad:?}"
            );
        }
    }
    for duplicate in [
        "USERNAME=\"alice\"",
        "REALM=\"example.com\"",
        "NONCE=\"n\"",
        "URI=\"sip:example.com\"",
        "RESPONSE=\"r\"",
        "QOP=auth",
        "NC=00000001",
        "CNONCE=\"one\"",
        "ALGORITHM=SHA-256",
    ] {
        assert!(Presented::parse(format!("{header}, {duplicate}").as_bytes()).is_none());
    }
    let valid = format!("{header}, extension=\"comma, equals= and escaped\\\"quote\"");
    assert_eq!(Presented::parse(valid.as_bytes()).unwrap(), original);
}

#[test]
fn adversary_final_durable_export_roundtrip_preserves_binding_and_redaction() {
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(directory).unwrap();
    for algorithm in algorithms() {
        for realm in ["", "example.com"] {
            let mut server = Authenticator::new(realm, [7; 32]).with_algorithm(algorithm);
            let challenge =
                Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
            let (record, request) = {
                let credentials = Credentials::new("alice", "durable-secret-password");
                let verifier = DigestVerifier::derive(&credentials, realm, algorithm).unwrap();
                let imported_uppercase = DigestVerifier::from_ha1(
                    "alice",
                    realm,
                    algorithm,
                    &verifier.expose_ha1().to_uppercase(),
                )
                .unwrap();
                assert_eq!(imported_uppercase.expose_ha1(), verifier.expose_ha1());
                assert!(!format!("{imported_uppercase:?}").contains(verifier.expose_ha1()));
                let record = format!(
                    "{}\n{}\n{}\n{}\n",
                    verifier.username(),
                    verifier.realm(),
                    verifier.algorithm().as_str(),
                    verifier.expose_ha1()
                );
                let request = Presented::parse(
                    respond(
                        &challenge,
                        &credentials,
                        "INVITE",
                        "sip:example.com",
                        1,
                        "one",
                    )
                    .as_bytes(),
                )
                .unwrap();
                (record, request)
            };
            let path = directory.join(format!(
                "digest-verifier-{}-{}.record",
                std::process::id(),
                algorithm.as_str()
            ));
            std::fs::write(&path, record).unwrap();
            let restored_record = std::fs::read_to_string(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            let mut fields = restored_record.lines();
            let user = fields.next().unwrap();
            let stored_realm = fields.next().unwrap();
            let stored_algorithm = Algorithm::parse(fields.next().unwrap()).unwrap();
            let secret = fields.next().unwrap();
            assert!(fields.next().is_none());
            let restored =
                DigestVerifier::from_ha1(user, stored_realm, stored_algorithm, secret).unwrap();
            assert_eq!(restored.realm(), realm);
            assert_eq!(restored.algorithm(), algorithm);
            assert_eq!(
                server.verify_with_verifier_at(&request, "INVITE", &restored, 100),
                Verdict::Authenticated
            );
            assert!(!format!("{restored:?}").contains(secret));
            assert!(!format!("{restored:?}").contains("durable-secret-password"));
            let mut changed = request.clone();
            changed.realm = "other.example.com".into();
            assert_eq!(
                server.verify_with_verifier_at(&changed, "INVITE", &restored, 100),
                Verdict::Rejected(Reason::Mismatch)
            );
            changed = request.clone();
            changed.username = "bob".into();
            assert_eq!(
                server.verify_with_verifier_at(&changed, "INVITE", &restored, 100),
                Verdict::Rejected(Reason::Mismatch)
            );
            let changed_challenge = Challenge {
                algorithm: if algorithm == Algorithm::Md5 {
                    Algorithm::Md5Sess
                } else {
                    Algorithm::Md5
                },
                ..challenge
            };
            assert_eq!(
                restored.respond(&changed_challenge, "INVITE", "sip:example.com", 1, "one"),
                Err(VerifierError::BindingMismatch)
            );
            let corrupt = format!("{secret}x");
            let error = DigestVerifier::from_ha1(user, stored_realm, stored_algorithm, &corrupt)
                .unwrap_err();
            assert_eq!(error, VerifierError::InvalidHa1);
            assert!(!format!("{error:?} {error}").contains(secret));
        }
    }
}
