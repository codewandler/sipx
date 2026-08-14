//! Registry-shaped compile proof for the complete Supported v1 endpoint path.

use std::net::{IpAddr, SocketAddr};

use sipx_call::{
    Call, CallConfig, Codecs, DialOptions, Direction, Dispatched, Dispatcher, IcePolicy, Keying,
    MediaAddress, MediaPolicy, SrtpSuite, TurnPolicy, dial, serve,
};
use sipx_sip::Uri;
use sipx_transport::{Handle, Incoming, Target};
use sipx_ua::{Config as RegistrationConfig, Flows, InstanceId, Power};
use tokio::sync::mpsc;

fn production_call_config(
    local: IpAddr,
    relay: SocketAddr,
) -> Result<CallConfig, sipx_call::TurnPolicyError> {
    let turn = TurnPolicy::new(relay, "1000", "relay-password")?;
    let media = MediaPolicy::default()
        .with_codecs(Codecs::G711)
        .with_ice(IcePolicy::Turn(turn))
        .with_keying(Keying::DtlsSrtp)
        .with_srtp_suite(SrtpSuite::AeadAes256Gcm);
    Ok(CallConfig::new(MediaAddress::new(local))
        .with_media_policy(media)
        .with_initial_direction(Direction::SendRecv))
}

async fn registration_lifetime(
    endpoint: Handle,
    config: RegistrationConfig,
    target: Target,
    instance: InstanceId,
) -> sipx_ua::Result<()> {
    let mut flows = Flows::for_instance(instance);
    let _reg_id = flows.add(endpoint, config, target)?;
    let lifetime = flows.start(Power::Unconstrained, None)?;
    let cleanup = lifetime.cancel().await;
    assert_eq!(cleanup.active, 0);
    Ok(())
}

async fn dial_role(
    endpoint: &Handle,
    target: Target,
    callee: &Uri,
    config: CallConfig,
) -> sipx_call::Result<Call> {
    let options = DialOptions::new("<sip:1000@example.com>", config.media_address().advertised())
        .with_call_config(config);
    dial(endpoint, target, callee, &options).await
}

async fn answer_or_reject_role(
    endpoint: &Handle,
    incoming: mpsc::Receiver<Incoming>,
    config: CallConfig,
    accept: bool,
) -> sipx_call::Result<()> {
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
    if let Some(Dispatched::Invitation(invitation)) = dispatcher.next().await {
        if accept {
            let mut call = invitation.answer_with_config(endpoint, config).await?;
            let (_, mut requests) = invitation.into_parts();
            let _ = serve(&mut call, &mut requests).await?;
        } else {
            invitation.refuse(endpoint, 486, "Busy Here").await?;
        }
    }
    Ok(())
}

async fn established_lifecycle(
    mut call: Call,
    other: &Call,
    target: &Uri,
) -> sipx_call::Result<()> {
    call.reinvite(Direction::SendOnly).await?;
    call.reinvite(Direction::SendRecv).await?;
    call.refer(target).await?;
    call.refer_attended(other).await?;
    call.hang_up().await
}

fn identity_policies(
    endpoint: Handle,
    incoming: mpsc::Receiver<Incoming>,
    outbound: sipx_call::OutboundIdentityPolicy,
    inbound: sipx_call::InboundIdentityPolicy,
) {
    let _dial = DialOptions::new(
        "<sip:1000@example.com>",
        "192.0.2.10".parse().expect("documentation address"),
    )
    .with_identity(outbound);
    let _dispatcher = Dispatcher::new(endpoint, incoming).with_identity(inbound);
}

fn registration_config(
    registrar: Uri,
    target: Target,
) -> RegistrationConfig {
    RegistrationConfig::new(
        "<sip:1000@example.com>",
        "<sip:1000@192.0.2.10:5060>",
        registrar,
        target,
    )
    .with_expires(std::time::Duration::from_secs(3_600))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local = "192.0.2.10".parse()?;
    let relay = "192.0.2.20:3478".parse()?;
    let configured = production_call_config(local, relay)?;
    assert_eq!(configured.media_address(), MediaAddress::new(local));
    assert_eq!(configured.initial_direction(), Direction::SendRecv);

    let defaults = CallConfig::new(MediaAddress::new(local));
    assert_eq!(defaults.media_policy().codecs(), Codecs::G711);
    assert_eq!(defaults.media_policy().ice(), &IcePolicy::Disabled);
    assert_eq!(defaults.media_policy().keying(), Keying::Auto);

    let _ = registration_lifetime;
    let _ = dial_role;
    let _ = answer_or_reject_role;
    let _ = established_lifecycle;
    let _ = identity_policies;
    let _ = registration_config;
    Ok(())
}
