//! Registration, digest authentication and the refresh the kernel owns (§5.2, §5.3, §8.6).
//!
//! The kernel never auto-retries a failed registration beyond the refresh it owes: retry policy
//! belongs to the application, bounded, per `T-33`'s no-unbounded-retry rule. A `"failed"` state
//! is terminal until the page asks again.

use std::time::Duration;

use sipx_sip::auth::{Challenge, Credentials, respond, strongest};
use sipx_sip::{HeaderName, Method, Request, Response, TransactionKey};

use super::{Kernel, Scheduled};
use crate::error::{Error, Result};
use crate::event::{Event, RegistrationState};
use crate::sip::{self, Dialog};

/// The fraction of the granted lifetime after which the kernel refreshes.
///
/// Nine tenths rather than the whole lifetime: a refresh that leaves on the expiry instant
/// arrives after it, and a registration that lapses between the request and the response is
/// indistinguishable to the far end from a client that went away.
const REFRESH_NUMERATOR: u64 = 9;
const REFRESH_DENOMINATOR: u64 = 10;

/// Registration state for the kernel's one AOR.
#[derive(Debug, Default)]
pub(crate) struct Registration {
    state: Option<RegistrationState>,
    dialog: Option<Dialog>,
    /// The lifetime the page asked for, and after a 2xx the lifetime the registrar granted.
    expires: u32,
    /// The in-flight REGISTER's transaction, its branch and the command awaiting its outcome.
    in_flight: Option<TransactionKey>,
    command: Option<u64>,
    /// Whether this exchange has already been retried with an `Authorization` header. One retry
    /// is RFC 3261 §22.2's exchange; a second would be a credential-guessing loop.
    challenged: bool,
    /// Whether the in-flight REGISTER is a deregistration.
    unregistering: bool,
}

impl Registration {
    /// The state §5.3 reports, defaulting to `"unregistered"` before anything has happened.
    pub(crate) fn state(&self) -> RegistrationState {
        self.state.unwrap_or(RegistrationState::Unregistered)
    }

    /// Whether a REGISTER is on the wire.
    fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Whether `key` is this registration's transaction.
    pub(crate) fn owns(&self, key: &TransactionKey) -> bool {
        self.in_flight.as_ref() == Some(key)
    }
}

impl Kernel {
    /// §5.2 `"register"`.
    pub(super) fn register(&mut self, id: u64, expires: u32) -> Result<()> {
        if self.registration.busy() {
            return Err(Error::State);
        }
        self.registration.expires = expires;
        self.registration.challenged = false;
        self.registration.unregistering = false;
        self.registration.command = Some(id);
        self.send_register(expires, None)
    }

    /// §5.2 `"unregister"`.
    pub(super) fn unregister(&mut self, id: u64) -> Result<()> {
        if self.registration.busy() {
            return Err(Error::State);
        }
        if self.registration.dialog.is_none() {
            return Err(Error::State);
        }
        // Stop refreshing first: the refresh the kernel owes is exactly what a deregistration
        // cancels, and a timer that survives it would re-register the AOR the page just dropped.
        self.clear_timer(&Scheduled::RegistrationRefresh);
        self.registration.challenged = false;
        self.registration.unregistering = true;
        self.registration.command = Some(id);
        self.send_register(0, None)
    }

