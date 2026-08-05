//! Bounded, versioned persistence for confirmed-dialog protocol state.
//!
//! A snapshot is not a serialized [`crate::Call`]. It deliberately excludes every socket,
//! transaction, task, clock instant, credential and media key. The host supplies fresh runtime
//! resources through [`DialogRestoreContext`] and this module validates the complete attachment
//! before [`crate::Call::restore_dialog`] publishes a call.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_media::{Codec, MediaSession};
use sipx_sdp::{Direction, RtcpMode};
use sipx_sip::Uri;
use sipx_sip::headers::Address;
use sipx_transport::{Handle, Target};
use tokio::time::Instant;

use crate::dialog::{DialogId, Role};
use crate::{Codecs, Keying, MediaAddress, MediaPolicy, MediaProfile, NegotiatedKeying};

const MAGIC: &[u8; 4] = b"SXD1";
const VERSION: u16 = 1;
const FLAG_CALLEE: u16 = 1 << 0;
const FLAG_PROTECTED: u16 = 1 << 1;
const FLAG_SESSION: u16 = 1 << 2;
const FLAG_PEER_UPDATE: u16 = 1 << 3;
const KNOWN_FLAGS: u16 = FLAG_CALLEE | FLAG_PROTECTED | FLAG_SESSION | FLAG_PEER_UPDATE;

/// The maximum accepted encoded snapshot size.
pub const MAX_SNAPSHOT_BYTES: usize = 262_144;
/// The maximum sum of all variable-length fields.
pub const MAX_VARIABLE_BYTES: usize = 131_072;
/// The maximum Call-ID or tag size.
pub const MAX_ID_BYTES: usize = 1_024;
/// The maximum party, target, or route size.
pub const MAX_FIELD_BYTES: usize = 8_192;
/// The maximum number of routes retained by one dialog.
pub const MAX_ROUTES: usize = 64;

/// Why a live call cannot be captured without losing active protocol work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DialogNotQuiescent {
    /// The call has already ended.
    #[error("the call has ended")]
    Ended,
    /// The dialog-forming 2xx is still awaiting its ACK.
    #[error("the dialog-forming response is still awaiting ACK")]
    AwaitingAck,
    /// An offer, answer, or UPDATE transaction remains outstanding.
    #[error("an offer, answer, or UPDATE remains outstanding")]
    OfferAnswer,
    /// A received or originated transfer usage remains attached.
    #[error("a transfer usage remains attached")]
    Transfer,
    /// Version one cannot preserve a live ICE generation or its credentials.
    #[error("a live ICE generation cannot be snapshotted")]
    Ice,
}

/// The explicit timer input a host must drive before restoration can continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DialogSessionAction {
    /// This side must originate the negotiated refresh.
    Refresh,
    /// The peer's refresh did not arrive and the session must expire.
    Expire,
}

/// A typed refusal from snapshot capture, decoding, or runtime attachment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DialogPersistenceError {
    /// The fixed format marker was absent.
    #[error("dialog snapshot has an invalid magic value")]
    InvalidMagic,
    /// The schema version is not implemented by this build.
    #[error("dialog snapshot schema version {0} is unsupported")]
    UnsupportedVersion(u16),
    /// An encoded flag that version one does not assign was set.
    #[error("dialog snapshot has reserved flags set: {0:#06x}")]
    ReservedFlags(u16),
    /// The input ended before one complete field was available.
    #[error("dialog snapshot is truncated in {field}")]
    Truncated {
        /// The field being read.
        field: &'static str,
    },
    /// One field exceeded its independently enforced limit.
    #[error("dialog snapshot field {field} is {len} bytes; the limit is {max}")]
    FieldTooLarge {
        /// The field being read.
        field: &'static str,
        /// The declared or observed size.
        len: usize,
        /// The accepted size.
        max: usize,
    },
    /// The complete input exceeded the format ceiling.
    #[error("dialog snapshot is {len} bytes; the limit is {max}")]
    InputTooLarge {
        /// The observed input size.
        len: usize,
        /// The accepted size.
        max: usize,
    },
    /// Individually bounded fields exceeded their aggregate ceiling.
    #[error("dialog snapshot variable fields exceed {MAX_VARIABLE_BYTES} bytes")]
    VariableDataTooLarge,
    /// A text field was not UTF-8.
    #[error("dialog snapshot field {field} is not UTF-8")]
    InvalidUtf8 {
        /// The field being read.
        field: &'static str,
    },
    /// A field did not satisfy its named protocol invariant.
    #[error("dialog snapshot has an invalid {field}")]
    InvalidValue {
        /// The field that failed validation.
        field: &'static str,
    },
    /// An optional marker had a non-canonical value other than zero or one.
    #[error("dialog snapshot has a non-canonical presence marker for {field}")]
    NonCanonicalPresence {
        /// The optional field being read.
        field: &'static str,
    },
    /// The two independently generated dialog tags are identical.
    #[error("dialog snapshot repeats the same local and remote tag")]
    DuplicateTags,
    /// The route count exceeded the fixed collection bound.
    #[error("dialog snapshot has {count} routes; the limit is {MAX_ROUTES}")]
    TooManyRoutes {
        /// The declared route count.
        count: usize,
    },
    /// Bytes remained after the last version-one field.
    #[error("dialog snapshot has trailing bytes")]
    TrailingBytes,
    /// A local request cannot advance the retained sequence number.
    #[error("dialog snapshot local CSeq is exhausted")]
    CseqExhausted,
    /// A live call still owns runtime-only protocol work.
    #[error("dialog is not quiescent: {0}")]
    NotQuiescent(DialogNotQuiescent),
    /// The captured or restored session deadline is already due.
    #[error("dialog session action is already due: {0:?}")]
    SessionActionDue(DialogSessionAction),
    /// Session duration values contradict their negotiated interval.
    #[error("dialog snapshot session timer values are contradictory")]
    SessionContradiction,
    /// A positive remaining duration cannot be represented by the injected clock.
    #[error("dialog session deadline overflows the injected clock")]
    ClockOverflow,
    /// Protected dialog state was attached to a clear signalling target.
    #[error("protected dialog state cannot be restored through clear signalling")]
    SecurityDowngrade,
    /// The fresh media keying differs from the retained negotiated class.
    #[error("fresh media security does not match the dialog snapshot")]
    MediaSecurityMismatch,
    /// One non-secret media wire fact differs from the snapshot.
    #[error("fresh media does not match the snapshot field {field}")]
    MediaContractMismatch {
        /// The mismatching fact.
        field: &'static str,
    },
    /// An RTP payload type cannot be represented by the seven-bit wire field.
    #[error("dialog snapshot {field} {value} exceeds the RTP payload type range 0..=127")]
    PayloadTypeOutOfRange {
        /// The audio or DTMF payload field.
        field: &'static str,
        /// The refused unmasked value.
        value: u8,
    },
    /// This fresh runtime attachment was already consumed by a successful restore.
    #[error("dialog restore context is already attached to a call")]
    ContextAlreadyAttached,
    /// Version one cannot run a codec represented by these bytes.
    #[error("dialog snapshot codec id {0} is unavailable in this build")]
    UnsupportedCodec(u8),
    /// The fresh media address would be unusable in SDP.
    #[error("the fresh advertised media address must not be unspecified")]
    UnspecifiedMediaAddress,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionSnapshot {
    pub(crate) interval: Duration,
    pub(crate) we_refresh: bool,
    pub(crate) remaining: Duration,
}

