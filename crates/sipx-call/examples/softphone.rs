//! Talk into a call: microphone → DSP graph → SIP endpoint, and the far end into your speakers.
//!
//! ```text
//! cargo run --example softphone -- sip:1000@192.0.2.10:5066
//! cargo run --example softphone -- sip:alice@192.0.2.10 --advertise 203.0.113.7 --record out.wav
//! ```
//!
//! Where [`call_and_listen`](../call_and_listen.rs) only listens, this one speaks, and everything it
//! sends passes through an ordered outbound DSP graph first — the same
//! [`GraphPlan`](sipx_media::dsp::GraphPlan) a real deployment attaches, not a private code path:
//!
//! | Stage | Why it is in the chain |
//! |---|---|
//! | `HighPass` at 120 Hz | Desk rumble, mains hum and the room's low end carry no speech and eat the codec's budget. |
//! | `SubbandSuppressor` | Per-band noise estimate; steady noise falls away and speech does not. |
//! | `Peaking` at 2.6 kHz | Consonants live here. A narrow lift buys intelligibility a wideband gain cannot. |
//! | `Gain` | Makeup for what the two filters removed, smoothed so it cannot step. |
//!
//! `--no-dsp` sends the microphone untouched, which is the comparison worth hearing: the point of
//! the chain is not that it is audible but that the far end's echo of it is *cleaner*.
//!
//! # When the far end answers and you hear nothing
//!
//! Answering proves signalling arrived; it says nothing about media, which takes its own path. The
//! usual cause is that the address in this side's SDP is not one the far end can send UDP back to —
//! a private address behind NAT, or one that collides with a range the far end already uses for
//! something else. `--advertise <ip>` puts a different address in the SDP while the socket stays
//! bound locally, which is what you want whenever the far end sees you arriving from an address
//! that is not the one you are bound to.
//!
//! # What this is not
//!
//! Not a softphone you would register with, despite the name: no registration, no hold, no transfer,
//! one call and then it exits. It is the shortest path from a microphone to a SIP endpoint that
//! still runs the real media stack.

