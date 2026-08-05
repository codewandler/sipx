//! The schema — §4 of [`specs/host-config.md`](../../../../docs/specs/host-config.md).
//!
//! What a table may contain, what a value may be, and what has to be true of the document as a
//! whole. Everything here refuses rather than defaults: an unknown key, a known key in the wrong
//! table, a missing required key and a listener that routes nowhere are all document errors, found
//! at load rather than by the first call that runs into one.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use crate::harness::policy::{Failure, FailurePolicy, OnFailure};

use super::syntax::{self, Entry, Table, Value};
use super::{
    AppBinding, AppConfig, ConfigError, Grants, HostConfig, Listener, Protocol, Route, SecretRef,
    Transport, failure_keys,
};

/// Every key a `sip` listener may set, and every key a `session` listener may not.
const LISTENER_KEYS: [&str; 6] = [
    "protocol",
    "bind",
    "transport",
    "advertise",
    "app",
    "no_app",
];
/// Every key an `[app.<name>]` table may set, across all four bindings.
const APP_KEYS: [&str; 9] = [
    "binding",
    "url",
    "signing_secrets",
    "bearer_secret",
    "handler",
    "endpoint",
    "model",
    "instructions",
    "api_key_secret",
];
/// Every key a `[app.<name>.grants]` table may set.
const GRANT_KEYS: [&str; 3] = ["play_roots", "dial_headers", "originate"];

/// The tables belonging to one app.
#[derive(Default)]
struct AppTables<'a> {
    main: Option<&'a Table>,
    failure: Option<&'a Table>,
    grants: Option<&'a Table>,
}

/// Read a document, whole or not at all.
pub(super) fn load(text: &str) -> Result<HostConfig, ConfigError> {
    let document = syntax::parse(text)?;

    let mut listener_tables: Vec<&Table> = Vec::new();
    let mut app_tables: BTreeMap<&str, AppTables<'_>> = BTreeMap::new();

    for table in &document.tables {
        let parts: Vec<&str> = table.path.iter().map(String::as_str).collect();
        match parts.as_slice() {
            ["listener", _] => listener_tables.push(table),
            ["app", name] => app_tables.entry(name).or_default().main = Some(table),
            ["app", name, "on_failure"] => {
                app_tables.entry(name).or_default().failure = Some(table);
            }
            ["app", name, "grants"] => app_tables.entry(name).or_default().grants = Some(table),
            _ => return Err(ConfigError::unknown_table(table.line, &table.header())),
        }
    }

    let mut apps: BTreeMap<String, AppConfig> = BTreeMap::new();
    for (name, tables) in &app_tables {
        apps.insert((*name).to_owned(), app_of(name, tables)?);
    }

    let declared: BTreeSet<&str> = apps.keys().map(String::as_str).collect();
    let mut listeners: BTreeMap<String, Listener> = BTreeMap::new();
    for table in listener_tables {
        let listener = listener_of(table, &declared)?;
        listeners.insert(listener.name.clone(), listener);
    }

    Ok(HostConfig { listeners, apps })
}

/// One app, from its up-to-three tables.
fn app_of(name: &str, tables: &AppTables<'_>) -> Result<AppConfig, ConfigError> {
    let Some(main) = tables.main else {
        let orphan = tables.failure.or(tables.grants);
        let line = orphan.map_or(1, |table| table.line);
        let header = orphan.map_or_else(|| format!("app.{name}"), Table::header);
        return Err(ConfigError::unknown_table(line, &header));
    };

    Ok(AppConfig {
        name: name.to_owned(),
        binding: binding_of(main)?,
        failure: tables
            .failure
            .map_or_else(|| Ok(FailurePolicy::declared()), failure_of)?,
        grants: tables
            .grants
            .map_or_else(|| Ok(Grants::denied()), grants_of)?,
    })
}