    /// Build and send one REGISTER, optionally carrying an `Authorization` header.
    fn send_register(
        &mut self,
        expires: u32,
        authorization: Option<(HeaderName, String)>,
    ) -> Result<()> {
        // §4.7's consumption order: a new registration consumes Call-ID then From-tag, and the
        // client transaction consumes its branch at serialisation. A retry reuses the dialog and
        // draws only the branch — plus the cnonce, which the authorization header took before
        // this point.
        if self.registration.dialog.is_none() {
            let call_id = self.entropy.call_id()?;
            let local_tag = self.entropy.tag()?;
            self.registration.dialog = Some(Dialog {
                call_id,
                local_tag,
                remote_tag: None,
                local_uri: self.config.aor.clone(),
                remote_uri: self.config.aor.clone(),
                remote_target: None,
                local_cseq: 0,
            });
        }
        let branch = self.entropy.branch()?;

        let Some(dialog) = self.registration.dialog.as_mut() else {
            self.poison("the registration dialog vanished between two statements");
            return Ok(());
        };
        dialog.local_cseq = dialog.local_cseq.saturating_add(1);
        let cseq = dialog.local_cseq;
        let dialog = dialog.clone();

        let Ok(mut request) = sip::register(&self.config, &dialog, &branch, cseq, expires) else {
            self.poison("a REGISTER this kernel composed did not build");
            return Ok(());
        };
        if let Some((name, value)) = authorization
            && let Ok(header) = sipx_sip::message::Header::build(name, value)
        {
            request.headers.push(header);
        }

        self.dispatch_register(request);
        self.ask_for_entropy_if_low();
        Ok(())
    }

    /// Hand the REGISTER to the transaction layer and report `"registering"`.
    fn dispatch_register(&mut self, request: Request) {
        let Some((key, outputs)) = self.transactions.send_request(request, Self::reliability())
        else {
            self.poison("the transaction layer refused a REGISTER");
            return;
        };
        self.registration.in_flight = Some(key.clone());
        if !self.registration.unregistering {
            self.registration.state = Some(RegistrationState::Registering);
            self.emit(&Event::Registration {
                state: RegistrationState::Registering,
                expires: Some(u64::from(self.registration.expires)),
                status: None,
                reason: None,
            });
        }
        self.drive(&key, outputs);
    }

    /// A response to the in-flight REGISTER.
    pub(super) fn on_registration_response(&mut self, response: &Response) {
        let status = response.status.code();
        if response.status.is_provisional() {
            return;
        }
        self.registration.in_flight = None;

        match status {
            401 | 407 => self.on_challenge(response),
            _ if response.status.is_success() => self.on_registration_success(response),
            _ => {
                let reason = String::from_utf8_lossy(&response.reason).into_owned();
                self.fail_registration(u64::from(status), reason);
            }
        }
    }

    /// A 401 or 407: answer it once, with a cnonce drawn from the tape.
    fn on_challenge(&mut self, response: &Response) {
        if !self.config.may_authenticate() {
            // §8.6. The host declared the transport insecure; a digest response over it hands a
            // replayable credential to anyone on the path, so the kernel refuses rather than
            // "helpfully" authenticating.
            self.fail_registration(
                u64::from(response.status.code()),
                "credentials are not sent over a transport declared insecure",
            );
            return;
        }
        if self.registration.challenged {
            self.fail_registration(
                u64::from(response.status.code()),
                "the registrar challenged the authenticated request again",
            );
            return;
        }

        let mut challenges = Vec::new();
        for (name, from_proxy) in [
            (HeaderName::WwwAuthenticate, false),
            (HeaderName::ProxyAuthenticate, true),
        ] {
            for header in response.headers.get_all(&name) {
                if let Some(challenge) = Challenge::parse(&header.value(), from_proxy) {
                    challenges.push(challenge);
                }
            }
        }
        let Some(challenge) = strongest(challenges) else {
            self.fail_registration(
                u64::from(response.status.code()),
                "the challenge named no algorithm this kernel performs",
            );
            return;
        };

        // §4.7: the cnonce is consumed when the authorization header is built, which is before
        // the retry's branch is drawn at serialisation.
        let Ok(cnonce) = self.entropy.cnonce() else {
            self.fail_registration(
                u64::from(response.status.code()),
                "the entropy pool could not cover a digest cnonce",
            );
            self.ask_for_entropy_if_low();
            return;
        };

        let request_uri = format!("sip:{}", self.config.aor_domain());
        let credentials =
            Credentials::new(self.config.username.clone(), self.config.password.clone());
        let value = respond(
            &challenge,
            &credentials,
            Method::Register.to_string().as_str(),
            &request_uri,
            1,
            &cnonce,
        );
        let header = challenge.response_header();

        self.registration.challenged = true;
        let expires = if self.registration.unregistering {
            0
        } else {
            self.registration.expires
        };
        let _ = self.send_register(expires, Some((header, value)));
    }