/// Immutable version-one facts needed to continue a confirmed dialog.
#[derive(Clone)]
pub struct DialogSnapshot {
    role: Role,
    id: DialogId,
    local_party: String,
    remote_party: String,
    remote_target: Uri,
    route_set: Vec<String>,
    local_cseq: u32,
    remote_cseq: Option<u32>,
    protected_signalling: bool,
    media_keying: NegotiatedKeying,
    media_profile: MediaProfile,
    codecs: Codecs,
    codec: Codec,
    payload_type: u8,
    dtmf_payload_type: Option<u8>,
    rtcp_mode: RtcpMode,
    hold: Direction,
    peer_allows_update: bool,
    session: Option<SessionSnapshot>,
}

impl fmt::Debug for DialogSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DialogSnapshot")
            .field("version", &VERSION)
            .field("role", &self.role)
            .field("call_id_bytes", &self.id.call_id.len())
            .field("local_tag_bytes", &self.id.local_tag.len())
            .field("remote_tag_bytes", &self.id.remote_tag.len())
            .field("local_party_bytes", &self.local_party.len())
            .field("remote_party_bytes", &self.remote_party.len())
            .field("remote_target_bytes", &self.remote_target.to_bytes().len())
            .field("routes", &self.route_set.len())
            .field("local_cseq", &self.local_cseq)
            .field("remote_cseq", &self.remote_cseq)
            .field("protected_signalling", &self.protected_signalling)
            .field("media_keying", &self.media_keying)
            .field("media_profile", &self.media_profile)
            .field("codecs", &self.codecs)
            .field("codec", &self.codec)
            .field("payload_type", &self.payload_type)
            .field("dtmf_payload_type", &self.dtmf_payload_type)
            .field("rtcp_mode", &self.rtcp_mode)
            .field("hold", &self.hold)
            .field("peer_allows_update", &self.peer_allows_update)
            .field("has_session_timer", &self.session.is_some())
            .finish()
    }
}

impl DialogSnapshot {
    /// The schema version emitted by this build.
    #[must_use]
    pub const fn version(&self) -> u16 {
        VERSION
    }

    /// Which dialog role was captured.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// The retained dialog identifier.
    #[must_use]
    pub const fn dialog_id(&self) -> &DialogId {
        &self.id
    }

    /// The validated local party value, without the separately retained dialog tag.
    #[must_use]
    pub fn local_party(&self) -> &str {
        &self.local_party
    }

    /// The validated remote party value, without the separately retained dialog tag.
    #[must_use]
    pub fn remote_party(&self) -> &str {
        &self.remote_party
    }

    /// The last locally used in-dialog sequence number.
    #[must_use]
    pub const fn local_cseq(&self) -> u32 {
        self.local_cseq
    }

    /// The greatest accepted remote sequence number.
    #[must_use]
    pub const fn remote_cseq(&self) -> Option<u32> {
        self.remote_cseq
    }

    /// The retained remote target from the latest target refresh.
    #[must_use]
    pub const fn remote_target(&self) -> &Uri {
        &self.remote_target
    }

    /// The route set in send order.
    #[must_use]
    pub fn route_set(&self) -> &[String] {
        &self.route_set
    }

    /// Whether the captured signalling path was protected.
    #[must_use]
    pub const fn protected_signalling(&self) -> bool {
        self.protected_signalling
    }

    /// The negotiated media keying class. No key bytes are retained.
    #[must_use]
    pub const fn media_keying(&self) -> NegotiatedKeying {
        self.media_keying
    }

    /// The retained named media profile.
    #[must_use]
    pub const fn media_profile(&self) -> MediaProfile {
        self.media_profile
    }

    /// The exact codec policy used for later offers.
    #[must_use]
    pub const fn codecs(&self) -> Codecs {
        self.codecs
    }

    /// The negotiated audio codec.
    #[must_use]
    pub const fn codec(&self) -> Codec {
        self.codec
    }

    /// The RTP payload type carrying the negotiated codec.
    #[must_use]
    pub const fn payload_type(&self) -> u8 {
        self.payload_type
    }

    /// The dynamic RTP payload type carrying telephone events, when enabled.
    #[must_use]
    pub const fn dtmf_payload_type(&self) -> Option<u8> {
        self.dtmf_payload_type
    }

    /// The negotiated RTCP socket shape.
    #[must_use]
    pub const fn rtcp_mode(&self) -> RtcpMode {
        self.rtcp_mode
    }

