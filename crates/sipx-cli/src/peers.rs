//! `sipx peers` — who is there to call.
//!
//! This command consults nothing but a file. It is deliberately not async and opens no socket:
//! the registrar source (`S-24`) and the local link (`T-24`) are separate stories, and the peer
//! book has to be useful with neither.
//!
//! **The book's format.** One peer per line, a name and a URI separated by whitespace, `#` for
//! a comment, blank lines ignored:
//!
//! ```text
//! # who this phone knows about
//! alice   sip:alice@192.0.2.17:5060
//! bob     sips:bob@example.com
//! ```
//!
//! Chosen because principle 6 wants it usable from a shell, and this is the shape a shell
//! already has verbs for — `echo "alice sip:alice@host" >> "$book"` writes it and
//! `while read -r name uri` reads it. Nothing structured (TOML, JSON, INI) is writable from a
//! script without either a parser or a `sed` invocation that corrupts the file on the second
//! run, and none of them is worth a dependency for two fields. The reasoning is recorded in
//! `docs/designs/discovery.md`.
//!
//! **Strict, not forgiving.** A line that is not a name and a URI fails the whole listing,
//! naming the line. Skipping it would print a list that is short by one and says so nowhere,
//! and the design is explicit that a partial list must never be presented as complete.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx peers — list what can be called

USAGE:
    sipx peers [OPTIONS]

OPTIONS:
    --book <FILE>   Read this peer book rather than the default one
    --json          Report as JSON, one object per peer
    --help          Show this message

THE PEER BOOK:
    One peer per line: a name, whitespace, and the URI to dial. Blank lines and lines
    starting with `#` are ignored.

        # who this phone knows about
        alice   sip:alice@192.0.2.17:5060

    Looked for in --book, then $SIPX_PEERS, then $XDG_CONFIG_HOME/sipx/peers, then
    $HOME/.config/sipx/peers. A book that cannot be read is an error and not an empty
    list — a fresh machine with no book has not told you there is nobody to call.
";

/// Where a peer was learned from.
///
/// Carried on every entry even though there is only one variant today. A peer typed into a file
/// and one that answered a multicast query thirty seconds ago are not the same kind of fact, and
/// a list that flattens them cannot gain `S-24`'s registrar or `T-24`'s local link without
/// breaking every script that already reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Written down locally, by a person or a script.
    Book,
}

impl Source {
    /// The stable name a script matches on.
    fn as_str(self) -> &'static str {
        match self {
            Self::Book => "book",
        }
    }
}

/// One thing that can be called.
///
/// A name for a person to use and `P-6` to look up, and a URI that is enough to dial — nothing
/// else. This is a phone book and not a dial plan: it is consulted when someone names a peer,
/// never while routing an inbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Peer {
    /// What to call it by.
    pub(crate) name: String,
    /// What to dial.
    pub(crate) uri: String,
    /// Where it was learned from.
    pub(crate) source: Source,
}

impl Peer {
    /// The entry as a result line, in the form `P-1` set for every other command.
    fn report(&self) -> Report {
        Report::new()
            .text("status", "peer")
            .text("name", self.name.as_str())
            .text("uri", self.uri.as_str())
            .text("source", self.source.as_str())
    }
}

/// Why the peer book could not be listed.
///
/// A typed error rather than an empty list: "I could not read the book" and "the book is empty"
/// are different facts, and a script that cannot tell them apart will call nobody and report
/// success.
#[derive(Debug)]
pub(crate) enum Error {
    /// Nothing said where the book is, and the environment names no default.
    NoLocation,
    /// The book is where it should be and could not be read.
    Unreadable {
        /// The path that was tried.
        path: PathBuf,
        /// What the filesystem said.
        cause: std::io::Error,
    },
    /// A line in the book is not a peer.
    Malformed {
        /// The book being read.
        path: PathBuf,
        /// The offending line, counting from one, as an editor counts.
        line: usize,
        /// What was expected there.
        reason: &'static str,
    },
}

