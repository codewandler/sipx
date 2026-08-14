//! Product-boundary endpoint for the independent native-browser audio proof (M-51).
//!
//! This is proof infrastructure, not a supported command-line surface. The browser peer and
//! process owner live under `tests/browser-audio/`; this executable exercises only public sipx
//! transport and call APIs.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{Value, json};
use sipx_call::{
    Call, DialOptions, Error, MediaAddress, MediaPolicy, MediaProfile, NegotiatedKeying, SrtpSuite,
    answer_with_policy_at, dial, serve,
};
use sipx_media::Codec;
use sipx_media::browser::ComponentState;
use sipx_sdp::browser_audio::ProfileError;
use sipx_sip::{HeaderName, Host, HostName, Method, ResponseBuilder, StatusCode, Uri};
use sipx_transport::tls::{Identity, ServerTls};
use sipx_transport::{Config, Target, TransportKind, bind};

const OPERATION_BOUND: Duration = Duration::from_secs(90);
const MEDIA_BOUND: Duration = Duration::from_secs(20);
const MEDIA_SAMPLES: usize = 4_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    BrowserOfferer,
    BrowserAnswerer,
}

impl Role {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "browser-offerer" => Ok(Self::BrowserOfferer),
            "browser-answerer" => Ok(Self::BrowserAnswerer),
            _ => Err(format!("unknown browser role {value}")),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::BrowserOfferer => "browser-offerer",
            Self::BrowserAnswerer => "browser-answerer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Positive,
    FingerprintMismatch,
    NoNominatedPair,
    WeakerMedia,
    KdfPerturbation,
}

impl Case {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "positive" => Ok(Self::Positive),
            "FingerprintMismatch" => Ok(Self::FingerprintMismatch),
            "NoNominatedPair" => Ok(Self::NoNominatedPair),
            "WeakerMedia" => Ok(Self::WeakerMedia),
            "KdfPerturbation" => Ok(Self::KdfPerturbation),
            _ => Err(format!("unknown proof case {value}")),
        }
    }

    const fn expected(self) -> Option<ProfileError> {
        match self {
            Self::Positive | Self::KdfPerturbation => None,
            Self::FingerprintMismatch => Some(ProfileError::FingerprintMismatch),
            Self::NoNominatedPair => Some(ProfileError::NoNominatedPair),
            Self::WeakerMedia => Some(ProfileError::WeakerMedia),
        }
    }

    const fn error_name(self) -> &'static str {
        match self {
            Self::Positive => "",
            Self::FingerprintMismatch => "FingerprintMismatch",
            Self::NoNominatedPair => "NoNominatedPair",
            Self::WeakerMedia => "WeakerMedia",
            Self::KdfPerturbation => "KdfPerturbation",
        }
    }
}

struct Arguments {
    role: Role,
    case: Case,
    media_address: IpAddr,
    certificate: PathBuf,
    key: PathBuf,
    result: PathBuf,
    srtp_suite: Option<SrtpSuite>,
}

impl Arguments {
    fn read() -> Result<Self, String> {
        let mut role = None;
        let mut case = None;
        let mut media_address = None;
        let mut certificate = None;
        let mut key = None;
        let mut result = None;
        let mut srtp_suite = None;
        let mut values = std::env::args().skip(1);
        while let Some(flag) = values.next() {
            let value = values
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--role" => role = Some(Role::parse(&value)?),
                "--case" => case = Some(Case::parse(&value)?),
                "--media-address" => {
                    media_address = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid media address {value}"))?,
                    );
                }
                "--cert" => certificate = Some(PathBuf::from(value)),
                "--key" => key = Some(PathBuf::from(value)),
                "--result" => result = Some(PathBuf::from(value)),
                "--srtp-suite" => srtp_suite = Some(parse_srtp_suite(&value)?),
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        Ok(Self {
            role: role.ok_or_else(|| "--role is required".to_owned())?,
            case: case.ok_or_else(|| "--case is required".to_owned())?,
            media_address: media_address.ok_or_else(|| "--media-address is required".to_owned())?,
            certificate: certificate.ok_or_else(|| "--cert is required".to_owned())?,
            key: key.ok_or_else(|| "--key is required".to_owned())?,
            result: result.ok_or_else(|| "--result is required".to_owned())?,
            srtp_suite,
        })
    }

