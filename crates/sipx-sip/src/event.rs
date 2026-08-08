//! Subscriptions and notifications, as decisions (RFC 6665).
//!
//! The pure half of the event framework: what a `Subscription-State` says, when a subscription is
//! over, what a refresh does to its expiry, and whether a package is one this side serves. No
//! dialog, no clock, no socket — those belong to whoever drives it.
//!
//! sipx has had exactly one subscription since `S-9`: the implicit one a REFER creates. That one
//! works and is not a framework. What makes this a framework is that a *package* is a name and a
//! body type, and everything else — establishing, refreshing, expiring, terminating, refusing an
//! unknown one — is the same whichever package it is.

use std::time::Duration;

/// The state of a subscription, as a `Subscription-State` header says it (RFC 6665 §4.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Accepted and authorised. Notifications are flowing.
    Active,
    /// Received, but the notifier has not decided yet.
    ///
    /// A distinct state rather than a slow `active`, because §4.1.3 makes it one: a subscriber
    /// that treated `pending` as active would report a presence it has not been granted.
    Pending,
    /// Over. Nothing further will arrive on this subscription.
    Terminated,
}

impl State {
    /// The token as it appears in the header.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Terminated => "terminated",
        }
    }

    /// The state a token names.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        [Self::Active, Self::Pending, Self::Terminated]
            .into_iter()
            .find(|candidate| token.trim().eq_ignore_ascii_case(candidate.as_str()))
    }
}

/// Why a subscription ended (RFC 6665 §4.1.3).
///
/// The distinction that matters to a subscriber is whether to try again, and these are not
/// interchangeable about it: `deactivated` says re-subscribe now, `probation` says wait,
/// `rejected` says do not, and `noresource` says there is nothing left to subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The subscription ended and the subscriber should re-subscribe immediately.
    Deactivated,
    /// Ended for now; re-subscribe after `retry-after`.
    Probation,
    /// Refused by policy. Do not re-subscribe.
    Rejected,
    /// The subscription simply expired.
    Timeout,
    /// The notifier could not continue, for a reason of its own.
    GiveUp,
    /// The resource being watched no longer exists.
    NoResource,
    /// A subscription the notifier can no longer honour in the terms agreed.
    Invariant,
    /// The filter was not one the notifier could apply.
    BadFilter,
}

impl Reason {
    /// The token as it appears in the header.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deactivated => "deactivated",
            Self::Probation => "probation",
            Self::Rejected => "rejected",
            Self::Timeout => "timeout",
            Self::GiveUp => "giveup",
            Self::NoResource => "noresource",
            Self::Invariant => "invariant",
            Self::BadFilter => "badfilter",
        }
    }

    /// The reason a token names.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        [
            Self::Deactivated,
            Self::Probation,
            Self::Rejected,
            Self::Timeout,
            Self::GiveUp,
            Self::NoResource,
            Self::Invariant,
            Self::BadFilter,
        ]
        .into_iter()
        .find(|candidate| token.trim().eq_ignore_ascii_case(candidate.as_str()))
    }

    /// Whether a subscriber should try again.
    ///
    /// §4.1.3 gives each reason its own answer, and collapsing them is how a client either gives up
    /// on a subscription that was only briefly unavailable, or hammers one it has been refused.
    #[must_use]
    pub fn should_resubscribe(self) -> bool {
        matches!(self, Self::Deactivated | Self::Probation | Self::Timeout)
    }
}

/// A parsed `Subscription-State` (RFC 6665 §4.1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// Which state.
    pub state: State,
    /// How long is left, for `active` and `pending`.
    ///
    /// §4.1.3 makes it a SHOULD there and meaningless on `terminated`.
    pub expires: Option<Duration>,
    /// Why it ended, for `terminated`.
    pub reason: Option<Reason>,
    /// How long to wait before re-subscribing, when the reason gives one.
    pub retry_after: Option<Duration>,
}

impl Subscription {
    /// An active subscription with this much left.
    #[must_use]
    pub fn active(expires: Duration) -> Self {
        Self {
            state: State::Active,
            expires: Some(expires),
            reason: None,
            retry_after: None,
        }
    }

    /// A terminated subscription, with the reason it ended.
    #[must_use]
    pub fn terminated(reason: Reason) -> Self {
        Self {
            state: State::Terminated,
            expires: None,
            reason: Some(reason),
            retry_after: None,
        }
    }

    /// Whether this says the subscription is over.
    #[must_use]
    pub fn is_terminated(self_: &Self) -> bool {
        self_.state == State::Terminated
    }

