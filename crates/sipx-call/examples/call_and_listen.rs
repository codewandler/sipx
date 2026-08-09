//! Call a SIP endpoint and hear it: `sip:<user>@<host>[:port]` out of your speakers.
//!
//! ```text
//! cargo run --example call_and_listen -- sip:alice@192.0.2.10
//! cargo run --example call_and_listen -- sip:1000@192.0.2.10:5066 --seconds 30 --record out.wav
//! ```
//!
//! The far end's audio goes to the default output device as it arrives. Nothing is buffered to a
//! file first, so what you hear is what the call carried, at the rate the two ends negotiated.
//!
//! **It sends a tone by default, and that is not decoration.** An *echo* server echoes what you send
//! it, so a client that only listens hears silence and the silence looks exactly like a broken
//! media path. `--tone 0` turns it off for a far end that talks first, like an IVR or an
//! announcement.
//!
//! # Reaching a SIP endpoint inside Kubernetes
//!
//! A `ClusterIP` is not routable from outside the cluster — it exists only in the nodes' iptables —
//! so a VPN that carries the pod and node network still cannot reach one. Dial the **pod** address:
//!
//! ```text
//! kubectl -n <namespace> get pod -l app=<service> -o jsonpath='{.items[0].status.podIP}'
//! kubectl -n <namespace> get svc <service> -o jsonpath='{.spec.ports[0].port}'
//! ```
//!
//! Cluster DNS is the same story: `<service>.<namespace>.svc.cluster.local` is served from inside
//! the cluster, at one of those same unroutable addresses, and a host on a VPN resolves through its
//! own resolver, which has never heard of it. `docs/designs/cluster-names.md` records what it would
//! take for sipx to resolve those names itself, and why ordinary DNS must be tried first regardless.
//!
//! # What this is not
//!
//! Not a softphone. There is no microphone here — this listens. `sipx dial --play` sends audio, and
//! the `device-audio` feature of `sipx-cli` selects an exact input or output by identifier. This
//! example takes whichever output device the host calls default, because an example that made you
//! choose one before you could hear anything would be a worse example.

// Samples are read before they are run, so they are written for readability.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sipx_call::{DialOptions, dial};
use sipx_sip::Uri;
use sipx_transport::{Config, Target, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))?;

    // The address to send to, and the URI to put in the request. A SIP URI's host may be a name a
    // registrar resolves rather than something to connect to, so the two are separate here — this
    // example dials the host in the URI because that is what "call this endpoint" means.
    let target: SocketAddr = format!("{}:{}", options.host, options.port).parse()?;
    let to = Uri::parse(bytes::Bytes::from(format!(
        "sip:{}@{}",
        options.user, options.host
    )))?;

    // Bind on the interface that reaches the target rather than on every address: the SDP has to
    // advertise something the far end can send RTP back to, and `0.0.0.0` is not that.
    let local = local_address_towards(target)?;
    let (endpoint, _incoming) = bind(Config::new(SocketAddr::new(local, 0)))
        .await
        .map_err(|error| format!("cannot bind a local endpoint on {local}: {error}"))?;
    println!("calling {} from {}", target, endpoint.local_addr());

    let dial_options = DialOptions::new(format!("<sip:{}@sipx.invalid>", options.user), local)
        .with_timeout(Duration::from_secs(20));
    let mut call = dial(&endpoint, Target::udp(target), &to, &dial_options).await?;
    println!("answered; media encrypted: {}", call.is_encrypted());

    let rate = call.media().clock_rate();
    let speaker = Speaker::open(rate)?;
    println!("playing at {rate} Hz — Ctrl-C to hang up early");

    // Talk, unless told not to. Sent on its own task so the receive loop below is not waiting on
    // playback to finish: against an echo server the two directions overlap by definition.
    let talking = (options.tone_hz > 0).then(|| {
        let media = call.media_handle();
        let samples = tone(rate, options.tone_hz, options.seconds);
        let per_packet = call.media().samples_per_packet();
        println!(
            "sending a {} Hz tone so an echo server has something to echo",
            options.tone_hz
        );
        tokio::spawn(async move { media.play(&samples, per_packet).await })
    });

    let mut recorded: Vec<i16> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(options.seconds);
    let mut heard = 0usize;
    // Each frame goes straight out: the speaker was opened at the negotiated clock, so nothing here
    // has to convert. The loop ends when the call does or when the time asked for is up, and
    // neither ending is worth distinguishing — the sample count below says which happened.
    while let Ok(Some(samples)) = tokio::time::timeout_at(deadline, call.media().recv()).await {
        heard += samples.len();
        speaker.play(&samples);
        if options.record.is_some() {
            recorded.extend_from_slice(&samples);
        }
    }

    if let Some(talking) = talking {
        talking.abort();
    }
    call.hang_up().await?;
    println!(
        "heard {heard} samples ({:.1}s of audio)",
        heard as f64 / f64::from(rate)
    );
    if heard == 0 {
        // Silence is a result, and the most likely cause is worth naming rather than leaving to a
        // packet capture: signalling reached the far end (the call was answered) and RTP did not
        // come back, which is what a one-way network path looks like from this side.
        eprintln!(
            "no audio arrived. The call was answered, so signalling reached the far end and RTP \
             did not come back. If this is an echo server, check --tone is not 0: it returns what \
             you send and a silent caller hears silence. Otherwise check that {local} is reachable \
             from the far end for UDP media, not only that you can reach it."
        );
    }
    if let Some(path) = &options.record {
        sipx_audio::write_wav(
            std::fs::File::create(path)?,
            &sipx_audio::Wav {
                sample_rate: rate,
                samples: recorded,
            },
        )?;
        println!("wrote {path}");
    }
    Ok(())
}