    fn validate(&self) -> Result<(), String> {
        let valid = match self.case {
            Case::Positive => true,
            Case::FingerprintMismatch => self.role == Role::BrowserOfferer,
            Case::NoNominatedPair | Case::WeakerMedia => self.role == Role::BrowserAnswerer,
            Case::KdfPerturbation => {
                self.role == Role::BrowserAnswerer
                    && self.srtp_suite == Some(SrtpSuite::AeadAes256Gcm)
            }
        };
        valid.then_some(()).ok_or_else(|| {
            format!(
                "{} is not defined for {}",
                self.case.error_name(),
                self.role.as_str()
            )
        })
    }

    fn media_policy(&self) -> MediaPolicy {
        match self.srtp_suite {
            Some(suite) => MediaPolicy::browser_audio().with_srtp_suite(suite),
            None => MediaPolicy::browser_audio(),
        }
    }
}

fn parse_srtp_suite(value: &str) -> Result<SrtpSuite, String> {
    match value {
        "AEAD_AES_128_GCM" => Ok(SrtpSuite::AeadAes128Gcm),
        "AEAD_AES_256_GCM" => Ok(SrtpSuite::AeadAes256Gcm),
        "AES_CM_128_HMAC_SHA1_80" => Ok(SrtpSuite::AesCm128HmacSha1_80),
        _ => Err(format!("unknown SRTP suite {value}")),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::read().map_err(std::io::Error::other)?;
    arguments.validate().map_err(std::io::Error::other)?;
    let result = Box::pin(tokio::time::timeout(OPERATION_BOUND, execute(&arguments)))
        .await
        .map_err(|_| {
            std::io::Error::other("browser-audio proof operation exceeded 90 seconds")
        })??;
    let encoded = serde_json::to_vec(&result)?;
    std::fs::write(&arguments.result, &encoded)?;
    println!("{}", String::from_utf8(encoded)?);
    Ok(())
}

async fn execute(arguments: &Arguments) -> Result<Value, Box<dyn std::error::Error>> {
    let certificate = std::fs::read(&arguments.certificate)?;
    let key = std::fs::read(&arguments.key)?;
    let identity = Identity::from_pem(&certificate, &key)?;
    let mut config = Config::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    "localhost".clone_into(&mut config.sent_by);
    config.wss_server = Some((ServerTls::new(identity)?, 0));
    config.pool.reuse_inbound_for_outbound = true;
    let (endpoint, mut incoming) = bind(config).await?;
    let wss_address = endpoint
        .wss_addr()
        .ok_or_else(|| std::io::Error::other("WSS listener was not bound"))?;
    println!("{}", json!({"status": "listening", "address": wss_address}));

    let attempt = match arguments.role {
        Role::BrowserOfferer => {
            let invitation = next_method(&mut incoming, Method::Invite, |request| {
                &request.request.method
            })
            .await?;
            answer_with_policy_at(
                &endpoint,
                &invitation,
                MediaAddress::new(arguments.media_address),
                arguments.media_policy(),
            )
            .await
        }
        Role::BrowserAnswerer => {
            let readiness = next_method(&mut incoming, Method::Options, |request| {
                &request.request.method
            })
            .await?;
            let target = Target::new(readiness.source, TransportKind::Wss);
            let response = ResponseBuilder::to_request(
                &readiness.request,
                StatusCode::new(200).ok_or_else(|| std::io::Error::other("invalid status"))?,
                "OK",
            )?
            .header(
                HeaderName::Contact,
                Bytes::from_static(b"<sip:sipx@localhost>"),
            )?
            .build();
            endpoint.respond(&readiness.key, response).await?;
            let to = Uri::sip(Host::Name(HostName::new("localhost")?));
            let options = DialOptions::new("<sip:sipx@localhost>", arguments.media_address)
                .with_media_policy(arguments.media_policy())
                // This proof deliberately runs both media endpoints on one CI host. The shared
                // private range is therefore its topology, not an accidental advertised route.
                .with_media_range_overlap(true)
                .with_timeout(Duration::from_secs(20));
            dial(&endpoint, target, &to, &options).await
        }
    };

    if arguments.case == Case::KdfPerturbation {
        return match attempt {
            Ok(call) => perturbed_kdf_result(&call, arguments).await,
            Err(error) => Err(std::io::Error::other(format!(
                "KDF perturbation failed before media authentication at {error}"
            ))
            .into()),
        };
    }

    match (attempt, arguments.case.expected()) {
        (Ok(mut call), None) => {
            let received_audio_peak = exercise_media(&call).await?;
            let result = positive_result(&call, arguments, received_audio_peak)?;
            match arguments.role {
                Role::BrowserOfferer => serve(&mut call, &mut incoming).await?,
                Role::BrowserAnswerer => call.hang_up().await?,
            }
            Ok(result)
        }
        (Err(Error::Profile(actual)), Some(expected)) if actual == expected => Ok(json!({
            "error": arguments.case.error_name(),
            "typed_error": actual.to_string(),
        })),
        (Ok(_), Some(expected)) => Err(std::io::Error::other(format!(
            "negative unexpectedly established a call instead of {expected}"
        ))
        .into()),
        (Err(error), Some(expected)) => {
            Err(std::io::Error::other(format!("negative failed at {error}, not {expected}")).into())
        }
        (Err(error), None) => Err(error.into()),
    }
}

async fn perturbed_kdf_result(
    call: &Call,
    arguments: &Arguments,
) -> Result<Value, Box<dyn std::error::Error>> {
    let media_failure = match exercise_media(call).await {
        Ok(_) => {
            return Err(std::io::Error::other(
                "perturbed AEAD key derivation unexpectedly carried audio",
            )
            .into());
        }
        Err(error) => error.to_string(),
    };
    let component = call
        .browser_component()
        .ok_or_else(|| std::io::Error::other("browser component facts are absent"))?;
    Ok(json!({
        "error": arguments.case.error_name(),
        "media_failure": media_failure,
        "requested_srtp_suite": arguments.srtp_suite.map_or("any", SrtpSuite::name),
        "negotiated_srtp_suite": negotiated_srtp_suite(call),
        "packets_sent": call.media().packets_sent(),
        "packets_received": call.media().packets_received(),
        "srtp_authentication_failures": component.counts.srtp_authentication_failures,
    }))
}

async fn next_method<T>(
    incoming: &mut tokio::sync::mpsc::Receiver<T>,
    method: Method,
    method_of: impl Fn(&T) -> &Method,
) -> Result<T, Box<dyn std::error::Error>> {
    while let Some(item) = incoming.recv().await {
        if method_of(&item) == &method {
            return Ok(item);
        }
    }
    Err(std::io::Error::other("WSS endpoint stopped").into())
}

async fn exercise_media(call: &Call) -> Result<u16, Box<dyn std::error::Error>> {
    let tone: Vec<i16> = (0..24_000)
        .map(|sample| {
            if (sample / 34) % 2 == 0 {
                12_000
            } else {
                -12_000
            }
        })
        .collect();
    if !call.play(&tone).await {
        return Err(std::io::Error::other("outbound tone did not play to completion").into());
    }
    let heard = call.record_at_least(MEDIA_SAMPLES, MEDIA_BOUND).await;
    let peak = heard
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    if heard.len() < MEDIA_SAMPLES || peak == 0 {
        return Err(std::io::Error::other("browser audio was absent or silent").into());
    }
    Ok(peak)
}

fn positive_result(
    call: &Call,
    arguments: &Arguments,
    received_audio_peak: u16,
) -> Result<Value, Box<dyn std::error::Error>> {
    let component = call
        .browser_component()
        .ok_or_else(|| std::io::Error::other("browser component facts are absent"))?;
    let selected = component
        .selected
        .ok_or_else(|| std::io::Error::other("browser component has no nominated pair"))?;
    let media_profile = match call.media_profile() {
        MediaProfile::Standard => "standard",
        MediaProfile::BrowserAudio => "browser-audio",
        _ => "unknown",
    };
    let negotiated_codec = match call.media().codec() {
        Codec::Pcmu => "pcmu",
        Codec::Pcma => "pcma",
        Codec::G722 => "g722",
        Codec::L16 => "l16",
        #[cfg(feature = "opus")]
        Codec::Opus => "opus",
        // See `media_profile` above: a value this build does not know is named as unknown rather
        // than approximated, because this proof's output is evidence and not a summary.
        _ => "unknown",
    };
    let negotiated_keying = match call.negotiated_keying() {
        NegotiatedKeying::Plain => "plain",
        NegotiatedKeying::Sdes => "sdes",
        NegotiatedKeying::DtlsSrtp => "dtls-srtp",
        _ => "unknown",
    };
    let negotiated_srtp_suite = negotiated_srtp_suite(call);
    let media_state = match component.state {
        ComponentState::IceChecking => "ice-checking",
        ComponentState::Nominated => "nominated",
        ComponentState::DtlsHandshaking => "dtls-handshaking",
        ComponentState::KeysInstalled => "keys-installed",
        ComponentState::Running => "running",
        ComponentState::Closed => "closed",
        _ => "unknown",
    };
    Ok(json!({
        "status": "answered",
        "media_profile": media_profile,
        "negotiated_codec": negotiated_codec,
        "negotiated_payload_type": call.negotiated_payload_type(),
        "negotiated_clock_rate": call.negotiated_clock_rate(),
        "negotiated_keying": negotiated_keying,
        "requested_srtp_suite": arguments.srtp_suite.map_or("any", SrtpSuite::name),
        "negotiated_srtp_suite": negotiated_srtp_suite,
        "browser_role": arguments.role.as_str(),
        "ice_component": 1,
        "nominated_local": selected.local,
        "nominated_remote": selected.remote,
        "ice_generation": selected.ice_generation,
        "local_candidate_type": selected.local_kind.as_str(),
        "remote_candidate_type": selected.remote_kind.as_str(),
        "media_state": media_state,
        "ingress_drops_total": component.counts.total(),
        "packets_sent": call.media().packets_sent(),
        "packets_received": call.media().packets_received(),
        "received_audio_peak": received_audio_peak,
    }))
}

fn negotiated_srtp_suite(call: &Call) -> &'static str {
    match call.media().srtp_profile() {
        Some(sipx_rtp::srtp::Profile::AesCm128HmacSha1_80) => "AES_CM_128_HMAC_SHA1_80",
        Some(sipx_rtp::srtp::Profile::AeadAes128Gcm) => "AEAD_AES_128_GCM",
        Some(sipx_rtp::srtp::Profile::AeadAes256Gcm) => "AEAD_AES_256_GCM",
        Some(_) => "unknown",
        None => "none",
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::time::Duration;

    use sipx_sip::Method;

    use super::next_method;

    #[tokio::test(start_paused = true)]
    async fn first_browser_method_is_not_raced_by_a_shorter_startup_timer() {
        let (sender, mut incoming) = tokio::sync::mpsc::channel(1);
        let delayed_browser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(11)).await;
            sender
                .send(Method::Invite)
                .await
                .expect("the role is still listening");
        });

        let method = next_method(&mut incoming, Method::Invite, |value| value).await;
        delayed_browser
            .await
            .expect("the delayed browser task completes");
        assert_eq!(method.expect("the method arrives"), Method::Invite);
    }
}
