//! Library crates are observers of the host's tracing configuration, never output owners.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sipx_testkit::call::CallHarness;

fn rust_sources(root: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("read crate source directory") {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            rust_sources(&path, into);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            into.push(path);
        }
    }
}

#[test]
fn library_sources_do_not_write_or_install_a_global_subscriber() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("testkit is under crates/ in the workspace");
    let crates = workspace.join("crates");
    let mut violations = Vec::new();

    for manifest in fs::read_dir(&crates).expect("read crates directory") {
        let crate_path = manifest.expect("read crate entry").path();
        let source = crate_path.join("src");
        if !source.join("lib.rs").is_file() {
            continue;
        }
        let cargo = fs::read_to_string(crate_path.join("Cargo.toml")).expect("read crate manifest");
        if cargo.contains("tracing-subscriber") {
            violations.push(format!(
                "{} depends on tracing-subscriber",
                crate_path.display()
            ));
        }
        let mut files = Vec::new();
        rust_sources(&source, &mut files);
        for path in files {
            if path
                .components()
                .any(|component| component.as_os_str() == "bin")
                || path.file_name().and_then(|name| name.to_str()) == Some("main.rs")
            {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read Rust source");
            for denied in [
                "println!(",
                "eprintln!(",
                "dbg!(",
                "set_global_default(",
                "log::error!(",
                "log::warn!(",
                "log::info!(",
                "log::debug!(",
                "log::trace!(",
            ] {
                if text.contains(denied) {
                    violations.push(format!("{} contains {denied}", path.display()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "library code must stay quiet unless its host installs tracing:\n{}",
        violations.join("\n")
    );
}

#[test]
fn constructing_the_public_harness_does_not_install_output() {
    assert!(
        !tracing::dispatcher::has_been_set(),
        "the test process begins without a global tracing subscriber"
    );
    let _call = CallHarness::perfect();
    assert!(
        !tracing::dispatcher::has_been_set(),
        "using the library must leave output ownership with the host"
    );
}

const QUIET_CHILD: &str = "SIPX_TESTKIT_QUIET_CHILD";

#[test]
fn quiet_control_child() {}

#[test]
fn quiet_library_child() {
    let _call = CallHarness::perfect();
}

fn run_child(name: &str) -> Output {
    Command::new(std::env::current_exe().expect("locate this test executable"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(QUIET_CHILD, "1")
        .output()
        .expect("run isolated output probe")
}

fn normalize(output: &[u8], name: &str) -> String {
    String::from_utf8_lossy(output)
        .replace(name, "quiet_child")
        .lines()
        .map(|line| line.split("; finished in ").next().unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_library_call_without_a_subscriber_emits_no_process_output() {
    let control = run_child("quiet_control_child");
    let library = run_child("quiet_library_child");
    assert!(control.status.success(), "control child must run");
    assert!(library.status.success(), "library child must run");
    assert_eq!(
        normalize(&library.stdout, "quiet_library_child"),
        normalize(&control.stdout, "quiet_control_child"),
        "the library invocation must add nothing to stdout"
    );
    assert_eq!(
        normalize(&library.stderr, "quiet_library_child"),
        normalize(&control.stderr, "quiet_control_child"),
        "the library invocation must add nothing to stderr"
    );
}
