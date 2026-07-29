//! The host configuration's vector set — §8 of
//! [`specs/host-config.md`](../../../../docs/specs/host-config.md).
//!
//! A vector is **data**: a document as text, and what must come of it. `HC-1` … `HC-30` are the
//! rows of the spec's table, in order, and each is tagged with the normative points it pins — which
//! is what makes "every normative point has a vector" a test rather than a claim. They run in
//! [`tests/config_vectors.rs`](../../../../crates/sipx-app/tests/config_vectors.rs).
//!
//! Two of them run the `A-7` harness rather than only comparing structs, because what they assert
//! is what happens to a **call**: `HC-9` shows a declared `on_5xx` ending one, and `HC-28` shows a
//! live call surviving the reload that redeclared it. A configuration knob nothing consults is not
//! a knob, and this is where that stops being possible to write.

use std::time::Duration;

use crate::harness::binding::{Outcome, Reply};
use crate::harness::policy::{Failure, FailurePolicy, OnFailure};
use crate::harness::{Conclusion, EndCause, EventKind, Scenario, Step};

use super::{Admission, AppBinding, Grants, HostConfig, Route, Running};

/// The reference document of §4.1: three listeners, three apps, one of each binding.
pub const REFERENCE: &str = r#"
[listener.pbx]
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
advertise = "sip.example.net:5060"
app       = "reception"

[listener.trunk]
protocol  = "sip"
transport = "tcp"
bind      = "0.0.0.0:5061"
no_app    = 503

[listener.apps]
protocol = "session"
bind     = "127.0.0.1:8088"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook-2026-07", "reception-hook-2026-04"]

[app.reception.on_failure]
timeout_ms     = 2000
on_timeout     = "continue"
on_5xx         = "continue"
on_unreachable = "continue"
on_4xx         = { reject = 500 }

[app.reception.grants]
play_roots   = ["/var/lib/sipx-app/audio/reception"]
dial_headers = ["x-campaign"]
originate    = false

[app.notifier]
binding       = "session"
bearer_secret = "notifier-bearer"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
"#;

/// [`REFERENCE`] with one word changed: what a 5xx does to a live call.
pub const REFERENCE_ON_5XX_HANGUP: &str = r#"
[listener.pbx]
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
advertise = "sip.example.net:5060"
app       = "reception"

[listener.trunk]
protocol  = "sip"
transport = "tcp"
bind      = "0.0.0.0:5061"
no_app    = 503

[listener.apps]
protocol = "session"
bind     = "127.0.0.1:8088"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook-2026-07", "reception-hook-2026-04"]

[app.reception.on_failure]
on_5xx = "hangup"

[app.reception.grants]
play_roots   = ["/var/lib/sipx-app/audio/reception"]
dial_headers = ["x-campaign"]
originate    = false

[app.notifier]
binding       = "session"
bearer_secret = "notifier-bearer"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
"#;

/// One host, one app — the shape a multi-process future would deploy (§7).
pub const ONE_APP: &str = r#"
[listener.pbx]
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
app       = "reception"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook-2026-07"]

[app.reception.grants]
dial_headers = ["x-campaign"]
"#;

/// One host, three apps — and `reception` means exactly what it means in [`ONE_APP`].
pub const MANY_APPS: &str = r#"
[listener.pbx]
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
app       = "reception"

[listener.overflow]
protocol  = "sip"
transport = "tcp"
bind      = "0.0.0.0:5061"
app       = "afterhours"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook-2026-07"]

[app.reception.grants]
dial_headers = ["x-campaign"]

[app.afterhours]
binding         = "webhook"
url             = "https://apps.example.net/afterhours"
signing_secrets = ["afterhours-hook"]

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
"#;

/// A minimal, valid document the rejection vectors vary one line of.
const MINIMAL: &str = r#"
[listener.pbx]
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
app       = "reception"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook"]
"#;

/// What a call arriving on a listener must be met with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitExpect {
    /// Admitted to this app.
    App(&'static str),
    /// Refused with this status.
    Refuse(u16),
    /// No such listener.
    NoSuchListener,
    /// A session listener; calls do not arrive there.
    NotACallListener,
}

