//! `sipx register`.

use std::time::Duration;

use sipx_sip::push::Device;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config as TransportConfig, bind};
use sipx_ua::{Config, Credentials, Flow, InstanceId, RegId, UserAgent};

use crate::cli::RegisterOptions;
use crate::output::{Exit, Format, Report, fail};

#[allow(
    clippy::too_many_lines,
    reason = "the command lifecycle is kept in execution order so validation-before-I/O remains auditable"
)]
pub(crate) async fn run(options: RegisterOptions, format: Format) -> Exit {
    let aor = options.aor.as_str();
    let headers = match crate::header::from_options(&options.headers) {
        Ok(headers) => headers,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let Ok(parsed_aor) = Uri::parse(bytes::Bytes::from(aor.to_owned())) else {
        return fail(
            format,
            Exit::Usage,
            &format!("not a SIP address of record: {aor}"),
        );
    };

    let Some((user, domain)) = parse_aor(aor) else {
        return fail(
            format,
            Exit::Usage,
            &format!("not a SIP address of record: {aor}"),
        );
    };

    let password = options.password.clone();

    // Validated before any socket is opened, and every combination that cannot work is refused:
    // a flag that parses and is dropped is worse than one that errors (`P-7` was filed for
    // exactly that shape), so `--wake` without a push service, half a push pair, or an
    // `--instance` with no flow to name are usage errors here, never surprises later.
    let reach = match reachability(&options) {
        Ok(reach) => reach,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let mut transport = match crate::signalling::Selection::from_options(
        &options.signalling,
        parsed_aor.scheme().is_secure(),
    ) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let resolver = crate::destination::Resolver::system();
    let candidates = match resolver
        .resolve(
            &parsed_aor,
            options.target.as_deref(),
            transport,
            &options.signalling,
        )
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => return fail(format, error.exit(), &error.to_string()),
    };
    let target = match crate::destination::first(&candidates) {
        Ok(target) => target.clone(),
        Err(error) => return fail(format, error.exit(), &error.to_string()),
    };
    transport = transport.negotiated(target.transport);
    let local = options.local;

    let mut config = TransportConfig::new(local);
    // The `Via` sent-by names where *this* client expects responses (RFC 3261 §18.1.1); the
    // bound address may be 0.0.0.0, which names every interface and reaches none.
    config.sent_by = crate::advertise::reachable_ip(local, target.addr.ip()).to_string();
    crate::apply_capture(&options.capture, &mut config);
    if let Err(message) = transport.configure_client(&options.signalling, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Armed here, not at the end: the run that most needs the numbers is the one that fails, and
    // every `return fail(…)` below now takes the counters file with it.
    let export = crate::counters::Export::arm(&options.capture, &handle);
    // The registrar stores this binding and routes every call to the address-of-record at it
    // (RFC 3261 §10.2.6), so it carries the advertised address — reachable host, real port —
    // not the bound one.
    let contact = format!("<sip:{user}@{}>", handle.advertised());

    let Ok(host) = HostName::new(domain.clone()) else {
        return fail(format, Exit::Usage, &format!("not a hostname: {domain}"));
    };
    let registrar = Uri::sip(Host::Name(host));

    let mut ua_config = Config::new(format!("<sip:{user}@{domain}>"), contact, registrar, target);
    for header in headers {
        ua_config = ua_config.with_header(header);
    }
    ua_config.expires = Duration::from_secs(options.expires);
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

    // Both reports name the address of record the same way: the normalised AOR, not the argument
    // as typed, so a caller matching on it does not have to.
    let aor = format!("sip:{user}@{domain}");

    match register_candidates(&handle, &ua_config, &candidates).await {
        Ok((mut agent, lease, negotiated_transport)) => {
            let mut report = transport.report(
                Report::new()
                    .text("status", "registered")
                    .text("aor", aor.clone())
                    .seconds("expires", lease.granted)
                    .seconds("refresh_in", lease.refresh_after),
                negotiated_transport,
            );
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
            report = match export.into_report(report) {
                Ok(report) => report,
                Err(message) => return fail(format, Exit::Failed, &message),
            };
            report.emit(format);

            if wake {
                // §4.1.3: a push is answered with a binding-refresh REGISTER, and only then is
                // there a flow for the request the push was sent for. `woken` is that ordering;
                // when to call it is the application's, and this flag is the CLI acting as its
                // own push service for one ring.
                if let Err(exit) = report_wake(&mut agent, format, &aor).await {
                    return exit;
                }
            }

            if options.keep_alive {
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

async fn register_candidates(
    handle: &sipx_transport::Handle,
    config: &Config,
    candidates: &[sipx_transport::Target],
) -> Result<(UserAgent, sipx_ua::Lease, sipx_transport::TransportKind), sipx_ua::Error> {
    let mut last_transport = None;
    for target in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
        let mut attempt = config.clone();
        attempt.target = target.clone();
        let mut agent = UserAgent::new(handle.clone(), attempt);
        match agent.register().await {
            Ok(lease) => return Ok((agent, lease, target.transport)),
            Err(error @ sipx_ua::Error::Transport(_)) => last_transport = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_transport.unwrap_or(sipx_ua::Error::NoResponse))
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
fn reachability(options: &RegisterOptions) -> Result<Reachability, String> {
    let provider = options.push_provider.as_deref();
    let prid = options.push_prid.as_deref();
    let param = options.push_param.as_deref();

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

    let wake = options.wake;
    if wake && device.is_none() {
        return Err(
            "--wake acts as though a push notification arrived, and no push service was named: \
             give --push-provider and --push-prid with it"
                .to_owned(),
        );
    }

    let instance = options.instance.as_deref();
    let flow = if options.outbound {
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

/// Drive RFC 8599 §4.1.3's binding-refresh REGISTER and report the lease it returned.
///
/// A wake is reported as its own line rather than as fields on the registration's, because it is
/// a second exchange with the registrar: what was true after registering stays true, and a caller
/// reading the stream sees both answers instead of one overwritten by the other.
async fn report_wake(agent: &mut UserAgent, format: Format, aor: &str) -> Result<(), Exit> {
    match agent.woken().await {
        Ok(pending) => {
            let mut report = Report::new()
                .text("status", "woken")
                .text("aor", aor)
                .seconds("expires", pending.lease.granted)
                .seconds("refresh_in", pending.lease.refresh_after);
            // §4.2: the PURR is the registrar's, assigned when it has one to give. Absent is the
            // ordinary case, so the field appears only when it was actually handed out.
            if let Some(purr) = &pending.purr {
                report = report.text("purr", purr);
            }
            report.emit(format);
            Ok(())
        }
        Err(error) => Err(report_failure(format, &error)),
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
pub(crate) fn parse_aor(aor: &str) -> Option<(String, String)> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use clap::Parser as _;

    use crate::cli::{Cli, Command};

    fn command(items: &[&str]) -> Result<RegisterOptions, String> {
        let cli = Cli::try_parse_from(std::iter::once("sipx").chain(items.iter().copied()))
            .map_err(|error| error.to_string())?;
        let Some(Command::Register(options)) = cli.command else {
            return Err("register command expected".to_owned());
        };
        Ok(options)
    }

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
            command(&["register", "sips:bob@192.0.2.1"]).expect("valid syntax"),
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

    fn parsed(items: &[&str]) -> Result<Reachability, String> {
        let options = command(items)?;
        reachability(&options)
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
            command(&["register", "sip:alice@example.com", "--wake"]).expect("valid syntax"),
            Format::Text,
        )
        .await;
        assert_eq!(exit.code(), Exit::Usage.code());
    }
}
