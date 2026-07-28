//! `sipx` — a command line SIP softphone.
//!
//! Scriptable by design. Every command reports its result as a line of JSON on request, uses a
//! distinct exit code per outcome, and keeps logging off stdout — so a shell can place a call,
//! assert on what happened, and branch on why it did not.

mod answer;
mod dial;
mod output;
mod register;

use std::process::ExitCode;

use output::{Exit, Format};

const USAGE: &str = "\
sipx — a command line SIP softphone

USAGE:
    sipx <COMMAND> [OPTIONS]

COMMANDS:
    register    Register with a registrar
    dial        Place a call
    answer      Wait for and answer a call
    help        Show this message
    version     Show the version

GLOBAL OPTIONS:
    --json      Report results as JSON on stdout
    -v, -vv     Log to stderr; never to stdout, which carries results
    -h, --help  Show help for a command

EXIT CODES:
    0  success        3  rejected       5  timeout
    1  failed         4  unauthorized   6  busy
    2  usage
";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let format = if args.iter().any(|a| a == "--json") {
        Format::Json
    } else {
        Format::Text
    };

    // Logging goes to stderr. One stray line on stdout turns valid JSON into a parse error at
    // the far end of a pipe, where the cause is invisible.
    let verbosity = args.iter().filter(|a| a.starts_with("-v")).count();
    init_logging(verbosity);

    let exit = match args.first().map(String::as_str) {
        Some("register") => register::run(&args, format).await,
        Some("dial") => dial::run(&args, format).await,
        Some("answer") => answer::run(&args, format).await,
        Some("version" | "--version" | "-V") => {
            println!("sipx {}", env!("CARGO_PKG_VERSION"));
            Exit::Success
        }
        Some("help" | "--help" | "-h") | None => {
            print!("{USAGE}");
            Exit::Success
        }
        Some(unknown) => {
            eprint!("{USAGE}");
            output::fail(format, Exit::Usage, &format!("unknown command: {unknown}"))
        }
    };

    ExitCode::from(u8::try_from(exit.code()).unwrap_or(1))
}

fn init_logging(verbosity: usize) {
    let level = match verbosity {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Shared argument parsing.
///
/// Deliberately small rather than a dependency: sipx needs flags and one positional, and a
/// parser for that is smaller than the code to configure a general one.
#[derive(Debug)]
pub(crate) struct Args<'a> {
    raw: &'a [String],
}

impl<'a> Args<'a> {
    /// Wrap the raw arguments, skipping the subcommand.
    #[must_use]
    pub(crate) fn new(raw: &'a [String]) -> Self {
        Self { raw }
    }

    /// The value of `--name`, if given.
    #[must_use]
    pub(crate) fn value(&self, name: &str) -> Option<&'a str> {
        let flag = format!("--{name}");
        let mut iter = self.raw.iter();
        while let Some(arg) = iter.next() {
            if arg == &flag {
                return iter.next().map(String::as_str);
            }
            if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
                return Some(rest);
            }
        }
        None
    }

    /// Whether `--name` is present.
    #[must_use]
    pub(crate) fn flag(&self, name: &str) -> bool {
        let flag = format!("--{name}");
        self.raw.iter().any(|arg| arg == &flag)
    }

    /// The first argument that is not a flag or a flag's value.
    #[must_use]
    pub(crate) fn positional(&self) -> Option<&'a str> {
        let mut skip_next = false;
        for (index, arg) in self.raw.iter().enumerate() {
            if index == 0 {
                continue; // the subcommand
            }
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg.starts_with('-') {
                // A flag with a separate value consumes the next argument.
                skip_next = !arg.contains('=') && VALUED_FLAGS.iter().any(|f| arg == f);
                continue;
            }
            return Some(arg);
        }
        None
    }

    /// A numeric option.
    #[must_use]
    pub(crate) fn number(&self, name: &str) -> Option<u64> {
        self.value(name)?.parse().ok()
    }
}

/// Flags that take a separate value, so positional detection can skip past them.
const VALUED_FLAGS: &[&str] = &[
    "--password",
    "--play",
    "--record",
    "--duration",
    "--dtmf",
    "--from",
    "--expires",
    "--local",
    "--target",
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_flag_value_is_read_in_either_form() {
        let raw = args(&["dial", "--password", "secret", "sip:a@b"]);
        assert_eq!(Args::new(&raw).value("password"), Some("secret"));

        let raw = args(&["dial", "--password=secret", "sip:a@b"]);
        assert_eq!(Args::new(&raw).value("password"), Some("secret"));
    }

    #[test]
    fn a_missing_flag_reads_as_absent() {
        let raw = args(&["dial", "sip:a@b"]);
        assert_eq!(Args::new(&raw).value("password"), None);
        assert!(!Args::new(&raw).flag("json"));
    }

    /// A flag's value must not be mistaken for the positional argument. Getting this wrong
    /// makes `sipx dial --password secret sip:a@b` try to call "secret".
    #[test]
    fn a_flag_value_is_not_mistaken_for_the_positional() {
        let raw = args(&["dial", "--password", "secret", "sip:bob@example.com"]);
        assert_eq!(Args::new(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "sip:bob@example.com", "--password", "secret"]);
        assert_eq!(Args::new(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "--json", "sip:bob@example.com"]);
        assert_eq!(Args::new(&raw).positional(), Some("sip:bob@example.com"));
    }

    #[test]
    fn a_numeric_option_parses_or_reads_as_absent() {
        let raw = args(&["dial", "--duration", "30"]);
        assert_eq!(Args::new(&raw).number("duration"), Some(30));

        let raw = args(&["dial", "--duration", "thirty"]);
        assert_eq!(Args::new(&raw).number("duration"), None);
    }
}
