//! The RFC 4028 session timer: the armed deadline, what firing it means, and the refresh.
//!
//! One deadline per call, held as an absolute instant. Any re-INVITE or UPDATE on the dialog
//! refreshes it (§7.2, §7.4), the refresher re-offers before it fires, and the other side
//! hangs up a call whose refresher has gone silent (§10).

use super::{
    Bytes, Call, Duration, EndCause, Error, HeaderName, Instant, MinSe, Response, Result,
    SessionExpires, in_dialog_target, session,
};

/// A negotiated session timer and the deadline it is currently counting down to.
#[derive(Debug, Clone, Copy)]
pub(super) struct SessionState {
    pub(super) terms: session::Session,
    /// When [`Call::on_session_deadline`] should be called.
    ///
    /// Held as an absolute instant rather than recomputed from "now" on each poll, so that a
    /// call driven by a loop that also does other work cannot have its timer pushed back
    /// indefinitely by its own busyness.
    pub(super) act_at: Instant,
}

impl SessionState {
    pub(super) fn armed(terms: session::Session) -> Self {
        Self {
            terms,
            act_at: Instant::now() + terms.act_after(),
        }
    }
}

/// The `Min-SE` a `422` demands, if it named one (RFC 4028 §6).
pub(super) fn required_interval(response: &Response) -> Option<Duration> {
    response
        .headers
        .typed::<MinSe>()
        .and_then(std::result::Result::ok)
        .map(|min| min.0)
}

impl Call {
    /// The negotiated session interval, and whether this side is the one refreshing it.
    ///
    /// `None` means no timer was agreed, so nothing will ever notice a far end that stops
    /// answering — worth being able to check, because that is a property of the *peer*, not of
    /// what this side asked for.
    #[must_use]
    pub fn session_interval(&self) -> Option<(Duration, bool)> {
        self.session
            .map(|state| (state.terms.interval, state.terms.we_refresh))
    }

    /// When [`Self::on_session_deadline`] next needs to be called, if a timer was negotiated.
    ///
    /// Returned as an instant rather than as a future on purpose. A future would borrow the
    /// call for as long as it was being awaited, which is exactly the borrow
    /// [`Self::handle`] needs in the other arm of the `select!` this is written for.
    #[must_use]
    pub fn session_deadline(&self) -> Option<Instant> {
        self.session.map(|state| state.act_at)
    }

    /// Do whatever the session timer's deadline asked for (RFC 4028 §10).
    ///
    /// For the refresher that is an UPDATE or a re-INVITE — whichever the peer's `Allow` says
    /// it can take (§7.4); for the other side it is a BYE,
    /// because nothing arrived and the far end is presumed gone. Calling this early is harmless
    /// — it re-reads the deadline and does nothing if it has not passed.
    pub async fn on_session_deadline(&mut self) -> Result<()> {
        let Some(state) = self.session else {
            return Ok(());
        };
        if Instant::now() < state.act_at {
            return Ok(());
        }
        if !state.terms.we_refresh {
            // §10: the side that is not refreshing "SHOULD send a BYE to terminate the
            // session". The media stops with it — a half-torn-down call that keeps streaming
            // is the failure this whole mechanism exists to end, not a gentler version of it.
            self.end(EndCause::Timeout).await?;
            return Err(Error::SessionExpired);
        }
        match self.refresh_session().await {
            Ok(()) => Ok(()),
            // §10: a refresh that times out or draws a 408 or 481 means the dialog is gone at
            // the far end, and RFC 3261 §12.2.1.2 says to BYE. Any other failure is about the
            // refresh, not the call: a 491 glare or a 500 leaves the session running until the
            // deadline we do not move, so the next attempt is the retry.
            Err(Error::NoResponse) => {
                self.end(EndCause::Timeout).await?;
                Err(Error::SessionExpired)
            }
            Err(Error::Rejected { status, reason }) => {
                const REQUEST_TIMEOUT: u16 = 408;
                const NO_SUCH_DIALOG: u16 = 481;
                if status == REQUEST_TIMEOUT || status == NO_SUCH_DIALOG {
                    self.end(EndCause::Timeout).await?;
                    return Err(Error::SessionExpired);
                }
                // Push the retry out so a peer answering 500 to every refresh is not asked
                // again immediately for the rest of the session interval.
                self.rearm();
                Err(Error::Rejected { status, reason })
            }
            Err(other) => {
                self.rearm();
                Err(other)
            }
        }
    }

    /// Restart the countdown, because the session was refreshed.
    pub(super) fn rearm(&mut self) {
        if let Some(state) = self.session.as_mut() {
            state.act_at = Instant::now() + state.terms.act_after();
        }
    }

    /// Take the session terms from the 2xx to a refresh we sent (RFC 4028 §7.2).
    ///
    /// Shared by the re-INVITE and the UPDATE paths: §7.2 measures the expiration from the 2xx
    /// and says nothing about which request drew it, so reading it in two places would be two
    /// chances to read it differently.
    pub(super) fn adopt_session(&mut self, response: &Response) {
        if let Some(agreed) = session::adopt(
            response
                .headers
                .typed::<SessionExpires>()
                .and_then(std::result::Result::ok),
            self.session.map(|state| state.terms.interval),
        ) && let Some(state) = self.session.as_mut()
        {
            state.terms = agreed;
        }
    }

    /// Refresh the session, by whichever method the peer allows (RFC 4028 §7.4).
    ///
    /// > "If a UAC knows that its peer supports the UPDATE method, it is RECOMMENDED that
    /// > UPDATE be used instead of a re-INVITE."
    ///
    /// It is only *known* from the peer's `Allow` (RFC 3311 §4), so that is what decides.
    /// Guessing the other way costs a working call: a refresh the far end answers 405 is a
    /// refresh that never happens, and the deadline behind it hangs up on a peer that is alive.
    ///
    /// The UPDATE carries **no body**. A refresh changes nothing — the description in force
    /// stays in force — and re-offering it would put a liveness check under §5.2's offer/answer
    /// rules, where it could be refused 491 or 500 for a reason that has nothing to do with
    /// whether the far end is still there.
    async fn refresh_session(&mut self) -> Result<()> {
        if !self.peer_allows_update {
            return self.reinvite(self.hold).await;
        }

        let (mut builder, routes) =
            crate::update::request(&self.endpoint, &mut self.dialog, &self.target, None)?;
        // §7.4: a refresh names the interval and the refresher in force, so proxies on the path
        // can see the value and object to it. `Min-SE` is this side's own floor, and it is a
        // defence rather than a courtesy (§11.2).
        builder = builder.header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uac
                } else {
                    session::Refresher::Uas
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(
                    HeaderName::MinSe,
                    Bytes::from(session::ABSOLUTE_MIN_INTERVAL.as_secs().to_string()),
                )?;
        }

        let request = crate::update::finish(builder, &routes)?;
        let response = crate::update::send(&self.endpoint, request, self.target.clone()).await?;

        if !response.status.is_success() {
            const INTERVAL_TOO_SMALL: u16 = 422;
            if response.status.code() == INTERVAL_TOO_SMALL
                && let Some(required) = required_interval(&response)
                && let Some(state) = self.session.as_mut()
            {
                // As on the re-INVITE path: only a 2xx extends the expiration, so adopting the
                // longer interval does not buy time. The next attempt is the one that has to
                // land, and it has to land before the deadline that is already running.
                state.terms.interval = required.max(session::ABSOLUTE_MIN_INTERVAL);
            }
            return Err(crate::update::rejected(&response));
        }

        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());
        self.adopt_session(&response);
        self.rearm();
        Ok(())
    }
}