/// What an app's binding must be — stated as the names a document writes rather than built out of
/// the loader's own types, so a vector says what the *document* means and cannot accidentally
/// assert what the reader happened to produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingExpect {
    /// Document mode.
    Webhook {
        /// Where events are `POST`ed.
        url: &'static str,
        /// The signing keys, by name, the head first.
        signing_secrets: &'static [&'static str],
    },
    /// Session mode.
    Session {
        /// The bearer's name.
        bearer_secret: &'static str,
    },
    /// The embedded runtime.
    Embedded {
        /// The handler file.
        handler: &'static str,
    },
}

impl BindingExpect {
    /// Whether a loaded binding is this one.
    fn holds(&self, got: &AppBinding) -> bool {
        match (self, got) {
            (
                Self::Webhook {
                    url,
                    signing_secrets,
                },
                AppBinding::Webhook {
                    url: got_url,
                    signing_secrets: got_secrets,
                },
            ) => {
                got_url == url
                    && got_secrets.len() == signing_secrets.len()
                    && got_secrets
                        .iter()
                        .zip(signing_secrets.iter())
                        .all(|(got, want)| got.as_str() == *want)
            }
            (Self::Session { bearer_secret }, AppBinding::Session { bearer_secret: got }) => {
                got.as_str() == *bearer_secret
            }
            (Self::Embedded { handler }, AppBinding::Embedded { handler: got }) => got == handler,
            _ => false,
        }
    }
}

