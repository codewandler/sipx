//! I/O-free build-capability validation for every typed command.

use crate::cli::{Command, WorkloadMode};

/// Refuse a command whose selected capability is absent from this binary.
///
/// This runs before command dispatch, so it must not resolve a destination, open a local resource
/// or construct a transport. Command modules retain their full protocol and resource validation.
pub(crate) fn command(command: &Command) -> Result<(), String> {
    match command {
        Command::Dial(options) => {
            crate::media::preflight(&options.media)?;
            crate::device::preflight(&options.audio)
        }
        Command::Answer(options) => {
            crate::media::preflight(&options.media)?;
            crate::device::preflight(&options.audio)
        }
        Command::Scenario(options) => crate::media::preflight(&options.media.complete()),
        Command::Load(options) => {
            baseline_workload(options.mode);
            Ok(())
        }
        Command::LoadResponder(options) => {
            baseline_workload(options.mode);
            Ok(())
        }
        Command::Register(_) | Command::Devices(_) | Command::Peers(_) | Command::Version(_) => {
            Ok(())
        }
    }
}

/// Both bounded workload modes use baseline capabilities in every build.
const fn baseline_workload(mode: WorkloadMode) {
    match mode {
        WorkloadMode::Signalling | WorkloadMode::GeneratedMedia => {}
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::Cli;

    fn parsed(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("valid command syntax")
    }

    #[test]
    fn baseline_load_modes_are_available_in_every_build() {
        for arguments in [
            [
                "sipx",
                "load",
                "sip:load@127.0.0.1",
                "--rate",
                "1",
                "--concurrency",
                "1",
                "--calls",
                "1",
                "--mode",
                "generated-media",
            ]
            .as_slice(),
            [
                "sipx",
                "load-responder",
                "--max-active",
                "1",
                "--calls",
                "1",
                "--cleanup",
                "1",
                "--mode",
                "signalling",
            ]
            .as_slice(),
        ] {
            let cli = parsed(arguments);
            command(cli.command.as_ref().expect("working command")).expect("baseline workload");
        }
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn every_call_role_uses_the_same_codec_preflight() {
        for arguments in [
            ["sipx", "dial", "sip:a@invalid", "--codec", "opus"].as_slice(),
            ["sipx", "answer", "--codec", "opus"].as_slice(),
            ["sipx", "scenario", "--codec", "opus"].as_slice(),
        ] {
            let cli = parsed(arguments);
            let error = command(cli.command.as_ref().expect("working command"))
                .expect_err("Opus is unavailable");
            assert!(error.contains("`opus` feature"), "{error}");
        }
    }
}
