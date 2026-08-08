//! The app, as the host's decision logic sees it.
//!
//! **This trait is where acceptance point 4 is enforced.** A binding does not *wait* for an app and
//! return what it said — it **declares, up front, when it will answer and with what**:
//!
//! ```text
//! fn respond(&mut self, event: &Event) -> Reply     // Reply { after: Duration, outcome }
//! ```
//!
//! A real HTTP client cannot implement that honestly, because it cannot know `after` before it has
//! made the call. There is no `async`, no socket, no `Result<_, io::Error>`, and no way to reach
//! wall-clock time from inside — so "the app took 300 ms" is *data the scenario states* rather than
//! something the machine running the test happens to produce. A scenario needing a real socket or
//! real time is not merely discouraged here; it cannot be written down.
//!
//! The real bindings (`A-2` document mode, `A-4` session mode) are adapters *outside* this trait:
//! they own the socket and the clock, and feed the same decision logic. What they must agree with
//! is the vector set in [`super::vectors`], which is the point of acceptance 3 — one set of
//! failure-semantics scenarios, shared, rather than rewritten per binding.

use std::collections::VecDeque;
use std::time::Duration;

use super::contract::{Document, Event};

/// What the app said, once it said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A document — the entire new program (§6.3). An empty one means "keep going".
    Document(Document),
    /// Raw document bytes. The protocol interpreter, not the harness, decides whether they are a
    /// valid whole document (§6.4).
    Body(String),
    /// The app answered 4xx: the request itself is wrong.
    ClientError {
        /// The status.
        status: u16,
    },
    /// The app answered 5xx.
    ServerError {
        /// The status.
        status: u16,
    },
    /// The app could not be reached at all — no connection, no listener, nothing.
    ///
    /// Distinct from a slow app: `on_unreachable` and `on_timeout` are separate knobs, and a host
    /// that collapsed them would make one of them undeclarable.
    Unreachable,
    /// The app never answers. The callback timer decides what happens (§9.2 `on_timeout`).
    Silent,
}

/// When the app answers, and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// How long after delivery the answer arrives. Compared against the policy's `timeout`, which
    /// is what makes "the slow app" an ordinary test case rather than a flaky one.
    pub after: Duration,
    /// What it says.
    pub outcome: Outcome,
}

impl Reply {
    /// An immediate document.
    #[must_use]
    pub fn now(document: Document) -> Self {
        Self {
            after: Duration::ZERO,
            outcome: Outcome::Document(document),
        }
    }

    /// A document, this long after the event was delivered.
    #[must_use]
    pub fn after(delay: Duration, document: Document) -> Self {
        Self {
            after: delay,
            outcome: Outcome::Document(document),
        }
    }

    /// Raw response bytes, interpreted only by `sipx-app-protocol`.
    #[must_use]
    pub fn body(body: impl Into<String>) -> Self {
        Self {
            after: Duration::ZERO,
            outcome: Outcome::Body(body.into()),
        }
    }

    /// A failure, this long after delivery.
    #[must_use]
    pub fn failing(delay: Duration, outcome: Outcome) -> Self {
        Self {
            after: delay,
            outcome,
        }
    }

    /// An app that never answers.
    #[must_use]
    pub fn silent() -> Self {
        Self {
            after: Duration::ZERO,
            outcome: Outcome::Silent,
        }
    }

    /// An app that is not there.
    ///
    /// Answers immediately, because "connection refused" is what being absent looks like — an
    /// absent app that took the full timeout would be indistinguishable from a slow one, and §9.2
    /// gives them different knobs.
    #[must_use]
    pub fn unreachable() -> Self {
        Self {
            after: Duration::ZERO,
            outcome: Outcome::Unreachable,
        }
    }
}

/// An app the harness can drive.
///
/// See the module docs: implementing this for anything that touches a network is not possible in
/// good faith, which is the design.
pub trait Binding {
    /// What this app will answer to `event`, and when.
    fn respond(&mut self, event: &Event) -> Reply;
}

/// An app scripted turn by turn.
///
/// Replies are consumed in order, one per *delivered* event. A redelivery (§5.1, the same `seq`
/// again) consumes the next reply too — that is exactly how AC-4 poses its question: the app is
/// allowed to answer the redelivery differently, and the host is required not to care.
#[derive(Debug, Default)]
pub struct ScriptedApp {
    replies: VecDeque<Reply>,
    /// What to answer once the script runs out.
    then: Option<Reply>,
    seen: Vec<u64>,
}

impl ScriptedApp {
    /// An app that answers these, in order.
    #[must_use]
    pub fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: replies.into(),
            then: None,
            seen: Vec::new(),
        }
    }

    /// An app that is never there, whatever it is asked.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            replies: VecDeque::new(),
            then: Some(Reply::unreachable()),
            seen: Vec::new(),
        }
    }

    /// After the script runs out, answer this every time.
    ///
    /// Without it a scenario that runs longer than its script would have nothing to say, and
    /// "keep going" is the answer that changes nothing — so a scenario asserting on an early
    /// exchange is not forced to script the whole call.
    #[must_use]
    pub fn then(mut self, reply: Reply) -> Self {
        self.then = Some(reply);
        self
    }

    /// The `seq` of every event this app was asked about, redeliveries included.
    #[must_use]
    pub fn delivered(&self) -> &[u64] {
        &self.seen
    }
}

impl Binding for ScriptedApp {
    fn respond(&mut self, event: &Event) -> Reply {
        self.seen.push(event.seq);
        self.replies.pop_front().unwrap_or_else(|| {
            self.then
                .clone()
                .unwrap_or_else(|| Reply::now(Document::keep_going()))
        })
    }
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
    use crate::harness::contract::EventKind;

    fn event(seq: u64) -> Event {
        Event {
            seq,
            kind: EventKind::Incoming,
        }
    }

    #[test]
    fn a_scripted_app_answers_in_order_and_records_what_it_was_asked() {
        let mut app = ScriptedApp::new(vec![
            Reply::now(Document::keep_going()),
            Reply::failing(
                Duration::from_millis(10),
                Outcome::ServerError { status: 500 },
            ),
        ]);

        assert_eq!(app.respond(&event(1)).after, Duration::ZERO);
        assert_eq!(
            app.respond(&event(2)).outcome,
            Outcome::ServerError { status: 500 }
        );
        assert_eq!(app.delivered(), &[1, 2]);
    }

    /// Running out of script is not a failure — it is "keep going", so a scenario asserting on the
    /// first exchange does not have to script a whole call.
    #[test]
    fn an_exhausted_script_keeps_going_unless_told_otherwise() {
        let mut app = ScriptedApp::new(vec![]);
        assert_eq!(
            app.respond(&event(1)).outcome,
            Outcome::Document(Document::keep_going())
        );

        let mut absent = ScriptedApp::absent();
        assert_eq!(absent.respond(&event(1)).outcome, Outcome::Unreachable);
    }

    /// An absent app answers at once. A host that made it wait the full timeout could not tell
    /// `on_unreachable` from `on_timeout`, and §9.2 gives them separate knobs.
    #[test]
    fn an_absent_app_fails_immediately_rather_than_slowly() {
        assert_eq!(Reply::unreachable().after, Duration::ZERO);
    }
}