/// What a document must load as. Every field is optional — a vector asserts what it is about.
#[derive(Debug, Default)]
pub struct Expect {
    listeners: Option<Vec<&'static str>>,
    apps: Option<Vec<&'static str>>,
    routes: Option<Vec<(&'static str, Route)>>,
    policies: Option<Vec<(&'static str, FailurePolicy)>>,
    grants: Option<Vec<(&'static str, Grants)>>,
    bindings: Option<Vec<(&'static str, BindingExpect)>>,
    secrets: Option<Vec<&'static str>>,
    admits: Option<Vec<(&'static str, AdmitExpect)>>,
    behaviour: Option<(&'static str, Failure, Conclusion)>,
}

impl Expect {
    /// Expecting nothing in particular.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Exactly these listeners, in name order.
    #[must_use]
    pub fn listeners(mut self, names: Vec<&'static str>) -> Self {
        self.listeners = Some(names);
        self
    }

    /// Exactly these apps, in name order.
    #[must_use]
    pub fn apps(mut self, names: Vec<&'static str>) -> Self {
        self.apps = Some(names);
        self
    }

    /// This listener routes here.
    #[must_use]
    pub fn route(mut self, listener: &'static str, route: Route) -> Self {
        self.routes.get_or_insert_default().push((listener, route));
        self
    }

    /// This app declares this policy.
    #[must_use]
    pub fn policy(mut self, app: &'static str, policy: FailurePolicy) -> Self {
        self.policies.get_or_insert_default().push((app, policy));
        self
    }

    /// This app is granted exactly this.
    #[must_use]
    pub fn grants(mut self, app: &'static str, grants: Grants) -> Self {
        self.grants.get_or_insert_default().push((app, grants));
        self
    }

    /// This app binds like this.
    #[must_use]
    pub fn binding(mut self, app: &'static str, binding: BindingExpect) -> Self {
        self.bindings.get_or_insert_default().push((app, binding));
        self
    }

    /// The document references exactly these secrets, by name.
    #[must_use]
    pub fn secrets(mut self, names: Vec<&'static str>) -> Self {
        self.secrets = Some(names);
        self
    }

    /// A call on this listener is met with this.
    #[must_use]
    pub fn admits(mut self, listener: &'static str, outcome: AdmitExpect) -> Self {
        self.admits
            .get_or_insert_default()
            .push((listener, outcome));
        self
    }

    /// And under the harness, this app's policy does this to a call that meets this failure.
    #[must_use]
    pub fn behaviour(mut self, app: &'static str, failure: Failure, ends: Conclusion) -> Self {
        self.behaviour = Some((app, failure, ends));
        self
    }

    fn check(&self, name: &str, config: &HostConfig) -> Result<(), String> {
        self.check_shape(name, config)?;
        self.check_apps(name, config)?;
        if let Some(expected) = &self.secrets
            && &config.secret_names() != expected
        {
            return Err(format!(
                "{name}: secrets expected {expected:?}, got {:?}",
                config.secret_names()
            ));
        }
        for (listener, outcome) in self.admits.iter().flatten() {
            let mut running = Running::start(config.clone());
            let got = running.admit("vector-call", listener);
            let holds = match (outcome, &got) {
                (AdmitExpect::App(want), Admission::App(policy)) => &policy.app == want,
                (AdmitExpect::Refuse(want), Admission::Refuse(status)) => status == want,
                (AdmitExpect::NoSuchListener, Admission::NoSuchListener)
                | (AdmitExpect::NotACallListener, Admission::NotACallListener) => true,
                _ => false,
            };
            if !holds {
                return Err(format!(
                    "{name}: a call on `{listener}` expected {outcome:?}, got {got:?}"
                ));
            }
        }
        if let Some((app, failure, ends)) = &self.behaviour {
            let policy = config
                .app(app)
                .ok_or_else(|| format!("{name}: no app `{app}` to run"))?
                .failure
                .clone();
            let got = under_harness(policy, *failure);
            if &got != ends {
                return Err(format!(
                    "{name}: a {} under `{app}`'s declaration expected {ends:?}, got {got:?}",
                    failure.knob()
                ));
            }
        }
        Ok(())
    }

    fn check_shape(&self, name: &str, config: &HostConfig) -> Result<(), String> {
        if let Some(expected) = &self.listeners {
            let got: Vec<&str> = config.listeners().map(|l| l.name.as_str()).collect();
            if &got != expected {
                return Err(format!(
                    "{name}: listeners expected {expected:?}, got {got:?}"
                ));
            }
        }
        if let Some(expected) = &self.apps {
            let got: Vec<&str> = config.apps().map(|a| a.name.as_str()).collect();
            if &got != expected {
                return Err(format!("{name}: apps expected {expected:?}, got {got:?}"));
            }
        }
        for (listener, route) in self.routes.iter().flatten() {
            let got = config
                .listener(listener)
                .ok_or_else(|| format!("{name}: no listener `{listener}`"))?;
            if &got.route != route {
                return Err(format!(
                    "{name}: `{listener}` routes to {:?}, expected {route:?}",
                    got.route
                ));
            }
        }
        Ok(())
    }

    fn check_apps(&self, name: &str, config: &HostConfig) -> Result<(), String> {
        for (app, policy) in self.policies.iter().flatten() {
            let got = config
                .app(app)
                .ok_or_else(|| format!("{name}: no app `{app}`"))?;
            if &got.failure != policy {
                return Err(format!(
                    "{name}: `{app}` declares {:?}, expected {policy:?}",
                    got.failure
                ));
            }
        }
        for (app, grants) in self.grants.iter().flatten() {
            let got = config
                .app(app)
                .ok_or_else(|| format!("{name}: no app `{app}`"))?;
            if &got.grants != grants {
                return Err(format!(
                    "{name}: `{app}` is granted {:?}, expected {grants:?}",
                    got.grants
                ));
            }
        }
        for (app, binding) in self.bindings.iter().flatten() {
            let got = config
                .app(app)
                .ok_or_else(|| format!("{name}: no app `{app}`"))?;
            if !binding.holds(&got.binding) {
                return Err(format!(
                    "{name}: `{app}` binds as {:?}, expected {binding:?}",
                    got.binding
                ));
            }
        }
        Ok(())
    }
}

/// What a reload must do.
#[derive(Debug)]
pub enum ReloadExpect {
    /// Applied; this app now declares this.
    Accepted {
        /// The app to look at afterwards.
        app: &'static str,
        /// What it must declare.
        failure: FailurePolicy,
    },
    /// Refused with this code — and the configuration in force is unchanged.
    Refused {
        /// The refusal's code.
        code: &'static str,
    },
}

/// What a vector does.
#[derive(Debug)]
pub enum Case {
    /// A document that must load, and what it must load as.
    Accepts {
        /// The document.
        doc: &'static str,
        /// What it means.
        expect: Expect,
    },
    /// A document that must be refused, with this code.
    Rejects {
        /// The document.
        doc: &'static str,
        /// The refusal's code.
        code: &'static str,
    },
    /// One document in force, then another offered.
    Reload {
        /// What is running.
        from: &'static str,
        /// What is offered.
        to: &'static str,
        /// What must come of it.
        expect: ReloadExpect,
    },
    /// A live call across an accepted reload (N11), observed as what a 5xx does to it.
    Retention {
        /// What is running when the call is admitted.
        from: &'static str,
        /// What replaces it while the call is live.
        to: &'static str,
        /// The listener the call arrives on.
        listener: &'static str,
        /// What a 5xx does to the call that was already live.
        live: Conclusion,
        /// What it does to one admitted after the reload.
        fresh: Conclusion,
    },
    /// Two documents that must agree about an app they share (N12).
    SameMeaning {
        /// The one-app document.
        one: &'static str,
        /// The many-app document.
        many: &'static str,
        /// The app both declare.
        app: &'static str,
        /// The listener both declare.
        listener: &'static str,
    },
}

/// One row of the spec's §8 table.
#[derive(Debug)]
pub struct Vector {
    /// `HC-n`.
    pub name: &'static str,
    /// What it is about, in one line.
    pub about: &'static str,
    /// The normative points it pins.
    pub points: &'static [&'static str],
    /// What it does.
    pub case: Case,
}

impl Vector {
    /// The refusal code this vector expects, if it expects one.
    ///
    /// Used to hold §5's closed set of codes against the vectors: a code no vector produces is a
    /// refusal nobody has ever seen happen.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match &self.case {
            Case::Rejects { code, .. }
            | Case::Reload {
                expect: ReloadExpect::Refused { code },
                ..
            } => Some(*code),
            _ => None,
        }
    }

    /// Run it.
    ///
    /// # Errors
    /// Whatever disagreed, naming the vector.
    pub fn check(&self) -> Result<(), String> {
        let name = self.name;
        match &self.case {
            Case::Accepts { doc, expect } => {
                let config = HostConfig::parse(doc)
                    .map_err(|error| format!("{name}: expected acceptance, got {error}"))?;
                expect.check(name, &config)
            }
            Case::Rejects { doc, code } => match HostConfig::parse(doc) {
                Ok(_) => Err(format!(
                    "{name}: expected {code}, the document was accepted"
                )),
                Err(error) if error.code() == *code => Ok(()),
                Err(error) => Err(format!("{name}: expected {code}, got {error}")),
            },
            Case::Reload { from, to, expect } => check_reload(name, from, to, expect),
            Case::Retention {
                from,
                to,
                listener,
                live,
                fresh,
            } => check_retention(name, from, to, listener, live, fresh),
            Case::SameMeaning {
                one,
                many,
                app,
                listener,
            } => check_same_meaning(name, one, many, app, listener),
        }
    }
}

/// A reload, and the promise that a refused one changes nothing.
fn check_reload(name: &str, from: &str, to: &str, expect: &ReloadExpect) -> Result<(), String> {
    let before = HostConfig::parse(from).map_err(|error| format!("{name}: {error}"))?;
    let mut running = Running::start(before.clone());
    let outcome = running.reload(to);

    match (expect, outcome) {
        (ReloadExpect::Accepted { app, failure }, Ok(())) => {
            let got = running
                .current()
                .app(app)
                .ok_or_else(|| format!("{name}: no app `{app}` after the reload"))?;
            if &got.failure == failure {
                Ok(())
            } else {
                Err(format!(
                    "{name}: after the reload `{app}` declares {:?}, expected {failure:?}",
                    got.failure
                ))
            }
        }
        (ReloadExpect::Accepted { .. }, Err(error)) => {
            Err(format!("{name}: expected the reload to apply, got {error}"))
        }
        (ReloadExpect::Refused { code }, Err(error)) if error.code() == *code => {
            if running.current() == &before {
                Ok(())
            } else {
                Err(format!(
                    "{name}: a refused reload changed the configuration"
                ))
            }
        }
        (ReloadExpect::Refused { code }, Err(error)) => {
            Err(format!("{name}: expected {code}, got {error}"))
        }
        (ReloadExpect::Refused { code }, Ok(())) => {
            Err(format!("{name}: expected {code}, the reload applied"))
        }
    }
}

/// A live call across a reload, observed as what a 5xx does to it.
fn check_retention(
    name: &str,
    from: &str,
    to: &str,
    listener: &str,
    live: &Conclusion,
    fresh: &Conclusion,
) -> Result<(), String> {
    let mut running =
        Running::start(HostConfig::parse(from).map_err(|error| format!("{name}: {error}"))?);
    let Admission::App(_) = running.admit("live", listener) else {
        return Err(format!("{name}: `{listener}` did not admit a call"));
    };
    running
        .reload(to)
        .map_err(|error| format!("{name}: the reload was refused: {error}"))?;

    let held = running
        .policy_of("live")
        .ok_or_else(|| format!("{name}: the reload ended a live call"))?
        .failure
        .clone();
    let got = under_harness(held, Failure::ServerError);
    if &got != live {
        return Err(format!(
            "{name}: the live call expected {live:?}, got {got:?}"
        ));
    }

    let Admission::App(after) = running.admit("fresh", listener) else {
        return Err(format!("{name}: `{listener}` stopped admitting calls"));
    };
    let got = under_harness(after.failure.clone(), Failure::ServerError);
    if &got != fresh {
        return Err(format!(
            "{name}: a call admitted after the reload expected {fresh:?}, got {got:?}"
        ));
    }
    Ok(())
}

/// One app, two documents, one meaning.
fn check_same_meaning(
    name: &str,
    one: &str,
    many: &str,
    app: &str,
    listener: &str,
) -> Result<(), String> {
    let one = HostConfig::parse(one).map_err(|error| format!("{name}: {error}"))?;
    let many = HostConfig::parse(many).map_err(|error| format!("{name}: {error}"))?;

    if one.apps().count() != 1 {
        return Err(format!(
            "{name}: the one-app document declares more than one"
        ));
    }
    if many.apps().count() < 2 {
        return Err(format!(
            "{name}: the many-app document declares fewer than two"
        ));
    }
    if one.app(app) != many.app(app) {
        return Err(format!("{name}: `{app}` means different things in the two"));
    }
    if one.listener(listener) != many.listener(listener) {
        return Err(format!(
            "{name}: `{listener}` means different things in the two"
        ));
    }
    Ok(())
}

/// What a declared policy does to a call that meets a given failure, under the `A-7` harness.
fn under_harness(policy: FailurePolicy, failure: Failure) -> Conclusion {
    let reply = match failure {
        Failure::Timeout => Reply::silent(),
        Failure::ServerError => {
            Reply::failing(Duration::ZERO, Outcome::ServerError { status: 500 })
        }
        Failure::Unreachable => Reply::unreachable(),
        Failure::ClientError => {
            Reply::failing(Duration::ZERO, Outcome::ClientError { status: 400 })
        }
    };

    Scenario::new("a knob read from a document")
        .policy(policy)
        .then(reply)
        .steps(vec![Step::event(0, EventKind::Incoming)])
        .until(10_000)
        .run()
        .conclusion
}

/// The `[app.<name>.grants]` of the reference document.
fn reference_grants() -> Grants {
    Grants {
        play_roots: vec!["/var/lib/sipx-app/audio/reception".to_owned()],
        dial_headers: vec!["x-campaign".to_owned()],
        originate: false,
    }
}

/// Every vector, in the spec's order.
#[must_use]
pub fn all() -> Vec<Vector> {
    let mut vectors = vec![reference()];
    vectors.extend(syntax_vectors());
    vectors.extend(failure_vectors());
    vectors.extend(grant_vectors());
    vectors.extend(routing_vectors());
    vectors.extend(secret_vectors());
    vectors.extend(binding_vectors());
    vectors.extend(reload_vectors());
    vectors.extend(open_stance_vectors());
    vectors
}

/// HC-1 — the reference document, read whole.
fn reference() -> Vector {
    Vector {
        name: "HC-1",
        about: "the reference document of §4.1 loads exactly as it reads",
        points: &["N4", "N5", "N6", "N7"],
        case: Case::Accepts {
            doc: REFERENCE,
            expect: Expect::new()
                .listeners(vec!["apps", "pbx", "trunk"])
                .apps(vec!["greeter", "notifier", "reception"])
                .route("pbx", Route::App("reception".to_owned()))
                .route("trunk", Route::Refuse(503))
                .route("apps", Route::Sessions)
                .policy("reception", FailurePolicy::declared())
                .grants("reception", reference_grants())
                .grants("notifier", Grants::denied())
                .binding(
                    "reception",
                    BindingExpect::Webhook {
                        url: "https://apps.example.net/reception",
                        signing_secrets: &["reception-hook-2026-07", "reception-hook-2026-04"],
                    },
                )
                .binding(
                    "notifier",
                    BindingExpect::Session {
                        bearer_secret: "notifier-bearer",
                    },
                )
                .binding(
                    "greeter",
                    BindingExpect::Embedded {
                        handler: "greeter.ts",
                    },
                )
                .secrets(vec![
                    "notifier-bearer",
                    "reception-hook-2026-04",
                    "reception-hook-2026-07",
                ]),
        },
    }
}

/// HC-2 … HC-7 — the syntax, and what it refuses to guess at.
fn syntax_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-2",
            about: "a basic string may not span lines",
            points: &["N1"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net\n\"\n",
                code: "syntax",
            },
        },
        Vector {
            name: "HC-3",
            about: "a key before any table header belongs to nothing",
            points: &["N1"],
            case: Case::Rejects {
                doc: "binding = \"webhook\"\n\n[app.reception]\n",
                code: "syntax",
            },
        },
        Vector {
            name: "HC-4",
            about: "a misspelled knob is refused rather than silently taking its default",
            points: &["N2"],
            case: Case::Rejects {
                doc: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\napp = \"reception\"\n\n[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.on_failure]\non_5xxx = \"hangup\"\n",
                code: "unknown-key",
            },
        },
        Vector {
            name: "HC-5",
            about: "a table this schema does not define is refused",
            points: &["N2"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.retry]\nattempts = 3\n",
                code: "unknown-table",
            },
        },
        Vector {
            name: "HC-6",
            about: "a key set twice is a merge, and a merge is not a declaration",
            points: &["N3"],
            case: Case::Rejects {
                doc: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\nbind = \"0.0.0.0:5061\"\napp = \"reception\"\n",
                code: "duplicate-key",
            },
        },
        Vector {
            name: "HC-7",
            about: "a table opened twice is refused",
            points: &["N3"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\n\n[app.reception]\nurl = \"https://apps.example.net\"\n",
                code: "duplicate-table",
            },
        },
    ]
}

