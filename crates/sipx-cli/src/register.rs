//! `sipx register`.

use std::time::Duration;

use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};
use sipx_ua::{Config, Credentials, UserAgent};

use crate::Args;
use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx register — register with a registrar

USAGE:
    sipx register <AOR> [OPTIONS]

ARGS:
    <AOR>    The address of record, e.g. sip:alice@example.com

OPTIONS:
    --password <P>   Password. Prefer SIPX_PASSWORD, since argv is world-readable.
    --target <ADDR>  Where to send, if not derived from the AOR (host:port)
    --expires <S>    Lease to ask for, in seconds (default 3600)
    --local <ADDR>   Local address to bind (default 0.0.0.0:0)
    --tcp            Use TCP rather than UDP
    --keep-alive     Keep refreshing until interrupted
    --json           Report as JSON
";

pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    let args = Args::new(raw);
    if args.flag("help") || raw.iter().any(|a| a == "-h") {
        print!("{HELP}");
        return Exit::Success;
    }

    let Some(aor) = args.positional() else {
        eprint!("{HELP}");
        return fail(format, Exit::Usage, "an address of record is required");
    };

    // Same refusal as `dial`, and it matters more here: a registration challenged with digest
    // sends a credential, so a silent downgrade to UDP puts it on the wire in the clear (`S-27`).
    if let Some(why) = crate::insecure_scheme_refusal(aor) {
        return fail(format, Exit::Usage, &why);
    }

    let Some((user, domain)) = parse_aor(aor) else {
        return fail(
            format,
            Exit::Usage,
            &format!("not a SIP address of record: {aor}"),
        );
    };

    // A password on the command line is visible to every process on the machine, so the
    // environment is the documented route and the flag is the convenience.
    let password = args
        .value("password")
        .map(str::to_owned)
        .or_else(|| std::env::var("SIPX_PASSWORD").ok());

    let transport = if args.flag("tcp") {
        TransportKind::Tcp
    } else {
        TransportKind::Udp
    };

    let target = match resolve_target(args.value("target"), &domain, transport) {
        Ok(target) => target,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let local = args.value("local").unwrap_or("0.0.0.0:0");
    let Ok(local) = local.parse() else {
        return fail(format, Exit::Usage, &format!("not an address: {local}"));
    };

    let mut config = TransportConfig::new(local);
    // The `Via` sent-by names where *this* client expects responses (RFC 3261 §18.1.1); the
    // bound address may be 0.0.0.0, which names every interface and reaches none.
    config.sent_by = crate::advertise::reachable_ip(local, target.addr.ip()).to_string();
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };
    // The registrar stores this binding and routes every call to the address-of-record at it
    // (RFC 3261 §10.2.6), so it carries the advertised address — reachable host, real port —
    // not the bound one.
    let contact = format!("<sip:{user}@{}>", handle.advertised());

    let Ok(host) = HostName::new(domain.clone()) else {
        return fail(format, Exit::Usage, &format!("not a hostname: {domain}"));
    };
    let registrar = Uri::sip(Host::Name(host));

    let mut ua_config = Config::new(format!("<sip:{user}@{domain}>"), contact, registrar, target);
    ua_config.expires = Duration::from_secs(args.number("expires").unwrap_or(3600));
    if let Some(password) = password {
        ua_config = ua_config.with_credentials(Credentials::new(user.clone(), password));
    }

    let mut agent = UserAgent::new(handle, ua_config);

    match agent.register().await {
        Ok(lease) => {
            Report::new()
                .text("status", "registered")
                .text("aor", format!("sip:{user}@{domain}"))
                .seconds("expires", lease.granted)
                .seconds("refresh_in", lease.refresh_after)
                .emit(format);

            if args.flag("keep-alive") {
                // `keep_registered` refreshes forever; its success type is uninhabited, so the
                // only way out is a failure. The single-arm match says so, where an `if let`
                // would read as though the other case were possible.
                let Err(error) = agent.keep_registered().await;
                return report_failure(format, &error);
            }
            Exit::Success
        }
        Err(error) => report_failure(format, &error),
    }
}