    /// The most recently negotiated media direction.
    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.hold
    }

    /// Whether the peer advertised support for UPDATE.
    #[must_use]
    pub const fn peer_allows_update(&self) -> bool {
        self.peer_allows_update
    }

    /// Negotiated interval, local refresher role, and remaining duration at capture.
    #[must_use]
    pub const fn session_timer(&self) -> Option<(Duration, bool, Duration)> {
        match self.session {
            Some(session) => Some((session.interval, session.we_refresh, session.remaining)),
            None => None,
        }
    }

    /// Encode this value in the deterministic version-one binary format.
    ///
    /// Values can only be obtained from validated capture or decoding, so this operation is
    /// infallible and never writes a partial result to a caller-owned buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(self.encoded_len().min(MAX_SNAPSHOT_BYTES));
        encoded.extend_from_slice(MAGIC);
        put_u16(&mut encoded, VERSION);
        let mut flags = 0u16;
        if self.role == Role::Callee {
            flags |= FLAG_CALLEE;
        }
        if self.protected_signalling {
            flags |= FLAG_PROTECTED;
        }
        if self.session.is_some() {
            flags |= FLAG_SESSION;
        }
        if self.peer_allows_update {
            flags |= FLAG_PEER_UPDATE;
        }
        put_u16(&mut encoded, flags);
        put_bytes(&mut encoded, &self.id.call_id);
        put_bytes(&mut encoded, &self.id.local_tag);
        put_bytes(&mut encoded, &self.id.remote_tag);
        put_bytes(&mut encoded, self.local_party.as_bytes());
        put_bytes(&mut encoded, self.remote_party.as_bytes());
        put_bytes(&mut encoded, &self.remote_target.to_bytes());
        put_u16(
            &mut encoded,
            u16::try_from(self.route_set.len()).unwrap_or(u16::MAX),
        );
        for route in &self.route_set {
            put_bytes(&mut encoded, route.as_bytes());
        }
        put_u32(&mut encoded, self.local_cseq);
        put_optional_u32(&mut encoded, self.remote_cseq);
        encoded.push(keying_id(self.media_keying));
        encoded.push(profile_id(self.media_profile));
        let preferences: Vec<_> = self.codecs.preferences().collect();
        encoded.push(u8::try_from(preferences.len()).unwrap_or(u8::MAX));
        for preference in preferences {
            encoded.push(preference_id(preference));
        }
        encoded.push(codec_id(self.codec));
        encoded.push(self.payload_type);
        put_optional_u8(&mut encoded, self.dtmf_payload_type);
        encoded.push(rtcp_id(self.rtcp_mode));
        encoded.push(direction_id(self.hold));
        // Version one only admits an idle offer state. Writing the state explicitly makes a
        // future state additive without letting an old decoder mistake it for idle.
        encoded.push(0);
        if let Some(session) = self.session {
            put_u64(&mut encoded, session.interval.as_secs());
            encoded.push(u8::from(session.we_refresh));
            let nanos = u64::try_from(session.remaining.as_nanos()).unwrap_or(u64::MAX);
            put_u64(&mut encoded, nanos);
        }
        encoded
    }

    /// Decode and validate one complete snapshot.
    ///
    /// The total input bound is applied before any field is read. Per-field and aggregate bounds
    /// are then checked before each allocation, so hostile length prefixes cannot request their
    /// declared amount of memory.
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered read keeps the canonical field order and pre-allocation checks auditable"
    )]
    pub fn decode(input: &[u8]) -> Result<Self, DialogPersistenceError> {
        if input.len() > MAX_SNAPSHOT_BYTES {
            return Err(DialogPersistenceError::InputTooLarge {
                len: input.len(),
                max: MAX_SNAPSHOT_BYTES,
            });
        }
        let mut reader = Reader::new(input);
        if reader.exact(4, "magic")? != MAGIC {
            return Err(DialogPersistenceError::InvalidMagic);
        }
        let version = reader.u16("version")?;
        if version != VERSION {
            return Err(DialogPersistenceError::UnsupportedVersion(version));
        }
        let flags = reader.u16("flags")?;
        if flags & !KNOWN_FLAGS != 0 {
            return Err(DialogPersistenceError::ReservedFlags(flags & !KNOWN_FLAGS));
        }
        let role = if flags & FLAG_CALLEE == 0 {
            Role::Caller
        } else {
            Role::Callee
        };
        let call_id = reader.bytes("Call-ID", MAX_ID_BYTES)?;
        let local_tag = reader.bytes("local tag", MAX_ID_BYTES)?;
        let remote_tag = reader.bytes("remote tag", MAX_ID_BYTES)?;
        let local_party = reader.string("local party", MAX_FIELD_BYTES)?;
        let remote_party = reader.string("remote party", MAX_FIELD_BYTES)?;
        let target_bytes = reader.bytes("remote target", MAX_FIELD_BYTES)?;
        let remote_target = Uri::parse(Bytes::from(target_bytes)).map_err(|_| {
            DialogPersistenceError::InvalidValue {
                field: "remote target",
            }
        })?;
        let route_count = usize::from(reader.u16("route count")?);
        if route_count > MAX_ROUTES {
            return Err(DialogPersistenceError::TooManyRoutes { count: route_count });
        }
        let mut route_set = Vec::with_capacity(route_count);
        for _ in 0..route_count {
            route_set.push(reader.string("route", MAX_FIELD_BYTES)?);
        }
        let local_cseq = reader.u32("local CSeq")?;
        let remote_cseq = reader.optional_u32("remote CSeq")?;
        let media_keying = decode_keying(reader.u8("media keying")?)?;
        let media_profile = decode_profile(reader.u8("media profile")?)?;
        let preference_count = usize::from(reader.u8("codec preference count")?);
        if preference_count == 0 || preference_count > 3 {
            return Err(DialogPersistenceError::InvalidValue {
                field: "codec preference count",
            });
        }
        let mut preferences = Vec::with_capacity(preference_count);
        for _ in 0..preference_count {
            preferences.push(decode_preference(reader.u8("codec preference")?)?);
        }
        let codecs =
            Codecs::ordered(&preferences).map_err(|_| DialogPersistenceError::InvalidValue {
                field: "codec preferences",
            })?;
        let codec = decode_codec(reader.u8("codec")?)?;
        let payload_type = reader.u8("payload type")?;
        let dtmf_payload_type = reader.optional_u8("DTMF payload type")?;
        let rtcp_mode = decode_rtcp(reader.u8("RTCP mode")?)?;
        let hold = decode_direction(reader.u8("hold direction")?)?;
        if reader.u8("offer state")? != 0 {
            return Err(DialogPersistenceError::InvalidValue {
                field: "offer state",
            });
        }
        let session = if flags & FLAG_SESSION == 0 {
            None
        } else {
            let interval = Duration::from_secs(reader.u64("session interval")?);
            let we_refresh = match reader.u8("session refresher")? {
                0 => false,
                1 => true,
                _ => {
                    return Err(DialogPersistenceError::NonCanonicalPresence {
                        field: "session refresher",
                    });
                }
            };
            let remaining = Duration::from_nanos(reader.u64("session remaining")?);
            Some(SessionSnapshot {
                interval,
                we_refresh,
                remaining,
            })
        };
        if !reader.is_empty() {
            return Err(DialogPersistenceError::TrailingBytes);
        }
        let snapshot = Self {
            role,
            id: DialogId {
                call_id,
                local_tag,
                remote_tag,
            },
            local_party,
            remote_party,
            remote_target,
            route_set,
            local_cseq,
            remote_cseq,
            protected_signalling: flags & FLAG_PROTECTED != 0,
            media_keying,
            media_profile,
            codecs,
            codec,
            payload_type,
            dtmf_payload_type,
            rtcp_mode,
            hold,
            peer_allows_update: flags & FLAG_PEER_UPDATE != 0,
            session,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(crate) fn from_parts(parts: SnapshotParts) -> Result<Self, DialogPersistenceError> {
        let snapshot = Self {
            role: parts.role,
            id: parts.id,
            local_party: parts.local_party,
            remote_party: parts.remote_party,
            remote_target: parts.remote_target,
            route_set: parts.route_set,
            local_cseq: parts.local_cseq,
            remote_cseq: parts.remote_cseq,
            protected_signalling: parts.protected_signalling,
            media_keying: parts.media_keying,
            media_profile: parts.media_profile,
            codecs: parts.codecs,
            codec: parts.codec,
            payload_type: parts.payload_type,
            dtmf_payload_type: parts.dtmf_payload_type,
            rtcp_mode: parts.rtcp_mode,
            hold: parts.hold,
            peer_allows_update: parts.peer_allows_update,
            session: parts.session,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(crate) fn validate_restore(
        &self,
        context: &DialogRestoreContext,
    ) -> Result<Option<(Duration, bool, Instant)>, DialogPersistenceError> {
        self.validate()?;
        if self.protected_signalling && !context.target.transport.is_secure() {
            return Err(DialogPersistenceError::SecurityDowngrade);
        }
        if requires_secure_route(&self.remote_target, &self.route_set)?
            && !context.target.transport.is_secure()
        {
            return Err(DialogPersistenceError::SecurityDowngrade);
        }
        validate_media(self, context)?;
        match self.session {
            None => Ok(None),
            Some(session) if session.remaining.is_zero() => {
                Err(DialogPersistenceError::SessionActionDue(session.action()))
            }
            Some(session) => {
                let deadline = context
                    .now
                    .checked_add(session.remaining)
                    .ok_or(DialogPersistenceError::ClockOverflow)?;
                Ok(Some((session.interval, session.we_refresh, deadline)))
            }
        }
    }

    pub(crate) fn dialog(&self) -> crate::Dialog {
        crate::Dialog {
            role: self.role,
            id: self.id.clone(),
            local_uri: self.local_party.clone(),
            remote_uri: self.remote_party.clone(),
            remote_target: self.remote_target.clone(),
            local_cseq: self.local_cseq,
            remote_cseq: self.remote_cseq,
            route_set: self.route_set.clone(),
        }
    }

    pub(crate) const fn media_profile_value(&self) -> MediaProfile {
        self.media_profile
    }

    pub(crate) const fn codecs_value(&self) -> Codecs {
        self.codecs
    }

    pub(crate) const fn negotiated(&self, remote: SocketAddr) -> crate::call::Negotiated {
        crate::call::Negotiated {
            remote,
            codec: self.codec,
            payload_type: Some(self.payload_type),
            dtmf: self.dtmf_payload_type,
            rtcp_mode: self.rtcp_mode,
        }
    }

    pub(crate) const fn peer_allows_update_value(&self) -> bool {
        self.peer_allows_update
    }

    fn encoded_len(&self) -> usize {
        64usize
            .saturating_add(self.variable_len())
            .saturating_add(self.route_set.len().saturating_mul(4))
    }

    fn variable_len(&self) -> usize {
        self.id
            .call_id
            .len()
            .saturating_add(self.id.local_tag.len())
            .saturating_add(self.id.remote_tag.len())
            .saturating_add(self.local_party.len())
            .saturating_add(self.remote_party.len())
            .saturating_add(self.remote_target.to_bytes().len())
            .saturating_add(
                self.route_set
                    .iter()
                    .fold(0usize, |sum, route| sum.saturating_add(route.len())),
            )
    }

    fn validate(&self) -> Result<(), DialogPersistenceError> {
        validate_bytes("Call-ID", &self.id.call_id, MAX_ID_BYTES, false)?;
        if self.id.call_id.iter().any(|byte| *byte <= 0x20) {
            return Err(DialogPersistenceError::InvalidValue { field: "Call-ID" });
        }
        validate_token("local tag", &self.id.local_tag)?;
        validate_token("remote tag", &self.id.remote_tag)?;
        if self.id.local_tag == self.id.remote_tag {
            return Err(DialogPersistenceError::DuplicateTags);
        }
        validate_party("local party", &self.local_party)?;
        validate_party("remote party", &self.remote_party)?;
        validate_uri("remote target", &self.remote_target)?;
        if self.route_set.len() > MAX_ROUTES {
            return Err(DialogPersistenceError::TooManyRoutes {
                count: self.route_set.len(),
            });
        }
        for route in &self.route_set {
            validate_route(route)?;
        }
        if self.variable_len() > MAX_VARIABLE_BYTES {
            return Err(DialogPersistenceError::VariableDataTooLarge);
        }
        if self.local_cseq == u32::MAX {
            return Err(DialogPersistenceError::CseqExhausted);
        }
        if requires_secure_route(&self.remote_target, &self.route_set)?
            && !self.protected_signalling
        {
            return Err(DialogPersistenceError::InvalidValue {
                field: "SIPS security state",
            });
        }
        if self.media_keying == NegotiatedKeying::Sdes && !self.protected_signalling {
            return Err(DialogPersistenceError::InvalidValue {
                field: "SDES signalling security",
            });
        }
        if !self.codecs.carries(self.codec) {
            return Err(DialogPersistenceError::InvalidValue {
                field: "negotiated codec selection",
            });
        }
        validate_payload_type("payload type", self.payload_type)?;
        if let Some(payload_type) = self.dtmf_payload_type {
            validate_payload_type("DTMF payload type", payload_type)?;
        }
        if self.dtmf_payload_type == Some(self.payload_type) {
            return Err(DialogPersistenceError::InvalidValue {
                field: "DTMF payload type",
            });
        }
        if self.rtcp_mode == RtcpMode::Mux
            && [Some(self.payload_type), self.dtmf_payload_type]
                .into_iter()
                .flatten()
                .any(|payload| (64..=95).contains(&payload))
        {
            return Err(DialogPersistenceError::InvalidValue {
                field: "RTCP-mux payload type",
            });
        }
        if self.media_profile != MediaProfile::Standard {
            return Err(DialogPersistenceError::InvalidValue {
                field: "restorable media profile",
            });
        }
        if let Some(session) = self.session
            && (session.interval < sipx_sip::session::ABSOLUTE_MIN_INTERVAL
                || session.remaining > session.interval)
        {
            return Err(DialogPersistenceError::SessionContradiction);
        }
        if self.encoded_len() > MAX_SNAPSHOT_BYTES {
            return Err(DialogPersistenceError::InputTooLarge {
                len: self.encoded_len(),
                max: MAX_SNAPSHOT_BYTES,
            });
        }
        Ok(())
    }
}

impl SessionSnapshot {
    pub(crate) const fn action(self) -> DialogSessionAction {
        if self.we_refresh {
            DialogSessionAction::Refresh
        } else {
            DialogSessionAction::Expire
        }
    }
}

pub(crate) struct SnapshotParts {
    pub(crate) role: Role,
    pub(crate) id: DialogId,
    pub(crate) local_party: String,
    pub(crate) remote_party: String,
    pub(crate) remote_target: Uri,
    pub(crate) route_set: Vec<String>,
    pub(crate) local_cseq: u32,
    pub(crate) remote_cseq: Option<u32>,
    pub(crate) protected_signalling: bool,
    pub(crate) media_keying: NegotiatedKeying,
    pub(crate) media_profile: MediaProfile,
    pub(crate) codecs: Codecs,
    pub(crate) codec: Codec,
    pub(crate) payload_type: u8,
    pub(crate) dtmf_payload_type: Option<u8>,
    pub(crate) rtcp_mode: RtcpMode,
    pub(crate) hold: Direction,
    pub(crate) peer_allows_update: bool,
    pub(crate) session: Option<SessionSnapshot>,
}

/// Fresh runtime resources and policy required to attach a decoded dialog.
///
/// The context is borrowed during restoration. A refusal therefore does not consume or stop its
/// media session, mutate its endpoint, or create a transaction. The `now` value is explicit so no
/// persisted process-local instant is ever interpreted in a new process.
pub struct DialogRestoreContext {
    pub(crate) endpoint: Handle,
    pub(crate) target: Target,
    pub(crate) media: Arc<MediaSession>,
    pub(crate) media_address: MediaAddress,
    pub(crate) remote_media: SocketAddr,
    pub(crate) policy: MediaPolicy,
    pub(crate) direction: Direction,
    pub(crate) now: Instant,
    claimed: AtomicBool,
}

impl DialogRestoreContext {
    /// Describe already-created runtime resources without starting any new work.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: Handle,
        target: Target,
        media: Arc<MediaSession>,
        media_address: MediaAddress,
        remote_media: SocketAddr,
        policy: MediaPolicy,
        direction: Direction,
        now: Instant,
    ) -> Self {
        Self {
            endpoint,
            target,
            media,
            media_address,
            remote_media,
            policy,
            direction,
            now,
            claimed: AtomicBool::new(false),
        }
    }

    pub(crate) fn claim(&self) -> Result<(), DialogPersistenceError> {
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| DialogPersistenceError::ContextAlreadyAttached)
    }
}

impl fmt::Debug for DialogRestoreContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DialogRestoreContext")
            .field("endpoint_local", &self.endpoint.local_addr())
            .field("target", &self.target)
            .field("media_local", &self.media.local_addr())
            .field("media_advertised", &self.media_address.advertised())
            .field("media_bind", &self.media_address.bind())
            .field("remote_media", &self.remote_media)
            .field("policy", &self.policy)
            .field("direction", &self.direction)
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

fn validate_media(
    snapshot: &DialogSnapshot,
    context: &DialogRestoreContext,
) -> Result<(), DialogPersistenceError> {
    if context.media_address.advertised().is_unspecified() {
        return Err(DialogPersistenceError::UnspecifiedMediaAddress);
    }
    if context.remote_media.ip().is_unspecified() || context.remote_media.port() == 0 {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "remote media address",
        });
    }
    if context.media.local_addr().ip() != context.media_address.bind() {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "media bind address",
        });
    }
    if context.media.runs_ice() || context.policy.ice != crate::IcePolicy::Disabled {
        return Err(DialogPersistenceError::MediaContractMismatch { field: "ICE state" });
    }
    if context.policy.profile != snapshot.media_profile {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "media profile",
        });
    }
    if context.policy.codecs != snapshot.codecs {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "codec policy",
        });
    }
    if context.direction != snapshot.hold {
        return Err(DialogPersistenceError::MediaContractMismatch { field: "direction" });
    }
    let expected_keying = match context.policy.keying {
        Keying::Plain => NegotiatedKeying::Plain,
        Keying::Sdes => NegotiatedKeying::Sdes,
        Keying::DtlsSrtp => NegotiatedKeying::DtlsSrtp,
        Keying::Auto => return Err(DialogPersistenceError::MediaSecurityMismatch),
    };
    if expected_keying != snapshot.media_keying
        || context.media.is_encrypted() != (snapshot.media_keying != NegotiatedKeying::Plain)
    {
        return Err(DialogPersistenceError::MediaSecurityMismatch);
    }
    if snapshot.media_keying == NegotiatedKeying::Sdes && !context.target.transport.is_secure() {
        return Err(DialogPersistenceError::SecurityDowngrade);
    }
    if context.media.codec() != snapshot.codec {
        return Err(DialogPersistenceError::MediaContractMismatch { field: "codec" });
    }
    if context.media.wire_payload_type() != snapshot.payload_type {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "payload type",
        });
    }
    if context.media.dtmf_payload_type() != snapshot.dtmf_payload_type {
        return Err(DialogPersistenceError::MediaContractMismatch {
            field: "DTMF payload type",
        });
    }
    if context.media.rtcp_mode() != snapshot.rtcp_mode {
        return Err(DialogPersistenceError::MediaContractMismatch { field: "RTCP mode" });
    }
    Ok(())
}

