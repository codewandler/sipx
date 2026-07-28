//! A certificate authority the tests control.
//!
//! Every certificate in the TLS and WSS tests is generated at run time from this, so nothing
//! depends on a public CA, on a clock beyond the test's control, or on anyone else's expiry
//! date. It lives here rather than in each test file because a fixture copied three times is
//! three fixtures that can quietly stop meaning the same thing.

// Panicking is the right failure here and the only useful one. These are fixtures: a test
// whose certificate could not be generated has not found a bug, it has failed to start, and
// threading a `Result` out of every fixture call would put error handling into every test for
// a case that means the test harness is broken.
#![allow(clippy::expect_used)]

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair, SanType,
};

/// A private CA, and the certificates it issues.
#[derive(Debug)]
pub struct Ca {
    pem: String,
    issuer: Issuer<'static, KeyPair>,
}

impl Ca {
    /// A fresh authority, trusted by nobody until a test says so.
    #[must_use]
    pub fn new() -> Self {
        let key = KeyPair::generate().expect("a key");
        let mut params = CertificateParams::new(Vec::new()).expect("params");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "sipx test CA");
        params.distinguished_name = name;

        let certificate = params.clone().self_signed(&key).expect("a CA certificate");
        Self {
            pem: certificate.pem(),
            issuer: Issuer::new(params, key),
        }
    }

    /// The authority's own certificate, to be added as a trust anchor.
    #[must_use]
    pub fn pem(&self) -> String {
        self.pem.clone()
    }

    /// Issue a certificate carrying these subject alternative names and this common name.
    ///
    /// The two are separate arguments on purpose: a certificate whose SAN and CN disagree is
    /// the case RFC 6125 §6.4.4 exists for, and it cannot be constructed if they are one field.
    #[must_use]
    pub fn issue(&self, sans: &[SanType], common_name: &str) -> (String, String) {
        let key = KeyPair::generate().expect("a key");
        let mut params = CertificateParams::new(Vec::new()).expect("params");
        params.subject_alt_names = sans.to_vec();
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, common_name);
        params.distinguished_name = name;

        let signed = params
            .signed_by(&key, &self.issuer)
            .expect("a leaf certificate");
        (signed.pem(), key.serialize_pem())
    }

    /// Issue a certificate for one DNS name, which is the common case.
    #[must_use]
    pub fn issue_for(&self, host: &str) -> (String, String) {
        self.issue(&[dns(host)], host)
    }
}

impl Default for Ca {
    fn default() -> Self {
        Self::new()
    }
}

/// A `dNSName` subject alternative name.
#[must_use]
pub fn dns(name: &str) -> SanType {
    SanType::DnsName(name.try_into().expect("a DNS name"))
}
