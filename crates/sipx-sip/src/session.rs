//! Session timers (RFC 4028).
//!
//! A SIP dialog has no keepalive. If the far end loses power, its socket never closes and no BYE
//! is ever sent, so both sides sit in a call that no longer exists — one of them streaming audio
//! into the void, the other gone. Session timers are the periodic "are you still there" that
//! makes that detectable: the two ends agree an interval, one of them refreshes inside it, and
//! whoever stops seeing refreshes tears the call down locally.
//!
//! Everything here is pure. The interval negotiation, the choice of who refreshes and the
//! deadlines that follow from them are values computed from headers; the waiting and the sending
//! happen a layer up, where there is a clock.

use std::time::Duration;

use crate::error::HeaderError;
use crate::headers::grammar::{find_param_start, parse_params, parse_u64, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;

/// The option tag that advertises support, in `Supported` and `Require` (RFC 4028 §4).
pub const OPTION_TAG: &str = "timer";

/// The floor the RFC puts under any minimum interval (RFC 4028 §9).
///
/// A UAS "MUST NOT" advertise a `Min-SE` below this, and the reason is an amplification attack:
/// a short interval is a way to make a compliant peer send requests as fast as the attacker
/// likes. Ninety seconds is the RFC's own bound on how much amplification is allowed.
pub const ABSOLUTE_MIN_INTERVAL: Duration = Duration::from_secs(90);

/// What sipx asks for when nothing else is configured (RFC 4028 §4's example value).
///
/// Half an hour is long enough that the refresh traffic is negligible and short enough that a
/// dead call is not billed for an afternoon.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1800);

/// How far before expiry the side that is *not* refreshing gives up (RFC 4028 §10).
///
/// The BYE goes out slightly early rather than exactly on time, because the RFC's concern is
/// middleboxes: a NAT or firewall that has already dropped the pinhole at the expiry instant
/// will not pass a BYE sent after it, and the call would be torn down on one side only.
const EARLY_BYE_CAP: Duration = Duration::from_secs(32);

/// Split a header value into its numeric part and its parameter tail.
///
/// `get` rather than a slice index. The offset comes from [`find_param_start`] and is in range,
/// but "in range because of what the caller did" is exactly the reasoning that stops being true
/// after an edit, and this crate parses hostile input.
fn split_at_params(value: &[u8]) -> (&[u8], &[u8]) {
    let at = find_param_start(value).unwrap_or(value.len());
    (
        value.get(..at).unwrap_or(value),
        value.get(at..).unwrap_or_default(),
    )
}

/// Who refreshes the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresher {
    /// The party that sent the INVITE.
    Uac,
    /// The party that answered it.
    Uas,
}

impl Refresher {
    /// The token as it appears in the `refresher` parameter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uac => "uac",
            Self::Uas => "uas",
        }
    }
}

/// The `Session-Expires` header (RFC 4028 §4), also spelled `x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionExpires {
    /// The interval within which the session must be refreshed.
    pub interval: Duration,
    /// Who does the refreshing, when the parameter is present.
    ///
    /// Absent in a request means "I have no preference"; absent in a response is a peer that
    /// has not read RFC 4028 §9, which requires it.
    pub refresher: Option<Refresher>,
}

impl TypedHeader for SessionExpires {
    const NAME: HeaderName = HeaderName::SessionExpires;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let (delta, tail) = split_at_params(value);
        let seconds = parse_u64(trim(delta), "Session-Expires")?;
        let params = parse_params(tail, "Session-Expires")?;
        let refresher = crate::headers::grammar::param(&params, "refresher")
            .and_then(|p| p.value.as_deref())
            .map(|v| match v {
                v if v.eq_ignore_ascii_case(b"uac") => Ok(Refresher::Uac),
                v if v.eq_ignore_ascii_case(b"uas") => Ok(Refresher::Uas),
                // A refresher we do not recognise is not the same as none: "none" means the
                // peer left the choice open, and treating an unknown token that way would let
                // us appoint ourselves refresher against an instruction we failed to read.
                _ => Err(HeaderError::Syntax {
                    header: "Session-Expires",
                }),
            })
            .transpose()?;
        Ok(Self {
            interval: Duration::from_secs(seconds),
            refresher,
        })
    }
}

