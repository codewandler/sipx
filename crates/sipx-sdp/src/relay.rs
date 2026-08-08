//! Relaying one endpoint's description into another dialog (RFC 7092 §3.1.3).
//!
//! Nothing here moves media, and nothing here describes a local media endpoint. A
//! [`DescriptionRelay`] takes the description one endpoint wrote and produces the description to
//! put in front of the other, changing exactly one line: the `o=` that identifies whose
//! description this is and which revision of it (RFC 8866 §5.2, RFC 3264 §8). Addresses, ports,
//! payload types, keying and direction are the endpoints' own and pass through untouched, which
//! is what keeps the element that relays them off the media path.
//!
//! The rewrite is performed on the text rather than on a re-serialized [`SessionDescription`].
//! Parsing is a *view* over the lines here, and rendering that view back normalizes line order,
//! multicast TTLs, `m=` port counts and whitespace — differences that are harmless in a
//! description this crate authored and are not ours to introduce into one it did not.

use crate::SdpError;
use crate::session::Origin;

/// Why a description could not be put in front of the other endpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RelayError {
    /// It is not a session description.
    #[error("the body is not a session description: {0}")]
    Malformed(#[from] SdpError),
    /// It describes no media at all, so there is nothing to put in front of the peer.
    #[error("the description carries no media stream")]
    NoMedia,
    /// An accepted stream names no address, at either level. A relaying element cannot supply
    /// one without becoming the destination itself.
    #[error("the {media} stream names no connection address")]
    NoConnection {
        /// The `m=` type that named no address.
        media: String,
    },
}

/// The description identity one dialog sees, and the revision counter that goes with it.
///
/// One relay per direction: the descriptions a coupling emits *into* a dialog are a sequence of
/// its own, and RFC 3264 §8 requires that sequence's version to increase whenever the
/// description changes and to stay put whenever it does not. The two dialogs of a coupling
/// therefore never share one, because their offer/answer sequences are not the same sequence.
#[derive(Debug, Clone)]
pub struct DescriptionRelay {
    origin: Origin,
    /// The last description emitted, with its origin line removed — the exact thing RFC 3264 §8
    /// compares to decide whether the version must move.
    emitted: Option<String>,
}

impl DescriptionRelay {
    /// A relay that will emit descriptions under this origin.
    #[must_use]
    pub fn new(origin: Origin) -> Self {
        Self {
            origin,
            emitted: None,
        }
    }

    /// The origin the next emitted description will carry.
    #[must_use]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Map a description written by one endpoint into this dialog.
    ///
    /// Everything except the `o=` line survives byte for byte. The version advances only when
    /// the rest of the description differs from the last one emitted here (RFC 3264 §8: an
    /// unchanged version promises an unchanged description).
    pub fn relay(&mut self, description: &str) -> Result<String, RelayError> {
        let parsed = crate::parse(description)?;
        if parsed.media.is_empty() {
            return Err(RelayError::NoMedia);
        }
        for media in &parsed.media {
            if !media.is_rejected() && media.connection.is_none() && parsed.connection.is_none() {
                return Err(RelayError::NoConnection {
                    media: media.media.clone(),
                });
            }
        }
        // `parse` returned, so the description has an origin line to replace.
        let (before, after) = split_origin(description).ok_or(SdpError::Missing("o="))?;
        let mut unchanged = String::with_capacity(before.len() + after.len());
        unchanged.push_str(before);
        unchanged.push_str(after);
        match &self.emitted {
            Some(last) if *last == unchanged => {}
            Some(_) => self.origin.session_version = self.origin.session_version.saturating_add(1),
            None => {}
        }
        self.emitted = Some(unchanged);
        Ok(format!("{before}{}{after}", origin_line(&self.origin)))
    }
}

/// The `o=` line for an origin, without its terminator.
fn origin_line(origin: &Origin) -> String {
    format!(
        "o={} {} {} IN {} {}",
        origin.username,
        origin.session_id,
        origin.session_version,
        origin.address.address_type(),
        origin.address
    )
}

