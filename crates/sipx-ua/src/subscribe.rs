//! The subscriptions a notifier is serving (RFC 6665).
//!
//! [`sipx_sip::event`] decides what a `Subscription-State` means. This holds the ones that exist:
//! establishing them, refreshing them, expiring them, and — the part that matters most — making
//! sure a terminated one stays terminated.
//!
//! Time is a parameter, not a call to a clock, for the same reason it is in the timer queue: a
//! notifier driven by a scheduler somebody else owns has to be able to say what "now" is, and a
//! test that wants to watch a subscription expire should not have to wait an hour.
//! **Experimental** (`A-8`): public and tested. `sipx-call::Notifier` now drives this exact store
//! from the dispatcher; the pre-1.0 observation and runtime API may still change shape.
//!

use std::time::Duration;

use sipx_sip::event::{
    BAD_EVENT, Packages, Reason, State, Subscription, granted_expiry, is_unsubscribe,
};
use sipx_sip::headers::{CSeq, Expires, From as FromHeader};
use sipx_sip::{HeaderName, Method, Request};

/// What identifies one subscription (RFC 6665 §4.4.1).
///
/// The dialog plus the event package — *not* the dialog alone. §4.4.1 allows several subscriptions
/// in one dialog as long as their `Event` differs, so keying on the dialog would have a second
/// subscription silently replace the first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Id {
    /// The dialog's `Call-ID`.
    pub call_id: String,
    /// The subscriber's tag.
    pub from_tag: String,
    /// The exact `Event` type, with only its `id` parameter if it has one.
    pub event: String,
}

impl Id {
    /// Read the identity out of a SUBSCRIBE.
    #[must_use]
    pub fn from_request(request: &Request) -> Option<Self> {
        if request.headers.count(&HeaderName::CallId) != 1
            || request.headers.count(&HeaderName::From) != 1
            || request.headers.count(&HeaderName::Event) != 1
        {
            return None;
        }
        let call_id = request.headers.value(&HeaderName::CallId)?;
        let call_id = std::str::from_utf8(&call_id).ok()?;
        if call_id.is_empty() {
            return None;
        }
        let from = request.headers.typed::<FromHeader>()?.ok()?;
        let mut tags = from.params.iter().filter(|parameter| parameter.is("tag"));
        let tag = tags.next()?.value.as_deref()?;
        if tags.next().is_some() || tag.is_empty() || !tag.iter().copied().all(is_token_char) {
            return None;
        }
        Some(Self {
            call_id: call_id.to_owned(),
            from_tag: std::str::from_utf8(tag).ok()?.to_owned(),
            event: event_identity(&request.headers.value(&HeaderName::Event)?)?,
        })
    }
}

fn event_identity(value: &[u8]) -> Option<String> {
    let segments = event_segments(value)?;
    let event = trim_ows(segments.first()?);
    if !valid_event_type(event) {
        return None;
    }

    let mut id = None;
    for segment in segments.iter().skip(1) {
        let parameter = trim_ows(segment);
        if parameter.is_empty() {
            return None;
        }
        let (name, value) =
            parameter
                .iter()
                .position(|byte| *byte == b'=')
                .map_or((parameter, None), |equals| {
                    (
                        trim_ows(parameter.get(..equals).unwrap_or_default()),
                        Some(trim_ows(
                            parameter
                                .get(equals.saturating_add(1)..)
                                .unwrap_or_default(),
                        )),
                    )
                });
        if name.is_empty() || !name.iter().copied().all(is_token_char) {
            return None;
        }
        if name.eq_ignore_ascii_case(b"id") {
            let value = value?;
            if id.is_some() || value.is_empty() || !value.iter().copied().all(is_token_char) {
                return None;
            }
            id = Some(value);
        } else if !value.is_none_or(valid_generic_value) {
            return None;
        }
    }

    let event = std::str::from_utf8(event).ok()?;
    match id {
        Some(id) => Some(format!("{event};id={}", std::str::from_utf8(id).ok()?)),
        None => Some(event.to_owned()),
    }
}

fn event_segments(value: &[u8]) -> Option<Vec<&[u8]>> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in value.iter().copied().enumerate() {
        if quoted && escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b';' if !quoted => {
                segments.push(value.get(start..index)?);
                start = index.saturating_add(1);
            }
            _ => {}
        }
    }
    if quoted || escaped {
        return None;
    }
    segments.push(value.get(start..)?);
    Some(segments)
}