impl std::fmt::Display for SessionExpires {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.interval.as_secs())?;
        if let Some(refresher) = self.refresher {
            write!(f, ";refresher={}", refresher.as_str())?;
        }
        Ok(())
    }
}

/// The `Min-SE` header (RFC 4028 §5): the shortest interval the sender will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinSe(pub Duration);

impl TypedHeader for MinSe {
    const NAME: HeaderName = HeaderName::MinSe;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        // The ABNF allows generic parameters after the value; none are defined, and an
        // unknown one is not a reason to reject a header we otherwise understand.
        let (delta, _) = split_at_params(value);
        parse_u64(trim(delta), "Min-SE").map(|s| Self(Duration::from_secs(s)))
    }
}

/// What a UAS should do about the session timer on an incoming request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// No timer on this dialog: the peer neither asked for one nor said it could run one.
    None,
    /// Run a timer on these terms, and say so in the 2xx.
    Accept(Accepted),
    /// Refuse with `422 Session Interval Too Small`, carrying this `Min-SE`.
    TooBrief(Duration),
}

/// Terms the UAS accepted, ready to be written into the 2xx.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    /// The agreed interval.
    pub interval: Duration,
    /// Who will refresh.
    pub refresher: Refresher,
    /// Whether the 2xx should carry `Require: timer` (RFC 4028 §9).
    pub require: bool,
}

/// Decide the UAS side of the negotiation (RFC 4028 §9, Table 2).
///
/// `floor` is local policy — the shortest interval this side is willing to be driven at. It is
/// raised to [`ABSOLUTE_MIN_INTERVAL`] rather than trusted, because a floor below ninety seconds
/// is exactly the amplification the RFC forbids, and a configuration mistake should not become a
/// protocol violation.
#[must_use]
pub fn answer(
    peer_supports: bool,
    requested: Option<SessionExpires>,
    peer_min_se: Option<Duration>,
    floor: Duration,
) -> Answer {
    let floor = floor.max(ABSOLUTE_MIN_INTERVAL);

    let Some(requested) = requested else {
        // §9: `Supported: timer` without `Session-Expires` means the peer can run a timer but
        // is not asking for one. We may still ask. A peer that said nothing at all gets
        // nothing: putting a timer on a dialog with a UA that cannot refresh, and cannot read
        // the response saying so, would arm a teardown the far end has no way to prevent.
        if !peer_supports {
            return Answer::None;
        }
        return Answer::Accept(Accepted {
            interval: DEFAULT_INTERVAL.max(peer_min_se.unwrap_or(Duration::ZERO)),
            refresher: Refresher::Uas,
            require: true,
        });
    };

    if requested.interval < floor {
        return Answer::TooBrief(floor);
    }

    // §9: the UAS may reduce the interval but never increase it, and never below the peer's
    // own `Min-SE`. sipx keeps what was asked for — reducing it only makes both sides work
    // harder for a detection window the peer already said it was happy with.
    let refresher = match requested.refresher {
        // "the UAS cannot override the UAC's choice of refresher, if it made one."
        Some(chosen) => chosen,
        // Table 2 row 4 leaves the choice to us when both support the extension. sipx takes
        // the job. The refresher learns of a dead peer in one transaction timeout; the other
        // side has to wait out the whole interval, so refreshing is the faster detector.
        None => Refresher::Uas,
    };
    Answer::Accept(Accepted {
        interval: requested.interval,
        // §9: `Require: timer` is mandatory when the UAC refreshes, because the UAC has to
        // read the response to learn that. When we refresh it is only a SHOULD, and only
        // meaningful to a peer that understands the tag.
        require: refresher == Refresher::Uac || peer_supports,
        refresher,
    })
}

