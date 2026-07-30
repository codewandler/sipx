//! `sipx` — a command line SIP softphone.
//!
//! Scriptable by design. Every command reports its result as a line of JSON on request, uses a
//! distinct exit code per outcome, and keeps logging off stdout — so a shell can place a call,
//! assert on what happened, and branch on why it did not.
//!
//! # Stability
//!
//! sipx is pre-1.0, so this does not mean frozen; `1.0.0` is what freezes an interface and its
//! predicates are in `docs/roadmap.md`.
//!
//! **This crate's promise is its command-line surface, not its Rust API.** Nothing here is `pub`, it
//! ships no library target, and `cargo doc -p sipx-cli` renders under the binary name — so a reader
//! following a `sipx_cli` link finds nothing. The contract is the commands, flags, environment
//! variables and exit codes documented in `website/docs/reference/cli.md` and asserted in
//! `tests/cli.rs`.
//!
//! **Supported**: `register`, `dial`, `answer`, `peers`, their flags, `SIPX_PASSWORD`, the `--book`
//! lookup order and the exit codes.
//!
//! Refused rather than silently unsupported, because a flag that is accepted and dropped is worse than
//! one that errors: a `sips:` URI (`S-27`, no TLS transport here) and `dial --password` (`S-28`, a call
//! cannot answer a challenge yet).
//!

mod advertise;
mod answer;
mod dial;
mod output;
mod peers;
mod register;

use std::process::ExitCode;

use output::{Exit, Format};

/// Why this URI cannot be honoured securely, if it asks to be.
///
/// A `sips:` URI is not a hint. RFC 3261 §19.1.1 makes TLS on every hop the URI's *meaning*, and
/// §26.2.2 requires it. This CLI has no TLS transport to offer — `--tcp` selects TCP and there is no
/// `--tls` — so the only two honest answers are to use TLS or to refuse.
///
/// It used to do neither, in **both** commands that send: `dial` and `register` each strip `sips:` in
/// the same `or_else` as `sip:` and throw the distinction away, so the request went out in cleartext
/// and nothing said so (`S-27`). That is the one outcome the scheme exists to forbid, and it is
/// invisible to the person who asked for it — the call connects, the registration succeeds. On
/// `register` it is worse than on `dial`, because what travels is a digest credential.
///
/// Lives here rather than in either command because it is a policy both share; putting it in one of
/// them is how the second one came to be missed.
pub(crate) fn insecure_scheme_refusal(uri: &str) -> Option<String> {
    uri.strip_prefix("sips:").map(|_| {
        format!(
            "{uri} asks for TLS on every hop, and this CLI has no TLS transport — refusing rather \
             than sending it in the clear. Use a sip: URI, or the library, which does have TLS and WSS"
        )
    })
}

const USAGE: &str = "\
sipx — a command line SIP softphone

USAGE:
    sipx <COMMAND> [OPTIONS]

COMMANDS:
    register    Register with a registrar
    dial        Place a call
    answer      Wait for and answer a call
    peers       List what can be called
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
        // Not async, and deliberately so: listing what can be called reads a file and opens no
        // socket. The registrar and local-link sources are separate stories.
        Some("peers") => peers::run(&args, format),
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

/// Whether these arguments ask for help.
///
/// Read from the raw arguments, and before they are validated: `--help` is a request for
/// documentation, so refusing to print it because some *other* flag on the line is malformed
/// answers a question nobody asked.
#[must_use]
pub(crate) fn wants_help(raw: &[String]) -> bool {
    raw.iter().any(|arg| arg == "--help" || arg == "-h")
}

/// A command's arguments, or the exit it should return instead of running.
///
/// Every command opens the same way — answer `--help`, then refuse an argument list that cannot be
/// honoured — and it lives here rather than four times over because the *order* is the part worth
/// getting right once. Help is answered first so that `sipx dial --help --play` documents the
/// command instead of complaining about `--play`.
///
/// `Err` carries the exit to return, which is `Exit::Success` when help was printed: from the
/// caller's side both arms are "stop here", and distinguishing them would only give four commands
/// the chance to disagree about it.
pub(crate) fn arguments<'a>(
    raw: &'a [String],
    help: &str,
    format: Format,
) -> Result<Args<'a>, Exit> {
    if wants_help(raw) {
        print!("{help}");
        return Err(Exit::Success);
    }
    Args::new(raw).map_err(|message| output::fail(format, Exit::Usage, &message))
}

