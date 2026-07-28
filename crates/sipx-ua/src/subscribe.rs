//! The subscriptions a notifier is serving (RFC 6665).
//!
//! [`sipx_sip::event`] decides what a `Subscription-State` means. This holds the ones that exist:
//! establishing them, refreshing them, expiring them, and — the part that matters most — making
//! sure a terminated one stays terminated.
//!
//! Time is a parameter, not a call to a clock, for the same reason it is in the timer queue: a
//! notifier driven by a scheduler somebody else owns has to be able to say what "now" is, and a
//! test that wants to watch a subscription expire should not have to wait an hour.

use std::time::Duration;

use sipx_sip::event::{
    BAD_EVENT, Packages, Reason, State, Subscription, granted_expiry, is_unsubscribe,
};
use sipx_sip::{HeaderName, Request};

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
    /// The `Event` package, with its `id` parameter if it has one.
    pub event: String,
}

impl Id {
    /// Read the identity out of a SUBSCRIBE.
    #[must_use]
    pub fn from_request(request: &Request) -> Option<Self> {
        let text = |name: &HeaderName| {
            request
                .headers
                .value(name)
                .map(|value| String::from_utf8_lossy(&value).into_owned())
        };
        Some(Self {
            call_id: text(&HeaderName::CallId)?,
            from_tag: tag_of(&text(&HeaderName::From)?)?,
            event: text(&HeaderName::Event)?.trim().to_owned(),
        })
    }
}

fn tag_of(address: &str) -> Option<String> {
    let start = address.to_ascii_lowercase().find(";tag=")? + 5;
    let rest = address.get(start..)?;
    let end = rest.find(';').unwrap_or(rest.len());
    Some(rest.get(..end)?.trim().to_owned())
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
    /// The request could not be read as a SUBSCRIBE at all.
    Malformed,
}

/// The subscriptions one notifier is serving.
#[derive(Debug)]
pub struct Subscriptions {
    packages: Packages,
    policy_maximum: Duration,
    held: Vec<Served>,
}

impl Subscriptions {
    /// A notifier serving these packages, granting at most this long.
    #[must_use]
    pub fn new(packages: Packages, policy_maximum: Duration) -> Self {
        Self {
            packages,
            policy_maximum,
            held: Vec::new(),
        }
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
        let Some(id) = Id::from_request(request) else {
            return Answer::Malformed;
        };
        if !self.packages.serves(&id.event) {
            // §4.2.1.1. Refused by name rather than accepted and then never notified — a
            // subscriber left waiting for a notification cannot tell that from a slow notifier.
            return Answer::Unserved { status: BAD_EVENT };
        }

        let requested = request
            .headers
            .value(&HeaderName::Expires)
            .and_then(|value| String::from_utf8_lossy(&value).trim().parse::<u64>().ok())
            .map_or(self.policy_maximum, Duration::from_secs);

        if is_unsubscribe(requested) {
            // Marked terminated rather than removed, so a NOTIFY that crosses it on the wire finds
            // a terminated subscription rather than no subscription at all — which is the
            // difference between "this is over" and "this never existed".
            if let Some(held) = self.held.iter_mut().find(|held| held.id == id) {
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
            if held.state == State::Terminated {
                return Answer::Unserved { status: BAD_EVENT };
            }
            held.expires_at = expires_at;
            held.state = State::Active;
            return Answer::Refreshed { id, expires };
        }

        self.held.push(Served {
            id: id.clone(),
            state: State::Active,
            expires_at,
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
        let expires_line =
            expires.map_or_else(String::new, |seconds| format!("Expires: {seconds}\r\n"));
        let text = format!(
            "SUBSCRIBE sip:alice@sipx.test SIP/2.0\r\n\
             Via: SIP/2.0/UDP watcher.example;branch=z9hG4bKx\r\n\
             To: <sip:alice@sipx.test>\r\n\
             From: <sip:watcher@example.net>;tag={tag}\r\n\
             Call-ID: sub-1@watcher\r\n\
             CSeq: 1 SUBSCRIBE\r\n\
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
        let refresh = subscribe("dialog", Some(600), "w1");
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

        let again = notifier.on_subscribe(&subscribe("dialog", Some(900), "w1"), NOW + 300);
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
            notifier.on_subscribe(&subscribe("dialog", Some(0), "w1"), NOW),
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
}