/// HC-8 … HC-11 — §9.2, and only §9.2.
fn failure_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-8",
            about: "an app that declares nothing gets the contract's five defaults",
            points: &["N4"],
            case: Case::Accepts {
                doc: MINIMAL,
                expect: Expect::new()
                    .policy("reception", FailurePolicy::declared())
                    .behaviour("reception", Failure::ServerError, Conclusion::Live),
            },
        },
        Vector {
            name: "HC-9",
            about: "a declared on_5xx ends the call it is declared for",
            points: &["N4"],
            case: Case::Accepts {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.on_failure]\ntimeout_ms = 500\non_5xx = \"hangup\"\non_4xx = { reject = 603 }\n",
                expect: Expect::new()
                    .policy(
                        "reception",
                        FailurePolicy::declared()
                            .with_timeout(Duration::from_millis(500))
                            .on_5xx(OnFailure::Hangup)
                            .on_4xx(OnFailure::Reject { status: 603 }),
                    )
                    .behaviour(
                        "reception",
                        Failure::ServerError,
                        Conclusion::Ended(EndCause::Hangup),
                    ),
            },
        },
        Vector {
            name: "HC-10",
            about: "a refusal without a status is not an action",
            points: &["N4"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.on_failure]\non_4xx = \"reject\"\n",
                code: "bad-value",
            },
        },
        Vector {
            name: "HC-11",
            about: "a refusal carries a SIP status, not a number",
            points: &["N4"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.on_failure]\non_5xx = { reject = 1000 }\n",
                code: "bad-value",
            },
        },
    ]
}