    /// Read a `Subscription-State` value.
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(value);
        let mut parts = text.split(';');
        let state = State::parse(parts.next()?)?;
        let mut subscription = Self {
            state,
            expires: None,
            reason: None,
            retry_after: None,
        };
        for parameter in parts {
            let Some((name, value)) = parameter.split_once('=') else {
                continue;
            };
            let (name, value) = (name.trim(), value.trim().trim_matches('"'));
            if name.eq_ignore_ascii_case("expires") {
                subscription.expires = value.parse().ok().map(Duration::from_secs);
            } else if name.eq_ignore_ascii_case("reason") {
                subscription.reason = Reason::parse(value);
            } else if name.eq_ignore_ascii_case("retry-after") {
                subscription.retry_after = value.parse().ok().map(Duration::from_secs);
            }
        }
        Some(subscription)
    }

    /// Render as a `Subscription-State` value.
    #[must_use]
    pub fn to_value(&self) -> String {
        use std::fmt::Write as _;
        let mut out = self.state.as_str().to_owned();
        if self.state != State::Terminated
            && let Some(expires) = self.expires
        {
            let _ = write!(out, ";expires={}", expires.as_secs());
        }
        if let Some(reason) = self.reason {
            let _ = write!(out, ";reason={}", reason.as_str());
        }
        if let Some(retry) = self.retry_after {
            let _ = write!(out, ";retry-after={}", retry.as_secs());
        }
        out
    }
}

/// The expiry a notifier grants for a requested one (RFC 6665 §4.2.1.1).
///
/// "The server MAY shorten the interval but MUST NOT lengthen it" — the same rule REGISTER has, and
/// for the same reason: the shorter of the two is what both sides can agree on without one of them
/// believing in a subscription the other has forgotten.
///
/// A request for zero is an unsubscribe (§3.1.1) and stays zero however generous the policy.
#[must_use]
pub fn granted_expiry(requested: Duration, policy_maximum: Duration) -> Duration {
    requested.min(policy_maximum)
}

/// Whether a SUBSCRIBE is asking to end the subscription rather than to have one (§3.1.1).
///
/// "A SUBSCRIBE request with an 'Expires' of 0 constitutes a request to unsubscribe from the
/// matching subscription." It is not a degenerate subscription of no duration — the notifier still
/// owes a terminating NOTIFY (§4.2.1.4), which is the part that is easy to miss.
#[must_use]
pub fn is_unsubscribe(requested: Duration) -> bool {
    requested.is_zero()
}

/// The packages a notifier serves, by `Event` name.
///
/// A framework rather than a switch statement: a package is a name here, and everything the
/// framework does — establishing, refreshing, expiring, terminating, refusing — is the same
/// whichever one it is.
#[derive(Debug, Clone, Default)]
pub struct Packages {
    names: Vec<String>,
}

impl Packages {
    /// A notifier that serves nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve this package.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !self
            .names
            .iter()
            .any(|held| held.eq_ignore_ascii_case(&name))
        {
            self.names.push(name);
        }
        self
    }

    /// Whether a package name is one this side serves.
    ///
    /// Case-insensitively, and the *template* is what is matched: RFC 6665 §8.2.1 makes an `Event`
    /// value `package` optionally followed by `.template`, and a notifier that serves `dialog`
    /// serves `dialog.winfo` requests to the extent of recognising them.
    #[must_use]
    pub fn serves(&self, event: &str) -> bool {
        let package = event.split(';').next().unwrap_or_default().trim();
        let base = package.split('.').next().unwrap_or_default();
        self.names
            .iter()
            .any(|held| held.eq_ignore_ascii_case(base) || held.eq_ignore_ascii_case(package))
    }

    /// The names, for an `Allow-Events` header.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The `Allow-Events` value advertising them (RFC 6665 §4.4.5).
    #[must_use]
    pub fn allow_events(&self) -> String {
        self.names.join(", ")
    }
}

