//! Getting the signalling path's numbers out of the process, beside the capture.
//!
//! `docs/specs/sip-transport.md` §12.3. `X-18` built the counters and `--capture`, and `X-51`
//! found what was still missing: **nothing outside each crate's own tests ever read either
//! snapshot.** `Handle::counters` and `Calls::counts` existed, were correct, and were unreachable
//! from a shell — so M12's clause, "every discard in the signalling path is counted **and
//! exportable next to** a capture of the traffic that caused it", described two features that
//! existed separately.
//!
//! # The shape, and why this one
//!
//! **A file, written once when the run ends, in the format the command was already speaking.**
//! Not a metrics endpoint and not an exposition format: §12 refuses to pick one of those for every
//! user of the library, and a CLI that picked one anyway would be making the same decision one
//! level up.
//!
//! **`--counters <FILE>` names it, and `--capture <FILE>` implies it.** Two rules rather than one,
//! because the two operators asking are different people:
//!
//! - Whoever ran `--capture` is assembling a bug report. §13 is explicit that the capture holds
//!   call content and identities, and it is going to be handed to someone outside the trust
//!   boundary it was recorded in. The numbers explaining it belong in the same bundle, so they are
//!   written to `<capture>.counters.json` without being asked for — one flag, one thing an
//!   operator does. Both paths are named in the command's own report, so nothing appears that the
//!   run did not mention.
//! - Whoever ran `--counters` alone must **not** be made to record call content to find out how
//!   much was discarded. That is the whole reason this is not simply a field of the capture: a
//!   rate is not personal data and demanding a pcapng to read one would be the worse default.
//!
//! # What is written, and what is deliberately not
//!
//! Every field of [`SignallingCounts`], flattened. Where the joined snapshot says "not measured",
//! this writes nothing rather than a zero — §12.2's first rule applied to the export: a counter
//! that overstates its own accuracy is worse than a missing one, because it will be used to rule a
//! cause out. The three commands here each hold a single call and use `sipx_call::dial` and
//! `answer` directly rather than a `Dispatcher`, so the dispatcher half is genuinely unmeasured,
//! and `dispatch_measured: false` says exactly that.

use std::path::{Path, PathBuf};

use sipx_call::SignallingCounts;

use crate::Args;
use crate::output::{Format, Report};

/// The suffix appended to a capture's path when `--capture` implies a counters file.
///
/// Appended rather than substituted, so `signalling.pcapng` and `signalling.pcapng.counters.json`
/// sort next to each other in a directory listing and neither can shadow the other.
const BESIDE_CAPTURE: &str = ".counters.json";

/// Where this run's counters go, if anywhere.
///
/// `--counters` wins over the capture's sibling: an operator who named a path meant that path.
pub(crate) fn destination(args: &Args<'_>) -> Option<PathBuf> {
    if let Some(path) = args.value("counters") {
        return Some(PathBuf::from(path));
    }
    args.value("capture")
        .map(|capture| PathBuf::from(format!("{capture}{BESIDE_CAPTURE}")))
}

/// The snapshot, flattened into the report shape both output formats already share.
///
/// Field names carry their section of the snapshot as a prefix, so a reader who has the spec open
/// can find each one: `shed_*` is §10, `unsent_*` and `discard_*` are §12.1, `capture_*` is §13.
pub(crate) fn report(counts: &SignallingCounts) -> Report {
    let transport = &counts.transport;
    let mut report = Report::new()
        .boolean("any_loss", counts.any_loss())
        .number("messages_in", cast(transport.messages_in()))
        .number("messages_out", cast(transport.messages_out()))
        .number("parse_failures", cast(transport.parse_failures()))
        .number("shed_requests", cast(transport.shed.requests))
        .number("shed_acks", cast(transport.shed.acks))
        .number("shed_unmatched", cast(transport.shed.unmatched))
        .number("unmatched_responses", cast(transport.unmatched_responses))
        .number("retransmissions_sent", cast(transport.retransmissions_sent))
        .number("timeout_b", cast(transport.timeouts.b))
        .number("timeout_f", cast(transport.timeouts.f))
        .number("timeout_h", cast(transport.timeouts.h))
        .number(
            "discard_transaction_events",
            cast(transport.discards.transaction_events),
        )
        .number("discard_unanswered", cast(transport.discards.unanswered))
        .number(
            "discard_no_destination",
            cast(transport.discards.no_destination),
        )
        .number(
            "discard_send_failures",
            cast(transport.discards.send_failures),
        )
        .number(
            "discard_stun_unmatched",
            cast(transport.discards.stun_unmatched),
        )
        .number("unsent_invite", cast(transport.unsent.invite))
        .number("unsent_ack", cast(transport.unsent.ack))
        .number("unsent_bye", cast(transport.unsent.bye))
        .number("unsent_cancel", cast(transport.unsent.cancel))
        .number("unsent_other", cast(transport.unsent.other))
        .number("capture_records", cast(transport.capture.records))
        .number("capture_dropped", cast(transport.capture.dropped))
        .number("capture_errors", cast(transport.capture.errors))
        .boolean("dispatch_measured", counts.dispatch.is_some());

    // Written only when a dispatcher was actually running. A zero here would claim the dialog
    // layer refused nothing, when the truth is that nobody asked it (§12.2).
    if let Some(dispatch) = counts.dispatch {
        report = report
            .number("dispatch_shed", cast(dispatch.shed))
            .number("dispatch_acks", cast(dispatch.acks))
            .number("dispatch_unmatched", cast(dispatch.unmatched))
            .number("dispatch_unsupported", cast(dispatch.unsupported))
            .number("dispatch_malformed", cast(dispatch.malformed))
            .number("dispatch_merged", cast(dispatch.merged));
    }
    report
}