fn validate_payload_type(field: &'static str, value: u8) -> Result<(), DialogPersistenceError> {
    if value > 0x7f {
        return Err(DialogPersistenceError::PayloadTypeOutOfRange { field, value });
    }
    Ok(())
}

fn validate_bytes(
    field: &'static str,
    value: &[u8],
    max: usize,
    allow_empty: bool,
) -> Result<(), DialogPersistenceError> {
    if value.len() > max {
        return Err(DialogPersistenceError::FieldTooLarge {
            field,
            len: value.len(),
            max,
        });
    }
    if !allow_empty && value.is_empty() {
        return Err(DialogPersistenceError::InvalidValue { field });
    }
    if !value.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        return Err(DialogPersistenceError::InvalidValue { field });
    }
    Ok(())
}

fn validate_token(field: &'static str, value: &[u8]) -> Result<(), DialogPersistenceError> {
    validate_bytes(field, value, MAX_ID_BYTES, false)?;
    if !value.iter().copied().all(is_token_char) {
        return Err(DialogPersistenceError::InvalidValue { field });
    }
    Ok(())
}

fn is_token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
        )
}

fn validate_party(field: &'static str, value: &str) -> Result<(), DialogPersistenceError> {
    validate_bytes(field, value.as_bytes(), MAX_FIELD_BYTES, false)?;
    let address = Address::parse(value.as_bytes(), field)
        .map_err(|_| DialogPersistenceError::InvalidValue { field })?;
    if address.tag().is_some() || address.uri.password().is_some() || !address.uri.scheme().is_sip()
    {
        return Err(DialogPersistenceError::InvalidValue { field });
    }
    Ok(())
}

