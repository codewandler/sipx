//! Pure SDP policy for the named browser-compatible audio profile.
//!
//! The profile is deliberately narrow: one audio stream, one ICE component, multiplexed RTCP,
//! DTLS-SRTP, and a fixed required codec vocabulary. Validation is pure and fail-closed; socket
//! ownership and protocol state remain in `sipx-media`.

use std::net::IpAddr;

use crate::answer::{fingerprint_of, negotiate_direction, setup_of};
use crate::fingerprint::{Fingerprint, HashFunc, Setup, SetupCapabilities};
use crate::ice::{Candidate, CandidateType, ComponentId, Credentials, ICE2};
use crate::{Attribute, Connection, Direction, MediaDescription, SessionDescription};

const PROTOCOL: &str = "UDP/TLS/RTP/SAVPF";
const LOCAL_FORMATS: [&str; 5] = ["111", "0", "8", "13", "101"];
const MAX_CANDIDATES: usize = 32;
const MAX_CANDIDATE_LINE: usize = 512;

/// Which side authored a description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAudioRole {
    /// The description is an offer.
    Offerer,
    /// The description is an answer.
    Answerer,
}

/// Payload numbers carrying the required audio vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserAudioPayloads {
    /// Opus at 48 kHz and two RTP channels.
    pub opus: u8,
    /// G.711 mu-law.
    pub pcmu: u8,
    /// G.711 A-law.
    pub pcma: u8,
    /// Comfort noise at 8 kHz.
    pub comfort_noise: u8,
    /// RFC 4733 telephone events at 8 kHz.
    pub telephone_event: u8,
}

/// A description that crossed every browser-audio SDP boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserAudioDescription {
    /// Whether the peer authored an offer or answer.
    pub role: BrowserAudioRole,
    /// Required payload mappings.
    pub payloads: BrowserAudioPayloads,
    /// First usable audio payload in preference order.
    pub selected_audio_payload: u8,
    /// Declared direction.
    pub direction: Direction,
    /// Current peer ICE credentials.
    pub ice: Credentials,
    /// Usable component-one candidates in wire order.
    pub candidates: Vec<Candidate>,
    /// Peer certificate fingerprint from signalling.
    pub fingerprint: Fingerprint,
    /// Peer's resolved DTLS setup declaration.
    pub setup: Setup,
    /// Default media address, retained as a fact and never as nomination.
    pub address: IpAddr,
    /// Default media port, retained as a fact and never as nomination.
    pub port: u16,
}

/// A validated answer and the complementary local DTLS role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserAudioAnswer {
    /// Validated remote answer facts.
    pub description: BrowserAudioDescription,
    /// Offerer's local DTLS role.
    pub local_setup: Setup,
}

/// Whether a subsequent description starts another ICE generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceChange {
    /// Both credentials are unchanged.
    Unchanged,
    /// Both credentials changed together.
    Restart,
}

/// A validated subsequent description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserAudioRenegotiation {
    /// New description facts.
    pub description: BrowserAudioDescription,
    /// Relationship to the current ICE generation.
    pub ice_change: IceChange,
}

/// Local facts required to emit a complete description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserAudioLocal {
    /// Default candidate address.
    pub address: IpAddr,
    /// Bound component-one port.
    pub port: u16,
    /// Stable SDP session identifier.
    pub session_id: u64,
    /// Increasing SDP session version.
    pub session_version: u64,
    /// Requested direction.
    pub direction: Direction,
    /// Fresh credentials for this ICE generation.
    pub ice: Credentials,
    /// Gathered component-one candidates.
    pub candidates: Vec<Candidate>,
    /// Local certificate's SHA-256 fingerprint.
    pub fingerprint: Fingerprint,
    /// DTLS roles available locally.
    pub setup: SetupCapabilities,
}