/// An armed counters export: writes the file however the command ends.
///
/// # Why a guard and not a call at the end
///
/// Because the run that most needs the numbers is the one that failed, and a call at the end runs
/// only when the end is reached. The first version of this was exactly that call — placed after the
/// call had already succeeded — so `sipx dial --capture ./sig.pcapng --timeout 3 <dead peer>` wrote
/// the capture and **no counters at all**, on precisely the run a bug report is about. That is
/// Acceptance item 3's own words inverted, and it contradicted this module's claim that a counters
/// file which silently did not appear is the §13.2 failure one level up.
///
/// Arming a guard immediately after `bind` moves the decision from "did the command reach its happy
/// path" to "was there an endpoint to count", which is the question that actually determines
/// whether there are numbers worth writing. Every `return fail(…)` after the bind now takes the
/// file with it.
///
/// [`SignallingCounts::of`] rather than `with_dispatcher`: none of the three commands runs a
/// `Dispatcher`, and the snapshot says so instead of reporting zeros for it.
pub(crate) struct Export {
    destination: Option<PathBuf>,
    endpoint: sipx_transport::Handle,
    written: bool,
}

impl Export {
    /// Arm the export for this run. Cheap and inert when neither flag asked for one.
    pub(crate) fn arm(args: &Args<'_>, endpoint: &sipx_transport::Handle) -> Self {
        Self {
            destination: destination(args),
            endpoint: endpoint.clone(),
            written: false,
        }
    }

    /// Write the file now and name it in the report — the path where a report still exists.
    ///
    /// Named in the report because a file that appears without being mentioned is the surprise
    /// `--capture` implying a counters file would otherwise be. The error is returned rather than
    /// swallowed, so nothing reports success beside a file that was not written.
    pub(crate) fn into_report(mut self, report: Report) -> Result<Report, String> {
        let Some(path) = self.destination.clone() else {
            return Ok(report);
        };
        write(&path, &SignallingCounts::of(&self.endpoint))?;
        self.written = true;
        Ok(report.text("counters", path.display().to_string()))
    }
}

impl Drop for Export {
    /// The failure path: the command has already emitted whatever it was going to say, so the file
    /// is written and named on stderr rather than in a report that has gone.
    fn drop(&mut self) {
        if self.written {
            return;
        }
        let Some(path) = &self.destination else {
            return;
        };
        match write(path, &SignallingCounts::of(&self.endpoint)) {
            Ok(()) => tracing::info!(counters = %path.display(), "wrote the signalling counters"),
            // Loud, because this is the one failure that would leave an operator holding a capture
            // with nothing to explain it and no indication that anything was missing.
            Err(message) => tracing::error!("{message}"),
        }
    }
}

/// A counter as the report's number type.
///
/// Saturating rather than wrapping: a count past `i64::MAX` is not a real endpoint, and a negative
/// number in the file would be read as a different kind of fault entirely.
fn cast(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Write this run's counters to `path`, as JSON.
///
/// Always JSON, whichever format the command itself is speaking. The file is read by whoever picks
/// up the bug report rather than by the person who ran the command, and a machine-readable
/// artefact that changes shape depending on a flag the reader cannot see is worse than one that
/// does not.
///
/// A failure to write is returned rather than swallowed, because a counters file that silently did
/// not appear is the §13.2 failure this whole story is about, one level up.
pub(crate) fn write(path: &Path, counts: &SignallingCounts) -> Result<(), String> {
    let body = format!("{}\n", report(counts).render(Format::Json));
    std::fs::write(path, body).map_err(|error| format!("counters {}: {error}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    /// `--capture` alone puts the numbers beside the traffic they explain — the clause's "next to".
    #[test]
    fn a_capture_implies_a_counters_file_beside_it() {
        let raw = args(&["dial", "--capture", "/tmp/run/signalling.pcapng", "sip:a@b"]);
        let parsed = Args::new(&raw).expect("well formed");
        assert_eq!(
            destination(&parsed),
            Some(PathBuf::from("/tmp/run/signalling.pcapng.counters.json")),
            "an operator assembling a bug report should not have to ask twice"
        );
    }

    /// An operator who must not record call content can still have the numbers.
    #[test]
    fn counters_alone_needs_no_capture() {
        let raw = args(&["dial", "--counters", "/tmp/run/counts.json", "sip:a@b"]);
        let parsed = Args::new(&raw).expect("well formed");
        assert_eq!(
            destination(&parsed),
            Some(PathBuf::from("/tmp/run/counts.json"))
        );
    }

    /// A named path wins: it was named on purpose.
    #[test]
    fn an_explicit_path_beats_the_capture_sibling() {
        let raw = args(&[
            "dial",
            "--capture",
            "/tmp/run/signalling.pcapng",
            "--counters",
            "/tmp/elsewhere/counts.json",
            "sip:a@b",
        ]);
        let parsed = Args::new(&raw).expect("well formed");
        assert_eq!(
            destination(&parsed),
            Some(PathBuf::from("/tmp/elsewhere/counts.json"))
        );
    }

    #[test]
    fn neither_flag_writes_nothing() {
        let raw = args(&["dial", "sip:a@b"]);
        let parsed = Args::new(&raw).expect("well formed");
        assert_eq!(destination(&parsed), None);
    }
}
