//! `sipx register`.

use std::time::Duration;

use sipx_sip::push::Device;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};
use sipx_ua::{Config, Credentials, Flow, InstanceId, RegId, UserAgent};

use crate::Args;
use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx register — register with a registrar

USAGE:
    sipx register <AOR> [OPTIONS]

ARGS:
    <AOR>    The address of record, e.g. sip:alice@example.com

OPTIONS:
    --password <P>       Password. Prefer SIPX_PASSWORD, since argv is world-readable.
    --target <ADDR>      Where to send, if not derived from the AOR (host:port)
    --expires <S>        Lease to ask for, in seconds (default 3600)
    --local <ADDR>       Local address to bind (default 0.0.0.0:0)
    --tcp                Use TCP rather than UDP
    --keep-alive         Keep refreshing until interrupted
    --outbound           Register as one Outbound flow (RFC 5626), and report whether the
                         registrar accepted it
    --instance <URN>     With --outbound: this device identity rather than a fresh one
    --push-provider <P>  Push notification service this device can be woken through (RFC 8599)
    --push-prid <T>      The identifier that service knows this device by
    --push-param <X>     Service-specific extra, when the service needs one
    --wake               Act as though a push arrived: refresh the binding and report the PURR
    --json               Report as JSON
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

    // Validated before any socket is opened, and every combination that cannot work is refused:
    // a flag that parses and is dropped is worse than one that errors (`P-7` was filed for
    // exactly that shape), so `--wake` without a push service, half a push pair, or an
    // `--instance` with no flow to name are usage errors here, never surprises later.
    let reach = match reachability(&args) {
        Ok(reach) => reach,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

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

    let Reachability { flow, device, wake } = reach;
    let outbound = flow.is_some();
    let provider = device.as_ref().map(|device| device.provider().to_owned());
    if let Some(flow) = flow {
        ua_config = ua_config.with_outbound(flow);
    }
    if let Some(device) = device {
        ua_config = ua_config.with_push(device);
    }

    let mut agent = UserAgent::new(handle, ua_config);

    match agent.register().await {
        Ok(lease) => {
            let mut report = Report::new()
                .text("status", "registered")
                .text("aor", format!("sip:{user}@{domain}"))
                .seconds("expires", lease.granted)
                .seconds("refresh_in", lease.refresh_after);
            if outbound {
                // §6: a registrar that performed an outbound registration says so in `Require`.
                // Asking and not getting it is not an error — the binding is an ordinary one —
                // but it must be *said*, or a script believes a flow is being kept that is not.
                report = report.boolean("flow", agent.flow_accepted());
            }
            if let Some(provider) = &provider {
                // §8.2: a 200 from a registrar that named a *different* push service is a
                // binding nothing will ever wake, and it looks exactly like success from
                // everywhere except this field.
                report = report.boolean("push", agent.push_support().supports(provider));
            }
            report.emit(format);

            if wake {
                // §4.1.3: a push is answered with a binding-refresh REGISTER, and only then is
                // there a flow for the request the push was sent for. `woken` is that ordering;
                // when to call it is the application's, and this flag is the CLI acting as its
                // own push service for one ring.
                match agent.woken().await {
                    Ok(pending) => {
                        let mut report = Report::new()
                            .text("status", "woken")
                            .text("aor", format!("sip:{user}@{domain}"))
                            .seconds("expires", pending.lease.granted)
                            .seconds("refresh_in", pending.lease.refresh_after);
                        if let Some(purr) = &pending.purr {
                            report = report.text("purr", purr);
                        }
                        report.emit(format);
                    }
                    Err(error) => return report_failure(format, &error),
                }
            }

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

/// What the reachability flags asked for: an Outbound flow (RFC 5626), a push service to be
/// woken through (RFC 8599), and whether to act as though a push arrived once registered.
///
/// Validated as a whole rather than flag by flag, because the mistakes are *combinations* —
/// half a push pair, a wake with nothing to wake through, an instance identity with no flow —
/// and each one left to pass silently would send a registration that means less than the person
/// typing it believes.
#[derive(Debug)]
struct Reachability {
    /// `--outbound`: the flow this registration is (§4.2), with the device identity it presents.
    flow: Option<Flow>,
    /// The push parameters for the `Contact` URI (§4.1.2), from the `--push-*` flags.
    device: Option<Device>,
    /// `--wake`: after registering, send §4.1.3's binding-refresh REGISTER and report the PURR.
    wake: bool,
}

/// Read the reachability flags, refusing every combination that would otherwise parse and be
/// dropped.
fn reachability(args: &Args) -> Result<Reachability, String> {
    let provider = args.value("push-provider");
    let prid = args.value("push-prid");
    let param = args.value("push-param");

    // §4.1.2 has a UA insert `pn-provider` and `pn-prid` together: either alone names no service
    // that could be asked to wake anything, so half a pair is refused rather than registered.
    let device = match (provider, prid) {
        (Some(provider), Some(prid)) => {
            let device = Device::new(provider, prid)
                .map_err(|error| format!("--push-provider/--push-prid: {error}"))?;
            let device = match param {
                Some(param) => device
                    .with_param(param)
                    .map_err(|error| format!("--push-param: {error}"))?,
                None => device,
            };
            Some(device)
        }
        (None, None) => {
            if param.is_some() {
                return Err(
                    "--push-param belongs with --push-provider and --push-prid: without them \
                     there is no push service for it to describe"
                        .to_owned(),
                );
            }
            None
        }
        _ => {
            return Err(
                "--push-provider and --push-prid belong together: RFC 8599 §4.1.2 has a UA name \
                 the service and the device in one breath, and either alone wakes nothing"
                    .to_owned(),
            );
        }
    };

    let wake = args.flag("wake");
    if wake && device.is_none() {
        return Err(
            "--wake acts as though a push notification arrived, and no push service was named: \
             give --push-provider and --push-prid with it"
                .to_owned(),
        );
    }

    let instance = args.value("instance");
    let flow = if args.flag("outbound") {
        let instance = match instance {
            // §4.1 wants the identity persistent across restarts. The CLI keeps no state, so the
            // default is a fresh one and the flag is how a caller supplies the one it persisted.
            Some(urn) => InstanceId::parse(urn).ok_or_else(|| {
                format!("--instance must be a URN — RFC 5626 §4.1's grammar is `instance-val = urn`: {urn}")
            })?,
            None => InstanceId::generate(),
        };
        // One flow per invocation, so it is flow 1: §4.2 numbers flows stably, which is what
        // makes a refresh replace this binding rather than add a second one beside it.
        let reg_id = RegId::new(1)
            .ok_or_else(|| "reg-id 1 is inside §4.2's range by construction".to_owned())?;
        Some(Flow { instance, reg_id })
    } else {
        if instance.is_some() {
            return Err(
                "--instance names the device an Outbound flow registers; without --outbound \
                 there is no flow to name it for"
                    .to_owned(),
            );
        }
        None
    };

    Ok(Reachability { flow, device, wake })
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

    fn parsed(items: &[&str]) -> Result<Reachability, String> {
        let raw: Vec<String> = items.iter().map(|item| (*item).to_owned()).collect();
        reachability(&Args::new(&raw))
    }

    /// The flags build the config they select: the flow, the push parameters, and the wake.
    #[test]
    fn the_reachability_flags_build_what_they_select() {
        let reach = parsed(&[
            "register",
            "sip:alice@example.com",
            "--outbound",
            "--push-provider",
            "webpush",
            "--push-prid",
            "c1a5b3e7d9f2",
            "--push-param",
            "7f3ad0",
            "--wake",
        ])
        .expect("a valid combination");

        let flow = reach.flow.expect("a flow was asked for");
        assert_eq!(flow.reg_id.value(), 1, "one invocation is one flow");
        assert!(flow.instance.urn().starts_with("urn:uuid:"));
        let device = reach.device.expect("a push service was named");
        assert_eq!(device.provider(), "webpush");
        assert_eq!(device.prid(), "c1a5b3e7d9f2");
        assert_eq!(device.param(), Some("7f3ad0"));
        assert!(reach.wake);
    }

    /// A persisted device identity is presented verbatim — that is the whole of §4.1's
    /// persistence rule, and the reason the flag exists.
    #[test]
    fn an_adopted_instance_identity_is_presented_verbatim() {
        let reach = parsed(&[
            "register",
            "sip:alice@example.com",
            "--outbound",
            "--instance",
            "urn:uuid:00000000-0000-4000-8000-0000000000ab",
        ])
        .expect("a valid combination");
        let flow = reach.flow.expect("a flow");
        assert_eq!(
            flow.instance.urn(),
            "urn:uuid:00000000-0000-4000-8000-0000000000ab"
        );
    }

    /// §4.1.2 inserts `pn-provider` and `pn-prid` together; either alone names no service
    /// that could be asked to wake anything.
    #[test]
    fn half_a_push_pair_is_refused() {
        for items in [
            &["register", "sip:a@b.c", "--push-provider", "webpush"][..],
            &["register", "sip:a@b.c", "--push-prid", "tok"][..],
        ] {
            let error = parsed(items).expect_err("half a pair is not a push service");
            assert!(error.contains("--push-provider"), "{error}");
            assert!(error.contains("--push-prid"), "{error}");
        }
    }

    #[test]
    fn a_push_param_without_the_pair_is_refused() {
        let error = parsed(&["register", "sip:a@b.c", "--push-param", "x"])
            .expect_err("a param with no service to describe");
        assert!(error.contains("--push-provider"), "{error}");
    }

    /// A wake with no push service named is the accepted-and-dropped shape: the flag would
    /// parse, and nothing would ever call `woken`.
    #[test]
    fn a_wake_without_a_push_service_is_refused() {
        let error =
            parsed(&["register", "sip:a@b.c", "--wake"]).expect_err("nothing to be woken through");
        assert!(error.contains("--push-provider"), "{error}");
    }

    #[test]
    fn an_instance_without_outbound_is_refused() {
        let error = parsed(&["register", "sip:a@b.c", "--instance", "urn:uuid:x"])
            .expect_err("no flow to name the device for");
        assert!(error.contains("--outbound"), "{error}");
    }

    #[test]
    fn an_instance_that_is_not_a_urn_is_refused() {
        let error = parsed(&[
            "register",
            "sip:a@b.c",
            "--outbound",
            "--instance",
            "device-7",
        ])
        .expect_err("§4.1's grammar is instance-val = urn");
        assert!(error.contains("--instance"), "{error}");
    }

    /// A `pn-prid` that RFC 3261's `pvalue` cannot hold would not be rejected by the
    /// registrar — it would silently become a different URI. Refused here instead.
    #[test]
    fn a_prid_that_a_uri_parameter_cannot_hold_is_refused() {
        let error = parsed(&[
            "register",
            "sip:a@b.c",
            "--push-provider",
            "webpush",
            "--push-prid",
            "tok;en",
        ])
        .expect_err("a ';' would start another parameter at the far end");
        assert!(error.contains("--push-prid"), "{error}");
    }

    /// The refusal is the command's answer, at the command's exit code — a usage error
    /// before any socket is opened, not a registration that quietly means less.
    #[tokio::test]
    async fn a_wake_without_push_is_a_usage_error_from_the_command() {
        let exit = run(
            &[
                "register".to_owned(),
                "sip:alice@example.com".to_owned(),
                "--wake".to_owned(),
            ],
            Format::Text,
        )
        .await;
        assert_eq!(exit.code(), Exit::Usage.code());
    }
}
