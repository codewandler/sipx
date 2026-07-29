//! The UPDATE method (RFC 3311).
//!
//! A re-INVITE renegotiates a session that is already up. It cannot renegotiate one that is
//! not: until the INVITE has a final response there is a transaction in progress, and a second
//! INVITE inside it is not a thing SIP has. UPDATE is the request that fills that hole — an
//! in-dialog renegotiation that runs alongside the INVITE transaction without disturbing it —
//! and RFC 4028 §7.4 then reuses it as the cheaper way to refresh a session timer.
//!
//! Everything here is pure. What is written down is the offer/answer bookkeeping a dialog has
//! to keep in order to decide whether an UPDATE may be sent or accepted, and the three
//! different refusals §5.2 requires when it may not. The sending, the clock and the randomness
//! live a layer up; see `docs/specs/sip-update.md`.

use crate::headers::misc::Allow;
use crate::message::{Headers, TypedHeader as _};
use crate::name::HeaderName;

/// The `Allow` value sipx advertises (RFC 3311 §4, RFC 3261 §20.5).
///
/// One constant rather than a literal at each site that writes the header, because §4 makes
/// this list the *only* way a peer is permitted to decide it may send an UPDATE at all. A copy
/// that drifts is a peer that silently falls back to a re-INVITE forever, on a path no test
/// looks at — the failure is invisible from this side, which is exactly the kind that survives.
pub const ALLOW: &str = "INVITE, ACK, CANCEL, BYE, OPTIONS, UPDATE";

/// The largest `Retry-After` a §5.2 refusal may name.
///
/// §5.2 asks for "a randomly chosen value between 0 and 10 seconds". The number is drawn by the
/// caller and passed in: this crate reads no clock and no entropy source, and reaching for one
/// here would be the first I/O in a sans-IO core.
pub const RETRY_AFTER_MAX_SECS: u64 = 10;

/// Whether a peer's `Allow` lists UPDATE (RFC 3311 §4).
///
/// Absent means no. §4 is a `SHOULD` on the *sender*, so a peer that supports UPDATE and does
/// not say so is indistinguishable from one that does not support it — and guessing wrong turns
/// a session refresh into a request the far end answers 405, which is a call torn down for a
/// capability nobody needed.
#[must_use]
pub fn peer_allows(headers: &Headers) -> bool {
    // Every `Allow` row, not only the first: RFC 3261 §7.3 makes one row of `n` tokens and `n`
    // rows of one token the same message, and a peer that writes the second form would
    // otherwise be read as allowing whatever happened to land on line one.
    headers
        .get_all(&HeaderName::Allow)
        .filter_map(|header| Allow::decode(&header.value()).ok())
        .any(|allow| allow.contains(METHOD))
}

/// The method token, spelled once.
const METHOD: &str = "UPDATE";

/// Why an UPDATE cannot be processed now (RFC 3311 §5.2).
///
/// Three variants and not one, because the distinction is the whole value of the section: a
/// peer's retry logic is built on it. 491 means the two sides collided and both should wait a
/// randomised interval (RFC 3261 §14.1); 500 with `Retry-After` means the request was
/// well-formed and badly timed, and the same one will work shortly. A peer told the wrong one
/// either backs off when it did not need to or retries straight into the same wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A previous UPDATE has not had its final response yet.
    ///
    /// §5.2's first rule, and the only one that applies to an UPDATE carrying no offer at all:
    /// it is about the transaction, not about any description.
    InProgress,
    /// An offer arrived while this side's own offer is unanswered — glare.
    Glare,
    /// An offer arrived while this side still owes an answer to one already received.
    AnswerOwed,
}

impl Refusal {
    /// The status code to answer with.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            // §5.2: "MUST reject the UPDATE with a 491 response".
            Self::Glare => 491,
            // §5.2: both of the others are 500 with a `Retry-After`. They stay separate
            // variants even so — the reason a request was too early is worth logging, and a
            // caller that wants to distinguish them can, which a single `TooEarly` would have
            // made impossible for everyone.
            Self::InProgress | Self::AnswerOwed => 500,
        }
    }

    /// The reason phrase for that status.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Glare => "Request Pending",
            Self::InProgress | Self::AnswerOwed => "Server Internal Error",
        }
    }

    /// Whether the response must carry a `Retry-After` (§5.2).
    #[must_use]
    pub const fn retry_after(self) -> bool {
        match self {
            Self::Glare => false,
            Self::InProgress | Self::AnswerOwed => true,
        }
    }
}