fn report_failure(format: Format, error: &sipx_ua::Error) -> Exit {
    let exit = match error {
        sipx_ua::Error::Rejected { status, .. } => Exit::for_status(*status),
        // A 555 is a refusal by the far end (RFC 8599 §8.1), so it exits the way every other
        // refusal does — `Exit::for_status(555)` is `Rejected` too, and a status code that
        // changed the exit code by being modelled more precisely would be a change to this
        // CLI's documented interface rather than a change to what happened. What the caller
        // gains is the message, which names the refusal instead of numbering it.
        sipx_ua::Error::PushNotSupported { .. } => Exit::Rejected,
        sipx_ua::Error::AuthenticationFailed | sipx_ua::Error::CredentialsRequired => {
            Exit::Unauthorized
        }
        sipx_ua::Error::NoResponse => Exit::Timeout,
        _ => Exit::Failed,
    };
    fail(format, exit, &error.to_string())
}

/// Split `sip:user@domain` into its two halves.
fn parse_aor(aor: &str) -> Option<(String, String)> {
    let rest = aor
        .strip_prefix("sip:")
        .or_else(|| aor.strip_prefix("sips:"))?;
    let (user, domain) = rest.split_once('@')?;
    (!user.is_empty() && !domain.is_empty()).then(|| {
        (
            user.to_owned(),
            domain.split(';').next().unwrap_or(domain).to_owned(),
        )
    })
}

/// Where to send. An explicit `--target` wins; otherwise the domain must already be an address,
/// since resolving a name is the resolver's job and this command does not carry one.
fn resolve_target(
    explicit: Option<&str>,
    domain: &str,
    transport: TransportKind,
) -> Result<Target, String> {
    let raw = explicit.unwrap_or(domain);
    if let Ok(addr) = raw.parse::<std::net::SocketAddr>() {
        return Ok(Target::new(addr, transport));
    }
    if let Ok(ip) = raw.parse::<std::net::IpAddr>() {
        return Ok(Target::new(
            std::net::SocketAddr::new(ip, transport.default_port()),
            transport,
        ));
    }
    Err(format!(
        "cannot reach {raw}: give --target host:port, since name resolution is not wired into \
         this command yet"
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_address_of_record_splits_into_user_and_domain() {
        assert_eq!(
            parse_aor("sip:alice@example.com"),
            Some(("alice".to_owned(), "example.com".to_owned()))
        );
        assert_eq!(
            parse_aor("sips:bob@secure.example"),
            Some(("bob".to_owned(), "secure.example".to_owned()))
        );
    }

    /// Parameters belong to the URI, not the domain. Leaving them on makes the registrar name
    /// `example.com;transport=tcp`, which resolves to nothing.
    /// `S-27`, the half that costs more than `dial`'s: a challenged registration sends a digest
    /// credential, so a silent downgrade to UDP puts it on the wire in the clear. The first argument
    /// must be the subcommand — `Args::positional` skips index 0.
    #[tokio::test]
    async fn a_sips_aor_is_refused_rather_than_registered_in_the_clear() {
        let exit = run(
            &["register".to_owned(), "sips:bob@192.0.2.1".to_owned()],
            Format::Text,
        )
        .await;
        assert_eq!(
            exit.code(),
            Exit::Usage.code(),
            "registering a sips: AOR must be refused, not sent in the clear"
        );
    }

    #[test]
    fn uri_parameters_are_not_part_of_the_domain() {
        assert_eq!(
            parse_aor("sip:alice@example.com;transport=tcp"),
            Some(("alice".to_owned(), "example.com".to_owned()))
        );
    }

    #[test]
    fn something_that_is_not_an_aor_is_refused() {
        for bad in [
            "alice@example.com",
            "sip:example.com",
            "sip:@example.com",
            "sip:alice@",
            "",
        ] {
            assert!(parse_aor(bad).is_none(), "{bad} is not an AOR");
        }
    }

    #[test]
    fn an_explicit_target_wins_over_the_domain() {
        let target = resolve_target(Some("192.0.2.1:5080"), "example.com", TransportKind::Udp)
            .expect("an address");
        assert_eq!(target.addr.to_string(), "192.0.2.1:5080");
    }

    #[test]
    fn a_bare_address_gets_the_transport_default_port() {
        assert_eq!(
            resolve_target(Some("192.0.2.1"), "x", TransportKind::Udp)
                .expect("an address")
                .addr
                .port(),
            5060
        );
        assert_eq!(
            resolve_target(Some("192.0.2.1"), "x", TransportKind::Tls)
                .expect("an address")
                .addr
                .port(),
            5061
        );
    }

    /// A name with nothing to resolve it says so, rather than failing later with something
    /// that looks like a network problem.
    #[test]
    fn a_name_with_no_resolver_is_a_named_usage_error() {
        let error =
            resolve_target(None, "example.com", TransportKind::Udp).expect_err("cannot be reached");
        assert!(error.contains("--target"), "{error}");
    }
}