fn valid_event_type(value: &[u8]) -> bool {
    !value.is_empty()
        && value
            .split(|byte| *byte == b'.')
            .all(|token| !token.is_empty() && token.iter().copied().all(is_token_nodot_char))
}

fn valid_generic_value(value: &[u8]) -> bool {
    if value.len() >= 2 && value.first() == Some(&b'"') && value.last() == Some(&b'"') {
        return true;
    }
    !value.is_empty()
        && value.iter().all(|byte| {
            byte.is_ascii_graphic() && !matches!(byte, b';' | b',' | b'"' | b'<' | b'>')
        })
}

fn trim_ows(mut value: &[u8]) -> &[u8] {
    while matches!(value.first(), Some(b' ' | b'\t')) {
        value = value.get(1..).unwrap_or_default();
    }
    while matches!(value.last(), Some(b' ' | b'\t')) {
        value = value
            .get(..value.len().saturating_sub(1))
            .unwrap_or_default();
    }
    value
}

fn is_token_char(byte: u8) -> bool {
    is_token_nodot_char(byte) || byte == b'.'
}

fn is_token_nodot_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
        )
}

/// One subscription being served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// Which subscription.
    pub id: Id,
    /// Its state.
    pub state: State,
    /// When it expires, in seconds on the caller's clock.
    pub expires_at: u64,
    /// The last accepted remote SUBSCRIBE sequence number.
    pub remote_cseq: u32,
}

/// What answering a SUBSCRIBE concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// A new subscription. Answer 2xx, then send the first NOTIFY.
    Established {
        /// Which one.
        id: Id,
        /// The expiry granted, which may be shorter than the one asked for.
        expires: Duration,
    },
    /// An existing subscription's timer was pushed out.
    Refreshed {
        /// Which one.
        id: Id,
        /// The expiry granted.
        expires: Duration,
    },
    /// `Expires: 0` — the subscriber is leaving (§3.1.1).
    ///
    /// The notifier still owes a terminating NOTIFY (§4.2.1.4), which is the part that is easy to
    /// miss: an unsubscribe is not a subscription of no duration, it is an ending.
    Unsubscribed {
        /// Which one.
        id: Id,
    },
    /// The `Event` names a package this notifier does not serve. Answer [`BAD_EVENT`].
    Unserved {
        /// The status to answer with — 489, and not 400 or 501.
        status: u16,
    },
    /// A new subscription would exceed this notifier's configured peer-driven resource bound.
    AtCapacity,
    /// The request could not be read as a SUBSCRIBE at all.
    Malformed,
    /// A matching subscription received an equal or lower remote `CSeq`.
    OutOfOrder {
        /// The subscription that was deliberately left unchanged.
        id: Id,
    },
}

/// The subscriptions one notifier is serving.
#[derive(Debug)]
pub struct Subscriptions {
    packages: Packages,
    policy_maximum: Duration,
    capacity: usize,
    held: Vec<Served>,
}

impl Subscriptions {
    /// A notifier serving these packages, granting at most this long.
    #[must_use]
    pub fn new(packages: Packages, policy_maximum: Duration) -> Self {
        Self {
            packages,
            policy_maximum,
            capacity: 1024,
            held: Vec::new(),
        }
    }