/// What a UAC learns from the 2xx to its own session refresh request (RFC 4028 §7.2).
///
/// `asked_for` is the interval this side put in the request, if any. It matters because a 2xx
/// with no `Session-Expires` from a peer that never claimed to support timers does not mean "no
/// timer" — §7.2 says the UAC may run one anyway, as refresher, purely for its own benefit.
///
/// The agreed interval is floored at [`ABSOLUTE_MIN_INTERVAL`]. §9 forbids a UAS from returning
/// anything shorter, and §11.2 explains what a shorter one would be: a way to make this side
/// emit requests as fast as the far end likes. Trusting the number because it arrived in a 2xx
/// would leave the defence entirely in the hands of the party it defends against.
#[must_use]
pub fn adopt(response: Option<SessionExpires>, asked_for: Option<Duration>) -> Option<Session> {
    match (response, asked_for) {
        (Some(agreed), _) => Some(Session {
            interval: agreed.interval.max(ABSOLUTE_MIN_INTERVAL),
            // §7.2 says the parameter "will always be present" when Require: timer is. A peer
            // that omits it anyway has told us an interval and not told us whose job it is;
            // taking the job is the only reading that cannot leave the call unrefreshed.
            we_refresh: agreed.refresher != Some(Refresher::Uas),
        }),
        (None, Some(interval)) => Some(Session {
            interval: interval.max(ABSOLUTE_MIN_INTERVAL),
            we_refresh: true,
        }),
        // §7.2: "If the 2xx response did not contain a Session-Expires header field, there is
        // no session expiration." A timer can be switched off mid-dialog this way.
        (None, None) => None,
    }
}

/// A live session timer: the agreed interval and which side keeps it alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    /// The negotiated interval.
    pub interval: Duration,
    /// Whether this side sends the refreshes.
    pub we_refresh: bool,
}

