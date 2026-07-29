//! GRUUs — Globally Routable User Agent URIs (RFC 5627), at the URI level.
//!
//! A GRUU is an ordinary SIP URI with one extra URI parameter. §4.5: "A GRUU is identified by
//! the presence of the 'gr' URI parameter, and this URI parameter might or might not have a
//! value." Nothing here takes one apart beyond that, because §4.2 requires a UA to treat the
//! user and host parts as it received them.
//!
//! What is here is a **comparison**, and it exists because RFC 3261's is the wrong one for this
//! job. §5.4 states it plainly: "A public GRUU will always be equivalent to the AOR based on
//! URI equality rules", the reason being that §19.1.4 ignores a URI parameter that appears in
//! only one of the two URIs. So a UA deciding "was this request sent to *my* GRUU?" with
//! [`Uri::equivalent`] alone would say yes to a request addressed at the plain address of
//! record — and the address of record names every device the user has registered, which is
//! exactly the set a GRUU exists to narrow down to one.
//!
//! Obtaining and using GRUUs is a registration concern and lives in `sipx-ua`; minting them is
//! a registrar's job (§5), and sipx is not a registrar.

use crate::Uri;

/// The URI parameter that makes a URI a GRUU.
///
/// §7's grammar: `gr-param = "gr" [ "=" pvalue ]`.
pub const GR_PARAM: &str = "gr";

/// Whether this URI is a GRUU (§4.5).
///
/// Presence is the entire test. A valueless `gr` is as much a GRUU as one carrying a value —
/// §7 makes the value optional, and it is the form a registrar commonly mints temporary GRUUs
/// in, where the whole URI is opaque and there is nothing for a value to add.
#[must_use]
pub fn is_gruu(uri: &Uri) -> bool {
    uri.params().is_some_and(|params| params.contains(GR_PARAM))
}

/// The value of the `gr` parameter, when it has one.
///
/// Returned raw and deliberately not interpreted. A *public* GRUU's value happens to be the
/// instance ID it was minted for, but a temporary one's must not be: §5.4 requires that "given
/// a pair of GRUUs, it MUST be computationally infeasible to determine whether they were issued
/// for the same AOR or instance ID or for different AORs and instance IDs". Code that read an
/// instance out of this parameter would work against public GRUUs and quietly mis-attribute
/// temporary ones, which is the failure mode nobody notices until it matters.
#[must_use]
pub fn gr_value(uri: &Uri) -> Option<&[u8]> {
    uri.params().and_then(|params| params.value(GR_PARAM))
}