/// Shared argument parsing.
///
/// Deliberately small rather than a dependency: sipx needs flags and one positional, and a
/// parser for that is smaller than the code to configure a general one.
///
/// **Holding an `Args` means every valued flag on the line was given a value.** That invariant is
/// the reason the constructor is fallible. `value` used to answer `None` both for "the flag was
/// last, so nothing followed it" and for "the flag was absent" — one answer for two different
/// facts — so every caller took its absent-branch and the command ran on a default nobody typed:
/// `sipx register sip:alice@example.com --outbound --instance` exited 0 having generated an
/// instance URN that was never asked for (`S-30`). Establishing it once here rather than at each
/// call site is what stops the next flag from rediscovering it: a caller cannot forget a check it
/// does not have to make, and `None` from `value` now means absent and nothing else.
#[derive(Debug)]
pub(crate) struct Args<'a> {
    raw: &'a [String],
}

impl<'a> Args<'a> {
    /// Wrap the raw arguments, refusing any valued flag that was given no value.
    ///
    /// Both ways a value goes missing are refused: nothing following the flag at all, and an empty
    /// value in either form (`--target=` or `--target ""`).
    ///
    /// The empty value is refused for *every* flag rather than for some. Nothing in `VALUED_FLAGS`
    /// has a meaningful empty value — not a password, not a path, not an address, not a count of
    /// seconds — and omitting the flag is already how a caller asks for the default, so an empty
    /// value can only be an accident. It is a common one: an unset shell variable expands to
    /// exactly this, which is how `--target "$ADDR"` arrives here. A per-flag exception list would
    /// be a second registry to hold in step with this one, for no case that wants it.
    ///
    /// The guarantee reaches only as far as `VALUED_FLAGS`, which is what
    /// `every_valued_flag_in_the_help_text_is_registered` exists to keep complete.
    pub(crate) fn new(raw: &'a [String]) -> Result<Self, String> {
        for (index, arg) in raw.iter().enumerate() {
            let Some(body) = arg.strip_prefix("--") else {
                continue;
            };
            // `--flag=value` carries its own value, even when that value is empty; `--flag value`
            // takes the next argument, if the caller left one there.
            let (name, given) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, raw.get(index + 1).map(String::as_str)),
            };
            let flag = format!("--{name}");
            if !VALUED_FLAGS.contains(&flag.as_str()) {
                continue;
            }
            match given {
                None => {
                    return Err(format!(
                        "{flag} takes a value and nothing followed it. A flag in final position is \
                         not an absent one — reading it as absent would run this command on a \
                         default that was not asked for"
                    ));
                }
                Some("") => {
                    return Err(format!(
                        "{flag} takes a value and was given an empty one. No flag here has a \
                         meaningful empty value, and an unset shell variable expands to exactly \
                         this, so it is refused rather than read as absent"
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(Self { raw })
    }

    /// The value of `--name`, if given.
    ///
    /// `None` means the flag was absent. It cannot mean "given with no value" — `new` refused that
    /// argument list — so a caller may take its absent-branch on `None` without checking twice.
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
///
/// A flag missing from this list has its *value* read as the positional argument, which turns
/// `sipx dial --timeout 30 sip:bob@host` into an attempt to call "30". There is a test below
/// asserting every flag the help text documents appears here, because the failure is silent
/// and the list is easy to forget.
const VALUED_FLAGS: &[&str] = &[
    "--password",
    "--play",
    "--record",
    "--duration",
    "--timeout",
    "--wait",
    "--dtmf",
    "--from",
    "--expires",
    "--local",
    "--target",
    "--book",
    "--instance",
    "--push-provider",
    "--push-prid",
    "--push-param",
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// `Args` over an argument list that is expected to be well formed.
    fn parsed(raw: &[String]) -> Args<'_> {
        Args::new(raw).expect("a well formed argument list")
    }

    #[test]
    fn a_flag_value_is_read_in_either_form() {
        let raw = args(&["dial", "--password", "secret", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), Some("secret"));

        let raw = args(&["dial", "--password=secret", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), Some("secret"));
    }

    #[test]
    fn a_missing_flag_reads_as_absent() {
        let raw = args(&["dial", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), None);
        assert!(!parsed(&raw).flag("json"));
    }

    /// A flag's value must not be mistaken for the positional argument. Getting this wrong
    /// makes `sipx dial --password secret sip:a@b` try to call "secret".
    #[test]
    fn a_flag_value_is_not_mistaken_for_the_positional() {
        let raw = args(&["dial", "--password", "secret", "sip:bob@example.com"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "sip:bob@example.com", "--password", "secret"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "--json", "sip:bob@example.com"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));
    }

    /// Every flag that takes a value must be listed, or its value is read as the positional
    /// argument. The failure is silent — the command tries to call "30" — so this checks the
    /// list against the help text of every command rather than trusting anyone to remember.
    #[test]
    fn every_valued_flag_in_the_help_text_is_registered() {
        let help = format!(
            "{}{}{}{}{}",
            USAGE,
            crate::register::HELP,
            crate::dial::HELP,
            crate::answer::HELP,
            crate::peers::HELP
        );

        // A documented flag takes a value if its help line shows a placeholder after it.
        let mut documented = Vec::new();
        for line in help.lines() {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("--") else {
                continue;
            };
            let Some((flag, tail)) = rest.split_once(char::is_whitespace) else {
                continue;
            };
            if tail.trim_start().starts_with('<') {
                documented.push(format!("--{flag}"));
            }
        }
        assert!(
            !documented.is_empty(),
            "the help text lists no valued flags"
        );

        for flag in documented {
            assert!(
                VALUED_FLAGS.contains(&flag.as_str()),
                "{flag} takes a value but is missing from VALUED_FLAGS, so its value would be \
                 read as the positional argument"
            );
        }
    }

    /// Every flag that takes a value refuses to be given none — checked over `VALUED_FLAGS`
    /// itself, so a flag added to the registry later is covered without anyone adding a case.
    ///
    /// Deriving it from the registry rather than from the four flags that exposed the defect is
    /// the point: the hole was in `Args::value`, so it belonged to every flag at once, and an
    /// enumeration here would have to be remembered. `tests/cli.rs` asserts the same rule through
    /// the binary, where the exit code lives; this one asserts it over the whole list.
    #[test]
    fn every_valued_flag_is_refused_when_it_is_given_no_value() {
        for flag in VALUED_FLAGS.iter().copied() {
            // Nothing follows the flag at all.
            let raw = args(&["register", flag]);
            let error =
                Args::new(&raw).expect_err("a valued flag in final position was given no value");
            assert!(
                error.contains(flag),
                "the refusal must name {flag}: {error}"
            );

            // An empty value, in both forms a shell can hand one over.
            let joined = format!("{flag}=");
            for items in [
                &["register", joined.as_str(), "sip:a@b.c"][..],
                &["register", flag, "", "sip:a@b.c"][..],
            ] {
                let raw = args(items);
                let error = Args::new(&raw).expect_err("an empty value is not a value");
                assert!(
                    error.contains(flag),
                    "the refusal must name {flag}: {error}"
                );
            }
        }
    }

    /// The refusal is about flags that take a value, and must not spread to the ones that do not:
    /// `--tcp` in final position is a complete argument, and `--json=` is not any flag's problem.
    #[test]
    fn a_valueless_flag_is_untouched_by_the_rule() {
        let raw = args(&["register", "sip:a@b.c", "--outbound", "--tcp"]);
        assert!(parsed(&raw).flag("tcp"));

        let raw = args(&["dial", "sip:a@b.c", "--json"]);
        assert!(parsed(&raw).flag("json"));

        // `--json=` is nothing this rule has an opinion about: the flag takes no value, so an
        // empty one is not a value gone missing.
        let raw = args(&["dial", "sip:a@b.c", "--json="]);
        assert!(Args::new(&raw).is_ok());
    }

    /// A positional argument may contain an `=` — a URI parameter is spelled with one — and that
    /// must not be mistaken for a flag being assigned an empty value.
    #[test]
    fn an_equals_in_the_positional_is_not_a_flag() {
        let raw = args(&["register", "sip:alice@example.com;transport=tcp"]);
        assert_eq!(
            parsed(&raw).positional(),
            Some("sip:alice@example.com;transport=tcp")
        );
    }

    /// The case the missing entry actually breaks.
    #[test]
    fn a_timeout_before_the_uri_does_not_become_the_uri() {
        let raw = args(&["dial", "--timeout", "30", "sip:bob@192.0.2.1:5060"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@192.0.2.1:5060"));
        assert_eq!(parsed(&raw).value("timeout"), Some("30"));

        let raw = args(&["answer", "--wait", "20", "--json"]);
        assert_eq!(
            parsed(&raw).positional(),
            None,
            "answer takes no positional"
        );
    }

    #[test]
    fn a_numeric_option_parses_or_reads_as_absent() {
        let raw = args(&["dial", "--duration", "30"]);
        assert_eq!(parsed(&raw).number("duration"), Some(30));

        let raw = args(&["dial", "--duration", "thirty"]);
        assert_eq!(parsed(&raw).number("duration"), None);
    }
}
