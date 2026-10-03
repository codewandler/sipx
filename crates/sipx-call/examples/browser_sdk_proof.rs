//! Native authenticated endpoint for the exact packed SDK proof. No application gateway policy.
use bytes::Bytes;
use serde_json::json;
use sipx_call::{Call, DialOptions, MediaAddress, MediaPolicy, answer_with_policy_at, dial, serve};
use sipx_sip::{HeaderName, Host, HostName, Method, ResponseBuilder, StatusCode, Uri};
use sipx_transport::tls::{Identity, ServerTls};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use sipx_ua::auth::Algorithm;
use sipx_ua::challenge::{Authenticator, Presented, Verdict};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
type AnyError = Box<dyn std::error::Error>;
fn failure(message: &str) -> AnyError {
    std::io::Error::other(message).into()
}
#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let args: Vec<String> = std::env::args().collect();
    if let [_, mode, path] = args.as_slice()
        && mode == "--validate-offer"
    {
        let text = std::fs::read_to_string(path)?;
        let description = sipx_sdp::parse::parse(&text)?;
        let verdict = sipx_sdp::browser_audio::validate(
            &description,
            sipx_sdp::browser_audio::BrowserAudioRole::Offerer,
        );
        println!("{verdict:?}");
        return verdict.map(|_| ()).map_err(Into::into);
    }
    let [_, certificate, key, role, result_path] = args.as_slice() else {
        return Err(failure(
            "usage: browser_sdk_proof certificate key role result",
        ));
    };
    let result = tokio::time::timeout(
        Duration::from_secs(65),
        execute(certificate, key, role, result_path),
    )
    .await;
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => json!({"error":error.to_string()}),
        Err(_) => json!({"error":"native proof deadline"}),
    };
    std::fs::write(result_path, serde_json::to_vec(&value)?)?;
    if value.get("error").is_some() {
        return Err(failure("native SDK proof failed"));
    }
    Ok(())
}
async fn authenticate(
    endpoint: &Handle,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    method: Method,
) -> Result<Incoming, AnyError> {
    let mut auth =
        Authenticator::new("sdk-proof.invalid", [37; 32]).with_algorithm(Algorithm::Sha256);
    let mut challenged = false;
    loop {
        let request = incoming
            .recv()
            .await
            .ok_or_else(|| failure("native listener closed"))?;
        if request.request.method != method {
            continue;
        }
        if let Some(presented) = Presented::from_request(&request.request, false) {
            if !challenged
                || presented.username != "browser"
                || presented.uri != request.request.uri.to_string()
                || auth.verify(&presented, &method.to_string(), "fixture-password")
                    != Verdict::Authenticated
            {
                return Err(failure("native Digest verification refused"));
            }
            println!(
                "{}",
                json!({"authenticated":method.to_string(),"algorithm":"SHA-256","challenged":true})
            );
            return Ok(request);
        }
        if challenged {
            return Err(failure("second unauthenticated request"));
        }
        let response = ResponseBuilder::to_request(
            &request.request,
            StatusCode::new(401).ok_or_else(|| failure("status"))?,
            "Unauthorized",
        )?
        .header(
            HeaderName::WwwAuthenticate,
            Bytes::from(auth.challenge(false)),
        )?
        .build();
        endpoint.respond(&request.key, response).await?;
        challenged = true;
    }
}
async fn execute(
    certificate: &str,
    key: &str,
    role: &str,
    result_path: &str,
) -> Result<serde_json::Value, AnyError> {
    let identity = Identity::from_pem(&std::fs::read(certificate)?, &std::fs::read(key)?)?;
    let mut config = Config::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    "localhost".clone_into(&mut config.sent_by);
    config.wss_server = Some((ServerTls::new(identity)?, 0));
    config.pool.reuse_inbound_for_outbound = true;
    let (endpoint, mut incoming) = bind(config).await?;
    println!(
        "{}",
        json!({"status":"listening","address":endpoint.wss_addr()})
    );
    let registration = authenticate(&endpoint, &mut incoming, Method::Register).await?;
    let response = ResponseBuilder::to_request(
        &registration.request,
        StatusCode::new(200).ok_or_else(|| failure("status"))?,
        "OK",
    )?
    .header(HeaderName::Expires, Bytes::from_static(b"600"))?
    .build();
    endpoint.respond(&registration.key, response).await?;
    let address: IpAddr = std::env::var("SIPX_SDK_PROOF_MEDIA_ADDRESS")?.parse()?;
    let mut call = if role == "browser-offerer" {
        let invite = authenticate(&endpoint, &mut incoming, Method::Invite).await?;
        if std::env::var("SIPX_SDK_PROOF_CASE").as_deref() == Ok("missing-ice") {
            return blackhole_answer(&endpoint, &invite, &mut incoming, address).await;
        }
        answer_with_policy_at(
            &endpoint,
            &invite,
            MediaAddress::new(address),
            MediaPolicy::browser_audio(),
        )
        .await?
    } else if role == "browser-answerer" {
        let to = Uri::sip(Host::Name(HostName::new("localhost")?));
        let options = DialOptions::new("<sip:native@localhost>", address)
            .with_media_policy(MediaPolicy::browser_audio())
            .with_media_range_overlap(true)
            .with_timeout(Duration::from_secs(20));
        dial(
            &endpoint,
            Target::new(registration.source, TransportKind::Wss),
            &to,
            &options,
        )
        .await?
    } else {
        return Err(failure("unknown proof role"));
    };
    let peak = audio(&call).await?;
    println!("{}", json!({"status":"media","received_peak":peak}));
    std::fs::write(
        result_path,
        serde_json::to_vec(&json!({"status":"media-ready","received_peak":peak}))?,
    )?;
    serve(&mut call, &mut incoming).await?;
    Ok(
        json!({"role":role,"registration_challenged":true,"invite_challenged":role=="browser-offerer","received_peak":peak,"closed":true}),
    )
}
async fn audio(call: &Call) -> Result<u16, AnyError> {
    let tone: Vec<i16> = (0..32000)
        .map(|sample| {
            if (sample / 34) % 2 == 0 {
                12000
            } else {
                -12000
            }
        })
        .collect();
    let (played, heard) = tokio::join!(
        call.play(&tone),
        call.record_at_least(4800, Duration::from_secs(20))
    );
    let peak = heard
        .iter()
        .map(|value| value.unsigned_abs())
        .max()
        .unwrap_or(0);
    if !played || heard.len() < 4800 || peak == 0 {
        return Err(failure("non-silent bidirectional native audio missing"));
    }
    Ok(peak)
}