impl Session {
    /// How long after the last refresh this side should act.
    ///
    /// Two different deadlines, because the two roles do different things. The refresher sends
    /// at half the interval (RFC 4028 §7.2), which leaves a whole half-interval to notice a
    /// failure and retry. The other side waits nearly the whole interval and then hangs up,
    /// stopping short by `min(32s, interval/3)` so the BYE goes out before any middlebox on
    /// the path decides the session is over (§10).
    #[must_use]
    pub fn act_after(self) -> Duration {
        if self.we_refresh {
            self.interval / 2
        } else {
            self.interval
                .saturating_sub(EARLY_BYE_CAP.min(self.interval / 3))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn parse(value: &str) -> SessionExpires {
        SessionExpires::decode(value.as_bytes()).expect("parses")
    }

    #[test]
    fn a_session_expires_carries_its_interval_and_refresher() {
        assert_eq!(
            parse("1800;refresher=uas"),
            SessionExpires {
                interval: Duration::from_secs(1800),
                refresher: Some(Refresher::Uas),
            }
        );
        assert_eq!(parse("90").refresher, None);
        assert_eq!(parse("1800;REFRESHER=UAC").refresher, Some(Refresher::Uac));
    }

    #[test]
    fn a_refresher_that_is_neither_side_is_rejected() {
        // Not pedantry: defaulting an unreadable value to "no preference" would let us appoint
        // ourselves refresher while the peer believes it holds the job, and then neither side
        // refreshes when both think the other does.
        assert!(SessionExpires::decode(b"1800;refresher=proxy").is_err());
    }

    #[test]
    fn a_session_expires_round_trips() {
        for value in ["1800;refresher=uac", "90;refresher=uas", "600"] {
            assert_eq!(parse(value).to_string(), value);
        }
    }

    #[test]
    fn a_min_se_survives_parameters_it_does_not_define() {
        assert_eq!(
            MinSe::decode(b"90").expect("parses").0,
            Duration::from_secs(90)
        );
        assert_eq!(
            MinSe::decode(b"120;ext=1").expect("parses").0,
            Duration::from_secs(120)
        );
    }

    #[test]
    fn an_interval_under_the_floor_is_refused_with_the_floor() {
        let asked = SessionExpires {
            interval: Duration::from_secs(60),
            refresher: None,
        };
        assert_eq!(
            answer(true, Some(asked), None, Duration::from_secs(120)),
            Answer::TooBrief(Duration::from_secs(120))
        );
    }

    #[test]
    fn a_floor_below_the_rfc_minimum_is_raised_to_it() {
        // Local policy cannot opt into being an amplifier. A floor of ten seconds would let a
        // peer drive us at six requests a minute per call.
        let asked = SessionExpires {
            interval: Duration::from_secs(30),
            refresher: None,
        };
        assert_eq!(
            answer(true, Some(asked), None, Duration::from_secs(10)),
            Answer::TooBrief(ABSOLUTE_MIN_INTERVAL)
        );
    }

    #[test]
    fn table_2_governs_who_refreshes() {
        let with = |refresher| {
            let asked = SessionExpires {
                interval: Duration::from_secs(600),
                refresher,
            };
            match answer(true, Some(asked), None, ABSOLUTE_MIN_INTERVAL) {
                Answer::Accept(accepted) => accepted,
                other => panic!("expected acceptance, got {other:?}"),
            }
        };
        // Rows 5 and 6: the UAC's choice stands, whichever way it went.
        assert_eq!(with(Some(Refresher::Uac)).refresher, Refresher::Uac);
        assert_eq!(with(Some(Refresher::Uas)).refresher, Refresher::Uas);
        // Row 4: no choice made, so it is ours.
        assert_eq!(with(None).refresher, Refresher::Uas);
        // §9: Require is mandatory when the UAC refreshes, because it has to read the
        // response to find that out.
        assert!(with(Some(Refresher::Uac)).require);
    }

    #[test]
    fn a_peer_that_never_mentioned_timers_gets_none() {
        assert_eq!(
            answer(false, None, None, ABSOLUTE_MIN_INTERVAL),
            Answer::None
        );
    }

    #[test]
    fn support_without_a_request_lets_the_uas_ask() {
        let Answer::Accept(accepted) = answer(true, None, None, ABSOLUTE_MIN_INTERVAL) else {
            panic!("expected the uas to be able to ask for a timer");
        };
        assert_eq!(accepted.interval, DEFAULT_INTERVAL);
        assert_eq!(accepted.refresher, Refresher::Uas);
    }

    #[test]
    fn a_2xx_without_a_session_expires_leaves_the_asker_refreshing() {
        // RFC 4028 §7.2: the peer does not support timers, but we asked, so the timer is ours
        // to run — its whole benefit is ours too.
        let session = adopt(None, Some(Duration::from_secs(600))).expect("a timer");
        assert!(session.we_refresh);
        assert_eq!(session.interval, Duration::from_secs(600));
        // And with nothing asked for, there is no timer at all.
        assert_eq!(adopt(None, None), None);
    }

    #[test]
    fn a_2xx_cannot_drive_us_faster_than_the_floor() {
        // §11.2's rogue UAS: a very small interval in the 2xx is how a far end turns one call
        // into a request flood. The floor is ours to enforce, because it exists to protect us.
        let agreed = SessionExpires {
            interval: Duration::from_secs(5),
            refresher: Some(Refresher::Uac),
        };
        let session = adopt(Some(agreed), Some(DEFAULT_INTERVAL)).expect("a timer");
        assert_eq!(session.interval, ABSOLUTE_MIN_INTERVAL);
        // And the refresh still goes at half of *that*, not half of five seconds.
        assert_eq!(session.act_after(), Duration::from_secs(45));
    }

    #[test]
    fn the_two_roles_act_at_different_times() {
        let refreshing = Session {
            interval: Duration::from_secs(1800),
            we_refresh: true,
        };
        // Half the interval leaves the other half to notice a failure and retry.
        assert_eq!(refreshing.act_after(), Duration::from_secs(900));

        let waiting = Session {
            we_refresh: false,
            ..refreshing
        };
        // 1800 - min(32, 600) = 1768: just early enough to beat a middlebox to the punch.
        assert_eq!(waiting.act_after(), Duration::from_secs(1768));

        // On a short interval the cap does not apply and a third of it is used instead, so
        // the BYE never overtakes the refresh it is waiting for.
        let short = Session {
            interval: Duration::from_secs(90),
            we_refresh: false,
        };
        assert_eq!(short.act_after(), Duration::from_secs(60));
    }
}
