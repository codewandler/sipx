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

/// Every optional feature this binary was compiled with, named and sorted.
///
/// `sipx version` answers "which sipx is this", and until `X-121` it could not: two binaries built
/// from the same commit report the same version while refusing different commands, because the
/// capabilities [`command`] checks are chosen at compile time. A caller that has one of these on
/// `PATH` — or spawned from a test — cannot otherwise tell which one it has short of running a
/// command and reading the refusal, and *that* refusal is easy to read as broken hardware rather
/// than as a build without the driver compiled in.
pub(crate) fn features() -> Vec<&'static str> {
    // Sorted, so two reports of the same build compare equal without the reader sorting first.
    let compiled = [
        ("device-audio", cfg!(feature = "device-audio")),
        ("dtls", cfg!(feature = "dtls")),
        ("opus", cfg!(feature = "opus")),
    ];
    compiled
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect()
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

    /// `X-121`: a feature this list forgets is a capability `sipx version` silently under-reports,
    /// and the reader of that report has no way to notice. So the declaration is checked against
    /// the manifest rather than maintained beside it — the next optional feature fails here on the
    /// commit that adds it, instead of quietly narrowing what the version output means.
    #[test]
    fn every_declared_feature_is_reported_by_the_version_output() {
        let manifest = include_str!("../Cargo.toml");
        let table = manifest
            .split("[features]\n")
            .nth(1)
            .expect("a [features] table")
            .split("\n[")
            .next()
            .expect("the table ends at the next section");
        let declared: Vec<&str> = table
            .lines()
            .filter_map(|line| line.split('=').next())
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != "default")
            .collect();
        assert!(
            !declared.is_empty(),
            "the manifest declares optional features"
        );

        let source = include_str!("preflight.rs");
        for name in declared {
            assert!(
                source.contains(&format!("(\"{name}\", cfg!(feature = \"{name}\"))")),
                "`{name}` is declared in Cargo.toml but absent from `features()`"
            );
        }
    }

    /// The reported set has to be the compiled one, not a constant that outlived a `cfg`.
    #[test]
    fn the_reported_set_is_this_build() {
        assert_eq!(
            features().contains(&"device-audio"),
            cfg!(feature = "device-audio")
        );
        assert_eq!(features().contains(&"dtls"), cfg!(feature = "dtls"));
        assert_eq!(features().contains(&"opus"), cfg!(feature = "opus"));
        let mut sorted = features();
        sorted.sort_unstable();
        assert_eq!(features(), sorted, "reported in a stable order");
    }
}