/// A fail-closed profile boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// Required Opus capability is unavailable.
    #[error("Opus is required by the browser-audio profile")]
    OpusUnavailable,
    /// Signalling is not authenticated SIP over WSS.
    #[error("the browser-audio profile requires authenticated SIP over WSS")]
    InsecureSignalling,
    /// The description does not contain exactly one active audio section.
    #[error("the browser-audio profile requires exactly one active audio section")]
    MediaSectionCount,
    /// An initial offer names an unsupported protocol.
    #[error("the media protocol is not UDP/TLS/RTP/SAVPF")]
    WrongProtocol,
    /// Multiplexed RTCP or its single component is absent.
    #[error("the browser-audio profile requires multiplexed RTCP on component one")]
    RtcpMuxRequired,
    /// ICE credentials or candidates are absent or unusable.
    #[error("the browser-audio profile requires a complete usable ICE generation")]
    IceRequired,
    /// DTLS setup is absent, unresolved, or locally unavailable.
    #[error("the DTLS setup role is incompatible")]
    SetupRole,
    /// No usable SHA-256 fingerprint is present.
    #[error("a SHA-256 certificate fingerprint is required")]
    FingerprintRequired,
    /// The mandatory codec vocabulary is absent or ambiguous.
    #[error("the required browser-audio codec set is incomplete")]
    CodecSetIncomplete,
    /// A description attempts a weaker media mode.
    #[error("the description attempts to weaken the selected media profile")]
    WeakerMedia,
    /// A subsequent description removed a mandatory profile fact.
    #[error("a subsequent description removed the browser-audio profile")]
    ProfileRemoved,
    /// ICE ended before nomination.
    #[error("ICE produced no nominated component-one pair")]
    NoNominatedPair,
    /// The DTLS handshake expired.
    #[error("the DTLS handshake timed out")]
    DtlsTimeout,
    /// The media certificate differs from signalling.
    #[error("the DTLS certificate fingerprint did not match signalling")]
    FingerprintMismatch,
    /// DTLS selected no supported SRTP profile.
    #[error("DTLS negotiated no supported SRTP profile")]
    NoSrtpProfile,
    /// Cancellation won and cleanup completed.
    #[error("browser-audio setup was cancelled")]
    Cancelled,
}

/// Emit a complete initial offer.
pub fn offer(local: &BrowserAudioLocal) -> Result<SessionDescription, ProfileError> {
    validate_local(local)?;
    build(
        local,
        Setup::ActPass,
        &LOCAL_FORMATS,
        BrowserAudioPayloads {
            opus: 111,
            pcmu: 0,
            pcma: 8,
            comfort_noise: 13,
            telephone_event: 101,
        },
        local.direction,
    )
}

/// Emit an answer preserving the offered payload numbers and order.
pub fn answer(
    offered: &SessionDescription,
    local: &BrowserAudioLocal,
) -> Result<SessionDescription, ProfileError> {
    let remote = validate(offered, BrowserAudioRole::Offerer)?;
    validate_local(local)?;
    let setup = local
        .setup
        .answer_to(remote.setup)
        .map_err(|_| ProfileError::SetupRole)?;
    let required = [
        remote.payloads.opus,
        remote.payloads.pcmu,
        remote.payloads.pcma,
        remote.payloads.comfort_noise,
        remote.payloads.telephone_event,
    ];
    let offered_media = offered
        .media
        .first()
        .ok_or(ProfileError::MediaSectionCount)?;
    let formats: Vec<&str> = offered_media
        .formats
        .iter()
        .filter(|format| {
            format
                .parse::<u8>()
                .is_ok_and(|payload| required.contains(&payload))
        })
        .map(String::as_str)
        .collect();
    build(
        local,
        setup,
        &formats,
        remote.payloads,
        negotiate_direction(remote.direction, local.direction),
    )
}