impl Error {
    /// The exit code this failure leaves.
    ///
    /// A missing location is fixable on the command line, so it is a usage error; a book that
    /// exists and cannot be used is not the caller's spelling and exits `failed`.
    fn exit(&self) -> Exit {
        match self {
            Self::NoLocation => Exit::Usage,
            Self::Unreadable { .. } | Self::Malformed { .. } => Exit::Failed,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocation => write!(
                f,
                "no peer book: pass --book <FILE> or set SIPX_PEERS, since neither \
                 XDG_CONFIG_HOME nor HOME is set"
            ),
            Self::Unreadable { path, cause } => {
                write!(f, "cannot read the peer book {}: {cause}", path.display())
            }
            Self::Malformed { path, line, reason } => {
                write!(f, "{}:{line}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { cause, .. } => Some(cause),
            Self::NoLocation | Self::Malformed { .. } => None,
        }
    }
}

pub(crate) fn run(raw: &[String], format: Format) -> Exit {
    // A `--book` with no path is refused rather than silently falling through to the environment
    // and reporting whichever book happens to be there (`S-30`).
    let args = match crate::arguments(raw, HELP, format) {
        Ok(args) => args,
        Err(exit) => return exit,
    };

    let peers = match load(args.value("book")) {
        Ok(peers) => peers,
        Err(error) => return fail(format, error.exit(), &error.to_string()),
    };

    for (index, peer) in peers.iter().enumerate() {
        // One JSON object per line, which is what `P-1` set and what lets a reader split on
        // newlines. In the human form the entries are blocks, so a blank line separates them.
        if format == Format::Text && index > 0 {
            println!();
        }
        peer.report().emit(format);
    }

    Exit::Success
}

/// Read the book from wherever it lives.
fn load(explicit: Option<&str>) -> Result<Vec<Peer>, Error> {
    let path = locate(
        explicit,
        std::env::var("SIPX_PEERS").ok(),
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
    )?;
    let contents = std::fs::read_to_string(&path).map_err(|cause| Error::Unreadable {
        path: path.clone(),
        cause,
    })?;
    parse(&path, &contents)
}

/// Where the book lives.
///
/// An explicit path wins, then the environment, then the XDG config directory — the same order
/// `register` uses for a password, and for the same reason: the flag is the convenience and the
/// environment is what a script sets once.
fn locate(
    explicit: Option<&str>,
    from_env: Option<String>,
    xdg_config_home: Option<String>,
    home: Option<String>,
) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = from_env.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(config) = xdg_config_home.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(config).join("sipx").join("peers"));
    }
    if let Some(home) = home.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("sipx")
            .join("peers"));
    }
    Err(Error::NoLocation)
}