/// Whether a request whose Request-URI is `request_uri` was sent to `gruu` (§4.5).
///
/// Three conditions, and the first is the one RFC 3261 cannot express: **both** URIs must
/// actually be GRUUs. §5.4 warns that a public GRUU and its address of record are equivalent
/// under §19.1.4, so requiring `gr` on both sides is what keeps a request aimed at the address
/// of record from being read as one aimed at a single instance.
///
/// The rest is §19.1.4 as written: `gr` is a parameter present in both URIs, so
/// [`Uri::equivalent`] already requires the two values to agree — which is what separates one
/// instance's GRUU from another's, and a valued `gr` from a valueless one.
#[must_use]
pub fn addressed_to(request_uri: &Uri, gruu: &Uri) -> bool {
    is_gruu(request_uri) && is_gruu(gruu) && request_uri.equivalent(gruu)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use bytes::Bytes;

    const INSTANCE: &str = "urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

    fn uri(text: &str) -> Uri {
        Uri::parse(Bytes::from(text.to_owned())).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    #[test]
    fn a_gruu_is_a_uri_carrying_the_gr_parameter() {
        assert!(is_gruu(&uri(&format!(
            "sip:alice@example.com;gr={INSTANCE}"
        ))));
        // §7 makes the value optional, and §4.5 says so again in prose.
        assert!(is_gruu(&uri("sip:t7k2xq9f4m@example.com;gr")));
        assert!(!is_gruu(&uri("sip:alice@example.com")));
        // A URI parameter whose name merely starts with the letters is not the parameter.
        assert!(!is_gruu(&uri("sip:alice@example.com;group=5")));
    }

    #[test]
    fn the_gr_parameter_round_trips_through_the_parser_unchanged() {
        for text in [
            "sip:alice@example.com;gr=urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            "sip:t7k2xq9f4m@example.com;gr",
            "sips:alice@example.com;transport=tls;gr=urn:uuid:abc",
        ] {
            assert_eq!(
                uri(text).to_bytes(),
                Bytes::from(text.to_owned()),
                "§4.2 has a UA treat a GRUU as opaque, so it must go back out as it came in"
            );
        }
        assert_eq!(
            gr_value(&uri(&format!("sip:alice@example.com;gr={INSTANCE}"))),
            Some(INSTANCE.as_bytes())
        );
        assert_eq!(gr_value(&uri("sip:t7k2xq9f4m@example.com;gr")), None);
    }

    /// §5.4: "A public GRUU will always be equivalent to the AOR based on URI equality rules."
    ///
    /// This test asserts the RFC's own observation *and* that this module does not inherit it.
    /// If [`Uri::equivalent`] ever stopped agreeing, the first half would fail and someone would
    /// find out here rather than from a UA answering calls meant for another device.
    #[test]
    fn an_address_of_record_is_rfc3261_equivalent_to_a_public_gruu_but_is_not_sent_to_it() {
        let aor = uri("sip:alice@example.com");
        let gruu = uri(&format!("sip:alice@example.com;gr={INSTANCE}"));
        assert!(
            aor.equivalent(&gruu),
            "§5.4 says RFC 3261 §19.1.4 calls these the same URI, because it ignores a \
             parameter present in only one of them"
        );
        assert!(
            !addressed_to(&aor, &gruu),
            "a request to the address of record names every device the user registered; \
             answering it as though it named this instance is the whole bug §5.4 warns about"
        );
        assert!(addressed_to(&gruu, &gruu));
    }

    #[test]
    fn one_instances_gruu_is_not_another_instances() {
        let ours = uri(&format!("sip:alice@example.com;gr={INSTANCE}"));
        let theirs = uri("sip:alice@example.com;gr=urn:uuid:00000000-0000-4000-8000-000000000000");
        assert!(!addressed_to(&theirs, &ours));
        assert!(!addressed_to(&ours, &theirs));
    }

    #[test]
    fn a_valueless_temporary_gruu_matches_only_itself() {
        let temp = uri("sip:t7k2xq9f4m@example.com;gr");
        assert!(addressed_to(&temp, &temp));
        // A valueless `gr` and a valued one are not the same parameter value, so §19.1.4 already
        // separates them — but only once both sides are known to be GRUUs at all.
        assert!(!addressed_to(
            &uri(&format!("sip:t7k2xq9f4m@example.com;gr={INSTANCE}")),
            &temp
        ));
        assert!(!addressed_to(&uri("sip:t7k2xq9f4m@example.com"), &temp));
        // A different opaque user part is a different GRUU, which is the point of §5.4's
        // unlinkability requirement: nothing about the two says they came from one instance.
        assert!(!addressed_to(&uri("sip:h4d0s2p8v1@example.com;gr"), &temp));
    }

    /// The rest of §19.1.4 still applies: a GRUU is a SIP URI, and `sip:` is never `sips:`.
    #[test]
    fn the_ordinary_uri_rules_still_decide_everything_else() {
        let plain = uri(&format!("sip:alice@example.com;gr={INSTANCE}"));
        let secure = uri(&format!("sips:alice@example.com;gr={INSTANCE}"));
        assert!(!addressed_to(&secure, &plain));
        // Host case and an escaped parameter name are spellings, not differences.
        assert!(addressed_to(
            &uri(&format!("sip:alice@EXAMPLE.com;%67r={INSTANCE}")),
            &plain
        ));
    }
}
