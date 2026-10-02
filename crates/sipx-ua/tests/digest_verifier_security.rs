//! Password and persisted-verifier authentication share one guarded replay transition.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sipx_ua::auth::{Algorithm, Challenge, Credentials, DigestVerifier, respond};
use sipx_ua::challenge::{Authenticator, Presented, Reason, Verdict};

#[test]
fn alternating_credential_apis_share_history_without_failed_request_poisoning() {
    for algorithm in [
        Algorithm::Md5,
        Algorithm::Md5Sess,
        Algorithm::Sha256,
        Algorithm::Sha256Sess,
        Algorithm::Sha512_256,
        Algorithm::Sha512_256Sess,
    ] {
        for verifier_first in [false, true] {
            let credentials = Credentials::new("alice", "password");
            let verifier = DigestVerifier::derive(&credentials, "example.com", algorithm).unwrap();
            let mut server = Authenticator::new("example.com", [7; 32]).with_algorithm(algorithm);
            let challenge =
                Challenge::parse(server.challenge_at(false, 100).as_bytes(), false).unwrap();
            let answer = |count| {
                Presented::parse(
                    respond(
                        &challenge,
                        &credentials,
                        "INVITE",
                        "sip:example.com",
                        count,
                        "one",
                    )
                    .as_bytes(),
                )
                .unwrap()
            };
            let verify = |server: &mut Authenticator, p: &Presented, method, use_verifier| {
                if use_verifier {
                    server.verify_with_verifier_at(p, method, &verifier, 100)
                } else {
                    server.verify_at(p, method, "password", 100)
                }
            };
            let first = answer(1);
            assert_eq!(
                verify(&mut server, &first, "INVITE", verifier_first),
                Verdict::Authenticated
            );
            assert_eq!(
                verify(&mut server, &first, "INVITE", !verifier_first),
                Verdict::Authenticated,
                "an identical retransmission retains the same verdict across APIs"
            );
            // A fully signed higher count for a different method must not advance history.
            let high = answer(u32::MAX);
            for use_verifier in [false, true] {
                assert_eq!(
                    verify(&mut server, &high, "REGISTER", use_verifier),
                    Verdict::Rejected(Reason::Mismatch)
                );
                let mut wrong_response = high.clone();
                wrong_response.response.replace_range(..1, "!");
                assert_eq!(
                    verify(&mut server, &wrong_response, "INVITE", use_verifier),
                    Verdict::Rejected(Reason::Mismatch)
                );
                let mut wrong_realm = high.clone();
                wrong_realm.realm = "other.example.com".into();
                assert_eq!(
                    verify(&mut server, &wrong_realm, "INVITE", use_verifier),
                    Verdict::Rejected(Reason::Mismatch)
                );
            }
            assert_eq!(
                verify(&mut server, &answer(2), "INVITE", !verifier_first),
                Verdict::Authenticated,
                "failed higher counts must leave legitimate progress possible"
            );
            for use_verifier in [false, true] {
                assert_eq!(
                    verify(&mut server, &first, "INVITE", use_verifier),
                    Verdict::Rejected(Reason::Replay),
                    "neither API may bypass history written through the other"
                );
            }
        }
    }
}