/// HC-12, HC-13 — deny-by-default, and what lifting it looks like.
fn grant_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-12",
            about: "an app with no grants table is granted nothing",
            points: &["N5"],
            case: Case::Accepts {
                doc: MINIMAL,
                expect: Expect::new().grants("reception", Grants::denied()),
            },
        },
        Vector {
            name: "HC-13",
            about: "the three grants, declared",
            points: &["N5"],
            case: Case::Accepts {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n\n[app.reception.grants]\nplay_roots = [\n  \"/srv/audio/one\",\n  \"/srv/audio/two\",\n]\ndial_headers = [\"x-campaign\"]\noriginate = true\n",
                expect: Expect::new().grants(
                    "reception",
                    Grants {
                        play_roots: vec!["/srv/audio/one".to_owned(), "/srv/audio/two".to_owned()],
                        dial_headers: vec!["x-campaign".to_owned()],
                        originate: true,
                    },
                ),
            },
        },
    ]
}

/// HC-14 … HC-17, HC-30 — routing, which is total or the document is refused.
fn routing_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-14",
            about: "a listener that routes nowhere would meet a call with silence",
            points: &["N6"],
            case: Case::Rejects {
                doc: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\n",
                code: "unrouted-listener",
            },
        },
        Vector {
            name: "HC-15",
            about: "a refusal that can never apply is a declaration nobody should rely on",
            points: &["N6"],
            case: Case::Rejects {
                doc: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\napp = \"reception\"\nno_app = 503\n\n[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n",
                code: "unreachable-no-app",
            },
        },
        Vector {
            name: "HC-16",
            about: "a route to an app that is not declared is found at load, not by a call",
            points: &["N6"],
            case: Case::Rejects {
                doc: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\napp = \"typo\"\n\n[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n",
                code: "unknown-app",
            },
        },
        Vector {
            name: "HC-17",
            about: "each listener's answer to an arriving call, decided before it arrives",
            points: &["N6"],
            case: Case::Accepts {
                doc: REFERENCE,
                expect: Expect::new()
                    .admits("pbx", AdmitExpect::App("reception"))
                    .admits("trunk", AdmitExpect::Refuse(503))
                    .admits("apps", AdmitExpect::NotACallListener)
                    .admits("nowhere", AdmitExpect::NoSuchListener),
            },
        },
        Vector {
            name: "HC-30",
            about: "an app cannot be routed to a session listener; it names itself there",
            points: &["N2"],
            case: Case::Rejects {
                doc: "[listener.apps]\nprotocol = \"session\"\nbind = \"127.0.0.1:8088\"\napp = \"reception\"\n\n[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n",
                code: "key-not-allowed",
            },
        },
    ]
}