/// `binding`, and whatever that binding requires named.
fn binding_of(table: &Table) -> Result<AppBinding, ConfigError> {
    let mut reader = Reader::new(table);
    let Some(entry) = reader.get("binding") else {
        return Err(ConfigError::missing_key(
            &format!("{}.binding", table.header()),
            "an app declares which transport of the contract it speaks",
        ));
    };

    let binding = match string(entry, &reader.path_of("binding"))? {
        "webhook" => AppBinding::Webhook {
            url: url_of(&mut reader)?,
            signing_secrets: signing_secrets_of(&mut reader)?,
        },
        "session" => AppBinding::Session {
            bearer_secret: required_secret(&mut reader, "bearer_secret")?,
        },
        "embedded" => AppBinding::Embedded {
            handler: required_path(&mut reader, "handler")?,
        },
        "realtime" => AppBinding::Realtime {
            endpoint: realtime_endpoint(&mut reader)?,
            model: required_realtime_text(&mut reader, "model")?,
            instructions: required_realtime_text(&mut reader, "instructions")?,
            api_key_secret: required_secret(&mut reader, "api_key_secret")?,
        },
        other => {
            return Err(ConfigError::bad_value(
                entry.line,
                &reader.path_of("binding"),
                &format!(
                    "`{other}` is not a binding; it is webhook, session, embedded or realtime"
                ),
            ));
        }
    };

    reader.finish(&APP_KEYS, "is not a key this binding takes")?;
    Ok(binding)
}

/// The base URL of a realtime session: absolute WebSocket, with the model query owned elsewhere.
fn realtime_endpoint(reader: &mut Reader<'_>) -> Result<String, ConfigError> {
    let path = reader.path_of("endpoint");
    let Some(entry) = reader.get("endpoint") else {
        return Err(ConfigError::missing_key(
            &path,
            "a realtime app declares the WebSocket endpoint it connects to",
        ));
    };
    let text = string(entry, &path)?;
    let authority = text
        .strip_prefix("wss://")
        .or_else(|| text.strip_prefix("ws://"))
        .map(|rest| rest.split('/').next().unwrap_or_default());
    let valid = authority.is_some_and(|authority| !authority.is_empty())
        && !text.contains('?')
        && !text.contains('#');
    if valid {
        Ok(text.to_owned())
    } else {
        Err(ConfigError::bad_value(
            entry.line,
            &path,
            "an absolute ws or wss URI without a query or fragment is required",
        ))
    }
}

/// One required, non-empty realtime session string.
fn required_realtime_text(
    reader: &mut Reader<'_>,
    key: &'static str,
) -> Result<String, ConfigError> {
    let path = reader.path_of(key);
    let Some(entry) = reader.get(key) else {
        return Err(ConfigError::missing_key(
            &path,
            "a realtime app declares every session value before a call arrives",
        ));
    };
    let text = string(entry, &path)?;
    if text.is_empty() {
        return Err(ConfigError::bad_value(
            entry.line,
            &path,
            "must not be empty",
        ));
    }
    Ok(text.to_owned())
}

/// The `url` of a webhook app.
fn url_of(reader: &mut Reader<'_>) -> Result<String, ConfigError> {
    let path = reader.path_of("url");
    let Some(entry) = reader.get("url") else {
        return Err(ConfigError::missing_key(
            &path,
            "a webhook app is where its events are POSTed",
        ));
    };
    let text = string(entry, &path)?;
    let authority = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .map(|rest| rest.split('/').next().unwrap_or_default());
    match authority {
        Some(authority) if !authority.is_empty() => Ok(text.to_owned()),
        _ => Err(ConfigError::bad_value(
            entry.line,
            &path,
            "an absolute http or https URI is required",
        )),
    }
}

/// The `signing_secrets` of a webhook app: one key, or two during a rotation.
fn signing_secrets_of(reader: &mut Reader<'_>) -> Result<Vec<SecretRef>, ConfigError> {
    let path = reader.path_of("signing_secrets");
    let Some(entry) = reader.get("signing_secrets") else {
        return Err(ConfigError::missing_key(
            &path,
            "every webhook request carries Sipx-Signature, so its key must be named",
        ));
    };
    let names = string_list(entry, &path)?;
    if names.is_empty() || names.len() > 2 {
        return Err(ConfigError::bad_value(
            entry.line,
            &path,
            "one key signs; a second is a rotation window. One or two, not more",
        ));
    }
    names
        .iter()
        .map(|name| {
            SecretRef::parse(name).ok_or_else(|| ConfigError::not_a_secret_name(entry.line, &path))
        })
        .collect()
}