/// What to do with an UPDATE that has arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reception {
    /// Process it: renegotiate if it carried an offer, and answer 2xx.
    Accept,
    /// Refuse it, without disturbing the dialog.
    Refuse(Refusal),
}

/// An UPDATE that has been accepted and not yet answered, and what it brought with it.
///
/// The distinction is load-bearing rather than descriptive. Answering an UPDATE settles the
/// debt *that UPDATE created* — and an offerless one created none. Forgetting which kind it was
/// is how a session refresh comes to cancel the INVITE's outstanding offer; see
/// [`Negotiation::answered`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// It carried an offer, so its answer pays for that offer.
    WithOffer,
    /// It carried none — an RFC 4028 §7.4 refresh, say — so its 2xx settles nothing.
    Offerless,
}

/// One dialog's offer/answer bookkeeping, as far as UPDATE is concerned (RFC 3264, RFC 3311 §5).
///
/// Three pieces of state, and the reason they are three rather than one is §5.2: a dialog that
/// owes an answer and a dialog whose own offer is unanswered are different situations that
/// produce different refusals, and an UPDATE already being processed is a third that has
/// nothing to do with descriptions at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Negotiation {
    /// We have sent an offer and have not received its answer.
    offered: bool,
    /// We have received an offer and have not sent its answer.
    ///
    /// Set by an INVITE's offer as much as by an UPDATE's, which is why it cannot simply be
    /// cleared whenever an UPDATE is answered.
    owed: bool,
    /// The UPDATE accepted and not yet answered, if there is one.
    in_progress: Option<Pending>,
}

