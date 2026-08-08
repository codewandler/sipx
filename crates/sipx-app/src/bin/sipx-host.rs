//! The `sipx-host` binary: run a host from a configuration document.
//!
//! ```text
//! sipx-host host.toml [media-address]
//! ```
//!
//! The whole program is [`sipx_app::host::Host`]. What lives here is argument reading and an exit
//! code, because a `main` that contained logic would be logic no test could reach — and this
//! application is the definition of the stack's reachable surface (`X-38`), so every line of it that
//! matters has to sit where a test can call it.

// A binary's `main` is the one place a program may print and exit rather than return a value, and
// the argument reader has nowhere to hand an error to but the operator.
#![allow(clippy::print_stderr, clippy::print_stdout)]

// This crate's inline `#[cfg(test)]` modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::process::ExitCode;

use sipx_app::host::Host;

/// The address RTP is advertised on when the operator does not name one. Loopback rather than a
/// guess at the machine's public address: a host that advertises the wrong address produces a call
/// with one-way audio, which is worse than one that plainly only works locally.
const DEFAULT_MEDIA_ADDRESS: &str = "127.0.0.1";

#[tokio::main]
async fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(path) = arguments.next() else {
        eprintln!("usage: sipx-host <document.toml> [media-address]");
        return ExitCode::FAILURE;
    };

    let document = match std::fs::read_to_string(&path) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let media_address = arguments
        .next()
        .unwrap_or_else(|| DEFAULT_MEDIA_ADDRESS.to_owned());
    let media_address = match media_address.parse() {
        Ok(address) => address,
        Err(error) => {
            eprintln!("{media_address}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut host = match Host::start(&document, media_address) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let reports = host.take_realtime_reports();
    let reporter = tokio::spawn(async move {
        let Some(mut reports) = reports else {
            return;
        };
        while let Some(report) = reports.recv().await {
            println!("{}", report.to_json());
        }
    });

    let listener = match host.call_listener() {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (handle, incoming) = match host.bind_endpoint().await {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("sipx-host: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "sipx-host: answering on {} ({})",
        handle.local_addr(),
        listener.name
    );

    let served = host.serve(handle, incoming).await;
    drop(host);
    let _ = reporter.await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sipx-host: {error}");
            ExitCode::FAILURE
        }
    }
}