fn validate_uri(field: &'static str, value: &Uri) -> Result<(), DialogPersistenceError> {
    let bytes = value.to_bytes();
    validate_bytes(field, &bytes, MAX_FIELD_BYTES, false)?;
    if !value.scheme().is_sip() || value.password().is_some() {
        return Err(DialogPersistenceError::InvalidValue { field });
    }
    Ok(())
}

fn validate_route(value: &str) -> Result<(), DialogPersistenceError> {
    validate_bytes("route", value.as_bytes(), MAX_FIELD_BYTES, false)?;
    let route = Address::parse(value.as_bytes(), "Route")
        .map_err(|_| DialogPersistenceError::InvalidValue { field: "route" })?;
    if !route.uri.scheme().is_sip() || route.uri.password().is_some() {
        return Err(DialogPersistenceError::InvalidValue { field: "route" });
    }
    Ok(())
}

fn requires_secure_route(target: &Uri, routes: &[String]) -> Result<bool, DialogPersistenceError> {
    if target.scheme().is_secure() {
        return Ok(true);
    }
    for route in routes {
        let address = Address::parse(route.as_bytes(), "Route")
            .map_err(|_| DialogPersistenceError::InvalidValue { field: "route" })?;
        if address.uri.scheme().is_secure() {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Reader<'a> {
    remaining: &'a [u8],
    variable: usize,
}

impl<'a> Reader<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self {
            remaining: input,
            variable: 0,
        }
    }

    fn exact(
        &mut self,
        len: usize,
        field: &'static str,
    ) -> Result<&'a [u8], DialogPersistenceError> {
        let Some(value) = self.remaining.get(..len) else {
            return Err(DialogPersistenceError::Truncated { field });
        };
        self.remaining = self.remaining.get(len..).unwrap_or_default();
        Ok(value)
    }

    fn u8(&mut self, field: &'static str) -> Result<u8, DialogPersistenceError> {
        self.exact(1, field)?
            .first()
            .copied()
            .ok_or(DialogPersistenceError::Truncated { field })
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, DialogPersistenceError> {
        let value = self.exact(2, field)?;
        let octets: [u8; 2] = value
            .try_into()
            .map_err(|_| DialogPersistenceError::Truncated { field })?;
        Ok(u16::from_be_bytes(octets))
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, DialogPersistenceError> {
        let value = self.exact(4, field)?;
        let octets: [u8; 4] = value
            .try_into()
            .map_err(|_| DialogPersistenceError::Truncated { field })?;
        Ok(u32::from_be_bytes(octets))
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, DialogPersistenceError> {
        let value = self.exact(8, field)?;
        let octets: [u8; 8] = value
            .try_into()
            .map_err(|_| DialogPersistenceError::Truncated { field })?;
        Ok(u64::from_be_bytes(octets))
    }

    fn bytes(
        &mut self,
        field: &'static str,
        max: usize,
    ) -> Result<Vec<u8>, DialogPersistenceError> {
        let declared = usize::try_from(self.u32(field)?).map_err(|_| {
            DialogPersistenceError::FieldTooLarge {
                field,
                len: usize::MAX,
                max,
            }
        })?;
        if declared > max {
            return Err(DialogPersistenceError::FieldTooLarge {
                field,
                len: declared,
                max,
            });
        }
        let next_total = self
            .variable
            .checked_add(declared)
            .ok_or(DialogPersistenceError::VariableDataTooLarge)?;
        if next_total > MAX_VARIABLE_BYTES {
            return Err(DialogPersistenceError::VariableDataTooLarge);
        }
        let value = self.exact(declared, field)?.to_vec();
        self.variable = next_total;
        Ok(value)
    }

    fn string(
        &mut self,
        field: &'static str,
        max: usize,
    ) -> Result<String, DialogPersistenceError> {
        String::from_utf8(self.bytes(field, max)?)
            .map_err(|_| DialogPersistenceError::InvalidUtf8 { field })
    }

    fn optional_u8(&mut self, field: &'static str) -> Result<Option<u8>, DialogPersistenceError> {
        match self.u8(field)? {
            0 => Ok(None),
            1 => self.u8(field).map(Some),
            _ => Err(DialogPersistenceError::NonCanonicalPresence { field }),
        }
    }

    fn optional_u32(&mut self, field: &'static str) -> Result<Option<u32>, DialogPersistenceError> {
        match self.u8(field)? {
            0 => Ok(None),
            1 => self.u32(field).map(Some),
            _ => Err(DialogPersistenceError::NonCanonicalPresence { field }),
        }
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    put_u32(out, u32::try_from(value.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(value);
}

fn put_optional_u8(out: &mut Vec<u8>, value: Option<u8>) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            out.push(value);
        }
    }
}

fn put_optional_u32(out: &mut Vec<u8>, value: Option<u32>) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            put_u32(out, value);
        }
    }
}