/// Turn a book into peers, or say which line is not one.
fn parse(path: &Path, contents: &str) -> Result<Vec<Peer>, Error> {
    let mut peers = Vec::new();

    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let malformed = |reason| Error::Malformed {
            path: path.to_path_buf(),
            // Counting from one, as an editor counts.
            line: index + 1,
            reason,
        };

        let mut fields = line.split_whitespace();
        let (Some(name), Some(uri)) = (fields.next(), fields.next()) else {
            return Err(malformed(
                "a peer is a name and a URI, e.g. `alice sip:alice@192.0.2.17:5060`",
            ));
        };
        // A SIP URI contains no whitespace, so a third field is a trailing comment or a typo.
        // Taking the rest of the line as the URI would produce something undiallable and say
        // nothing about it until someone tried to call it.
        if fields.next().is_some() {
            return Err(malformed(
                "a peer is two fields; comments go on their own line, starting with `#`",
            ));
        }
        if !(uri.starts_with("sip:") || uri.starts_with("sips:")) {
            return Err(malformed("a peer's URI must start with `sip:` or `sips:`"));
        }

        peers.push(Peer {
            name: name.to_owned(),
            uri: uri.to_owned(),
            source: Source::Book,
        });
    }

    Ok(peers)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn book(contents: &str) -> Result<Vec<Peer>, Error> {
        parse(Path::new("/tmp/peers"), contents)
    }

    #[test]
    fn a_line_becomes_a_peer_carrying_the_source_it_came_from() {
        let peers = book("alice   sip:alice@192.0.2.17:5060\n").expect("a peer");
        assert_eq!(
            peers,
            vec![Peer {
                name: "alice".to_owned(),
                uri: "sip:alice@192.0.2.17:5060".to_owned(),
                source: Source::Book,
            }]
        );
        assert_eq!(peers[0].source.as_str(), "book");
    }

    /// File order, not sorted: it is the order the user wrote, it is stable between runs, and a
    /// script that appended a peer finds it where it put it.
    #[test]
    fn peers_are_listed_in_the_order_the_book_gives_them() {
        let peers = book("bob sip:bob@example.com\nalice sips:alice@example.com\n").expect("peers");
        assert_eq!(
            peers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["bob", "alice"]
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let peers = book(
            "# who this phone knows about\n\
             \n\
             alice sip:alice@example.com\n\
             \t  \n\
                # indented comments too\n",
        )
        .expect("peers");
        assert_eq!(peers.len(), 1);
    }

    /// A readable book with nothing in it is an empty list and a success. That is a different
    /// fact from a book that could not be read, which is the failure below.
    #[test]
    fn an_empty_book_is_an_empty_list_and_not_an_error() {
        assert_eq!(book("").expect("no peers"), vec![]);
        assert_eq!(book("# nobody yet\n").expect("no peers"), vec![]);
    }

    #[test]
    fn a_line_that_is_not_a_peer_names_the_line_it_failed_on() {
        let error = book("alice sip:alice@example.com\nbob\n").expect_err("not a peer");
        let Error::Malformed { line, .. } = error else {
            panic!("expected a malformed line, got {error:?}");
        };
        assert_eq!(line, 2, "counted the way an editor counts");
    }

    /// Taking the rest of the line as the URI would make this peer `sip:a@b # home`, which
    /// nothing can dial and nothing complains about until someone tries.
    #[test]
    fn a_trailing_comment_is_refused_rather_than_glued_onto_the_uri() {
        let error = book("alice sip:alice@example.com # home\n").expect_err("two fields only");
        assert!(
            error.to_string().contains("own line"),
            "the error must say where a comment goes: {error}"
        );
    }

    #[test]
    fn something_that_is_not_a_sip_uri_is_refused() {
        for bad in ["alice example.com", "alice tel:+15551234", "alice alice"] {
            let error = book(bad).expect_err("not a URI");
            assert!(error.to_string().contains("sip:"), "{bad}: {error}");
        }
    }

    #[test]
    fn an_explicit_path_wins_over_everything_else() {
        let path = locate(
            Some("/books/mine"),
            Some("/books/env".to_owned()),
            Some("/config".to_owned()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/books/mine"));
    }

    #[test]
    fn the_environment_wins_over_the_config_directory() {
        let path = locate(
            None,
            Some("/books/env".to_owned()),
            Some("/config".to_owned()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/books/env"));
    }

    #[test]
    fn the_default_is_the_xdg_config_path() {
        let path = locate(None, None, Some("/config".to_owned()), None).expect("a path");
        assert_eq!(path, PathBuf::from("/config/sipx/peers"));

        let path = locate(None, None, None, Some("/home/someone".to_owned())).expect("a path");
        assert_eq!(path, PathBuf::from("/home/someone/.config/sipx/peers"));
    }

    /// An unset variable and one set to nothing are the same intent, and treating the empty
    /// string as a path looks for the book at the filesystem root.
    #[test]
    fn an_empty_variable_is_the_same_as_an_unset_one() {
        let path = locate(
            None,
            Some(String::new()),
            Some(String::new()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/home/someone/.config/sipx/peers"));

        assert!(matches!(
            locate(None, None, None, None),
            Err(Error::NoLocation)
        ));
    }

    /// The exits a script branches on. A missing location is something the caller can fix on
    /// the command line; a book that will not parse is not.
    #[test]
    fn a_book_that_cannot_be_read_never_exits_zero() {
        for error in [
            Error::NoLocation,
            Error::Unreadable {
                path: PathBuf::from("/tmp/peers"),
                cause: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
            Error::Malformed {
                path: PathBuf::from("/tmp/peers"),
                line: 1,
                reason: "nope",
            },
        ] {
            assert_ne!(error.exit(), Exit::Success, "{error}");
        }
        assert_eq!(Error::NoLocation.exit(), Exit::Usage);
    }

    /// The two forms carry the same facts — `P-1`'s rule, applied to a list.
    #[test]
    fn both_forms_carry_the_name_the_uri_and_the_source() {
        let peer = Peer {
            name: "alice".to_owned(),
            uri: "sip:alice@192.0.2.17:5060".to_owned(),
            source: Source::Book,
        };
        let json = peer.report().render(Format::Json);
        let text = peer.report().render(Format::Text);

        for fact in ["alice", "sip:alice@192.0.2.17:5060", "book"] {
            assert!(json.contains(fact), "{fact} missing from {json}");
            assert!(text.contains(fact), "{fact} missing from {text}");
        }
        assert!(!json.contains('\n'), "one line per peer: {json}");
    }
}
