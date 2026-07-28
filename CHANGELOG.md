# Changelog

All notable changes to sipx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Cargo workspace with the ten `sipx-*` crates, shared lints (`unsafe_code = "forbid"`)
  and `MIT OR Apache-2.0` licensing.
- CI: rustfmt, clippy (`-D warnings`), tests, MSRV check, `cargo-deny`, and a provenance
  gate.