/// Validate an initial offer or answer without I/O.
pub fn validate(
    description: &SessionDescription,
    role: BrowserAudioRole,
) -> Result<BrowserAudioDescription, ProfileError> {
    let media = match description.media.as_slice() {
        [media] if media.media == "audio" && !media.is_rejected() => media,
        _ => return Err(ProfileError::MediaSectionCount),
    };
    if has_attribute(description, media, "crypto") {
        return Err(ProfileError::WeakerMedia);
    }
    if media.protocol != PROTOCOL {
        return match role {
            BrowserAudioRole::Answerer => Err(ProfileError::WeakerMedia),
            BrowserAudioRole::Offerer if is_weaker_protocol(&media.protocol) => {
                Err(ProfileError::WeakerMedia)
            }
            BrowserAudioRole::Offerer => Err(ProfileError::WrongProtocol),
        };
    }
    if !media.rtcp_mux()
        || media
            .attribute("rtcp")
            .is_some_and(|attribute| !is_mux_placeholder(attribute.value.as_deref()))
        || raw_candidates(media)
            .filter_map(raw_candidate_component)
            .any(|component| component != ComponentId::RTP)
    {
        return Err(ProfileError::RtcpMuxRequired);
    }
    if media.ice_mismatch()
        || !description
            .ice_options_for(media)
            .any(|option| matches!(option, ICE2 | "trickle"))
    {
        return Err(ProfileError::IceRequired);
    }
    let ice = description
        .ice_credentials_for(media)
        .ok_or(ProfileError::IceRequired)?;
    let raw_candidates: Vec<&str> = raw_candidates(media).collect();
    if raw_candidates.is_empty()
        || raw_candidates.len() > MAX_CANDIDATES
        || raw_candidates
            .iter()
            .any(|value| value.len() > MAX_CANDIDATE_LINE)
    {
        return Err(ProfileError::IceRequired);
    }
    let candidates: Vec<Candidate> = raw_candidates
        .into_iter()
        .map(Candidate::parse)
        .collect::<Option<_>>()
        .ok_or(ProfileError::IceRequired)?;
    if candidates.iter().any(|candidate| {
        candidate.component != ComponentId::RTP
            || !matches!(
                candidate.kind,
                CandidateType::Host | CandidateType::ServerReflexive
            )
    }) {
        return Err(ProfileError::IceRequired);
    }
    let setup = setup_of(description, media).ok_or(ProfileError::SetupRole)?;
    let setup_ok = match role {
        BrowserAudioRole::Offerer => setup == Setup::ActPass,
        BrowserAudioRole::Answerer => matches!(setup, Setup::Active | Setup::Passive),
    };
    if !setup_ok {
        return Err(ProfileError::SetupRole);
    }
    let fingerprint = fingerprint_of(description, media)
        .filter(|value| value.func == HashFunc::Sha256)
        .ok_or(ProfileError::FingerprintRequired)?;
    let payloads = payloads(media)?;
    let address = description
        .address_for(media)
        .filter(|value| !value.is_unspecified())
        .ok_or(ProfileError::IceRequired)?;
    let selected_audio_payload = media
        .formats
        .iter()
        .filter_map(|format| format.parse::<u8>().ok())
        .find(|payload| [payloads.opus, payloads.pcmu, payloads.pcma].contains(payload))
        .ok_or(ProfileError::CodecSetIncomplete)?;
    Ok(BrowserAudioDescription {
        role,
        payloads,
        selected_audio_payload,
        direction: media
            .declared_direction()
            .unwrap_or_else(|| description.direction()),
        ice,
        candidates,
        fingerprint,
        setup,
        address,
        port: media.port,
    })
}

/// Validate a complete exchange and resolve the offerer's local DTLS role.
pub fn validate_answer(
    offered: &SessionDescription,
    answered: &SessionDescription,
    local_setup: SetupCapabilities,
) -> Result<BrowserAudioAnswer, ProfileError> {
    let offered_profile = validate(offered, BrowserAudioRole::Offerer)?;
    let description = validate(answered, BrowserAudioRole::Answerer)?;
    let required = [
        offered_profile.payloads.opus,
        offered_profile.payloads.pcmu,
        offered_profile.payloads.pcma,
        offered_profile.payloads.comfort_noise,
        offered_profile.payloads.telephone_event,
    ];
    let offered_media = offered
        .media
        .first()
        .ok_or(ProfileError::MediaSectionCount)?;
    let answered_media = answered
        .media
        .first()
        .ok_or(ProfileError::MediaSectionCount)?;
    let expected: Vec<&str> = offered_media
        .formats
        .iter()
        .filter(|format| {
            format
                .parse::<u8>()
                .is_ok_and(|payload| required.contains(&payload))
        })
        .map(String::as_str)
        .collect();
    if expected != answered_media.formats || offered_profile.payloads != description.payloads {
        return Err(ProfileError::CodecSetIncomplete);
    }
    let local_setup = local_setup
        .from_answer(Some(description.setup))
        .map_err(|_| ProfileError::SetupRole)?;
    Ok(BrowserAudioAnswer {
        description,
        local_setup,
    })
}

/// Validate a subsequent description without mutating the current generation.
pub fn validate_reoffer(
    current: &SessionDescription,
    next: &SessionDescription,
    role: BrowserAudioRole,
) -> Result<BrowserAudioRenegotiation, ProfileError> {
    let current = validate(current, role)?;
    let description = validate(next, role).map_err(|error| match error {
        ProfileError::IceRequired => ProfileError::IceRequired,
        ProfileError::MediaSectionCount
        | ProfileError::WrongProtocol
        | ProfileError::RtcpMuxRequired
        | ProfileError::SetupRole
        | ProfileError::FingerprintRequired
        | ProfileError::CodecSetIncomplete
        | ProfileError::WeakerMedia => ProfileError::ProfileRemoved,
        other => other,
    })?;
    let ufrag_changed = current.ice.ufrag() != description.ice.ufrag();
    let pwd_changed = current.ice.pwd() != description.ice.pwd();
    let ice_change = match (ufrag_changed, pwd_changed) {
        (false, false) => IceChange::Unchanged,
        (true, true) => IceChange::Restart,
        _ => return Err(ProfileError::IceRequired),
    };
    if current.payloads != description.payloads
        || ice_change == IceChange::Unchanged
            && (current.fingerprint != description.fingerprint
                || current.setup != description.setup)
    {
        return Err(ProfileError::ProfileRemoved);
    }
    Ok(BrowserAudioRenegotiation {
        description,
        ice_change,
    })
}

