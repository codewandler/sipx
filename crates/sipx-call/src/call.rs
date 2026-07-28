//! Establishing a call: INVITE with an SDP offer, media bound to the answer, and BYE.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use sipx_media::{Codec, MediaPort, MediaSession};
use sipx_sdp::{Capabilities, Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Method, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};

use crate::dialog::Dialog;
use crate::error::{Error, Result};

/// A call in progress.
#[derive(Debug)]
pub struct Call {
    /// The dialog it runs in.
    pub dialog: Dialog,
    /// The audio.
    pub media: MediaSession,
    endpoint: Handle,
    target: Target,
}

impl Call {
    /// The media session.
    #[must_use]
    pub fn media(&self) -> &MediaSession {
        &self.media
    }

    /// End the call.
    ///
    /// The media stops first. A BYE that is answered while audio is still flowing leaves the
    /// far end hearing a call it has already torn down.
    pub async fn hang_up(&mut self) -> Result<()> {
        self.media.stop();

        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();
        let bye = RequestBuilder::new(Method::Bye, self.dialog.remote_target.clone())
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Bye)?
            .max_forwards(70)
            .build();

        let mut responses = self.endpoint.send(bye, self.target).await?;
        // A BYE that is never answered still ends the call locally: the alternative is a call
        // that cannot be hung up because the far end has already gone.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }
}

/// Place a call.
///
/// The offer is made from a media session bound before the INVITE goes out, because the offer
/// has to name the port audio will arrive on — and only a bound socket knows it.
pub async fn dial(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    from: &str,
    media_address: IpAddr,
) -> Result<Call> {
    // The offer has to name the port audio will arrive on, and only a bound socket knows it.
    // So the port is bound now and the session started once the answer says where and in what.
    let port = MediaPort::bind(SocketAddr::new(media_address, 0))
        .await
        .map_err(Error::Io)?;

    let mut capabilities = Capabilities::g711(media_address, port.local_addr().port());
    capabilities.direction = Direction::SendRecv;
    let offer = offer_from(&capabilities);

    let call_id = format!("{}@sipx", sipx_ua::auth::new_cnonce());
    let tag = sipx_ua::auth::new_cnonce();
    let invite = RequestBuilder::new(Method::Invite, to.clone())
        .header(
            HeaderName::To,
            Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
        )?
        .header(HeaderName::From, Bytes::from(format!("{from};tag={tag}")))?
        .header(HeaderName::CallId, Bytes::from(call_id))?
        .cseq(1, &Method::Invite)?
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:sipx@{}>", endpoint.local_addr())),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .max_forwards(70)
        .body(Bytes::from(offer.to_string_sdp()))
        .build();

    let mut responses = endpoint.send(invite.clone(), target).await?;
    let response = responses.final_response().await.ok_or(Error::NoResponse)?;

    if !response.status.is_success() {
        return Err(Error::Rejected {
            status: response.status.code(),
            reason: String::from_utf8_lossy(&response.reason).into_owned(),
        });
    }

    let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    let (remote_addr, codec) = negotiated(&answer)?;

    let dialog = Dialog::from_response(&invite, &response).ok_or(Error::NoDialog)?;

    // RFC 3261 §13.2.2.4: the ACK for a 2xx is a separate transaction the TU must build, and
    // it goes to the dialog's remote target rather than to where the INVITE was sent.
    send_ack(endpoint, &dialog, target).await?;

    let media = port.start(sipx_media::Config::new(remote_addr, codec));

    Ok(Call {
        dialog,
        media,
        endpoint: endpoint.clone(),
        target,
    })
}

/// Answer an incoming INVITE.
pub async fn answer(endpoint: &Handle, incoming: &Incoming, media_address: IpAddr) -> Result<Call> {
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;

    let bind = SocketAddr::new(media_address, 0);
    let (remote_addr, codec) = negotiated(&offer)?;
    let media = MediaSession::start(bind, sipx_media::Config::new(remote_addr, codec))
        .await
        .map_err(Error::Io)?;

    let mut capabilities = Capabilities::g711(media_address, media.local_addr().port());
    capabilities.direction = Direction::SendRecv;
    let answer_sdp = sipx_sdp::answer(&offer, &capabilities);

    let tag = sipx_ua::auth::new_cnonce();
    let to_with_tag = {
        let existing = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        format!(
            "{};tag={tag}",
            existing.split(';').next().unwrap_or(&existing)
        )
    };

    let response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).ok_or(Error::NoResponse)?,
        "OK",
    )?
    .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
    .header(
        HeaderName::Contact,
        Bytes::from(format!("<sip:sipx@{}>", endpoint.local_addr())),
    )?
    .header(
        HeaderName::ContentType,
        Bytes::from_static(b"application/sdp"),
    )?
    .body(Bytes::from(answer_sdp.to_string_sdp()))
    .build();

    endpoint.respond(&incoming.key, response).await?;

    let dialog = Dialog::from_request(&incoming.request, &tag).ok_or(Error::NoDialog)?;
    let target = Target::new(incoming.source, incoming.transport);

    Ok(Call {
        dialog,
        media,
        endpoint: endpoint.clone(),
        target,
    })
}

async fn send_ack(endpoint: &Handle, dialog: &Dialog, target: Target) -> Result<()> {
    let (local, remote) = dialog.local_and_remote();
    let ack = RequestBuilder::new(Method::Ack, dialog.remote_target.clone())
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        // The ACK for a 2xx carries the INVITE's sequence number, not a new one: it
        // acknowledges that request rather than being one of its own.
        .cseq(dialog.local_cseq, &Method::Ack)?
        .max_forwards(70)
        .build();
    endpoint.send(ack, target).await?;
    Ok(())
}

fn offer_from(capabilities: &Capabilities) -> SessionDescription {
    let mut sdp = SessionDescription::new(
        capabilities.address,
        capabilities.session_id,
        capabilities.session_version,
    );
    let mut audio = sipx_sdp::MediaDescription::audio(
        capabilities.audio_port,
        capabilities.audio_formats.clone(),
    );
    for (payload, mapping) in &capabilities.rtpmaps {
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "rtpmap",
            format!("{payload} {mapping}"),
        ));
    }
    audio.set_direction(capabilities.direction);
    sdp.media.push(audio);
    sdp
}

/// Where to send media, and in what codec, from a description.
fn negotiated(sdp: &SessionDescription) -> Result<(SocketAddr, Codec)> {
    let audio = sdp
        .media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())
        .ok_or(Error::NoCommonCodec)?;
    let address = sdp.address_for(audio).ok_or(Error::NoCommonCodec)?;

    // The first format both sides can carry. The list is already in the offerer's preference
    // order, so the first playable one is the one to use.
    let codec = audio
        .formats
        .iter()
        .find_map(|format| format.parse::<u8>().ok().and_then(Codec::from_payload_type))
        .ok_or(Error::NoCommonCodec)?;

    Ok((SocketAddr::new(address, audio.port), codec))
}

/// The Request-URI a request must carry to reach us, for tests and callers building their own.
#[must_use]
pub fn contact_for(endpoint: &Handle) -> String {
    format!("<sip:sipx@{}>", endpoint.local_addr())
}