/// Everything before the origin line's content, and everything from its terminator onwards.
fn split_origin(description: &str) -> Option<(&str, &str)> {
    let mut start = 0;
    while start < description.len() {
        let rest = description.get(start..)?;
        let len = rest.find('\n').map_or(rest.len(), |index| index + 1);
        let line = rest.get(..len)?;
        let content = line.trim_end_matches(['\n', '\r']);
        if content.starts_with("o=") {
            let end = start + content.len();
            return Some((description.get(..start)?, description.get(end..)?));
        }
        start += len;
    }
    None
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn relay() -> DescriptionRelay {
        DescriptionRelay::new(Origin::new("192.0.2.9".parse::<IpAddr>().unwrap(), 77, 1))
    }

    fn offer(port: u16) -> String {
        format!(
            "v=0\r\no=alice 2890844526 2890844526 IN IP4 198.51.100.4\r\ns=-\r\n\
             c=IN IP4 198.51.100.4\r\nt=0 0\r\nm=audio {port} RTP/AVP 0 8\r\n\
             a=rtpmap:0 PCMU/8000\r\na=sendonly\r\n"
        )
    }

    #[test]
    fn only_the_origin_line_changes() {
        let relayed = relay().relay(&offer(49_170)).unwrap();
        assert_eq!(
            relayed,
            "v=0\r\no=- 77 1 IN IP4 192.0.2.9\r\ns=-\r\n\
             c=IN IP4 198.51.100.4\r\nt=0 0\r\nm=audio 49170 RTP/AVP 0 8\r\n\
             a=rtpmap:0 PCMU/8000\r\na=sendonly\r\n"
        );
    }

    #[test]
    fn keying_and_transport_survive_untouched() {
        let secure = "v=0\r\no=- 1 1 IN IP4 198.51.100.4\r\ns=-\r\nc=IN IP4 198.51.100.4\r\n\
             t=0 0\r\nm=audio 6000 RTP/SAVP 0\r\n\
             a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj\r\n\
             a=fingerprint:sha-256 12:34\r\na=setup:actpass\r\na=sendrecv\r\n";
        let relayed = relay().relay(secure).unwrap();
        let dropped = |line: &str| !relayed.contains(line);
        assert!(
            !dropped("m=audio 6000 RTP/SAVP 0"),
            "the profile is the endpoints'"
        );
        assert!(
            !dropped(
                "a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj"
            ),
            "keying material is endpoint to endpoint"
        );
        assert!(!dropped("a=fingerprint:sha-256 12:34"));
        assert!(!dropped("a=setup:actpass"));
    }

    #[test]
    fn unknown_lines_and_line_order_survive() {
        let exotic = "v=0\r\no=- 1 1 IN IP4 198.51.100.4\r\ns=-\r\nc=IN IP4 198.51.100.4/127\r\n\
             b=AS:64\r\nt=0 0\r\nm=audio 6000/2 RTP/AVP 0\r\na=sendrecv\r\nz=something new\r\n";
        let relayed = relay().relay(exotic).unwrap();
        assert!(
            relayed.contains("c=IN IP4 198.51.100.4/127"),
            "the TTL survives"
        );
        assert!(
            relayed.contains("m=audio 6000/2 RTP/AVP 0"),
            "the port count survives"
        );
        assert!(
            relayed.contains("z=something new"),
            "an unmodelled line survives"
        );
    }

    #[test]
    fn the_version_moves_only_when_the_description_does() {
        let mut relay = relay();
        assert!(relay.relay(&offer(49_170)).unwrap().contains("o=- 77 1 "));
        assert!(
            relay.relay(&offer(49_170)).unwrap().contains("o=- 77 1 "),
            "an unchanged description keeps its version (RFC 3264 §8)"
        );
        assert!(
            relay.relay(&offer(49_172)).unwrap().contains("o=- 77 2 "),
            "a changed description advances it"
        );
        assert!(relay.relay(&offer(49_172)).unwrap().contains("o=- 77 2 "));
        assert!(relay.relay(&offer(49_170)).unwrap().contains("o=- 77 3 "));
    }

    #[test]
    fn the_source_origin_never_reaches_the_other_dialog() {
        let relayed = relay().relay(&offer(49_170)).unwrap();
        assert!(!relayed.contains("alice"));
        assert!(!relayed.contains("2890844526"));
    }

    #[test]
    fn a_body_that_is_not_a_description_is_refused() {
        assert!(matches!(
            relay().relay("this is not a session description\r\n"),
            Err(RelayError::Malformed(_))
        ));
    }

    #[test]
    fn a_description_with_no_media_is_refused() {
        assert_eq!(
            relay().relay("v=0\r\no=- 1 1 IN IP4 198.51.100.4\r\ns=-\r\nt=0 0\r\n"),
            Err(RelayError::NoMedia)
        );
    }

    #[test]
    fn an_accepted_stream_with_no_address_is_refused() {
        assert_eq!(
            relay().relay(
                "v=0\r\no=- 1 1 IN IP4 198.51.100.4\r\ns=-\r\nt=0 0\r\nm=audio 6000 RTP/AVP 0\r\n"
            ),
            Err(RelayError::NoConnection {
                media: "audio".to_owned()
            })
        );
    }

    #[test]
    fn a_rejected_stream_needs_no_address() {
        let rejected = "v=0\r\no=- 1 1 IN IP4 198.51.100.4\r\ns=-\r\nt=0 0\r\n\
             m=audio 6000 RTP/AVP 0\r\nc=IN IP4 198.51.100.4\r\nm=video 0 RTP/AVP 96\r\n";
        assert!(relay().relay(rejected).is_ok());
    }

    #[test]
    fn bare_line_feeds_survive_as_they_arrived() {
        let relayed = relay()
            .relay("v=0\no=alice 1 1 IN IP4 198.51.100.4\ns=-\nc=IN IP4 198.51.100.4\nt=0 0\nm=audio 6000 RTP/AVP 0\n")
            .unwrap();
        assert_eq!(
            relayed,
            "v=0\no=- 77 1 IN IP4 192.0.2.9\ns=-\nc=IN IP4 198.51.100.4\nt=0 0\nm=audio 6000 RTP/AVP 0\n"
        );
    }
}