/// A tone to send, so a far end that echoes has something to return.
///
/// An envelope on the onset, so a recording of it cannot be mistaken for silence and a listener can
/// tell the start of the echo from the start of the call.
fn tone(rate: u32, hz: u32, seconds: u64) -> Vec<i16> {
    let total = rate as usize * seconds as usize;
    (0..total)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            let envelope = (t * 8.0).min(1.0);
            ((t * f64::from(hz) * std::f64::consts::TAU).sin() * 10_000.0 * envelope) as i16
        })
        .collect()
}

/// Which of this host's addresses reaches `target`.
///
/// Asks the routing table by opening a UDP socket at it — no packet is sent, but the kernel picks
/// the source address it would use. That is the address the far end must send RTP back to, and
/// guessing it from the interface list is how a call ends up advertising an address on the wrong
/// network.
fn local_address_towards(
    target: SocketAddr,
) -> Result<std::net::IpAddr, Box<dyn std::error::Error>> {
    let probe = std::net::UdpSocket::bind(if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    probe.connect(target)?;
    Ok(probe.local_addr()?.ip())
}

/// The default output device, fed from the call.
///
/// A ring buffer between the call and the audio callback, because the callback runs on the host's
/// own real-time thread and must never wait for a network read. Underrun is silence rather than a
/// stall: a speaker that blocked would take the audio device down with it, and a gap in a
/// diagnostic call is information, not a failure.
struct Speaker {
    queue: Arc<Mutex<std::collections::VecDeque<i16>>>,
    _stream: cpal::Stream,
}

impl Speaker {
    fn open(rate: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("this host publishes no default output device")?;
        let described = device.description().map_or_else(
            |_| "the default output".to_owned(),
            |description| description.name().to_owned(),
        );
        println!("output: {described}");

        let queue: Arc<Mutex<std::collections::VecDeque<i16>>> = Arc::default();
        let feed = Arc::clone(&queue);
        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = device.build_output_stream::<i16, _, _>(
            config,
            move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let Ok(mut queue) = feed.lock() else {
                    // A poisoned lock means the feeding side panicked. Play silence rather than
                    // panicking on the audio thread, which would abort the process.
                    out.fill(0);
                    return;
                };
                for slot in out.iter_mut() {
                    *slot = queue.pop_front().unwrap_or(0);
                }
            },
            |error| eprintln!("audio output: {error}"),
            None,
        )?;
        stream.play()?;
        Ok(Self {
            queue,
            _stream: stream,
        })
    }

    fn play(&self, samples: &[i16]) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.extend(samples.iter().copied());
        }
    }
}

/// What the command line asked for.
struct Options {
    user: String,
    host: String,
    port: u16,
    seconds: u64,
    tone_hz: u32,
    record: Option<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let endpoint = args.next().ok_or(
            "usage: call_and_listen sip:<user>@<host>[:port] \
                 [--seconds N] [--tone HZ, 0 to stay silent] [--record PATH]",
        )?;
        let rest = endpoint.strip_prefix("sip:").unwrap_or(&endpoint);
        let (user, hostport) = rest
            .split_once('@')
            .ok_or_else(|| format!("{endpoint} is not sip:<user>@<host>"))?;
        // A bare IPv6 literal would need brackets; this splits on the last colon so `[::1]:5060`
        // and `10.0.0.1:5066` both work and `10.0.0.1` takes the default.
        let (host, port) = match hostport.rsplit_once(':') {
            Some((host, port))
                if !host.ends_with(']') || port.chars().all(|c| c.is_ascii_digit()) =>
            {
                (host.to_owned(), port.parse()?)
            }
            _ => (hostport.to_owned(), 5060),
        };

        let mut options = Self {
            user: user.to_owned(),
            host,
            port,
            seconds: 20,
            tone_hz: 440,
            record: None,
        };
        while let Some(flag) = args.next() {
            let mut value = || -> Result<String, String> {
                args.next().ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--seconds" => options.seconds = value()?.parse()?,
                "--tone" => options.tone_hz = value()?.parse()?,
                "--record" => options.record = Some(value()?),
                other => return Err(format!("unknown option {other}").into()),
            }
        }
        Ok(options)
    }
}