    /// A 2xx to REGISTER or to the deregistration.
    fn on_registration_success(&mut self, response: &Response) {
        if self.registration.unregistering {
            self.registration.dialog = None;
            self.registration.state = Some(RegistrationState::Unregistered);
            self.emit(&Event::Registration {
                state: RegistrationState::Unregistered,
                expires: None,
                status: None,
                reason: None,
            });
            self.finish_registration_command(true);
            return;
        }

        let granted = granted_expiry(response).unwrap_or(self.registration.expires);
        self.registration.expires = granted;
        self.registration.state = Some(RegistrationState::Registered);

        // The refresh timer is set **before** the events, because `BSDK-STATE-1` pins that
        // order: the host holds the timer before it is told the registration is up.
        let after = Duration::from_millis(
            u64::from(granted)
                .saturating_mul(1000)
                .saturating_mul(REFRESH_NUMERATOR)
                / REFRESH_DENOMINATOR,
        );
        self.set_timer(Scheduled::RegistrationRefresh, after);

        self.emit(&Event::Registration {
            state: RegistrationState::Registered,
            expires: Some(u64::from(granted)),
            status: None,
            reason: None,
        });
        self.finish_registration_command(true);
    }

    /// Report a registration that will not be retried.
    fn fail_registration(&mut self, status: u64, reason: impl Into<String>) {
        let reason = reason.into();
        self.registration.state = Some(RegistrationState::Failed);
        self.emit(&Event::Registration {
            state: RegistrationState::Failed,
            expires: None,
            status: Some(status),
            reason: Some(reason.clone()),
        });
        if let Some(id) = self.registration.command.take() {
            self.refuse(id, "registration-failed", reason);
        }
    }

    /// Complete the command that asked for this registration.
    fn finish_registration_command(&mut self, ok: bool) {
        if let Some(id) = self.registration.command.take() {
            if ok {
                self.succeed(id);
            } else {
                self.refuse(
                    id,
                    "registration-failed",
                    "the registration did not complete",
                );
            }
        }
    }

    /// The refresh timer fired: send another REGISTER for the same lifetime.
    pub(super) fn refresh_registration(&mut self) {
        if self.registration.busy() || self.registration.dialog.is_none() {
            return;
        }
        self.registration.challenged = false;
        self.registration.unregistering = false;
        let expires = self.registration.expires;
        let _ = self.send_register(expires, None);
    }

    /// The in-flight REGISTER timed out or its transport failed.
    pub(super) fn on_registration_timeout(&mut self) {
        self.registration.in_flight = None;
        self.fail_registration(408, "no answer from the registrar");
    }
}

/// The lifetime the registrar granted: the `Expires` header, else the `Contact`'s parameter.
fn granted_expiry(response: &Response) -> Option<u32> {
    if let Some(Ok(expires)) = response.headers.typed::<sipx_sip::headers::Expires>() {
        return Some(expires.0);
    }
    let raw = response.headers.value(&HeaderName::Contact)?;
    let contacts =
        <sipx_sip::headers::ContactValue as sipx_sip::TypedHeader>::decode_list(&raw).ok()?;
    contacts.into_iter().find_map(|value| match value {
        sipx_sip::headers::ContactValue::Address(address) => address
            .param("expires")
            .and_then(|raw| core::str::from_utf8(raw).ok()?.parse().ok()),
        sipx_sip::headers::ContactValue::Wildcard => None,
    })
}