impl Negotiation {
    /// Nothing outstanding in either direction.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            offered: false,
            owed: false,
            in_progress: None,
        }
    }

    /// The state of a UAC that has just sent an INVITE carrying an offer.
    #[must_use]
    pub const fn offering() -> Self {
        Self {
            offered: true,
            ..Self::idle()
        }
    }

    /// The state of a UAS that has just received an INVITE carrying an offer.
    #[must_use]
    pub const fn owing() -> Self {
        Self {
            owed: true,
            ..Self::idle()
        }
    }

    /// Whether an offer of ours is unanswered.
    #[must_use]
    pub const fn is_offering(self) -> bool {
        self.offered
    }

    /// Whether we owe the peer an answer.
    #[must_use]
    pub const fn owes_answer(self) -> bool {
        self.owed
    }

    /// Record that we put an offer on the wire.
    pub const fn sent_offer(&mut self) {
        self.offered = true;
    }

    /// Record that the answer to our offer arrived.
    pub const fn received_answer(&mut self) {
        self.offered = false;
    }

    /// Record that an offer arrived and is unanswered.
    pub const fn received_offer(&mut self) {
        self.owed = true;
    }

    /// Record that we answered the offer we were holding.
    pub const fn sent_answer(&mut self) {
        self.owed = false;
    }

    /// Whether an UPDATE this side sends may carry an offer (RFC 3311 §5.1).
    ///
    /// RFC 3264's one-offer-at-a-time rule, seen from the sending end: not while ours is
    /// unanswered, and not while we owe one. `in_progress` does not appear — that is the
    /// *peer's* transaction, and an offer of ours is unrelated to it.
    #[must_use]
    pub const fn may_offer(self) -> bool {
        !self.offered && !self.owed
    }

    /// Decide what to do with an incoming UPDATE (RFC 3311 §5.2), and record the decision.
    ///
    /// The order is normative rather than incidental: the in-progress rule is checked first
    /// because it applies to *every* UPDATE, including one with no body, and answering a
    /// second UPDATE 491 because the first one's offer is still open would tell the peer it
    /// collided with us when what actually happened is that it was early.
    ///
    /// A refusal changes nothing. It is itself a final response, so there is no transaction
    /// left in progress and no description has moved.
    pub const fn receive(&mut self, has_offer: bool) -> Reception {
        if self.in_progress.is_some() {
            return Reception::Refuse(Refusal::InProgress);
        }
        if has_offer {
            if self.offered {
                return Reception::Refuse(Refusal::Glare);
            }
            if self.owed {
                return Reception::Refuse(Refusal::AnswerOwed);
            }
            self.owed = true;
            self.in_progress = Some(Pending::WithOffer);
        } else {
            self.in_progress = Some(Pending::Offerless);
        }
        Reception::Accept
    }

    /// Record that the final response to the accepted UPDATE has gone out.
    ///
    /// Clears **only the debt that UPDATE created**. When it carried an offer the 2xx carried
    /// the answer (§5.2: the UAS "MUST ... generate an answer in the 2xx response") and the
    /// debt is paid; when it carried none — the RFC 4028 §7.4 refresh, which is the most
    /// ordinary UPDATE a peer sends — it created no debt and pays none.
    ///
    /// Clearing `owed` regardless was a real defect and not a tidiness point. An offerless
    /// refresh arriving in an early dialog would wipe the INVITE's outstanding offer, and the
    /// next UPDATE carrying one would then be *accepted* and answered 488 for a description
    /// that was perfectly good — where §5.2 rule 3 requires 500 with `Retry-After`, which is
    /// the difference between "your description is unusable" and "you are early".
    ///
    /// Calling this with nothing in progress is a no-op, so a caller that clears on an error
    /// path cannot destroy state it did not create.
    pub const fn answered(&mut self) {
        if matches!(self.in_progress, Some(Pending::WithOffer)) {
            self.owed = false;
        }
        self.in_progress = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{Limits, Message, parse_datagram};

    fn headers(allow: &str) -> Headers {
        let text = format!(
            "INVITE sip:b@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1;branch=z9hG4bKx\r\n\
             To: <sip:b@example.com>\r\n\
             From: <sip:a@example.net>;tag=1\r\n\
             Call-ID: c\r\n\
             CSeq: 1 INVITE\r\n\
             {allow}\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(bytes::Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Request(r) => r.headers,
            Message::Response(_) => panic!("a request"),
        }
    }

    /// §8.3 of the spec.
    #[test]
    fn the_peers_allow_is_the_only_permission_there_is() {
        assert!(peer_allows(&headers(
            "Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, UPDATE\r\n"
        )));
        assert!(!peer_allows(&headers("Allow: INVITE, ACK, BYE\r\n")));
        // RFC 3261 §7.3.1: tokens are case-insensitive and the spacing is free.
        assert!(peer_allows(&headers("Allow: invite,update\r\n")));
        // A token, not a substring. `UPDATEX` is a different method.
        assert!(!peer_allows(&headers("Allow: INVITE, UPDATEX\r\n")));
        // Silence means no. §4 is a SHOULD on the sender, so a peer that supports UPDATE and
        // does not say so cannot be told apart from one that does not.
        assert!(!peer_allows(&headers("")));
        // Spread over two rows is still one list.
        assert!(peer_allows(&headers("Allow: INVITE\r\nAllow: UPDATE\r\n")));
    }

    /// Our own advertisement has to contain the method, or §4 is unmet from this side.
    #[test]
    fn the_allow_we_advertise_lists_update() {
        assert!(peer_allows(&headers(&format!("Allow: {ALLOW}\r\n"))));
    }

    /// §8.1 of the spec, row by row.
    #[test]
    fn the_three_refusals_are_three_different_answers() {
        let accept = |mut state: Negotiation, offer| state.receive(offer);

        // Idle: both forms are accepted.
        assert_eq!(accept(Negotiation::idle(), true), Reception::Accept);
        assert_eq!(accept(Negotiation::idle(), false), Reception::Accept);

        // Rule 1 covers an UPDATE with no body at all — it is about the transaction.
        let mut busy = Negotiation::idle();
        assert_eq!(busy.receive(false), Reception::Accept);
        assert_eq!(
            busy.receive(false),
            Reception::Refuse(Refusal::InProgress),
            "a second UPDATE before the first was answered"
        );
        assert_eq!(busy.receive(true), Reception::Refuse(Refusal::InProgress));

        // Rule 2: our offer is unanswered, so this is glare and the peer may retry after a
        // randomised back-off.
        assert_eq!(
            accept(Negotiation::offering(), true),
            Reception::Refuse(Refusal::Glare)
        );
        // ...but an offerless UPDATE collides with nothing.
        assert_eq!(accept(Negotiation::offering(), false), Reception::Accept);

        // Rule 3: we owe an answer. Not glare — nothing of ours is outstanding, the peer is
        // simply early.
        assert_eq!(
            accept(Negotiation::owing(), true),
            Reception::Refuse(Refusal::AnswerOwed)
        );
        assert_eq!(accept(Negotiation::owing(), false), Reception::Accept);
    }

    /// Order matters: rule 1 is checked before rule 2, so a peer that is early is told it is
    /// early rather than told it collided with us.
    #[test]
    fn an_update_in_progress_outranks_glare() {
        let mut state = Negotiation::offering();
        assert_eq!(state.receive(false), Reception::Accept);
        assert_eq!(state.receive(true), Reception::Refuse(Refusal::InProgress));
    }

    #[test]
    fn each_refusal_carries_what_the_peer_needs_to_act_on_it() {
        assert_eq!(Refusal::Glare.status(), 491);
        assert!(
            !Refusal::Glare.retry_after(),
            "491 is resolved by RFC 3261 §14.1's randomised wait, not by a header we choose"
        );
        for refusal in [Refusal::InProgress, Refusal::AnswerOwed] {
            assert_eq!(refusal.status(), 500);
            assert!(
                refusal.retry_after(),
                "§5.2 requires Retry-After on both 500s; without it the peer learns only that \
                 it failed"
            );
        }
        assert_ne!(Refusal::Glare.reason(), Refusal::InProgress.reason());
    }

    /// §8.2 of the spec.
    #[test]
    fn an_offer_may_only_go_out_when_nothing_is_outstanding() {
        assert!(Negotiation::idle().may_offer());
        assert!(!Negotiation::offering().may_offer(), "ours is unanswered");
        assert!(!Negotiation::owing().may_offer(), "we owe theirs");

        // An UPDATE we are processing is the peer's transaction. It does not stop us offering.
        let mut busy = Negotiation::idle();
        assert_eq!(busy.receive(false), Reception::Accept);
        assert!(busy.may_offer());
    }

    /// The defect, stated as a test: an offerless UPDATE must not settle a debt it never took
    /// on. RFC 4028 §7.4's refresh is exactly such an UPDATE and arrives on every timed call.
    #[test]
    fn an_offerless_update_does_not_pay_a_debt_it_never_incurred() {
        // An early dialog: the INVITE's offer is in hand and unanswered.
        let mut state = Negotiation::owing();

        // A refresh comes through. Perfectly legal, and answered 200 with no description.
        assert_eq!(state.receive(false), Reception::Accept);
        state.answered();
        assert!(
            state.owes_answer(),
            "an offerless refresh cancelled the INVITE's outstanding offer"
        );

        // So the next offer is still refused for the right reason. Without this the UPDATE
        // would be accepted, renegotiated against a session whose first offer/answer never
        // completed, and — when that failed — answered 488, telling the peer its description
        // was unusable when the description was fine and the moment was not.
        assert_eq!(
            state.receive(true),
            Reception::Refuse(Refusal::AnswerOwed),
            "§5.2 rule 3 was lost to a refresh that arrived first"
        );
    }

    /// The mirror: an UPDATE that *did* carry an offer settles that offer and nothing else.
    #[test]
    fn an_offer_carrying_update_settles_exactly_its_own_offer() {
        let mut state = Negotiation::idle();
        assert_eq!(state.receive(true), Reception::Accept);
        assert!(state.owes_answer());
        state.answered();
        assert!(!state.owes_answer());

        // And a stray `answered` with nothing in progress cannot clear a debt either, which is
        // what makes it safe to call from an error path.
        let mut owing = Negotiation::owing();
        owing.answered();
        assert!(owing.owes_answer());
    }

    #[test]
    fn an_accepted_update_clears_when_it_is_answered() {
        let mut state = Negotiation::idle();
        assert_eq!(state.receive(true), Reception::Accept);
        assert!(state.owes_answer(), "the offer it carried is unanswered");
        assert!(!state.may_offer());

        state.answered();
        assert_eq!(state, Negotiation::idle());
        assert!(state.may_offer());
        // And the next one is accepted rather than refused as a duplicate.
        assert_eq!(state.receive(true), Reception::Accept);
    }

    #[test]
    fn the_two_directions_are_tracked_apart() {
        let mut state = Negotiation::offering();
        assert!(state.is_offering());
        assert!(!state.owes_answer());
        state.received_answer();
        assert_eq!(state, Negotiation::idle());

        state.received_offer();
        assert!(state.owes_answer());
        assert!(!state.is_offering());
        state.sent_answer();
        assert_eq!(state, Negotiation::idle());

        // Both at once is possible — a re-INVITE crossing an UPDATE — and neither flag may
        // clear the other.
        state.sent_offer();
        state.received_offer();
        state.received_answer();
        assert!(state.owes_answer(), "answering ours cleared theirs");
    }
}