/// A required secret name.
fn required_secret(reader: &mut Reader<'_>, key: &'static str) -> Result<SecretRef, ConfigError> {
    let path = reader.path_of(key);
    let Some(entry) = reader.get(key) else {
        return Err(ConfigError::missing_key(
            &path,
            "a session app authenticates at upgrade, so its bearer must be named",
        ));
    };
    let name = string(entry, &path)?;
    SecretRef::parse(name).ok_or_else(|| ConfigError::not_a_secret_name(entry.line, &path))
}

/// A required non-empty path.
fn required_path(reader: &mut Reader<'_>, key: &'static str) -> Result<String, ConfigError> {
    let path = reader.path_of(key);
    let Some(entry) = reader.get(key) else {
        return Err(ConfigError::missing_key(
            &path,
            "an embedded app is a handler file",
        ));
    };
    let text = string(entry, &path)?;
    if text.is_empty() {
        return Err(ConfigError::bad_value(
            entry.line,
            &path,
            "must not be empty",
        ));
    }
    Ok(text.to_owned())
}

/// `[app.<name>.on_failure]` — the contract's §9.2, and nothing else.
fn failure_of(table: &Table) -> Result<FailurePolicy, ConfigError> {
    let mut reader = Reader::new(table);
    let mut policy = FailurePolicy::declared();

    if let Some(entry) = reader.get("timeout_ms") {
        let path = reader.path_of("timeout_ms");
        let millis = integer(entry, &path)?;
        if !(1..=600_000).contains(&millis) {
            return Err(ConfigError::bad_value(
                entry.line,
                &path,
                "a callback budget is between 1 ms and 600000 ms",
            ));
        }
        policy = policy.with_timeout(Duration::from_millis(millis.unsigned_abs()));
    }

    for failure in Failure::all() {
        let key = failure.knob();
        let Some(entry) = reader.get(key) else {
            continue;
        };
        let action = action(entry, &reader.path_of(key))?;
        policy = match failure {
            Failure::Timeout => policy.on_timeout(action),
            Failure::ServerError => policy.on_5xx(action),
            Failure::Unreachable => policy.on_unreachable(action),
            Failure::ClientError => policy.on_4xx(action),
        };
    }

    reader.finish(
        &failure_keys(),
        "is not a knob the contract's §9.2 declares",
    )?;
    Ok(policy)
}

/// One of §9.2's three actions.
fn action(entry: &Entry, path: &str) -> Result<OnFailure, ConfigError> {
    match &entry.value {
        Value::String(text) if text == "continue" => Ok(OnFailure::Continue),
        Value::String(text) if text == "hangup" => Ok(OnFailure::Hangup),
        Value::Table(pairs) => match pairs.as_slice() {
            [(key, Value::Integer(status))] if key == "reject" => {
                let status = u16::try_from(*status)
                    .ok()
                    .filter(|s| (400..=699).contains(s));
                status
                    .map(|status| OnFailure::Reject { status })
                    .ok_or_else(|| {
                        ConfigError::bad_value(
                            entry.line,
                            path,
                            "a refusal carries a 4xx–6xx status",
                        )
                    })
            }
            _ => Err(ConfigError::bad_value(
                entry.line,
                path,
                "the only inline-table action is { reject = <status> }",
            )),
        },
        _ => Err(ConfigError::bad_value(
            entry.line,
            path,
            "an action is \"continue\", \"hangup\", or { reject = <status> }",
        )),
    }
}

/// `[app.<name>.grants]` — deny-by-default, so every key here only ever adds.
fn grants_of(table: &Table) -> Result<Grants, ConfigError> {
    let mut reader = Reader::new(table);
    let mut grants = Grants::denied();

    if let Some(entry) = reader.get("play_roots") {
        let path = reader.path_of("play_roots");
        grants.play_roots = non_empty_strings(entry, &path)?;
    }
    if let Some(entry) = reader.get("dial_headers") {
        let path = reader.path_of("dial_headers");
        grants.dial_headers = non_empty_strings(entry, &path)?;
    }
    if let Some(entry) = reader.get("originate") {
        grants.originate = boolean(entry, &reader.path_of("originate"))?;
    }

    reader.finish(&GRANT_KEYS, "is not a capability this host grants")?;
    Ok(grants)
}