const fn keying_id(value: NegotiatedKeying) -> u8 {
    match value {
        NegotiatedKeying::Plain => 0,
        NegotiatedKeying::Sdes => 1,
        NegotiatedKeying::DtlsSrtp => 2,
    }
}

fn decode_keying(value: u8) -> Result<NegotiatedKeying, DialogPersistenceError> {
    match value {
        0 => Ok(NegotiatedKeying::Plain),
        1 => Ok(NegotiatedKeying::Sdes),
        2 => Ok(NegotiatedKeying::DtlsSrtp),
        _ => Err(DialogPersistenceError::InvalidValue {
            field: "media keying",
        }),
    }
}

const fn profile_id(value: MediaProfile) -> u8 {
    match value {
        MediaProfile::Standard => 0,
        MediaProfile::BrowserAudio => 1,
    }
}

fn decode_profile(value: u8) -> Result<MediaProfile, DialogPersistenceError> {
    match value {
        0 => Ok(MediaProfile::Standard),
        1 => Ok(MediaProfile::BrowserAudio),
        _ => Err(DialogPersistenceError::InvalidValue {
            field: "media profile",
        }),
    }
}

const fn preference_id(value: crate::CodecPreference) -> u8 {
    match value {
        crate::CodecPreference::Pcmu => 0,
        crate::CodecPreference::Pcma => 1,
        crate::CodecPreference::Opus => 2,
    }
}

fn decode_preference(value: u8) -> Result<crate::CodecPreference, DialogPersistenceError> {
    match value {
        0 => Ok(crate::CodecPreference::Pcmu),
        1 => Ok(crate::CodecPreference::Pcma),
        2 => Ok(crate::CodecPreference::Opus),
        _ => Err(DialogPersistenceError::InvalidValue {
            field: "codec preference",
        }),
    }
}

const fn codec_id(value: Codec) -> u8 {
    match value {
        Codec::Pcmu => 0,
        Codec::Pcma => 1,
        #[cfg(feature = "opus")]
        Codec::Opus => 2,
    }
}

fn decode_codec(value: u8) -> Result<Codec, DialogPersistenceError> {
    match value {
        0 => Ok(Codec::Pcmu),
        1 => Ok(Codec::Pcma),
        #[cfg(feature = "opus")]
        2 => Ok(Codec::Opus),
        #[cfg(not(feature = "opus"))]
        2 => Err(DialogPersistenceError::UnsupportedCodec(2)),
        other => Err(DialogPersistenceError::UnsupportedCodec(other)),
    }
}