    /// Apply a finite concurrent-subscription bound.
    ///
    /// Zero is raised to one: a notifier configured with no capacity could advertise packages but
    /// never serve one, which is almost certainly a configuration mistake rather than policy.
    #[must_use]
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self
    }

    /// The concurrent-subscription bound.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The packages served, for an `Allow-Events` header.
    #[must_use]
    pub fn packages(&self) -> &Packages {
        &self.packages
    }

    /// How many subscriptions are live.
    #[must_use]
    pub fn active(&self) -> usize {
        self.held
            .iter()
            .filter(|held| held.state != State::Terminated)
            .count()
    }

    /// Every subscription being served, terminated ones included until they are swept.
    #[must_use]
    pub fn all(&self) -> &[Served] {
        &self.held
    }

    /// Answer a SUBSCRIBE.
    pub fn on_subscribe(&mut self, request: &Request, now: u64) -> Answer {
        let cseq = if request.headers.count(&HeaderName::CSeq) == 1 {
            match request.headers.typed::<CSeq>() {
                Some(Ok(CSeq {
                    sequence,
                    method: Method::Subscribe,
                })) => sequence,
                _ => return Answer::Malformed,
            }
        } else {
            return Answer::Malformed;
        };
        if request.headers.count(&HeaderName::Expires) > 1 {
            return Answer::Malformed;
        }
        let Some(id) = Id::from_request(request) else {
            return Answer::Malformed;
        };
        if !self.packages.serves(&id.event) {
            // §4.2.1.1. Refused by name rather than accepted and then never notified — a
            // subscriber left waiting for a notification cannot tell that from a slow notifier.
            return Answer::Unserved { status: BAD_EVENT };
        }

        let requested = match request.headers.typed::<Expires>() {
            None => self.policy_maximum,
            Some(Ok(expires)) => Duration::from_secs(u64::from(expires.0)),
            Some(Err(_)) => return Answer::Malformed,
        };

        if is_unsubscribe(requested) {
            // Marked terminated rather than removed, so a NOTIFY that crosses it on the wire finds
            // a terminated subscription rather than no subscription at all — which is the
            // difference between "this is over" and "this never existed".
            if let Some(held) = self.held.iter_mut().find(|held| held.id == id) {
                if cseq <= held.remote_cseq {
                    return Answer::OutOfOrder { id };
                }
                held.remote_cseq = cseq;
                held.state = State::Terminated;
            }
            return Answer::Unsubscribed { id };
        }

        let expires = granted_expiry(requested, self.policy_maximum);
        let expires_at = now.saturating_add(expires.as_secs());

        if let Some(held) = self.held.iter_mut().find(|held| held.id == id) {
            // §4.1.2.2: a refresh on an existing dialog pushes the timer out. A *terminated*
            // subscription is not refreshed back to life — §4.1.3 makes termination final, and a
            // subscriber that wants another one sends a SUBSCRIBE in a new dialog.
            if cseq <= held.remote_cseq {
                return Answer::OutOfOrder { id };
            }
            if held.state == State::Terminated {
                return Answer::Unserved { status: BAD_EVENT };
            }
            held.remote_cseq = cseq;
            held.expires_at = expires_at;
            held.state = State::Active;
            return Answer::Refreshed { id, expires };
        }

        if self.active() >= self.capacity {
            return Answer::AtCapacity;
        }

        self.held.push(Served {
            id: id.clone(),
            state: State::Active,
            expires_at,
            remote_cseq: cseq,
        });
        Answer::Established { id, expires }
    }

    /// End a subscription deliberately.
    ///
    /// Returns the state to put in the terminating NOTIFY, or `None` if there was nothing to end.
    pub fn terminate(&mut self, id: &Id, reason: Reason) -> Option<Subscription> {
        let held = self.held.iter_mut().find(|held| &held.id == id)?;
        if held.state == State::Terminated {
            // Already over. Reporting it again would send a second terminating NOTIFY for one
            // subscription, which a subscriber is entitled to find confusing.
            return None;
        }
        held.state = State::Terminated;
        Some(Subscription::terminated(reason))
    }

    /// Terminate everything that has run out of time, and say which (§4.1.3, `reason=timeout`).
    pub fn expire(&mut self, now: u64) -> Vec<Id> {
        let mut expired = Vec::new();
        for held in &mut self.held {
            if held.state != State::Terminated && held.expires_at <= now {
                held.state = State::Terminated;
                expired.push(held.id.clone());
            }
        }
        expired
    }

    /// The `Subscription-State` a NOTIFY for this subscription should carry.
    ///
    /// `None` when the subscription is terminated or unknown — **which is what stops a terminated
    /// subscription being notified**. A notifier that produced an `active` state here for a
    /// subscription it had ended would resurrect it, and the subscriber would go on believing it
    /// was watching something.
    #[must_use]
    pub fn notify_state(&self, id: &Id, now: u64) -> Option<Subscription> {
        let held = self.held.iter().find(|held| &held.id == id)?;
        match held.state {
            State::Terminated => None,
            state => Some(Subscription {
                state,
                expires: Some(Duration::from_secs(held.expires_at.saturating_sub(now))),
                reason: None,
                retry_after: None,
            }),
        }
    }

    /// Forget terminated subscriptions.
    ///
    /// Separate from terminating them, and deliberately so: a terminated subscription has to stay
    /// findable long enough for its terminating NOTIFY to be sent and for a crossing refresh to be
    /// refused rather than treated as new.
    pub fn sweep(&mut self) -> usize {
        let before = self.held.len();
        self.held.retain(|held| held.state != State::Terminated);
        before - self.held.len()
    }
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
    use sipx_sip::{Limits, Message, parse_datagram};

    const NOW: u64 = 1_700_000_000;

    fn subscribe(event: &str, expires: Option<u64>, tag: &str) -> Request {
        subscribe_with_cseq(event, expires, tag, 1)
    }

    fn subscribe_with_cseq(event: &str, expires: Option<u64>, tag: &str, cseq: u32) -> Request {
        let expires_line =
            expires.map_or_else(String::new, |seconds| format!("Expires: {seconds}\r\n"));
        let text = format!(
            "SUBSCRIBE sip:alice@sipx.test SIP/2.0\r\n\
             Via: SIP/2.0/UDP watcher.example;branch=z9hG4bKx\r\n\
             To: <sip:alice@sipx.test>\r\n\
             From: <sip:watcher@example.net>;tag={tag}\r\n\
             Call-ID: sub-1@watcher\r\n\
             CSeq: {cseq} SUBSCRIBE\r\n\
             Event: {event}\r\n\
             {expires_line}\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Request(request) => request,
            Message::Response(_) => panic!("a request"),
        }
    }

    fn notifier() -> Subscriptions {
        Subscriptions::new(
            Packages::new().with("dialog").with("presence"),
            Duration::from_secs(3600),
        )
    }

    #[test]
    fn a_new_subscription_is_refused_at_the_bound_but_a_refresh_is_not() {
        let mut notifier = notifier().with_capacity(1);
        let first = subscribe("dialog", Some(600), "w1");
        assert!(matches!(
            notifier.on_subscribe(&first, NOW),
            Answer::Established { .. }
        ));

        let second = subscribe("presence", Some(600), "w2");
        assert_eq!(notifier.on_subscribe(&second, NOW), Answer::AtCapacity);
        assert_eq!(notifier.active(), 1);

        let refresh = subscribe_with_cseq("dialog", Some(600), "w1", 2);
        assert!(matches!(
            notifier.on_subscribe(&refresh, NOW + 1),
            Answer::Refreshed { .. }
        ));
    }

    #[test]
    fn malformed_expiry_and_cseq_do_not_mutate_the_store() {
        let mut notifier = notifier();
        for (name, value) in [
            (HeaderName::Expires, "4294967296"),
            (HeaderName::CSeq, "not-a-cseq"),
            (HeaderName::CSeq, "2 MESSAGE"),
        ] {
            let mut request = subscribe("dialog", Some(600), "w1");
            request.headers.remove_all(&name);
            request
                .headers
                .push(sipx_sip::Header::build(name, value).expect("syntactic header"));
            assert_eq!(notifier.on_subscribe(&request, NOW), Answer::Malformed);
            assert_eq!(notifier.active(), 0);
            assert!(notifier.all().is_empty());
        }
    }

    /// The story's failing-first test.
    ///
    /// Once a subscription is over it stays over. A notifier that produced an `active` state for a
    /// terminated subscription would resurrect it, and the subscriber would go on believing it was
    /// watching something that has stopped being watched.
    #[test]
    fn a_terminated_subscription_stops_notifying() {
        let mut notifier = notifier();
        let request = subscribe("dialog", Some(600), "w1");
        let Answer::Established { id, .. } = notifier.on_subscribe(&request, NOW) else {
            panic!("a new subscription");
        };

        assert!(
            notifier.notify_state(&id, NOW).is_some(),
            "an active subscription is notified"
        );

        notifier
            .terminate(&id, Reason::NoResource)
            .expect("the terminating state");

        assert!(
            notifier.notify_state(&id, NOW).is_none(),
            "a terminated subscription must produce no further notifications"
        );
        assert_eq!(notifier.active(), 0);

        // And a refresh does not bring it back. §4.1.3 makes termination final; a subscriber that
        // wants another subscription starts a new dialog.
        let refresh = subscribe_with_cseq("dialog", Some(600), "w1", 2);
        assert_eq!(
            notifier.on_subscribe(&refresh, NOW),
            Answer::Unserved { status: BAD_EVENT },
            "a terminated subscription must not be refreshed back to life"
        );
        assert!(notifier.notify_state(&id, NOW).is_none());
    }

    #[test]
    fn a_subscribe_establishes_and_a_second_one_refreshes() {
        let mut notifier = notifier();
        let first = notifier.on_subscribe(&subscribe("dialog", Some(600), "w1"), NOW);
        let Answer::Established { id, expires } = first else {
            panic!("a new subscription, got {first:?}");
        };
        assert_eq!(expires, Duration::from_secs(600));
        assert_eq!(notifier.active(), 1);

        let again = notifier.on_subscribe(
            &subscribe_with_cseq("dialog", Some(900), "w1", 2),
            NOW + 300,
        );
        assert_eq!(
            again,
            Answer::Refreshed {
                id: id.clone(),
                expires: Duration::from_secs(900)
            }
        );
        assert_eq!(
            notifier.active(),
            1,
            "a refresh is not a second subscription"
        );
        assert_eq!(
            notifier
                .notify_state(&id, NOW + 300)
                .expect("active")
                .expires,
            Some(Duration::from_secs(900)),
            "the timer was pushed out"
        );
    }

    /// §4.2.1.1: "the server MAY shorten the interval but MUST NOT lengthen it".
    #[test]
    fn a_notifier_shortens_a_generous_request_to_its_policy() {
        let mut notifier = notifier();
        let answer = notifier.on_subscribe(&subscribe("dialog", Some(86400), "w1"), NOW);
        let Answer::Established { expires, .. } = answer else {
            panic!("a new subscription");
        };
        assert_eq!(expires, Duration::from_secs(3600), "the policy maximum");
    }

    /// §3.1.1: `Expires: 0` unsubscribes, and §4.2.1.4 still owes a terminating NOTIFY.
    #[test]
    fn an_expires_of_zero_ends_the_subscription() {
        let mut notifier = notifier();
        let Answer::Established { id, .. } =
            notifier.on_subscribe(&subscribe("dialog", Some(600), "w1"), NOW)
        else {
            panic!("a new subscription");
        };

        assert_eq!(
            notifier.on_subscribe(&subscribe_with_cseq("dialog", Some(0), "w1", 2), NOW,),
            Answer::Unsubscribed { id: id.clone() }
        );
        assert_eq!(notifier.active(), 0);
        assert!(
            notifier.notify_state(&id, NOW).is_none(),
            "nothing further is notified after an unsubscribe"
        );
        assert!(
            notifier.all().iter().any(|held| held.id == id),
            "and it is still findable, so a NOTIFY crossing it finds a terminated subscription \
             rather than none at all"
        );
    }

    /// §4.2.1.1: an unserved package is refused 489 — not accepted and then never notified, which
    /// a subscriber cannot tell from a slow notifier.
    #[test]
    fn a_package_this_notifier_does_not_serve_is_refused_with_489() {
        let mut notifier = notifier();
        assert_eq!(
            notifier.on_subscribe(&subscribe("message-summary", Some(600), "w1"), NOW),
            Answer::Unserved { status: 489 }
        );
        assert_eq!(notifier.active(), 0, "nothing was established");
    }

    #[test]
    fn a_subscription_that_runs_out_of_time_is_terminated() {
        let mut notifier = notifier();
        let Answer::Established { id, .. } =
            notifier.on_subscribe(&subscribe("dialog", Some(60), "w1"), NOW)
        else {
            panic!("a new subscription");
        };

        assert!(notifier.expire(NOW + 59).is_empty(), "not yet");
        assert_eq!(notifier.expire(NOW + 60), vec![id.clone()]);
        assert_eq!(notifier.active(), 0);
        assert!(notifier.notify_state(&id, NOW + 60).is_none());
    }

    /// §4.4.1: several subscriptions may share a dialog if their `Event` differs, so the package
    /// is part of the identity. Keying on the dialog alone lets a second subscription silently
    /// replace the first.
    #[test]
    fn two_packages_in_one_dialog_are_two_subscriptions() {
        let mut notifier = notifier();
        let first = notifier.on_subscribe(&subscribe("dialog", Some(600), "w1"), NOW);
        let second = notifier.on_subscribe(&subscribe("presence", Some(600), "w1"), NOW);
        assert!(matches!(first, Answer::Established { .. }));
        assert!(
            matches!(second, Answer::Established { .. }),
            "a different Event in the same dialog is a new subscription, not a refresh"
        );
        assert_eq!(notifier.active(), 2);
    }

    #[test]
    fn terminating_twice_reports_the_ending_once() {
        let mut notifier = notifier();
        let Answer::Established { id, .. } =
            notifier.on_subscribe(&subscribe("dialog", Some(600), "w1"), NOW)
        else {
            panic!("a new subscription");
        };
        assert!(notifier.terminate(&id, Reason::GiveUp).is_some());
        assert!(
            notifier.terminate(&id, Reason::GiveUp).is_none(),
            "one subscription gets one terminating NOTIFY"
        );
    }

    #[test]
    fn sweeping_forgets_only_what_has_ended() {
        let mut notifier = notifier();
        let Answer::Established { id, .. } =
            notifier.on_subscribe(&subscribe("dialog", Some(600), "w1"), NOW)
        else {
            panic!("a new subscription");
        };
        let _ = notifier.on_subscribe(&subscribe("presence", Some(600), "w1"), NOW);
        notifier.terminate(&id, Reason::Timeout);

        assert_eq!(notifier.sweep(), 1);
        assert_eq!(notifier.all().len(), 1);
        assert_eq!(notifier.active(), 1);
    }

    #[test]
    fn a_subscribe_without_an_event_is_malformed() {
        let text = "SUBSCRIBE sip:alice@sipx.test SIP/2.0\r\n\
             Via: SIP/2.0/UDP watcher.example;branch=z9hG4bKx\r\n\
             To: <sip:alice@sipx.test>\r\n\
             From: <sip:watcher@example.net>;tag=w1\r\n\
             Call-ID: sub-1@watcher\r\n\
             CSeq: 1 SUBSCRIBE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n";
        let request = match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses")
        {
            Message::Request(request) => request,
            Message::Response(_) => panic!("a request"),
        };
        assert_eq!(notifier().on_subscribe(&request, NOW), Answer::Malformed);
    }

    #[test]
    fn a_subscribe_with_no_expires_gets_the_policy_maximum() {
        let mut notifier = notifier();
        let answer = notifier.on_subscribe(&subscribe("dialog", None, "w1"), NOW);
        let Answer::Established { expires, .. } = answer else {
            panic!("a new subscription");
        };
        assert_eq!(expires, Duration::from_secs(3600));
    }

    #[test]
    fn replayed_subscribe_cseq_cannot_refresh_or_terminate() {
        let mut notifier = notifier();
        let Answer::Established { id, .. } =
            notifier.on_subscribe(&subscribe_with_cseq("dialog", Some(600), "w1", 20), NOW)
        else {
            panic!("a new subscription");
        };
        let before = notifier.notify_state(&id, NOW).expect("active");

        for request in [
            subscribe_with_cseq("dialog", Some(900), "w1", 20),
            subscribe_with_cseq("dialog", Some(0), "w1", 19),
        ] {
            assert_eq!(
                notifier.on_subscribe(&request, NOW + 30),
                Answer::OutOfOrder { id: id.clone() }
            );
            assert_eq!(notifier.notify_state(&id, NOW), Some(before.clone()));
            assert_eq!(notifier.active(), 1);
        }

        assert!(matches!(
            notifier.on_subscribe(
                &subscribe_with_cseq("dialog", Some(900), "w1", 21),
                NOW + 30,
            ),
            Answer::Refreshed { .. }
        ));
        assert_eq!(
            notifier
                .all()
                .iter()
                .find(|served| served.id == id)
                .expect("subscription remains")
                .remote_cseq,
            21
        );
    }

    #[test]
    fn event_identity_uses_only_exact_type_and_id_tokens() {
        let reordered = Id::from_request(&subscribe(
            "dialog;vendor=one;ID=Opaque-A;mode=full",
            Some(600),
            "w1",
        ))
        .expect("identity");
        let differently_ordered = Id::from_request(&subscribe(
            "dialog;mode=partial;id=Opaque-A;vendor=two",
            Some(600),
            "w1",
        ))
        .expect("identity");
        assert_eq!(reordered, differently_ordered);
        assert_eq!(reordered.event, "dialog;id=Opaque-A");

        let changed_type_case =
            Id::from_request(&subscribe("Dialog;id=Opaque-A", Some(600), "w1")).expect("identity");
        let changed_id_case =
            Id::from_request(&subscribe("dialog;id=opaque-a", Some(600), "w1")).expect("identity");
        assert_ne!(reordered, changed_type_case, "event-type is byte matched");
        assert_ne!(reordered, changed_id_case, "id is an opaque token");

        assert!(
            Id::from_request(&subscribe("dialog;id=one;ID=two", Some(600), "w1")).is_none(),
            "duplicate identity parameters fail closed"
        );
    }

    #[test]
    fn duplicate_identity_headers_fail_before_first_value_selection() {
        for name in [HeaderName::CallId, HeaderName::From, HeaderName::Event] {
            let mut request = subscribe("dialog", Some(600), "w1");
            let value = request.headers.value(&name).expect("header").into_owned();
            request
                .headers
                .push(sipx_sip::Header::build(name, value).expect("syntactic header"));
            assert!(Id::from_request(&request).is_none());
            assert_eq!(notifier().on_subscribe(&request, NOW), Answer::Malformed);
        }
    }
}