/// HC-18, HC-19 — the document is committable, and the grammar is why.
fn secret_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-18",
            about: "every secret in the reference document is a name",
            points: &["N7"],
            case: Case::Accepts {
                doc: REFERENCE,
                expect: Expect::new().secrets(vec![
                    "notifier-bearer",
                    "reception-hook-2026-04",
                    "reception-hook-2026-07",
                ]),
            },
        },
        Vector {
            name: "HC-19",
            about: "a pasted key does not fit through the name grammar",
            points: &["N7"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"aGVsbG8sIHRoaXMgaXMgbm90IGEgbmFtZQ==\"]\n",
                code: "not-a-secret-name",
            },
        },
    ]
}

/// HC-20 … HC-23 — a binding that cannot be used is a document error.
fn binding_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-20",
            about: "a webhook app must name the key its requests are signed with",
            points: &["N8"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\n",
                code: "missing-key",
            },
        },
        Vector {
            name: "HC-21",
            about: "a session app must name the bearer it presents at upgrade",
            points: &["N8"],
            case: Case::Rejects {
                doc: "[app.notifier]\nbinding = \"session\"\n",
                code: "missing-key",
            },
        },
        Vector {
            name: "HC-22",
            about: "an embedded app must name its handler",
            points: &["N8"],
            case: Case::Rejects {
                doc: "[app.greeter]\nbinding = \"embedded\"\n",
                code: "missing-key",
            },
        },
        Vector {
            name: "HC-23",
            about: "there are three bindings, and no fourth",
            points: &["N8"],
            case: Case::Rejects {
                doc: "[app.reception]\nbinding = \"carrier-pigeon\"\n",
                code: "bad-value",
            },
        },
    ]
}