const fn rtcp_id(value: RtcpMode) -> u8 {
    match value {
        RtcpMode::Separate => 0,
        RtcpMode::Mux => 1,
    }
}

fn decode_rtcp(value: u8) -> Result<RtcpMode, DialogPersistenceError> {
    match value {
        0 => Ok(RtcpMode::Separate),
        1 => Ok(RtcpMode::Mux),
        _ => Err(DialogPersistenceError::InvalidValue { field: "RTCP mode" }),
    }
}

const fn direction_id(value: Direction) -> u8 {
    match value {
        Direction::SendRecv => 0,
        Direction::SendOnly => 1,
        Direction::RecvOnly => 2,
        Direction::Inactive => 3,
    }
}

fn decode_direction(value: u8) -> Result<Direction, DialogPersistenceError> {
    match value {
        0 => Ok(Direction::SendRecv),
        1 => Ok(Direction::SendOnly),
        2 => Ok(Direction::RecvOnly),
        3 => Ok(Direction::Inactive),
        _ => Err(DialogPersistenceError::InvalidValue {
            field: "hold direction",
        }),
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
    use super::*;

    fn uri(value: &str) -> Uri {
        Uri::parse(Bytes::copy_from_slice(value.as_bytes())).expect("fixture URI")
    }

    fn parts() -> SnapshotParts {
        SnapshotParts {
            role: Role::Caller,
            id: DialogId {
                call_id: b"persist-1@example.net".to_vec(),
                local_tag: b"lt".to_vec(),
                remote_tag: b"rt".to_vec(),
            },
            local_party: "Alice <sip:alice@example.net>".to_owned(),
            remote_party: "Bob <sip:bob@example.org>".to_owned(),
            remote_target: uri("sip:refreshed@192.0.2.20:5070"),
            route_set: vec![
                "<sip:first.example;lr>".to_owned(),
                "<sip:second.example;lr>".to_owned(),
            ],
            local_cseq: 41,
            remote_cseq: Some(9),
            protected_signalling: false,
            media_keying: NegotiatedKeying::Plain,
            media_profile: MediaProfile::Standard,
            codecs: Codecs::G711,
            codec: Codec::Pcmu,
            payload_type: 0,
            dtmf_payload_type: Some(101),
            rtcp_mode: RtcpMode::Mux,
            hold: Direction::SendRecv,
            peer_allows_update: true,
            session: None,
        }
    }

    fn fixture() -> DialogSnapshot {
        DialogSnapshot::from_parts(parts()).expect("valid fixture")
    }

    /// Return the payload range of one of the six leading length-prefixed values.
    fn leading_field(bytes: &[u8], ordinal: usize) -> std::ops::Range<usize> {
        let mut at = 8usize;
        for index in 0..=ordinal {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("length")) as usize;
            let range = at + 4..at + 4 + len;
            if index == ordinal {
                return range;
            }
            at = range.end;
        }
        panic!("field ordinal exists")
    }

    fn after_routes(bytes: &[u8]) -> usize {
        let target = leading_field(bytes, 5);
        let mut at = target.end;
        let count = u16::from_be_bytes(bytes[at..at + 2].try_into().expect("route count"));
        at += 2;
        for _ in 0..count {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("route length"));
            at += 4 + usize::try_from(len).expect("route length fits");
        }
        at
    }

    fn payload_offsets(bytes: &[u8]) -> (usize, usize) {
        let mut at = after_routes(bytes) + 4;
        let remote_cseq_present = bytes[at];
        at += 1 + if remote_cseq_present == 1 { 4 } else { 0 };
        at += 2;
        let preference_count = usize::from(bytes[at]);
        at += 1 + preference_count;
        at += 1;
        let payload_type = at;
        let dtmf_marker = payload_type + 1;
        assert_eq!(bytes[dtmf_marker], 1, "fixture carries a DTMF payload");
        (payload_type, dtmf_marker + 1)
    }

    #[test]
    fn dp1_is_canonical_and_preserves_the_complete_dialog_order() {
        let snapshot = fixture();
        let bytes = snapshot.encode();
        let decoded = DialogSnapshot::decode(&bytes).expect("decodes");
        assert_eq!(decoded.encode(), bytes);
        assert_eq!(decoded.role(), Role::Caller);
        assert_eq!(decoded.local_cseq(), 41);
        assert_eq!(decoded.remote_cseq(), Some(9));
        assert_eq!(
            decoded.remote_target().to_bytes(),
            snapshot.remote_target().to_bytes()
        );
        assert_eq!(
            decoded.route_set(),
            ["<sip:first.example;lr>", "<sip:second.example;lr>",]
        );
    }

    #[test]
    fn dp2_rejects_an_unknown_version_before_reading_variable_fields() {
        let mut bytes = fixture().encode();
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes());
        bytes.truncate(6);
        assert_eq!(
            DialogSnapshot::decode(&bytes).unwrap_err(),
            DialogPersistenceError::UnsupportedVersion(2)
        );
    }

    #[test]
    fn dp3_checks_declared_field_bounds_before_allocation_or_copy() {
        let mut oversized = fixture().encode();
        oversized[8..12].copy_from_slice(&u32::try_from(MAX_ID_BYTES + 1).unwrap().to_be_bytes());
        assert_eq!(
            DialogSnapshot::decode(&oversized).unwrap_err(),
            DialogPersistenceError::FieldTooLarge {
                field: "Call-ID",
                len: MAX_ID_BYTES + 1,
                max: MAX_ID_BYTES,
            }
        );

        let mut truncated = fixture().encode();
        truncated[8..12].copy_from_slice(&1000u32.to_be_bytes());
        assert_eq!(
            DialogSnapshot::decode(&truncated).unwrap_err(),
            DialogPersistenceError::Truncated { field: "Call-ID" }
        );

        let huge = vec![0u8; MAX_SNAPSHOT_BYTES + 1];
        assert_eq!(
            DialogSnapshot::decode(&huge).unwrap_err(),
            DialogPersistenceError::InputTooLarge {
                len: MAX_SNAPSHOT_BYTES + 1,
                max: MAX_SNAPSHOT_BYTES,
            }
        );
    }

    #[test]
    fn dp4_rejects_empty_duplicate_and_malformed_identity_fields() {
        let bytes = fixture().encode();

        let mut empty_tag = bytes.clone();
        let local = leading_field(&empty_tag, 1);
        empty_tag[local.start - 4..local.start].copy_from_slice(&0u32.to_be_bytes());
        empty_tag.drain(local);
        assert_eq!(
            DialogSnapshot::decode(&empty_tag).unwrap_err(),
            DialogPersistenceError::InvalidValue { field: "local tag" }
        );

        let mut duplicate = bytes.clone();
        let local = leading_field(&duplicate, 1);
        let remote = leading_field(&duplicate, 2);
        let local_value = duplicate[local].to_vec();
        duplicate[remote].copy_from_slice(&local_value);
        assert_eq!(
            DialogSnapshot::decode(&duplicate).unwrap_err(),
            DialogPersistenceError::DuplicateTags
        );

        let mut malformed_target = bytes;
        let target = leading_field(&malformed_target, 5);
        malformed_target[target.clone()].fill(b'x');
        assert_eq!(
            DialogSnapshot::decode(&malformed_target).unwrap_err(),
            DialogPersistenceError::InvalidValue {
                field: "remote target"
            }
        );
    }

    #[test]
    fn dp4_rejects_route_flag_presence_and_trailing_noncanonical_forms() {
        let bytes = fixture().encode();

        let mut too_many_routes = bytes.clone();
        let route_count = leading_field(&too_many_routes, 5).end;
        too_many_routes[route_count..route_count + 2]
            .copy_from_slice(&u16::try_from(MAX_ROUTES + 1).unwrap().to_be_bytes());
        assert_eq!(
            DialogSnapshot::decode(&too_many_routes).unwrap_err(),
            DialogPersistenceError::TooManyRoutes {
                count: MAX_ROUTES + 1
            }
        );

        let mut reserved = bytes.clone();
        reserved[6..8].copy_from_slice(&(1u16 << 15).to_be_bytes());
        assert_eq!(
            DialogSnapshot::decode(&reserved).unwrap_err(),
            DialogPersistenceError::ReservedFlags(1u16 << 15)
        );

        let mut noncanonical = bytes.clone();
        let remote_marker = after_routes(&noncanonical) + 4;
        noncanonical[remote_marker] = 2;
        assert_eq!(
            DialogSnapshot::decode(&noncanonical).unwrap_err(),
            DialogPersistenceError::NonCanonicalPresence {
                field: "remote CSeq"
            }
        );

        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(
            DialogSnapshot::decode(&trailing).unwrap_err(),
            DialogPersistenceError::TrailingBytes
        );
    }

    #[test]
    fn cseq_exhaustion_and_session_contradictions_are_typed() {
        let mut exhausted = fixture().encode();
        let cseq = after_routes(&exhausted);
        exhausted[cseq..cseq + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            DialogSnapshot::decode(&exhausted).unwrap_err(),
            DialogPersistenceError::CseqExhausted
        );

        let mut contradictory = parts();
        contradictory.session = Some(SessionSnapshot {
            interval: Duration::from_secs(90),
            we_refresh: true,
            remaining: Duration::from_secs(91),
        });
        assert!(matches!(
            DialogSnapshot::from_parts(contradictory),
            Err(DialogPersistenceError::SessionContradiction)
        ));
    }

    #[test]
    fn hostile_payload_types_outside_the_rtp_header_range_are_typed_refusals() {
        let canonical = fixture().encode();
        let (payload_type, dtmf_payload_type) = payload_offsets(&canonical);
        for (offset, field) in [
            (payload_type, "payload type"),
            (dtmf_payload_type, "DTMF payload type"),
        ] {
            for value in [128, u8::MAX] {
                let mut hostile = canonical.clone();
                hostile[offset] = value;
                assert_eq!(
                    DialogSnapshot::decode(&hostile).unwrap_err(),
                    DialogPersistenceError::PayloadTypeOutOfRange { field, value }
                );
            }
        }
    }

    #[test]
    fn every_hostile_prefix_is_a_value_and_never_a_panic() {
        let canonical = fixture().encode();
        for length in 0..canonical.len() {
            assert!(DialogSnapshot::decode(&canonical[..length]).is_err());
        }
        for offset in 0..canonical.len() {
            let mut hostile = canonical.clone();
            hostile[offset] ^= 0xff;
            let _ = DialogSnapshot::decode(&hostile);
        }
        for length in [0usize, 1, 7, 31, 255, 4096] {
            let hostile = vec![0xff; length];
            let _ = DialogSnapshot::decode(&hostile);
        }
    }

    #[test]
    fn debug_output_redacts_party_and_identifier_values() {
        let snapshot = fixture();
        let rendered = format!("{snapshot:?}");
        for secret in ["Alice", "Bob", "persist-1", "refreshed", "first.example"] {
            assert!(
                !rendered.contains(secret),
                "debug leaked {secret}: {rendered}"
            );
        }
        assert!(rendered.contains("call_id_bytes"));
    }

    #[test]
    fn password_bearing_uris_are_never_accepted_as_durable_protocol_facts() {
        let mut target = parts();
        target.remote_target = uri("sip:alice:credential@example.net");
        assert!(matches!(
            DialogSnapshot::from_parts(target),
            Err(DialogPersistenceError::InvalidValue {
                field: "remote target"
            })
        ));

        let mut party = parts();
        party.local_party = "<sip:alice:credential@example.net>".to_owned();
        assert!(matches!(
            DialogSnapshot::from_parts(party),
            Err(DialogPersistenceError::InvalidValue {
                field: "local party"
            })
        ));
    }

    #[tokio::test]
    async fn dp6_refuses_mismatched_keying_without_consuming_the_fresh_media() {
        let mut secure = parts();
        secure.protected_signalling = true;
        secure.media_keying = NegotiatedKeying::Sdes;
        let snapshot = DialogSnapshot::from_parts(secure).expect("secure snapshot");

        let (endpoint, _incoming) = sipx_transport::bind(sipx_transport::Config::new(
            "127.0.0.1:0".parse().expect("endpoint address"),
        ))
        .await
        .expect("endpoint binds");
        let remote: SocketAddr = "127.0.0.1:40000".parse().expect("media remote");
        let mut media_config = sipx_media::Config::new(remote, Codec::Pcmu);
        media_config.rtcp_mode = RtcpMode::Mux;
        let media = Arc::new(
            MediaSession::start("127.0.0.1:0".parse().expect("media bind"), media_config)
                .await
                .expect("plain media starts"),
        );
        let context = DialogRestoreContext::new(
            endpoint.clone(),
            Target::new(endpoint.local_addr(), sipx_transport::TransportKind::Tls),
            Arc::clone(&media),
            MediaAddress::new("127.0.0.1".parse().expect("media address")),
            remote,
            MediaPolicy::default().with_keying(Keying::Sdes),
            snapshot.direction(),
            Instant::now(),
        );
        assert_eq!(endpoint.outstanding().await.expect("outstanding"), 0);
        assert_eq!(
            snapshot.validate_restore(&context).unwrap_err(),
            DialogPersistenceError::MediaSecurityMismatch
        );
        assert_eq!(endpoint.outstanding().await.expect("outstanding"), 0);
        assert_eq!(Arc::strong_count(&media), 2);

        drop(context);
        drop(media);
        endpoint.shutdown().await;
    }
}
