//! Regression guard for `X-110`: command-line syntax has one declarative owner.

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn production_has_no_handwritten_argument_parser() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = [
            "std::env::args(",
            "std::env::args_os(",
            "struct Args<'",
            "fn arguments<'",
            "fn wants_help(",
            "VALUED_FLAGS",
            "NUMERIC_FLAGS",
            "args.value(",
            "args.values(",
            "args.positional()",
            "args.number(",
            "match args.first()",
            ".first().map(String::as_str)",
            "Some(\"register\") =>",
        ];
        let mut findings = Vec::new();
        for entry in fs::read_dir(src).expect("read sipx-cli source") {
            let path = entry.expect("read source entry").path();
            if path.extension().and_then(|part| part.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read UTF-8 Rust source");
            for needle in forbidden {
                if text.contains(needle) {
                    findings.push(format!("{} contains {needle:?}", path.display()));
                }
            }
        }
        assert!(
            findings.is_empty(),
            "handwritten argument parsing returned:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn declarative_parser_is_the_command_line_owner() {
        let parser = include_str!("../src/cli.rs");
        assert!(parser.contains("derive(Debug, Parser)"));
        assert!(parser.contains("derive(Debug, Subcommand)"));
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_input_is_a_usage_error_not_a_panic() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;
        use std::process::Command;

        let invalid = OsString::from_vec(vec![b's', b'i', b'p', b':', 0xff]);
        let output = Command::new(env!("CARGO_BIN_EXE_sipx"))
            .arg("dial")
            .arg(invalid)
            .output()
            .expect("sipx process runs");

        assert_eq!(output.status.code(), Some(2), "usage exit");
        assert!(output.stdout.is_empty(), "usage emits no result");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("invalid UTF-8"), "{stderr}");
    }
}