/// One `[listener.<name>]`, routed (N6) and checked against the apps that exist.
fn listener_of(table: &Table, declared: &BTreeSet<&str>) -> Result<Listener, ConfigError> {
    let name = table.path.last().cloned().unwrap_or_default();
    let mut reader = Reader::new(table);

    let protocol = match reader.get("protocol") {
        Some(entry) => match string(entry, &reader.path_of("protocol"))? {
            "sip" => Protocol::Sip,
            "session" => Protocol::Session,
            other => {
                return Err(ConfigError::bad_value(
                    entry.line,
                    &reader.path_of("protocol"),
                    &format!("`{other}` is not a protocol; it is sip or session"),
                ));
            }
        },
        None => {
            return Err(ConfigError::missing_key(
                &reader.path_of("protocol"),
                "a listener declares what connects to it",
            ));
        }
    };

    let bind = bind_of(&mut reader)?;
    let (transport, advertise, route) = match protocol {
        Protocol::Sip => sip_listener(&mut reader, &name, declared)?,
        Protocol::Session => (None, None, Route::Sessions),
    };

    reader.finish(
        &LISTENER_KEYS,
        "may not be set on a session listener; an app arriving there names itself with its bearer",
    )?;

    Ok(Listener {
        name,
        protocol,
        bind,
        transport,
        advertise,
        route,
    })
}

/// The `bind` address every listener has.
fn bind_of(reader: &mut Reader<'_>) -> Result<SocketAddr, ConfigError> {
    let path = reader.path_of("bind");
    let Some(entry) = reader.get("bind") else {
        return Err(ConfigError::missing_key(
            &path,
            "a listener declares where it binds",
        ));
    };
    let text = string(entry, &path)?;
    SocketAddr::from_str(text).map_err(|_| {
        ConfigError::bad_value(
            entry.line,
            &path,
            "an <ip>:<port> socket address, IPv6 in brackets",
        )
    })
}

/// The keys only a `sip` listener has, and the routing decision they make total.
type SipListener = (Option<Transport>, Option<String>, Route);
fn sip_listener(
    reader: &mut Reader<'_>,
    name: &str,
    declared: &BTreeSet<&str>,
) -> Result<SipListener, ConfigError> {
    let path = reader.path_of("transport");
    let Some(entry) = reader.get("transport") else {
        return Err(ConfigError::missing_key(
            &path,
            "a sip listener declares its transport",
        ));
    };
    let transport = match string(entry, &path)? {
        "udp" => Transport::Udp,
        "tcp" => Transport::Tcp,
        "tls" => Transport::Tls,
        "ws" => Transport::Ws,
        "wss" => Transport::Wss,
        other => {
            return Err(ConfigError::bad_value(
                entry.line,
                &path,
                &format!("`{other}` is not a transport; it is udp, tcp, tls, ws or wss"),
            ));
        }
    };

    let advertise = match reader.get("advertise") {
        Some(entry) => Some(advertised(entry, &reader.path_of("advertise"))?),
        None => None,
    };

    let app = match reader.get("app") {
        Some(entry) => Some(string(entry, &reader.path_of("app"))?.to_owned()),
        None => None,
    };
    let no_app = match reader.get("no_app") {
        Some(entry) => {
            let path = reader.path_of("no_app");
            let status = integer(entry, &path)?;
            let status = u16::try_from(status)
                .ok()
                .filter(|s| (400..=699).contains(s));
            Some(status.ok_or_else(|| {
                ConfigError::bad_value(entry.line, &path, "a refusal carries a 4xx–6xx status")
            })?)
        }
        None => None,
    };

    let route = match (app, no_app) {
        (Some(app), None) => {
            if !declared.contains(app.as_str()) {
                return Err(ConfigError::unknown_app(name, &app));
            }
            Route::App(app)
        }
        (None, Some(status)) => Route::Refuse(status),
        (None, None) => return Err(ConfigError::unrouted_listener(name)),
        (Some(_), Some(_)) => return Err(ConfigError::unreachable_no_app(name)),
    };

    Ok((Some(transport), advertise, route))
}