/// The status a SUBSCRIBE naming an unserved package is refused with (RFC 6665 §4.2.1.1).
///
/// 489 and not 400 or 501. It is a specific answer to a specific question — "I do not have that
/// package" — and a subscriber that gets it knows not to retry, where a 400 tells it its request
/// was malformed and a 501 that the *method* is unimplemented.
pub const BAD_EVENT: u16 = 489;

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

    #[test]
    fn a_subscription_state_round_trips() {
        let active = Subscription::active(Duration::from_secs(3600));
        assert_eq!(active.to_value(), "active;expires=3600");
        assert_eq!(Subscription::parse(b"active;expires=3600"), Some(active));

        let ended = Subscription::terminated(Reason::Timeout);
        assert_eq!(ended.to_value(), "terminated;reason=timeout");
        assert_eq!(
            Subscription::parse(b"terminated;reason=timeout"),
            Some(ended)
        );
    }

    /// §4.1.3: `expires` is meaningless on a terminated subscription, and emitting one would
    /// suggest there is time left on something that is over.
    #[test]
    fn a_terminated_state_carries_no_expiry() {
        let mut ended = Subscription::terminated(Reason::NoResource);
        ended.expires = Some(Duration::from_secs(60));
        assert_eq!(ended.to_value(), "terminated;reason=noresource");
    }

    #[test]
    fn the_three_states_are_told_apart() {
        assert_eq!(State::parse("active"), Some(State::Active));
        assert_eq!(State::parse("PENDING"), Some(State::Pending));
        assert_eq!(State::parse(" terminated "), Some(State::Terminated));
        assert_eq!(State::parse("finished"), None);
        // `pending` is not a slow `active`: a subscriber that conflated them would report a
        // presence it has not been granted.
        assert_ne!(State::Pending, State::Active);
    }

    #[test]
    fn every_reason_the_rfc_defines_round_trips() {
        for reason in [
            Reason::Deactivated,
            Reason::Probation,
            Reason::Rejected,
            Reason::Timeout,
            Reason::GiveUp,
            Reason::NoResource,
            Reason::Invariant,
            Reason::BadFilter,
        ] {
            assert_eq!(Reason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(Reason::parse("because"), None);
    }

    /// The reasons are not interchangeable about whether to try again, and collapsing them means
    /// either giving up on a briefly unavailable subscription or hammering a refused one.
    #[test]
    fn a_refusal_and_a_timeout_lead_to_different_behaviour() {
        assert!(Reason::Timeout.should_resubscribe());
        assert!(Reason::Deactivated.should_resubscribe());
        assert!(Reason::Probation.should_resubscribe());
        assert!(!Reason::Rejected.should_resubscribe());
        assert!(!Reason::NoResource.should_resubscribe());
    }

    #[test]
    fn a_retry_after_survives_the_round_trip() {
        let parsed =
            Subscription::parse(b"terminated;reason=probation;retry-after=1800").expect("parses");
        assert_eq!(parsed.reason, Some(Reason::Probation));
        assert_eq!(parsed.retry_after, Some(Duration::from_secs(1800)));
        assert!(parsed.reason.expect("a reason").should_resubscribe());
    }

    /// §4.2.1.1: the notifier "MAY shorten the interval but MUST NOT lengthen it".
    #[test]
    fn a_notifier_may_shorten_an_expiry_and_never_lengthen_it() {
        let hour = Duration::from_secs(3600);
        let day = Duration::from_secs(86400);
        assert_eq!(granted_expiry(day, hour), hour, "shortened to the policy");
        assert_eq!(
            granted_expiry(hour, day),
            hour,
            "a generous policy does not lengthen what was asked for"
        );
    }

    /// §3.1.1: `Expires: 0` unsubscribes. It is not a subscription of no duration, and the
    /// notifier still owes a terminating NOTIFY — the part that is easy to miss.
    #[test]
    fn an_expiry_of_zero_is_an_unsubscribe() {
        assert!(is_unsubscribe(Duration::ZERO));
        assert!(!is_unsubscribe(Duration::from_secs(1)));
        assert_eq!(
            granted_expiry(Duration::ZERO, Duration::from_secs(3600)),
            Duration::ZERO,
            "a generous policy must not turn an unsubscribe into a subscription"
        );
    }

    #[test]
    fn a_package_is_served_by_name_whatever_its_parameters() {
        let packages = Packages::new().with("dialog").with("presence");
        assert!(packages.serves("dialog"));
        assert!(packages.serves("DIALOG"));
        assert!(packages.serves("dialog;call-id=x"));
        assert!(packages.serves("presence"));
        assert!(!packages.serves("refer"));
        assert!(!packages.serves(""));
    }

    /// §8.2.1: an `Event` value is a package optionally followed by a template.
    #[test]
    fn a_template_is_recognised_as_its_package() {
        let packages = Packages::new().with("dialog");
        assert!(packages.serves("dialog.winfo"));
    }

    #[test]
    fn allow_events_lists_what_is_served_and_a_package_is_not_listed_twice() {
        let packages = Packages::new()
            .with("dialog")
            .with("presence")
            .with("DIALOG");
        assert_eq!(packages.allow_events(), "dialog, presence");
        assert_eq!(packages.names().len(), 2);
    }

    /// 489 rather than 400 or 501: a specific answer to "I do not have that package", which tells
    /// a subscriber not to retry where the other two would mislead it.
    #[test]
    fn an_unserved_package_has_its_own_status() {
        assert_eq!(BAD_EVENT, 489);
    }
}