fn validate_local(local: &BrowserAudioLocal) -> Result<(), ProfileError> {
    if local.port == 0 || local.address.is_unspecified() {
        return Err(ProfileError::IceRequired);
    }
    if local.fingerprint.func != HashFunc::Sha256 {
        return Err(ProfileError::FingerprintRequired);
    }
    if local.candidates.is_empty()
        || local.candidates.len() > MAX_CANDIDATES
        || local.candidates.iter().any(|candidate| {
            candidate.component != ComponentId::RTP
                || !matches!(
                    candidate.kind,
                    CandidateType::Host | CandidateType::ServerReflexive
                )
                || candidate.to_value().len() > MAX_CANDIDATE_LINE
        })
    {
        return Err(ProfileError::IceRequired);
    }
    local
        .setup
        .answer_to(Setup::ActPass)
        .map_err(|_| ProfileError::SetupRole)?;
    Ok(())
}

fn build(
    local: &BrowserAudioLocal,
    setup: Setup,
    formats: &[&str],
    payloads: BrowserAudioPayloads,
    direction: Direction,
) -> Result<SessionDescription, ProfileError> {
    let mut description =
        SessionDescription::new(local.address, local.session_id, local.session_version);
    // The single stream owns its default; the normative profile carries no session `c=` line.
    description.connection = None;
    description
        .attributes
        .push(Attribute::valued("ice-options", ICE2));
    let mut media = MediaDescription {
        media: "audio".to_owned(),
        port: local.port,
        protocol: PROTOCOL.to_owned(),
        formats: formats.iter().map(|value| (*value).to_owned()).collect(),
        connection: Some(Connection::new(local.address)),
        attributes: vec![
            Attribute::flag(direction.as_str()),
            Attribute::flag("rtcp-mux"),
            Attribute::valued("ice-ufrag", local.ice.ufrag()),
            Attribute::valued("ice-pwd", local.ice.pwd()),
        ],
        other: Vec::new(),
    };
    media.attributes.extend(
        local
            .candidates
            .iter()
            .map(|candidate| Attribute::valued("candidate", candidate.to_value())),
    );
    media.attributes.extend([
        Attribute::valued("fingerprint", local.fingerprint.to_value()),
        Attribute::valued("setup", setup.as_str()),
        Attribute::valued("rtpmap", format!("{} opus/48000/2", payloads.opus)),
        Attribute::valued("rtpmap", format!("{} PCMU/8000", payloads.pcmu)),
        Attribute::valued("rtpmap", format!("{} PCMA/8000", payloads.pcma)),
        Attribute::valued("rtpmap", format!("{} CN/8000", payloads.comfort_noise)),
        Attribute::valued(
            "rtpmap",
            format!("{} telephone-event/8000", payloads.telephone_event),
        ),
        Attribute::valued("fmtp", format!("{} 0-16", payloads.telephone_event)),
    ]);
    description.media.push(media);
    let role = if setup == Setup::ActPass {
        BrowserAudioRole::Offerer
    } else {
        BrowserAudioRole::Answerer
    };
    validate(&description, role)?;
    Ok(description)
}

fn payloads(media: &MediaDescription) -> Result<BrowserAudioPayloads, ProfileError> {
    if media.formats.len() < 5 {
        return Err(ProfileError::CodecSetIncomplete);
    }
    let parsed: Vec<u8> = media
        .formats
        .iter()
        .map(|format| format.parse::<u8>())
        .collect::<Result<_, _>>()
        .map_err(|_| ProfileError::CodecSetIncomplete)?;
    let mut unique_formats = parsed.clone();
    unique_formats.sort_unstable();
    unique_formats.dedup();
    if unique_formats.len() != parsed.len()
        || parsed.iter().any(|payload| (64..=95).contains(payload))
    {
        return Err(ProfileError::CodecSetIncomplete);
    }
    let opus = find_mapping(media, "opus", 48_000, Some(2))?;
    let mu_law = static_or_mapping(media, 0, "PCMU", 8_000)?;
    let a_law = static_or_mapping(media, 8, "PCMA", 8_000)?;
    let comfort_noise = exact_mapping(media, 13, "CN", 8_000, None)?;
    let telephone_event = find_mapping(media, "telephone-event", 8_000, None)?;
    if !telephone_events_cover_dtmf(media, telephone_event) {
        return Err(ProfileError::CodecSetIncomplete);
    }
    let required = [opus, mu_law, a_law, comfort_noise, telephone_event];
    if required.iter().any(|payload| !parsed.contains(payload)) {
        return Err(ProfileError::CodecSetIncomplete);
    }
    let mut unique = required.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != 5 {
        return Err(ProfileError::CodecSetIncomplete);
    }
    Ok(BrowserAudioPayloads {
        opus,
        pcmu: mu_law,
        pcma: a_law,
        comfort_noise,
        telephone_event,
    })
}