// Samples are read before they are run, so they are written for readability.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sipx_audio::analysis::AudioDirection;
use sipx_audio::dsp::{Parameter, ParameterValue};
use sipx_call::{DialOptions, dial};
use sipx_media::dsp::{BuiltIn, GraphBounds, GraphPlan};
use sipx_sip::Uri;
use sipx_transport::{Config, Target, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))?;

    let target: SocketAddr = format!("{}:{}", options.host, options.port).parse()?;
    let to = Uri::parse(bytes::Bytes::from(format!(
        "sip:{}@{}",
        options.user, options.host
    )))?;

    // Bind on the interface that reaches the target: the SDP has to carry something the far end can
    // send RTP back to, and `0.0.0.0` is not that.
    let local = local_address_towards(target)?;
    let (endpoint, _incoming) = bind(Config::new(SocketAddr::new(local, 0)))
        .await
        .map_err(|error| format!("cannot bind a local endpoint on {local}: {error}"))?;

    // Advertised and bound are two decisions, not one. Behind NAT the far end must be told an
    // address it can reach, while the socket can only be bound to one this host actually holds.
    let advertised = options.advertise.unwrap_or(local);
    println!("calling {target} from {}", endpoint.local_addr());
    if advertised != local {
        println!("advertising media at {advertised}, bound on {local}");
    }

    let dial_options = DialOptions::new(format!("<sip:{}@sipx.invalid>", options.user), advertised)
        .with_media_bind_address(local)
        .with_timeout(Duration::from_secs(20));
    let mut call = dial(&endpoint, Target::udp(target), &to, &dial_options).await?;
    println!("answered; media encrypted: {}", call.is_encrypted());

    let rate = call.media().clock_rate();

    // The chain, attached before the first packet so no unprocessed audio precedes it.
    if options.dsp {
        call.media().attach_dsp(voice_chain()?)?;
        println!(
            "outbound chain: high-pass 120 Hz → subband suppressor → 2.6 kHz presence → makeup"
        );
    } else {
        println!("outbound chain: none (--no-dsp)");
    }

    let speaker = Speaker::open(rate)?;
    let microphone = Microphone::open(rate)?;
    println!("playing at {rate} Hz — speak, or Ctrl-C to hang up early");

    // Talk on its own task: against an echo server the two directions overlap by definition, and a
    // receive loop that waited its turn to send would hear its own audio in bursts.
    let sent = Arc::new(AtomicUsize::new(0));
    let talking = tokio::spawn({
        let media = call.media_handle();
        let counted = Arc::clone(&sent);
        let per_packet = call.media().samples_per_packet();
        let source = microphone.samples();
        async move {
            // One packet every packet-time. Pacing is the media session's job, but handing it a
            // packet per interval is this side's: a loop that queued as fast as it could would
            // drain the microphone's buffer into memory rather than onto the wire.
            let interval = Duration::from_secs_f64(per_packet as f64 / f64::from(rate));
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                let packet = take(&source, per_packet);
                counted.fetch_add(packet.len(), Ordering::Relaxed);
                if !media.send(packet).await {
                    break;
                }
            }
        }
    });

    let mut recorded: Vec<i16> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(options.seconds);
    let mut heard = 0usize;
    // Ends when the call does or when the time asked for is up. Both are ordinary endings, and
    // neither is worth distinguishing here: the sample count below says which happened.
    while let Ok(Some(samples)) = tokio::time::timeout_at(deadline, call.media().recv()).await {
        heard += samples.len();
        speaker.play(&samples);
        if options.record.is_some() {
            recorded.extend_from_slice(&samples);
        }
    }

    talking.abort();
    call.hang_up().await?;

    let seconds = |samples: usize| samples as f64 / f64::from(rate);
    println!(
        "sent {:.1}s from the microphone, heard {:.1}s back",
        seconds(sent.load(Ordering::Relaxed)),
        seconds(heard)
    );
    if heard == 0 {
        eprintln!(
            "no audio arrived. The call was answered, so signalling reached the far end — media \
             takes its own path. Check that {advertised} is an address the far end can send UDP to, \
             and try --advertise with the address it sees you arriving from."
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

/// The outbound chain, in the order it runs.
///
/// Built from the workspace's own stages rather than hand-rolled processors, so what this example
/// demonstrates is the graph a deployment would attach and not a private imitation of one.
fn voice_chain() -> Result<GraphPlan, Box<dyn std::error::Error>> {
    // Ratios are thousandths: `Ratio(2_600)` is 2.6, `Ratio(1_400)` is 1.4.
    Ok(GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(
            BuiltIn::HighPass,
            &[Parameter::new("cutoff_hz", ParameterValue::Integer(120))],
        )?
        .with_built_in(
            BuiltIn::SubbandSuppressor,
            &[
                // Floor rather than gate: a band driven to silence sounds like a dropout, and a
                // listener reads a dropout as a broken call.
                Parameter::new("min_band_gain", ParameterValue::Ratio(120)),
                Parameter::new("over_subtraction", ParameterValue::Ratio(1_600)),
                Parameter::new("adaptation_positions", ParameterValue::Integer(2_400)),
                Parameter::new("gain_slew_positions", ParameterValue::Integer(64)),
            ],
        )?
        .with_built_in(
            BuiltIn::Peaking,
            &[
                Parameter::new("centre_hz", ParameterValue::Integer(2_600)),
                Parameter::new("band_gain", ParameterValue::Ratio(1_800)),
            ],
        )?
        .with_built_in(
            BuiltIn::Gain,
            &[
                Parameter::new("gain", ParameterValue::Ratio(1_400)),
                Parameter::new("smoothing_positions", ParameterValue::Integer(80)),
            ],
        )?)
}

/// Take one packet from a shared queue, padding with silence when the microphone is behind.
///
/// Silence rather than a short packet: the far end's jitter buffer reads a short packet as a gap in
/// the timeline, and a microphone that is a millisecond late is not a gap.
fn take(source: &Arc<Mutex<std::collections::VecDeque<i16>>>, samples: usize) -> Vec<i16> {
    let mut packet = Vec::with_capacity(samples);
    if let Ok(mut queue) = source.lock() {
        for _ in 0..samples {
            packet.push(queue.pop_front().unwrap_or(0));
        }
    } else {
        packet.resize(samples, 0);
    }
    packet
}

/// Which of this host's addresses reaches `target`.
///
/// Asks the routing table by opening a UDP socket at it — no packet is sent, but the kernel picks
/// the source address it would use.
fn local_address_towards(target: SocketAddr) -> Result<IpAddr, Box<dyn std::error::Error>> {
    let probe = std::net::UdpSocket::bind(if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    probe.connect(target)?;
    Ok(probe.local_addr()?.ip())
}

/// The default input device, resampled to the call's rate.
///
/// Capture runs at whatever rate the device offers — asking a 48 kHz interface for 8 kHz is how an
/// example fails on hardware that was working — and a streaming resampler carries the phase across
/// callbacks so the seams between them are not audible.
struct Microphone {
    queue: Arc<Mutex<std::collections::VecDeque<i16>>>,
    _stream: cpal::Stream,
}

impl Microphone {
    fn open(call_rate: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("this host publishes no default input device")?;

        // Ask the device what it supports rather than assuming. A capture device that reports
        // `f32` refuses an `i16` stream outright — `DeviceNotAvailable`, which reads like an
        // unplugged microphone and is not one.
        let chosen = choose_input(&device, call_rate)?;
        println!(
            "input: {} at {} Hz, {} ch, {:?}",
            describe(&device),
            chosen.rate,
            chosen.channels,
            chosen.format
        );

        let queue: Arc<Mutex<std::collections::VecDeque<i16>>> = Arc::default();
        let feed = Arc::clone(&queue);
        let mut resampler = sipx_audio::LinearResampler::new(chosen.rate, call_rate)?;
        let channels = chosen.channels;
        let config = cpal::StreamConfig {
            channels,
            sample_rate: chosen.rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = device.build_input_stream_raw(
            config,
            chosen.format,
            move |data: &cpal::Data, _: &cpal::InputCallbackInfo| {
                let Some(interleaved) = to_i16(data) else {
                    return;
                };
                // Downmix first: the call is one channel, and resampling interleaved frames would
                // fold the channels into each other.
                let mono: Vec<i16> = if channels <= 1 {
                    interleaved
                } else {
                    interleaved
                        .chunks(channels as usize)
                        .map(|frame| {
                            let sum: i32 = frame.iter().map(|&sample| i32::from(sample)).sum();
                            let width = i32::try_from(frame.len()).unwrap_or(1).max(1);
                            (sum / width) as i16
                        })
                        .collect()
                };
                let converted = resampler.push_i16(&mono);
                if let Ok(mut queue) = feed.lock() {
                    queue.extend(converted);
                    // A bound, because nothing drains this if the call stalls. Dropping the oldest
                    // keeps the conversation live: a queue that grew would send audio from a
                    // sentence the far end has already stopped waiting for.
                    let excess = queue.len().saturating_sub(call_rate as usize);
                    drop(queue.drain(..excess));
                }
            },
            |error| eprintln!("audio input: {error}"),
            None,
        )?;
        stream.play()?;
        Ok(Self {
            queue,
            _stream: stream,
        })
    }

    fn samples(&self) -> Arc<Mutex<std::collections::VecDeque<i16>>> {
        Arc::clone(&self.queue)
    }
}

/// The default output device, fed from the call.
///
/// A ring buffer between the call and the audio callback, because the callback runs on the host's
/// own real-time thread and must never wait for a network read. Underrun is silence rather than a
/// stall: a speaker that blocked would take the audio device down with it.
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
        println!("output: {}", describe(&device));

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

/// A capture configuration the device actually offers.
struct ChosenInput {
    rate: u32,
    channels: u16,
    format: cpal::SampleFormat,
}

/// Pick a capture configuration the device will actually open.
///
/// The device's own default comes first, and not as a mere preference: a capture device reached
/// through a host's sound server or plugin layer advertises rate *ranges* it will not honour — it
/// offers 8 kHz mono and then refuses it with `DeviceNotAvailable`, which is indistinguishable from
/// an unplugged microphone. The default configuration is the one it has committed to. Resampling
/// and downmixing are this example's job either way, so taking the device's terms costs nothing.
fn choose_input(
    device: &cpal::Device,
    call_rate: u32,
) -> Result<ChosenInput, Box<dyn std::error::Error>> {
    if let Ok(default) = device.default_input_config() {
        return Ok(ChosenInput {
            rate: default.sample_rate(),
            channels: default.channels(),
            format: default.sample_format(),
        });
    }
    device
        .supported_input_configs()?
        .filter(|range| {
            range.channels() > 0
                && matches!(
                    range.sample_format(),
                    cpal::SampleFormat::I16 | cpal::SampleFormat::F32 | cpal::SampleFormat::U16
                )
        })
        .map(|range| {
            let rate = call_rate.clamp(range.min_sample_rate(), range.max_sample_rate());
            let format_rank = match range.sample_format() {
                cpal::SampleFormat::I16 => 0,
                cpal::SampleFormat::F32 => 1,
                _ => 2,
            };
            (
                (rate.abs_diff(call_rate), range.channels(), format_rank),
                ChosenInput {
                    rate,
                    channels: range.channels(),
                    format: range.sample_format(),
                },
            )
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, chosen)| chosen)
        .ok_or_else(|| "the default input device offers no usable capture format".into())
}

/// One callback's worth of samples as `i16`, whatever the device handed over.
fn to_i16(data: &cpal::Data) -> Option<Vec<i16>> {
    match data.sample_format() {
        cpal::SampleFormat::I16 => data.as_slice::<i16>().map(<[i16]>::to_vec),
        cpal::SampleFormat::F32 => data.as_slice::<f32>().map(|samples| {
            samples
                .iter()
                .map(|&sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
                .collect()
        }),
        cpal::SampleFormat::U16 => data.as_slice::<u16>().map(|samples| {
            samples
                .iter()
                .map(|&sample| (i32::from(sample) - 32_768) as i16)
                .collect()
        }),
        _ => None,
    }
}

/// What a device calls itself, or a stand-in when it will not say.
fn describe(device: &cpal::Device) -> String {
    device.description().map_or_else(
        |_| "unnamed device".to_owned(),
        |description| description.name().to_owned(),
    )
}

/// What the command line asked for.
struct Options {
    user: String,
    host: String,
    port: u16,
    seconds: u64,
    dsp: bool,
    advertise: Option<IpAddr>,
    record: Option<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let endpoint = args.next().ok_or(
            "usage: softphone sip:<user>@<host>[:port] \
             [--seconds N] [--no-dsp] [--advertise IP] [--record PATH]",
        )?;
        let rest = endpoint.strip_prefix("sip:").unwrap_or(&endpoint);
        let (user, hostport) = rest
            .split_once('@')
            .ok_or_else(|| format!("{endpoint} is not sip:<user>@<host>"))?;
        // A bare IPv6 literal would need brackets; this splits on the last colon so `[::1]:5060`
        // and `192.0.2.10:5066` both work and `192.0.2.10` takes the default.
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
            dsp: true,
            advertise: None,
            record: None,
        };
        while let Some(flag) = args.next() {
            let mut value = || -> Result<String, String> {
                args.next().ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--seconds" => options.seconds = value()?.parse()?,
                "--no-dsp" => options.dsp = false,
                "--advertise" => options.advertise = Some(value()?.parse()?),
                "--record" => options.record = Some(value()?),
                other => return Err(format!("unknown option {other}").into()),
            }
        }
        Ok(options)
    }
}