/// An admitted SDP answer points only at a separately owned UDP blackhole. There is deliberately
/// no ICE agent here, so peer-reflexive connectivity cannot turn this negative into a success.
async fn blackhole_answer(
    endpoint: &Handle,
    invite: &Incoming,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    address: IpAddr,
) -> Result<serde_json::Value, AnyError> {
    let port: u16 = std::env::var("SIPX_SDK_PROOF_BLACKHOLE_PORT")?.parse()?;
    let offer = sipx_sdp::parse::parse(std::str::from_utf8(invite.request.body())?)?;
    let local = sipx_sdp::browser_audio::BrowserAudioLocal {
        address, port, session_id: 1, session_version: 1, direction: sipx_sdp::Direction::SendRecv,
        ice: sipx_sdp::ice::Credentials::new("blackhole", "blackholePassword0123456789AB").ok_or_else(|| failure("fixture ICE credentials"))?,
        candidates: vec![sipx_sdp::ice::Candidate::parse(&format!("1 1 UDP 2130706431 {address} {port} typ host")).ok_or_else(|| failure("fixture candidate"))?],
        fingerprint: sipx_sdp::fingerprint::Fingerprint::parse("sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F").ok_or_else(|| failure("fixture fingerprint"))?,
        setup: sipx_sdp::fingerprint::SetupCapabilities::both(),
    };
    let answer = sipx_sdp::browser_audio::answer(&offer, &local)?;
    let to = invite
        .request
        .headers
        .value(&HeaderName::To)
        .ok_or_else(|| failure("missing To"))?;
    let response = ResponseBuilder::to_request(
        &invite.request,
        StatusCode::new(200).ok_or_else(|| failure("status"))?,
        "OK",
    )?
    .set_header(
        &HeaderName::To,
        Bytes::from(format!("{};tag=blackhole", String::from_utf8_lossy(&to))),
    )?
    .header(
        HeaderName::Contact,
        Bytes::from(format!(
            "<sip:blackhole@localhost:{};transport=wss>",
            endpoint
                .wss_addr()
                .ok_or_else(|| failure("WSS listener"))?
                .port()
        )),
    )?
    .header(
        HeaderName::ContentType,
        Bytes::from_static(b"application/sdp"),
    )?
    .body(Bytes::from(answer.to_string_sdp()))
    .build();
    endpoint.respond(&invite.key, response).await?;
    println!(
        "{}",
        json!({"fixture":"missing-ice","blackhole_port":port,"ice_agent":false})
    );
    while let Some(request) = incoming.recv().await {
        if request.request.method == Method::Bye {
            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).ok_or_else(|| failure("status"))?,
                "OK",
            )?
            .build();
            endpoint.respond(&request.key, response).await?;
            return Ok(
                json!({"fixture":"missing-ice","blackhole_port":port,"ice_agent":false,"closed":true}),
            );
        }
    }
    Err(failure("blackhole signalling closed without BYE"))
}
