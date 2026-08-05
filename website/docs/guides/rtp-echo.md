---
title: Check an RTP audio boundary
description: Run a finite PCMU echo peer with explicit addresses and cleanup.
---

# Check an RTP audio boundary

The packaged `sipx-testkit` RTP echo fixture gives a process one deliberately small job: accept a
finite number of RTP/AVP PCMU packets from one configured peer, decode them to samples through
`sipx-audio`, encode those samples again, and return them on a fresh RTP sequence and timestamp
timeline. It owns one UDP socket and spawns no task, so success, malformed input, timeout and
cancellation all release the same single resource.

This is a media test fixture, not a SIP user agent. It performs no registration, call setup or SDP
negotiation. It is also not acoustic-echo cancellation, a mixer, a general reflector service or a
load daemon. Use it to diagnose one RTP/PCMU boundary; use the call harness when the assertion is
about SIP call setup, and a deliberately specified load profile when the assertion is capacity.

Choose two local UDP ports, configure the system under test to send PCMU from the peer address to
the fixture's bind address, and run the example. Every argument is required. Both the packet count
and whole-run timeout are non-zero finite bounds; malformed, oversized, foreign-source or non-PCMU
input ends the run with an error.

The example below is compiled with the crate and inlined from its canonical source:

<!-- BEGIN generated:example crates/sipx-testkit/examples/rtp_echo.rs -->
```rust
//! Run one bounded RTP/PCMU echo fixture from a shell.
//!
//! ```text
//! cargo run -p sipx-testkit --example rtp_echo -- \
//!   --bind 127.0.0.1:40000 --peer 127.0.0.1:40002 --packets 50 --timeout-ms 10000
//! ```

use std::ffi::OsString;
use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use sipx_testkit::rtp_echo::{EchoConfig, RtpEcho};

const USAGE: &str = "usage: rtp_echo --bind IP:PORT --peer IP:PORT --packets N --timeout-ms N";

#[derive(Default)]
struct Arguments {
    bind: Option<SocketAddr>,
    peer: Option<SocketAddr>,
    packets: Option<NonZeroUsize>,
    timeout_ms: Option<NonZeroU64>,
}

fn text(value: OsString) -> Result<String, String> {
    value
        .into_string()
        .map_err(|_| "arguments must be valid UTF-8".to_owned())
}

fn value(arguments: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))
        .and_then(text)
}

fn set_once<T>(slot: &mut Option<T>, parsed: T, flag: &str) -> Result<(), String> {
    if slot.replace(parsed).is_some() {
        Err(format!("{flag} may be supplied only once"))
    } else {
        Ok(())
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<EchoConfig, String> {
    let mut arguments = arguments.into_iter();
    let mut parsed = Arguments::default();
    while let Some(flag) = arguments.next() {
        let flag = text(flag)?;
        match flag.as_str() {
            "--bind" => {
                let address = value(&mut arguments, "--bind")?
                    .parse()
                    .map_err(|error| format!("invalid --bind address: {error}"))?;
                set_once(&mut parsed.bind, address, "--bind")?;
            }
            "--peer" => {
                let address = value(&mut arguments, "--peer")?
                    .parse()
                    .map_err(|error| format!("invalid --peer address: {error}"))?;
                set_once(&mut parsed.peer, address, "--peer")?;
            }
            "--packets" => {
                let count = value(&mut arguments, "--packets")?
                    .parse()
                    .map_err(|error| format!("invalid --packets count: {error}"))?;
                set_once(&mut parsed.packets, count, "--packets")?;
            }
            "--timeout-ms" => {
                let duration = value(&mut arguments, "--timeout-ms")?
                    .parse()
                    .map_err(|error| format!("invalid --timeout-ms bound: {error}"))?;
                set_once(&mut parsed.timeout_ms, duration, "--timeout-ms")?;
            }
            _ => return Err(format!("unknown argument {flag:?}")),
        }
    }

    let bind = parsed.bind.ok_or_else(|| "--bind is required".to_owned())?;
    if bind.port() == 0 {
        return Err("--bind must name a non-zero port so an external peer can reach it".to_owned());
    }
    let peer = parsed.peer.ok_or_else(|| "--peer is required".to_owned())?;
    let packets = parsed
        .packets
        .ok_or_else(|| "--packets requires a non-zero finite count".to_owned())?;
    let timeout_ms = parsed
        .timeout_ms
        .ok_or_else(|| "--timeout-ms requires a non-zero finite bound".to_owned())?;
    EchoConfig::new(bind, peer, packets, Duration::from_millis(timeout_ms.get()))
        .map_err(|error| error.to_string())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(std::env::args_os().skip(1)).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{error}\n{USAGE}"),
        )
    })?;
    let echo = RtpEcho::bind(config).await?;
    println!(
        "ready: listening={} peer={} packets={} timeout_ms={}",
        echo.local_addr(),
        config.peer(),
        config.packets(),
        config.within().as_millis()
    );
    let report = echo.run().await?;
    println!(
        "complete: packets={} decoded_samples={}",
        report.packets, report.samples
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        values.iter().map(OsString::from)
    }

    #[test]
    fn every_address_and_bound_is_explicit() {
        let parsed = parse_args(args(&[
            "--bind",
            "127.0.0.1:40000",
            "--peer",
            "127.0.0.1:40002",
            "--packets",
            "3",
            "--timeout-ms",
            "1000",
        ]))
        .expect("complete bounded arguments");
        assert_eq!(parsed.bind_addr(), "127.0.0.1:40000".parse().unwrap());
        assert_eq!(parsed.peer(), "127.0.0.1:40002".parse().unwrap());
        assert_eq!(parsed.packets().get(), 3);
        assert_eq!(parsed.within(), Duration::from_secs(1));
    }

    #[test]
    fn missing_malformed_zero_duplicate_and_unknown_inputs_are_refused() {
        for invalid in [
            &[][..],
            &["--bind", "bad"][..],
            &[
                "--bind",
                "127.0.0.1:40000",
                "--peer",
                "127.0.0.1:40002",
                "--packets",
                "0",
                "--timeout-ms",
                "1000",
            ][..],
            &[
                "--bind",
                "127.0.0.1:40000",
                "--peer",
                "127.0.0.1:40002",
                "--packets",
                "3",
                "--timeout-ms",
                "0",
            ][..],
            &["--bind", "127.0.0.1:40000", "--bind", "127.0.0.1:40001"][..],
            &["--forever"][..],
        ] {
            assert!(parse_args(args(invalid)).is_err(), "accepted {invalid:?}");
        }
    }
}
```
<!-- END generated:example -->

Each reply has its own fixture-owned RTP identity. Sequence numbers begin at zero and advance once
per packet; timestamps begin at zero and advance by the decoded sample count. Those deterministic
values are for assertions, not a recommendation for production RTP identity generation. A clean
completion prints the exact packet and decoded-sample totals before the process exits.