/// HC-24 … HC-28 — reload, and what it may and may not touch.
fn reload_vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "HC-24",
            about: "an accepted reload applies wholly; the next call gets the new declaration",
            points: &["N9"],
            case: Case::Reload {
                from: REFERENCE,
                to: REFERENCE_ON_5XX_HANGUP,
                expect: ReloadExpect::Accepted {
                    app: "reception",
                    failure: FailurePolicy::declared().on_5xx(OnFailure::Hangup),
                },
            },
        },
        Vector {
            name: "HC-25",
            about: "a reload of a document that does not parse changes nothing",
            points: &["N9"],
            case: Case::Reload {
                from: REFERENCE,
                to: "[listener.pbx]\nbind = 0.0.0.0:5060\n",
                expect: ReloadExpect::Refused { code: "syntax" },
            },
        },
        Vector {
            name: "HC-26",
            about: "rebinding a listener is a restart, because a restart cannot half-happen",
            points: &["N10"],
            case: Case::Reload {
                from: MINIMAL,
                to: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5070\"\napp = \"reception\"\n\n[app.reception]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/reception\"\nsigning_secrets = [\"reception-hook\"]\n",
                expect: ReloadExpect::Refused {
                    code: "topology-changed",
                },
            },
        },
        Vector {
            name: "HC-27",
            about: "which app a listener routes to is policy, not topology",
            points: &["N10"],
            case: Case::Reload {
                from: MINIMAL,
                to: "[listener.pbx]\nprotocol = \"sip\"\ntransport = \"udp\"\nbind = \"0.0.0.0:5060\"\napp = \"afterhours\"\n\n[app.afterhours]\nbinding = \"webhook\"\nurl = \"https://apps.example.net/afterhours\"\nsigning_secrets = [\"afterhours-hook\"]\n\n[app.afterhours.on_failure]\non_5xx = \"hangup\"\n",
                expect: ReloadExpect::Accepted {
                    app: "afterhours",
                    failure: FailurePolicy::declared().on_5xx(OnFailure::Hangup),
                },
            },
        },
        Vector {
            name: "HC-28",
            about: "a live call keeps what it was admitted with; the next one does not",
            points: &["N11"],
            case: Case::Retention {
                from: REFERENCE,
                to: REFERENCE_ON_5XX_HANGUP,
                listener: "pbx",
                live: Conclusion::Live,
                fresh: Conclusion::Ended(EndCause::Hangup),
            },
        },
    ]
}

/// HC-29 — the stance §7 leaves open, as something a test can hold.
fn open_stance_vectors() -> Vec<Vector> {
    vec![Vector {
        name: "HC-29",
        about: "one app per document and many apps per document mean the same thing",
        points: &["N12"],
        case: Case::SameMeaning {
            one: ONE_APP,
            many: MANY_APPS,
            app: "reception",
            listener: "pbx",
        },
    }]
}
