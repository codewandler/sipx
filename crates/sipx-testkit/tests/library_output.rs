//! Library crates are observers of the host's tracing configuration, never output owners.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sipx_testkit::call::CallHarness;

fn denied_output(text: &str) -> Vec<&'static str> {
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let mut denied = Vec::new();
    for (needle, label) in [
        ("print!(", "print!"),
        ("println!(", "println!"),
        ("eprint!(", "eprint!"),
        ("eprintln!(", "eprintln!"),
        ("dbg!(", "dbg!"),
        ("set_global_default(", "global subscriber installation"),
        ("set_default(", "default subscriber installation"),
        ("::init()", "subscriber initialization"),
        (".init()", "subscriber initialization"),
        (".try_init()", "subscriber initialization"),
        ("log::error!(", "log output"),
        ("log::warn!(", "log output"),
        ("log::info!(", "log output"),
        ("log::debug!(", "log output"),
        ("log::trace!(", "log output"),
    ] {
        if compact.contains(needle) {
            denied.push(label);
        }
    }
    for destination in ["stdout()", "stderr()"] {
        if compact.contains(destination)
            && (compact.contains("write!(") || compact.contains("writeln!("))
        {
            denied.push("write to process output");
        }
    }
    denied
}

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
        if [
            "tracing-subscriber",
            "env_logger",
            "log4rs",
            "simple_logger",
        ]
        .iter()
        .any(|dependency| cargo.contains(dependency))
        {
            violations.push(format!(
                "{} depends on an output-owning subscriber",
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
            for denied in denied_output(&text) {
                violations.push(format!("{} contains {denied}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "library code must stay quiet unless its host installs tracing:\n{}",
        violations.join("\n")
    );
}

#[tokio::test]
async fn constructing_the_public_harness_does_not_install_output() {
    assert!(
        !tracing::dispatcher::has_been_set(),
        "the test process begins without a global tracing subscriber"
    );
    let _call = CallHarness::new();
    assert!(
        !tracing::dispatcher::has_been_set(),
        "using the library must leave output ownership with the host"
    );
}

const QUIET_CHILD: &str = "SIPX_TESTKIT_QUIET_CHILD";

#[test]
fn quiet_control_child() {}

#[tokio::test]
async fn quiet_library_child() {
    use sipx_call::DialOptions;
    use sipx_sip::{Host, HostName, Uri};

    let mut harness = CallHarness::new();
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("valid host"),
    ));
    let pending = harness
        .dial(to, DialOptions::new("sip:caller@example.net", loopback))
        .await
        .expect("real dial reaches the application");
    let _established = pending
        .answer(loopback)
        .await
        .expect("real answer and ACK complete");
}

#[test]
fn the_static_ratchet_recognizes_output_and_subscriber_variants() {
    for source in [
        "print!(\"x\");",
        "std::print ! (\"x\");",
        "eprint!(\"x\");",
        "write!(std::io::stdout(), \"x\");",
        "let out = std::io::stdout(); writeln ! (out, \"x\");",
        "writeln!(&mut std::io::stderr(), \"x\");",
        "tracing::subscriber::set_global_default(s);",
        "tracing_subscriber::fmt().try_init();",
        "tracing_subscriber::fmt::init();",
    ] {
        assert!(!denied_output(source).is_empty(), "missed {source}");
    }
    assert!(
        denied_output("write!(&mut String::new(), \"x\");").is_empty(),
        "formatting into an owned buffer is not process output"
    );
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
