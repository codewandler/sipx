//! Write a fixture CA and a server certificate for the interop harness.
//!
//! Run by `tests/interop/run.sh` before the server starts:
//!
//! ```text
//! cargo run -p sipx-testkit --example issue-certs -- <directory> <host>
//! ```
//!
//! It exists so the interop certificates come from the *same* fixture authority the unit tests
//! use. The alternative — an `openssl` invocation in a shell script — is a second way of
//! issuing certificates, and the interesting failures in TLS are exactly the ones where two
//! ways of building a certificate differ in a detail nobody looked at.

// A build helper, not production code. Every failure here means the harness cannot start, and
// the useful thing to do about it is stop and say which file could not be written.
#![allow(clippy::expect_used, clippy::panic, clippy::print_stdout)]

use std::path::Path;

use sipx_testkit::certs::Ca;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: issue-certs <directory> <host>");
    let host = args.next().expect("usage: issue-certs <directory> <host>");
    let dir = Path::new(&dir);

    std::fs::create_dir_all(dir).expect("the output directory");

    let ca = Ca::new();
    let (cert, key) = ca.issue_for(&host);

    write(dir, "ca.pem", &ca.pem());
    write(dir, "server.pem", &cert);
    write(dir, "server.key", &key);

    println!("issued a certificate for {host} under {}", dir.display());
}

fn write(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}