/// A `host:port` to advertise.
fn advertised(entry: &Entry, path: &str) -> Result<String, ConfigError> {
    let text = string(entry, path)?;
    let ok = text
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0));
    if ok {
        Ok(text.to_owned())
    } else {
        Err(ConfigError::bad_value(
            entry.line,
            path,
            "a <host>:<port> to put in Via and Contact",
        ))
    }
}

// ------------------------------------------------------------------ typed value access ----

fn string<'a>(entry: &'a Entry, path: &str) -> Result<&'a str, ConfigError> {
    match &entry.value {
        Value::String(text) => Ok(text),
        other => Err(ConfigError::bad_value(
            entry.line,
            path,
            &format!("expected a string, found {}", other.kind()),
        )),
    }
}

fn integer(entry: &Entry, path: &str) -> Result<i64, ConfigError> {
    match &entry.value {
        Value::Integer(value) => Ok(*value),
        other => Err(ConfigError::bad_value(
            entry.line,
            path,
            &format!("expected an integer, found {}", other.kind()),
        )),
    }
}

fn boolean(entry: &Entry, path: &str) -> Result<bool, ConfigError> {
    match &entry.value {
        Value::Boolean(value) => Ok(*value),
        other => Err(ConfigError::bad_value(
            entry.line,
            path,
            &format!("expected true or false, found {}", other.kind()),
        )),
    }
}

fn string_list<'a>(entry: &'a Entry, path: &str) -> Result<Vec<&'a str>, ConfigError> {
    let Value::Array(items) = &entry.value else {
        return Err(ConfigError::bad_value(
            entry.line,
            path,
            &format!("expected a list, found {}", entry.value.kind()),
        ));
    };
    items
        .iter()
        .map(|item| match item {
            Value::String(text) => Ok(text.as_str()),
            other => Err(ConfigError::bad_value(
                entry.line,
                path,
                &format!("every item is a string, found {}", other.kind()),
            )),
        })
        .collect()
}

fn non_empty_strings(entry: &Entry, path: &str) -> Result<Vec<String>, ConfigError> {
    let items = string_list(entry, path)?;
    if items.iter().any(|item| item.is_empty()) {
        return Err(ConfigError::bad_value(
            entry.line,
            path,
            "an empty item grants nothing and hides that it grants nothing",
        ));
    }
    Ok(items.into_iter().map(str::to_owned).collect())
}

// ------------------------------------------------------------------------- the reader ----

/// One table, read key by key, refusing whatever is left over.
///
/// The leftover pass is N2: a key nobody asked for is a key the operator believed in.
struct Reader<'a> {
    table: &'a Table,
    path: String,
    taken: Vec<&'a str>,
}

impl<'a> Reader<'a> {
    fn new(table: &'a Table) -> Self {
        Self {
            table,
            path: table.header(),
            taken: Vec::new(),
        }
    }

    fn path_of(&self, key: &str) -> String {
        format!("{}.{key}", self.path)
    }

    fn get(&mut self, key: &str) -> Option<&'a Entry> {
        let entry = self.table.entries.iter().find(|entry| entry.key == key)?;
        self.taken.push(entry.key.as_str());
        Some(entry)
    }

    /// Every key not read: refused as unknown, or — if the schema knows it but not here — as a key
    /// that may not appear where it does.
    fn finish(&self, known: &[&str], why: &str) -> Result<(), ConfigError> {
        for entry in &self.table.entries {
            if self.taken.contains(&entry.key.as_str()) {
                continue;
            }
            let path = self.path_of(&entry.key);
            return Err(if known.contains(&entry.key.as_str()) {
                ConfigError::key_not_allowed(entry.line, &path, why)
            } else {
                ConfigError::unknown_key(entry.line, &path)
            });
        }
        Ok(())
    }
}