fn find_mapping(
    media: &MediaDescription,
    encoding: &str,
    clock: u32,
    channels: Option<u8>,
) -> Result<u8, ProfileError> {
    let matches: Vec<u8> = media
        .formats
        .iter()
        .filter_map(|format| {
            let mapping = media.rtpmap(format)?;
            mapping_matches(mapping, encoding, clock, channels)
                .then(|| format.parse().ok())
                .flatten()
        })
        .collect();
    match matches.as_slice() {
        [payload] => Ok(*payload),
        _ => Err(ProfileError::CodecSetIncomplete),
    }
}

fn static_or_mapping(
    media: &MediaDescription,
    payload: u8,
    encoding: &str,
    clock: u32,
) -> Result<u8, ProfileError> {
    let format = payload.to_string();
    if !media.formats.contains(&format) {
        return Err(ProfileError::CodecSetIncomplete);
    }
    match media.rtpmap(&format) {
        None => Ok(payload),
        Some(mapping) if mapping_matches(mapping, encoding, clock, None) => Ok(payload),
        Some(_) => Err(ProfileError::CodecSetIncomplete),
    }
}

fn exact_mapping(
    media: &MediaDescription,
    payload: u8,
    encoding: &str,
    clock: u32,
    channels: Option<u8>,
) -> Result<u8, ProfileError> {
    let format = payload.to_string();
    match media.rtpmap(&format) {
        Some(mapping)
            if media.formats.contains(&format)
                && mapping_matches(mapping, encoding, clock, channels) =>
        {
            Ok(payload)
        }
        _ => Err(ProfileError::CodecSetIncomplete),
    }
}

fn mapping_matches(mapping: &str, encoding: &str, clock: u32, channels: Option<u8>) -> bool {
    let mut parts = mapping.split('/');
    let matches = parts
        .next()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(encoding))
        && parts.next().and_then(|actual| actual.parse::<u32>().ok()) == Some(clock);
    let actual_channels = parts.next().and_then(|actual| actual.parse::<u8>().ok());
    matches && actual_channels == channels && parts.next().is_none()
}

fn telephone_events_cover_dtmf(media: &MediaDescription, payload: u8) -> bool {
    let format = payload.to_string();
    let parameters = media.attributes.iter().find_map(|attribute| {
        if attribute.name != "fmtp" {
            return None;
        }
        let value = attribute.value.as_deref()?;
        let (actual, parameters) = value.split_once(' ')?;
        (actual == format).then_some(parameters)
    });
    let Some(parameters) = parameters else {
        // RFC 4733 §2.5.1.1 defines absent events as the telephone-event 0-15 default.
        return true;
    };
    (0_u8..=15).all(|wanted| {
        parameters.split(',').any(|part| {
            let (start, end) = part
                .split_once('-')
                .map_or((part, part), |(start, end)| (start, end));
            let Some(start) = start.parse::<u8>().ok() else {
                return false;
            };
            let Some(end) = end.parse::<u8>().ok() else {
                return false;
            };
            (start..=end).contains(&wanted)
        })
    })
}

fn has_attribute(description: &SessionDescription, media: &MediaDescription, name: &str) -> bool {
    description
        .attributes
        .iter()
        .chain(media.attributes.iter())
        .any(|attribute| attribute.name == name)
}

fn raw_candidates(media: &MediaDescription) -> impl Iterator<Item = &str> {
    media
        .attributes
        .iter()
        .filter(|attribute| attribute.name == "candidate")
        .filter_map(|attribute| attribute.value.as_deref())
}

fn raw_candidate_component(value: &str) -> Option<ComponentId> {
    let component = value.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    ComponentId::new(component)
}

fn is_weaker_protocol(protocol: &str) -> bool {
    matches!(protocol, "RTP/AVP" | "RTP/SAVP" | "UDP/TLS/RTP/SAVP")
}

fn is_mux_placeholder(value: Option<&str>) -> bool {
    matches!(
        value,
        Some("9 IN IP4 0.0.0.0" | "9 IN IP6 ::" | "9 IN IP6 0:0:0:0:0:0:0:0")
    )
}
