# Changelog

All notable changes to sipx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Published crate errors can grow additively, and every package has its own landing page
  (`A-9`).** Twenty-six public error enums are now `#[non_exhaustive]`; the sole exhaustive
  exception states its closed-domain reason beside the type, and a source-level guard refuses an
  unclassified enum. All eleven published crates now ship a README that links to the crate's one
  stability contract and states what the crate deliberately does not do. The front-door guard
  treats each README summary as a fifth checked claim surface — 55 in total — and packaging tests
  verify that Cargo includes every file.

## [1.0.0-alpha.4] — 2026-08-01

### Added

- **The fixed-sleep rule is enforced rather than swept for (`X-44`)** — `docs/designs/media.md` has
  said since `X-28` that a fixed wall-clock duration may bound a failure or define silence and may
  not stand in for a happens-before. It was swept twice, `0.12.0` claimed the workspace was clean,
  and nothing enforced it — so two fresh violations landed in the wave after the second sweep, one of
  them in production code. `scripts/check-fixed-sleep.py` is a 23rd gate step and a CI job.
  - **It reads the shape, not the word `sleep`.** `tokio::time::sleep`, `std::thread::sleep`,
    `sleep_until`, a bare `interval.tick()`, a hand-rolled deadline spin and a wait hidden behind a
    private helper in another crate are all refused. It also reports a loop whose every pass is
    bounded by a *relative* timeout, which is the only reason it catches `X-40`'s regression — that
    defect contains no sleep at all.
  - **It covers `src/` as well as `tests/`**, because this workspace keeps much of its suite in
    `#[cfg(test)]` modules beside the code, and 7 of the first 30 hits were under `src/`.
  - **There is no suppression list, under any name** (`X-35`'s standard). A site says at its own line
    which of four questions its duration answers, or the gate is red. The first run found **30
    clock-decided assertions and 2 that said which** — two were real defects and are now causal
    waits; the rest carry their reason.
  - Review found the first version evadable twice over — by moving a wait wrapper one file away, and
    by naming a constant documented in another crate — and both are closed, the second by deleting
    the cross-file lookup rather than narrowing it.

- **M10 — Reachable is delivered, on evidence rather than on mechanism (`X-52`)** — `X-50` checked
  M10's exit criterion against the tests meant to demonstrate it and found two of its three clauses
  held by the mechanism underneath them: the GRUU test was an `OPTIONS` against one agent, and the
  push test stopped when the INVITE arrived with nothing answering it. Both clauses are now
  demonstrated as they are written, in `crates/sipx-cli/tests/reachable.rs`.
  - **Two registrations of one address of record, called individually.** A call placed at one
    instance's GRUU is answered by that instance with audio both ways, and the other instance — its
    registration equally current — never sees the INVITE. The contrast carries it: the same routing
    applied to the address of record resolves to *both* bindings.
  - **A push into an answered call.** A client holding no connection is woken, refreshes its binding,
    answers the call it was woken for, and carries audio both ways, in RFC 8599 §4.1.3's order.
  - **Neither `T-20` nor `T-21` is reopened.** Each delivered the mechanism it was written for; this
    is the composition on top, which was never in their Acceptance.
  - Both tests passed on first run — M10 was short of evidence, not of behaviour — so each was
    **falsified against a real mutation** rather than trusted for passing: selecting a binding by
    position instead of by `+sip.instance` makes both instances adopt one GRUU, and discarding RFC
    8599 §8.2's PURR fails the push test.
  - **The GRUU demonstration assumes less than it first did**, and what it still assumes is written
    down. Both instances are passed to the call helper now, and the un-named one is asserted not to
    recognise the GRUU as its own. It is **not** asserted to refuse a call so addressed, because
    sipx cannot: `sipx-call` reads no `gr` parameter, and an INVITE for one instance's GRUU delivered
    to the other's flow is answered (`X-59`). In RFC 5627 a registrar mints the GRUU and a proxy
    resolves it to one binding; sipx is the UA half and implements neither, so **the resolution in
    the test is a double and always was.** What sipx holds is per-instance GRUU learning,
    presentation and recognition.

- **M12 — Provable is delivered: every discard in the signalling path is counted, and the numbers
  come out beside the capture (`X-54`)** — `X-18` counted the transport's losses and shipped the
  capture; the clause's other two words were still short. The guard that made the claim general
  scanned one crate, and nothing outside each crate's own tests ever read either snapshot.
  - **"Every" now covers `sipx-transport` and `sipx-call`.** Widening the enumeration exposed
    **sixteen** unexplained discard sites where a hand census had found seven — the argument for an
    enumeration over a sweep, made by the enumeration itself. All sixteen carry a counter or a reason.
  - **`UnsentCounts` counts, by method, every request the endpoint tried to put on the wire and could
    not** — taken at the transmit, so a refused connection, an unreachable peer and an over-MTU
    datagram are all in it. A failed BYE on a teardown path is the number an operator asking "why did
    that call linger" can finally read. `CallEvents::dropped` counts events a consumer was too far
    behind to receive, per call.
  - **`sipx --counters <FILE>`, and `--capture` implies `<capture>.counters.json`** — written on every
    path out of the command, not only the successful one, because the run that fails is the run the
    bug report is about. `SignallingCounts` embeds both crates' snapshots unaltered rather than
    recounting; `dispatch` is an `Option`, so "no dispatcher was running" survives into the JSON
    instead of being flattened into a zero.
  - **A counter that would have been believed, caught by review.** The first version incremented where
    a request was *handed over* rather than where the wire was missed, so it could never fire on the
    network failures it advertised — while the spec and seven call-site comments said otherwise.
    `docs/specs/sip-transport.md` §12.3 now states the rule the media half inherits: count where the
    loss happens, not where it is reported.

### Fixed

- **An unreachable RFC editor is a non-result, not a corpus finding (`X-58`)** — a failed fetch
  exited `1`, so a gate run with no route printed `1 of 25 steps failed` naming `rfc 5118 corpus`:
  a step that never read the archive claiming the committed messages had drifted. That is the exact
  confusion `X-34` built the disk guard to remove, in a third place.
  - **The importers disclaim their own run**, exiting `EX_TEMPFAIL` (75) from the fetch guard, which
    `gate.py` reports under a heading that is not a finding before exiting `2`. A distinct exit code
    rather than a pattern in `infrastructure_evidence`: the importers are ours, so the disclaimer is
    a claim they are entitled to make, where the alternative is a regex over curl's prose in whatever
    locale and version the machine has — the spelling-not-behaviour mistake this story removes.
  - **A disclaimed step never outranks a real finding.** With something genuinely red beside it the
    gate exits `1` and prints `1 of 2 steps failed` naming only the red one, disclaimer still shown.
    Exiting `2` there would say "re-run", and a broken tree needs reading. The one deliberate
    departure from the disk guard's shape, and pinned by a test so a refactor cannot flip it.
  - **The false reason for the guard is deleted where it stood.** `AGENTS.md` said "`curl -f` prints
    nothing and a bare exit code reads as a corpus that changed" — but the flags in use are `-fsSL`,
    and `-S` is *show errors*: curl prints `curl: (22) …` and exits 22. The premise had been copied
    into `gate.py`, both importers, this file and `X-56`. The guard is still worth having, and is now
    justified by what it does: name the corpus and the host. `AGENTS.md` is the file every future
    agent reads as the why, and a why that one command disproves is the defect this project keeps
    filing stories about. The claim that the corpus steps were the gate's only network-reaching
    checks went with it — `build-docs.sh` runs `npm ci` whenever the gitignored `website/node_modules`
    is absent, which is every fresh worktree.
  - **The shape assertion is replaced by a behavioural one.** The old test required the fetch line to
    `startswith("if ! curl")`, which a guard whose body is `then true; fi` passes while an equivalent
    `||` form fails. Review then caught its replacement repeating the mistake: the assertion read
    `stdout + stderr`, and the importers print `fetching <url>` on stdout *before* the fetch — a line
    already containing both the host and the corpus number, so a guard whose whole body was `exit 75`
    passed. It reads stderr only, under a mute curl.
  - **An unknown argument no longer takes the write path.** `[[ "${1:-}" == "--check" ]]` meant
    `--check=1`, `-check` and every typo selected the branch that overwrites the corpus with the RFC's
    own bytes and exits 0 — a green step that erases the hand edit the check exists to catch.
    Dispatch is on `$#`, which closes the one input `"${1:-}"` disagrees on: an empty argument, read
    as "no argument given", rewrote a tampered fixture and exited 0.

- **Both RFC corpora are tamper-evident from the gate and from CI (`X-56`)** — each corpus is
  recovered from its RFC's own Appendix A archive rather than transcribed, and the importer's
  `--check` re-recovers that archive and diffs it against the tree: the only thing that can tell a
  fixture edited by hand from the RFC's own bytes, since the suites read whatever is in the directory
  and pass. RFC 4475's check ran solely inside the `fuzz` job, which is in `NOT_RUN_LOCALLY`, so no
  local run covered it; RFC 5118's ran **nowhere**, and `ci.yml` did not mention 5118 at all.
  - A gate step and a CI job per corpus, so a red result names which corpus drifted. The gate is 25
    steps over 17 CI jobs, and one byte flipped in an RFC 5118 message now fails it.
  - **The `fuzz` job keeps its own RFC 4475 check.** That one runs after the fuzzer, in the tree the
    fuzzer wrote to, and proves a campaign deposited none of its generated inputs into committed seed
    data — a claim a fresh checkout cannot make. The ordering is pinned by a test now.
  - **The fetch is guarded**, so a failure names the corpus and the host that could not be reached
    rather than leaving curl's own exit code to be interpreted. These steps are network-dependent,
    though not the first such: `build-docs.sh` has always run `npm ci` when the gitignored
    `website/node_modules` is absent. An unreachable RFC editor is a **non-result, not a finding**
    — see `X-58`, which corrected both the exit code and the false reason first given here.

- **`-vv` reaches DEBUG, and `-v` has something to say (`X-57`)** — verbosity counted the number of
  *arguments* beginning with `-v`, so the documented `-vv` was a single match capped at INFO while
  `USAGE` promised DEBUG. Only the undocumented `-v -v` ever reached it, and `-v` alone was worse than
  quiet: the workspace's only two `tracing::info!` sites are a registration refresh and a transcoding
  bridge, so `sipx dial -v` narrated **nothing** through a call that worked.
  - **Counted by `v` letter now**, so `-vv` and `-v -v` are one request and `-vvv` agrees with
    `-v -v -v`. The ladder saturates at DEBUG, because the workspace has no `trace!` for a third `v`
    to reach and documenting a level identical to `-vv` would restate the defect rather than fix it.
  - **Only a cluster of `v`s is verbosity.** The old prefix match counted `-V` and `--verbose` too.
  - **A call reports itself at INFO** — `calling`, `answered` and `hung up` on the dialling side,
    `waiting for a call` on the answering side, all on stderr, and `USAGE` says what each level is
    good for. A documented level that produces silence is the same shape as a capture you can only
    switch on by editing code.

- **A story closed inside a merge commit is counted (`X-55`)** — `maturity.py` read its story facts
  with `git log -p` and `git log --diff-filter=A --name-only`, and **neither emits anything for a
  merge commit** unless asked. A `status: done` line whose only appearance was a merge was therefore
  invisible: that is how `M-34`'s closing went missing, leaving the journal one ahead of the snapshot
  and needing a hand repair nothing documented.
  - **Counted rather than refused.** The story offered both routes and asked for one. This history
    already contains two such closings and history is immutable, so a detector would have made the
    gate permanently red over a defect nobody can fix.
  - **`--diff-merges=first-parent` alone is wrong**, though the story guessed it might be free: it
    takes filed from 182 to 224 and closed from 144 to 180, because `git log` walks every parent and a
    branch fact is counted once on the branch and again in the merge. Pairing it with `--first-parent`
    makes a story fact **an event on the mainline, counted exactly once** wherever it was written.
  - **Three numbers moved and all three were wrong before**: `M-34`'s and `S-26`'s closings were
    missing, and `S-26` was counted as filed twice — one file independently created on two lines of
    history. Filed 182 → 181, closed 144 → 146.
  - **The repair is a documented command**, not a reverse-engineered one: `maturity.py
    --reseed-journal` rebuilds the date attribution from committed history, and both journal
    diagnostics now end by naming it. It refuses to run with `--check`, so the step that verifies the
    journal can never be the step that rewrites it.

## [1.0.0-alpha.3] — 2026-07-30

**One breaking change, and four measurements that turned out not to measure what they said.**
`collect_digits` now takes two durations, because one window spent on two questions is the defect
`X-40` already found a layer up. The rest of this release is the same theme applied to the
project's own instruments: two tests that could not fail for the reason they existed, a spec
claiming a knob nobody built, and two milestones whose exit criteria were read against story
statuses rather than against tests — one of which turned out not to be met after all.

### Changed — breaking

- **`MediaSession::collect_digits` takes two durations instead of one (`M-34`)** — it was
  `collect_digits(idle)`, and that single window was spent on two different questions: how long to
  wait for the **first** keypress, and how long a silence means the caller has **stopped**. Whichever
  was slower on the day won, and the result was not a short collection but an empty one, because the
  loop ended before its first iteration. That is the identical shape `X-40` measured one layer up,
  where it made `sipx answer` write a valid WAV containing zero samples.
  - **Migration:** `collect_digits(idle)` becomes `collect_digits(within, gap)`. `within` bounds the
    wait for the first digit and caps the whole collection — a bound on *failure*, so set it an order
    of magnitude above the honest answer, typically the call's own duration. `gap` is the silence
    that means dialling has finished, and is the only question a fixed window can answer here.
    Passing the old value as both restores the old behaviour exactly, including the defect.
  - An application that knows how many digits it expects should stop at that count with `recv_digit`
    rather than wait for a silence at all.

- **`sipx answer` now holds the call for its full `--duration` when no digits arrive** — a
  consequence of the above, measured end-to-end at 1.06 s before and 10.0 s after with
  `--duration 10`. This is closer to what `--duration` is documented to mean ("hang up after this
  many seconds") than the old early return was, but scripts that relied on the answerer returning as
  soon as the audio stopped will see calls last longer.

### Changed

- **M12's exit criterion has been checked against evidence, and it is one clause short (`X-51`)** —
  all four of M12's stories are closed, which prompted the question and is not an answer to it. Three
  clauses hold as written: the RFC 5118 corpus is fully classified with an empty deviation list, the
  interop matrix runs the identical test list against two independent peers with neither declaring a
  divergence, and the fuzzer drives the transaction layer with built message sequences rather than
  bytes over a corpus proven unmodified.
  - **The third does not.** "Every discard in the signalling path is counted and exportable next to a
    capture" fails in two of its own words: *every* reaches only `sipx-transport`, whose guard scans
    that crate and no other, while `sipx-call` drops a call event uncounted and discards six send
    results; and *next to* is false outside the process, because the counters are read by no shipped
    binary while `--capture` is on three commands with no counterpart for the numbers. Filed as
    `X-54`.
  - `M-32` staying open does **not** hold M12 open — the clause says *signalling* path, and the media
    counters are outside it. That was read off the clause's words rather than assumed either way.


- **M10's exit criterion is stated once, and it does not require TURN (`X-50`)** — `docs/roadmap.md`
  gave the milestone two exit criteria that disagreed: a `Done when` sentence about a media path
  symmetric RTP cannot provide, and an epic heading scoping M10 to all six children of `M-16`,
  including RFC 8656. The sentence governs. `M-24`'s relayed candidate is in the ICE epic and in no
  milestone, and the third clause is settled in writing as *some* endpoints symmetric RTP cannot
  connect rather than *any* — both ends behind symmetric NAT is precisely the residue a relay buys.
  - **M10 is still not recorded as reached, and the reason is not TURN.** Checked against tests
    rather than story statuses, only the ICE clause is demonstrated as written: the GRUU test is an
    `OPTIONS` to a single agent rather than two registrations of one address of record each taking a
    call, and the push test stops when the INVITE arrives without anything answering it. `X-52`
    carries that remaining distance.
  - The same question is now open for M12, whose four stories are all closed and whose `Done when`
    has never been checked against evidence (`X-51`).

### Fixed

- **`sip-tls.md` no longer claims a minimum-TLS-version knob that nobody built (`X-46`)** — §3.2
  listed the minimum protocol version as configurable, and neither `ClientTls` nor `ServerTls` takes
  a version. The spec was corrected rather than the knob built, for a reason worth recording: above
  the floor its only representable value is "1.3 only", and the *absence* of a version-selecting API
  is currently what evidences the RFC 8996 and 8446 rows — `docs/rfc/README.md` says those are
  "proved by the absence of an API", so building the knob would have falsified three documents to
  satisfy a fourth.
  - The sweep of §3.2's other entries found a second inaccuracy: trust anchors were described as
    defaulting to the system roots, when there is no default at all — anchors are required and an
    empty set is refused at construction.
  - **The claim is now guarded rather than merely corrected.** A new test holds every §3.2 entry
    against the public surface of `tls.rs`, so a future entry naming an API that does not exist fails
    the build. That is the difference between fixing this instance and fixing the class.


- **`verbose_logging_stays_off_stdout` can now observe logging at all (`X-53`)** — the second
  instance of the defect above, found by the same sweep. It ran a command refused as a usage error
  before any socket was bound, so the CLI emitted no log records and the assertion held identically
  whether logging went to stderr or stdout. Redirecting `init_logging` to stdout left it green in
  0.00 s.
  - It now places a real call against a verbose answerer and asserts four things the old one could
    not: no log records on stdout, every stdout line a parseable JSON object, records **present** on
    stderr (the control that makes the absence mean something), and none at DEBUG when the same call
    runs quietly — so the records are attributable to the flag rather than to the call.
  - The exit codes of both processes are asserted, which the old test never checked.
  - It surfaced a real CLI defect, filed as `X-57` rather than fixed here: `-vv` is documented,
    accepted, and inert.

- **`no_capture_flag_means_no_file` can now observe the flag it is named for (`X-45`)** — the test
  killed the answerer the moment it announced its port and then asserted one path did not exist, so
  both halves were vacuous: a capture is written while signalling flows, so a run with no call cannot
  see one being written, and the path it watched was one nothing would ever write to, because a
  capture nobody asks for is given no path. Making `--capture` unconditional — the exact defect it
  guards — left it green.
  - It now places a **real call** and asserts the directory both processes ran in is empty, with a
    positive control first: the same call, same machinery, `--capture` on, which must produce a
    capture containing an INVITE. Without that control an absence is equally consistent with capture
    being broken outright, which is the failure a test named for the flag being *off* is least able
    to notice.
  - A second instance of the same shape was found by the same sweep and filed as `X-53` rather than
    folded into this diff.

## [1.0.0-alpha.2] — 2026-07-30

**ICE stops being a library capability and becomes a call one.** `1.0.0-alpha` shipped a complete,
tested ICE agent that no call could reach; this release gives a call the choice, makes the session
survive a mid-call restart, and repairs a re-offer path that had been quietly telling peers ICE was
over. The default is unchanged and stays unchanged: a call that does not ask for ICE offers no
candidates, sends no checks, runs no timers, and is carried by symmetric RTP exactly as before.

It also repairs the measurement itself. Two CI jobs had been red on every commit and every pull
request, both accusing the maturity report's own journal of a discrepancy it did not have — the
history they compared it against was one commit deep. A green gate that cannot be reproduced in CI
is the failure this project keeps writing checks against, and it had become true of the checks.

The alpha's seven predicates still read 7 of 7, computed rather than asserted. v1's first predicate
asks for that to hold **across** a release rather than at the moment one is cut, and this is the
first release since `1.0.0-alpha` on which it does.

### Fixed

- **CI stopped checking the board against a history it had not fetched (`X-49`)** — `gate consistency`
  and `rfc compliance` were red on `main` and on every pull request, both reporting that
  `docs/maturity.md`'s event-date journal recorded 173 filed stories against a snapshot of 172. The
  journal was correct. `actions/checkout` defaults to a depth-1 clone, and the filing days are read
  from `git log -- docs/stories`: in a grafted single-commit checkout every story file present reads
  as *added by that commit*, so the count became the number of story files that exist. The two agreed
  until `P-6` was renumbered to `P-7` — two filings, one surviving file.
  - Both jobs now check out with `fetch-depth: 0`, as `provenance` already did for its history scan.
  - `maturity.py` **refuses a shallow checkout** and names the fix, rather than reporting a
    depth-dependent count that surfaces as a corrupt report. Degrading to "rate unavailable" was
    rejected: `--check` would then fail as report drift, the same misdirection one step further away.

### Added

- **A call recognises and offers an ICE restart, and keeps the audio up across it (`M-23`)** — a
  re-offer whose `ice-ufrag` *and* `ice-pwd` have both changed begins a new ICE session (RFC 8839
  §4.4.1.1.1): new credentials, a new tiebreaker, rebuilt checklists and a role that may be
  redetermined. Media keeps flowing on the pair the finished session selected until the new one
  selects its own, which is what makes a restart usable rather than a gap in the call.
  `Call::restart_ice` offers one; the answering side detects one with no application involvement.
  - **Every later description for a stream doing ICE now restates its ICE half.** RFC 8839 §6 makes
    a missing `candidate` attribute mean the peer has stopped doing ICE, so the previous behaviour —
    a re-INVITE built with no ICE attributes at all — told the far end to fall back to symmetric RTP
    in the middle of a call that had already agreed to checks. Hold, resume, codec changes and
    session refreshes all carry the half now, and none of them is a restart.
  - Hold remains `a=sendonly`/`a=inactive` and never `c=0.0.0.0`, which §4.4.1.1.1 would read as a
    restart on every mute. That was already true and is now asserted.
  - A restart re-signals the sockets the session is already running on; it does **not** re-run the
    STUN transaction, because the socket belongs to the receive loop once media is flowing. The
    limit is recorded in `docs/specs/ice.md` §13.5.

### Changed

- **A call can select ICE for its initial offer/answer (`M-27`)** — one call-layer media policy
  reaches dialing, direct and dispatched answers, ringing answers and reliable early answers.
  Host-only and configured-STUN gathering both retain fresh per-call state through media startup;
  an unavailable STUN server degrades to host candidates. The default remains no ICE, emitting no
  ICE attributes and running the existing symmetric-RTP path. A live-call test makes both default
  destinations unusable and proves audio crosses only the nominated pair.

- **The diagnostic phone can exercise every released signalling transport (`P-8`)** — `dial`,
  `answer` and `register` now select UDP, TCP, TLS, WS or WSS through one fail-closed policy. Secure
  URI schemes cannot choose cleartext, TLS and WSS verify the requested certificate identity and
  trust roots, and certificate failure is reported as a typed transport error without downgrade.
  Explicit selection reports both requested and negotiated transport in text and JSON, while
  legacy invocations retain their existing byte-for-byte output. A bounded real-socket command
  matrix covers all three commands and all five transports.

- **Conformance, capability and release readiness are assessed separately (`X-48`)** — the dated
  repository review measures the SIP core, endpoint library, high-level call/media framework and
  executable phone as distinct surfaces, so lower-layer code no longer earns product-level credit
  merely by existing. It records the 70-row RFC registry and verification baseline, gives an
  explicit per-use readiness verdict, and prioritizes the reachability and adoption gaps that must
  close before sipx can claim broad library-and-phone capability.

## [1.0.0-alpha.1] — 2026-07-30

**No code changed in this release.** No file under `crates/` differs from `1.0.0-alpha`; the library,
the CLI and every published crate are byte-identical to it. What changed is how the project measures
and reports itself, which is the alpha's own subject matter — predicate 7 exists because *the
distance to v1 is generated, not asserted*. Skip this release if you consume the code.

### Changed

- **The README states its metrics instead of asserting them, and its header stops overlapping the
  logo** — the badge row now carries the release, the MSRV, RFC coverage, the codecs and the licence,
  and **not one of those numbers is written by hand**. It is a `generated:badges` region, so
  `sync-website.py --check` fails the build when any of them drifts, which is the rule `X-47`
  established for public facts applied to the line of a README that is read most and re-checked
  least.
  - **The codec badge parses what `check-audio-claims.py` prints rather than recomputing the set.**
    A badge that counted codecs its own way could disagree with the check that fails the build, and
    a badge disagreeing with the gate is worse than no badge; if that check is red, the region
    refuses to render rather than publishing an unbacked claim.
  - **RFC coverage reads `32 implemented of 70`, and `partial` is not folded in.** `docs/maturity.md`
    refuses the same arithmetic for the same reason: one number would call a fully implemented row
    and a partial one the same thing.
  - Shields' query form is used rather than the `label-message-colour` path form, which escapes a
    hyphen by doubling it — that would have published `1.0.0--alpha.1`, a version string nobody can
    install.
  - The logo was `align="right"`, so the `h1`'s bottom rule ran underneath it. GitHub strips inline
    styles from Markdown, so there is no layering fix available; the float is gone and the header is
    centred instead.

- **Conformance, capability and release readiness are assessed separately (`X-48`)** — the dated
  repository review measures the SIP core, endpoint library, high-level call/media framework and
  executable phone as distinct surfaces, so lower-layer code no longer earns product-level credit
  merely by existing. It records the 70-row RFC registry and verification baseline, gives an
  explicit per-use readiness verdict, and prioritizes the reachability and adoption gaps that must
  close before sipx can claim broad library-and-phone capability.

## [1.0.0-alpha] — 2026-07-30

**All seven `1.0.0-alpha` predicates are met**, computed rather than asserted, and
[`docs/maturity.md`](docs/maturity.md) is where that is read. The last two to close were predicate 3,
*a red gate means a defect* (`X-39`, `X-40`, `X-41`), and predicate 4, *no known-wrong shipped path*
(the six defects the 2026-07-30 repository review filed). This is not a claim that v1 is close: v1's
first predicate requires these seven to have held **across at least one release** rather than at the
moment one was cut, so this release starts that clock rather than stopping it.

### Changed

- **The public docs are an adoption path rather than an internal status ledger (`X-47`)** — the
  README and website now lead with the shipped CLI and Rust workflows, state the WAV-only and
  secure-transport boundaries at the decision point, and keep the experimental SDK out of the main
  navigation. Security, troubleshooting and integrating an endpoint into an existing SIP system each
  have a canonical guide; the RFC table is published on-site. Release, MSRV, crate and RFC facts are
  generated or checked from their existing sources, and the docs build rejects internal work-item IDs
  and design/story links in public pages.

- **Negotiated media starts transactionally, and the constructors that can fail now say so
  (`M-35`, `M-36`, `M-37`)** — `MediaPort::start` returns `Result<MediaSession, SetupError>` and
  `Conference::new` returns `Result<Conference, ConferenceError>`. `sipx-call` carries the new error
  through as `Error::Media`. The rule behind all three: validation and codec construction complete
  before any worker is spawned, or startup returns a typed error and leaves no worker or socket
  alive. `docs/specs/media-runtime.md` is that boundary written down, and it is new.

### Fixed

- **Pool eviction closes the connection it evicts (`T-25`)** — `max_connections` bounded entries in
  the routing map, not sockets. Eviction removed the entry and left the task running, because a quiet
  peer keeps its read half open after every writer sender is gone, so the connection outlived the
  record describing it and the configured maximum bounded bookkeeping.
  - Every pooled connection now has an endpoint-owned cancellation signal and holds its slot until
    its task has **terminated**, not until its entry is dropped.
  - **A same-key replacement reserves its own generation instead of consuming a second victim.**
    That was the subtle half: a replacement arriving at a full pool while the cancelled task had not
    yet finished would evict an unrelated connection to make room for a slot that was about to free
    itself.
  - Every message, pong, transaction destination and close event now carries the generation that
    produced it, so a retiring generation gets exactly one close event, fails only its own
    transactions and keep-alive waiters, and cannot remove or answer for its live replacement.
    Vectors X17, X20, X21, X24 in `docs/specs/sip-transport.md`.

- **Unauthenticated TLS and WebSocket handshakes have a budget (`T-26`)** — there was none. The
  connection pool only bounds peers that already completed a handshake far enough to have a pool key,
  so everything before that point — accepted sockets and spawned handshake tasks — was unbounded, and
  an unauthenticated peer could grow it at will.
  - TLS, WebSocket and secure WebSocket now share **one budget of 64 live handshakes** per endpoint
    with a **10 second deadline**. A socket that cannot take a permit *without waiting* is closed
    immediately; there is deliberately no pre-handshake queue, because a queue is the same unbounded
    growth wearing a different name.
  - Timeout, protocol failure, successful adoption and endpoint shutdown each release exactly one
    permit. TLS followed by a WebSocket upgrade is **one** handshake with one deadline, not two.

- **Unusable endpoint configuration is refused before anything binds (`T-27`)** — a request-channel
  capacity of zero reached the runtime channel constructor and panicked; a zero WebSocket keepalive
  started a task whose timer terminated on its first tick. One validator now runs on every public
  construction path before a socket is bound or a task starts, and returns a typed error naming the
  field. Nothing is silently clamped — a clamp would make the configuration a suggestion.

- **Dropping a conference stops every participant collector (`M-35`)** — collectors, their media
  sessions and their sockets survived the conference that owned them, because nothing terminated them
  short of a participant producing another frame or closing its own session. The conference now keeps
  cancellation and completion ownership for every collector, and removal, explicit close and `Drop`
  all reach one idempotent shutdown. `Drop` initiates bounded cleanup; it does not block.

- **Zero worker intervals are rejected instead of killing the worker they configure (`M-36`)** — a
  zero packet duration, RTCP interval or conference mix interval each produced a timer that
  terminated on its first tick, so audio stopped with nothing reporting an error. All three are
  refused before a socket is bound. The floor is one millisecond and that is also the resolution
  samples-per-packet is derived from: **a positive sub-millisecond value passes a timer check and
  then derives an empty audio frame**, which is why "greater than zero" was not the test.

- **A codec that fails to construct no longer ships a different codec under its payload type
  (`M-37`)** — Opus encoder or decoder construction failure fell back to a PCMU pipeline while
  negotiation kept the Opus payload type, so the far end read the number it agreed to and decoded
  G.711 bytes as Opus. Failure is now a typed `SetupError::Codec` naming the codec and which half
  failed, and no diagnostic carries media, SRTP keys or DTLS key material.

- **Reading the media statistics no longer consumes the reporting window (`M-33`)** — two comments
  disagreed and the doc comment was the false one: `stats()` called `report_block()`, which takes
  `&mut self` and resets the counters. Two reads with nothing in between returned `fraction_lost: 51`
  then `0`, and a single poll between two RTCP intervals turned a 64 on the wire into 0.
  - **A reporting interval is closed by a report being sent, never by a read.** RFC 3550 §6.4.1
    defines the fraction over the previous SR/RR *packet*, so the boundary is a transmission. The
    split is now two functions rather than a corrected sentence — `pending_report_block(&self)` reads,
    `report_block(&mut self)` closes and belongs to the RTCP send loop alone — so the trap cannot
    come back.
  - The wire test derives §6.4.1's required fraction for whatever window each report *names* rather
    than arranging a boundary and assuming the timer fires between two batches. The arranged version
    failed 14 of 20 runs at 6× CPU oversubscription; the derived one is 0 failures in 12 runs at 6×
    and 20 at 15×. The technique is written down in `docs/designs/media.md`.

- **`sipx answer` and `sipx dial` record the call they were asked to record (`X-40`)** — filed as a
  flaky-test story and it was a production defect, deterministic, with two independent causes on one
  line of `answer.rs` and `dial.rs`.
  - **One window was answering two questions.** The recording was never bounded by `--duration`; it
    was bounded by `record_until_idle`'s 500 ms, which is *also* its only window for waiting for the
    **first** frame. Audio arriving 1.5 s into a `--duration 10` call produced a WAV with **zero
    samples** and `duration_ms: 801`. `X-28` fixed this in the library and left both production
    callers on the old primitive.
  - **`unwrap_or_default` discarded a full recording at the cap.** When the far end is still talking
    as the call's time runs out, the outer `timeout` fires and replaced everything recorded so far
    with silence: `duration_ms: 2006`, `samples_recorded: 0`.
  - In both cases the answerer reported `"status":"answered"` and **exited 0**, so no exit-status
    assertion could have caught it. Exit status is now asserted at all four sites that run the binary.
  - **The lesson is about the filing, not the fix.** "Observed once under load, 15/15 in isolation"
    reads like a test race and sent the search to the test; a 1.5 second sleep reproduces it 3/3.
    *Not reproducible in isolation* described the symptom, never the defect.

- **The docs-site step fails on a defect instead of printing it (`X-41`)** — `onBrokenAnchors` was
  unset, Docusaurus defaults it to `warn`, and the step printed `Docusaurus found broken anchors!`
  and exited 0. The gate reported green with a dead link in the published site, and it surfaced only
  because `S-30` read the output rather than trusting the result.
  - All **four** reporting handlers are now stated with a reason each, `onDuplicateRoutes` included —
    it is not a link defect but it had the identical print-and-exit-0 shape.
  - The step's contract is written where the step is defined, under one rule: **no check in this file
    may print a defect and exit 0.** Any `[WARNING]` in the build output fails it, with an
    intentionally empty exceptions list as the named place for a deliberate one.
  - **It proves the guard is armed rather than trusting the setting**, by building a page that links
    to an anchor no page emits and failing if that build succeeds.
  - `scripts/check-docs-links.py` is new: the internal-links check lifted out of a heredoc and
    extended to anchors, which the heredoc discarded with `link.split("#")[0]` — a link to a missing
    file failed the build and a link to a missing heading was invisible. 208 internal pages, every
    relative link and anchor resolving.
  - The dead-anchor count this turned up was **zero**; the story is carried by the setting, not by
    the cleanup.

- **The maturity report describes the commit being made, not the rest of the tree (`X-39`)** — the
  working-tree union shipped in `0.12.0` fixed the ordinary all-changes commit and left the selective
  one wrong: a staged report could count a story the commit does not contain, so the local check and
  a clean checkout of the resulting commit disagreed. That is the `X-22` failure class the gate
  section exists to prevent, arriving inside the gate's own measuring instrument.
  - **The snapshot is chosen by what is staged.** Any staged story change selects the index; with
    none, the complete worktree is the snapshot and the ordinary workflow is unchanged. Staging the
    report while story changes sit unstaged is refused rather than guessed at.
  - **Dates were the other half.** Pending facts took the wall-clock day while history groups by
    author date, so midnight or an amend moved a fact between rows. The generated region now carries
    an event-date journal keyed to the filed and closed story paths, so unchanged totals cannot
    conceal rewritten attribution, and a committed fact absent from the journal is still computed
    from history — forgetting to regenerate stays strict drift.

- **RFC 8996 cites the code that refuses TLS 1.0 and 1.1, not a document (`X-43`)** — the row claimed
  `implemented` against prose. **The refusal was already real**: all four tests in
  `crates/sipx-transport/tests/tls_versions.rs` passed the first time they ran, unchanged, at the
  merge base. The defect was the evidence, and it is reported as such.
  - The test writes a `ClientHello` byte by byte at a real listener with `client_version` 1.0 and 1.1
    and no `supported_versions`, and requires a fatal `protocol_version` alert with nothing reaching
    the application — `docs/specs/sip-tls.md` §6 vector **L9**, in the spec unrun since `T-7`. **The
    1.2 control is the load-bearing half**: rustls demands `signature_algorithms` before it looks at
    the version at all, so the first draft was refused with alert 40 and would have passed a looser
    assertion.
  - **The rule was adopted, not just the row fixed.** `prose_only_claims` now requires every
    `implemented` or `partial` row to cite at least one `crates/….rs` path. Measured before adopting:
    of 32 `implemented` and 22 `partial` rows, 8996 was the only one failing.

## [0.12.0] — 2026-07-30

### Added

- **The distance to `1.0.0-alpha` is generated, not estimated (`X-32`)** — someone asked how far sipx
  was from v1 and the honest answer was that the question had no denominator: the roadmap ran M0–M12
  and never named 1.0, and the only `v1` in the tree was `sipx.app.v1`, a protocol version. So the
  predicates were written down first, and `scripts/maturity.py` now generates `docs/maturity.md` from
  `docs/rfc/registry.toml`, story frontmatter and git. Two new gate steps, taking the gate to **20**.
  - **v1 is defined as five predicates** beside the alpha's seven, and they are separate because each
    needs something this repository cannot supply — chiefly *"the public API has been used from outside
    this repository"*, which no gate can assert and which the roadmap always gave as the reason to wait.
    **No feature count, no RFC total, no percentage** in either list: the vision makes maximum feature
    count a non-goal, so a coverage-based gate would contradict the document it serves.
  - **A predicate's state comes from the board and nowhere else.** Each names the stories that close it
    and is met only when all are `done`, so the table cannot drift. The nastiest failure mode is pinned
    by a test: a blocker list pointing at a story that does not exist reports **unknown**, not met —
    otherwise deleting a story would look like finishing it.
  - **Two predicates are reported as `attested` rather than `computed`, and say why.** "No known-wrong
    shipped path" cannot be computed, because a defect nobody has found leaves no trace in either
    source; what is reported is the absence of *open* stories describing one. `S-27` is the proof of the
    difference — a `sips:` URI dialled in cleartext, found by reading code, not by any report.
  - **The most useful output is the one nobody asked for first**: stories filed versus closed per day,
    from git. **−37 on 2026-07-28, +4 on 2026-07-29, +1 on 2026-07-30.** The report says plainly that
    the marker is not a single winning day but the date the crossover becomes *durable*, because a
    shrinking board means nothing while discovery still outpaces it.
  - **No aggregate percentage exists, and a test forbids one.** `media` is 15 RFCs with 11 partial;
    `security` is 11 with none. One number would call them alike. `partial` is counted as `partial` and
    never as a fraction of done.
  - It states its blind spot rather than implying precision: outside `media` and `security`,
    `implemented` means the code exists and has not been checked against a caller. Another test ties
    that caveat to `rfc-report.py`'s actual scope, so widening the check there fails here until the
    caveat follows.
  - The check earned itself on its first run: closing `X-32`'s own story changed the answer and turned
    the gate red until the report was regenerated.

- **Every transport discard is counted, and the signalling can be captured to a file (`X-18`)** — a
  running sipx is now observable from outside: `Handle::counters` returns a snapshot over shared atomics
  (the `shed()` shape, not the ask-the-loop shape, because a counter readable only by asking the loop is
  unreadable in exactly the situation it describes), and `sipx dial/answer/register --capture <FILE>`
  writes the signalling exchanged as pcapng for attaching to a bug report.
  - **Credentials are redacted, and that claim was earned the hard way.** A security review defeated the
    first implementation three ways and put a digest `response` into the file in cleartext — a header
    folded onto a continuation line (RFC 3261 §7.3.1), `Authorization :` with whitespace before the colon
    (HCOLON permits it), and a bare-LF message, which disabled redaction entirely. All three are
    spellings sipx accepts and processes normally. Redaction is now **structural** — split on CRLF/LF/CR,
    unfold continuations into one logical header, take the name as the bytes before the first colon — and
    **redacts conservatively when a line has no determinable name**, because where structure is absent a
    credential can be anywhere. Digest `response`/`nextnonce`/`rspauth`, opaque `Bearer`/`Basic`, every
    `a=crypto` key (RFC 4568 §9.1 permits several per line), SDP `k=`, push tokens, instance URNs, and
    credentials nested in `message/sipfrag` are all removed; challenge parameters are kept, because a
    nonce with no response beside it is not a credential and a digest failure is unreadable without them.
  - **Off by default, and now genuinely free when off** — the `getsockname(2)` per message and the
    unconditional re-serialisation of every inbound stream message are gone, with a test asserting the
    byte-producing closure never runs.
  - **TLS and WSS are recorded decrypted, and the CLI says so** — along with what redaction cannot
    remove: the file still says who called whom, when, and from where.
  - Body redaction is length-preserving, because shortening an SDP line would leave every message
    inconsistent with its own `Content-Length` and unparseable by the tool the capture exists to be read
    in.
  - Stream framing failures now report to the driver and are counted per transport, closing a
    `parse_failures` counter that was structurally stuck at zero for four of five transports.

- **sipx ships an application, and the reachable surface is what it uses (`X-38`)** — `crates/sipx-app`
  gains a `sipx-host` binary that binds the listeners a `HostConfig` declares, admits invitations,
  answers on `sipx-call`, and serves to the end. `scripts/check-app-surface.py` holds every crate's
  `# Stability` declaration against that application's real, **feature-resolved** dependency closure,
  and the gate is red when the two disagree. Two new gate steps, taking the gate to **22**.
  - **This closes the last open `1.0.0-alpha` predicate.** Three earlier attempts checked reachability by
    reading evidence *paths*, and each recorded the same limit: a path is satisfied by citing a file
    whose relevant branch is dead. An application has no dead branch to cite — either it builds and runs
    on the API or it does not. `docs/maturity.md` reports predicate 1 as **computed** rather than
    attested, and the "unverified against callers" caveat is resolved by use rather than by a rule.
  - **A Cargo feature is part of being selectable.** A capability behind a feature no shipped binary
    enables is *Experimental*, however thoroughly it is implemented. Opus is the worked example: complete,
    with vectors, RFC 6716 and 7587 cited against it, and reachable from the library and from no
    application. The rule runs **both** directions — an outside caller graduates an item to `Supported`
    with a changelog entry, and an application dropping a capability demotes it — because a surface that
    can only grow is a freeze arriving one item at a time.
  - **`dev-dependencies` are excluded: a test is not a caller.** That is why the suite could never settle
    this predicate.
  - Building the checker found **two modules silently over-claiming** — `sipx-audio::opus` and
    `sipx-media::dtls::openssl`, neither on anyone's list — and the first version of the checker had the
    very defect the predicate exists to remove: it walked dependency *names* and ignored features,
    confusing *compiled* with *selectable*.
  - `scripts/maturity.py` now **reads** each status word's definition out of `docs/rfc/README.md`'s
    schema table instead of restating it, after a restatement produced a false sentence. There is exactly
    one definition of each word and it cannot drift.

- **A call can select its codec set, and Opus is reachable through it (`M-30`)** — `M-13` built the
  Opus encoder, decoder and SDP half, and nothing built the *selection*, so no call had ever carried an
  Opus packet. `Codecs` (default `G711`) is now the application's choice, taken by `DialOptions::with_codecs`,
  `answer_with`, `answer_ringing_with`, `answer_replacing_with`, `Invitation::answer_with` and
  `ring_early_with`. Behind the off-by-default `opus` feature, since it links libopus.
  - **Six hardcoded `Capabilities::g711` sites are gone.** All construction now goes through
    `Codecs::capabilities`, including the early-dialog and answer paths — plumbing one level up instead
    would have moved the hardcode rather than removed it, and a half-reachable codec is how this defect
    recurs.
  - **`Codec::from_payload_type` still refuses Opus, and that refusal is correct.** RFC 7587 §7 assigns
    Opus no static payload type, so returning Opus for 111 would decode someone else's G.729 as Opus.
    The way in is the `a=rtpmap`, with the negotiated number carried on `Config::payload_type`.
  - **RFC 6716 and 7587 return to `implemented`**, the rows `X-33` demoted for having no caller. The
    notes state the boundaries at full weight: RFC 7587 §7.1's optional parameters are neither offered
    nor read, and Opus is reachable from the library and **not** from `sipx-cli`, which has no flag for
    it and no `[features]` block to forward one.
  - **The guard that would have blocked a false promotion was rebuilt, not just inverted.** `X-33`
    asserted the selector symbol was *absent*, which is conclusive; asserting *presence* is not, because
    a sentence naming a symbol satisfies a substring search. The check now reads Rust with comments
    stripped, and additionally requires the feature to be declared and the codec set to be exported —
    facts prose cannot fake. `X-33`'s `sipx-cli` absence check, dropped by the inversion, is restored,
    and it is what holds the note's claim about the binary.
  - **`check-features.sh` now builds `sipx-call` with the feature off and on**, with `--all-targets`
    because the conditional code is in its tests. A new optional feature on a published crate with no
    combination covering it is exactly how `tls` came to be broken for a release.
  - Fixed on the way: negotiation applied its selected-set filter *after* choosing a format rather than
    during the search, so an Opus-first offer to a default G.711 call failed outright while the answer
    on the wire named the PCMU further down the same list — invisible in the default build, live under
    `--all-features`. And a redundant `a=rtpmap` for a static payload type no longer restarts the media
    session, which was an audible gap on a merely re-worded SDP.

- **The RFC 5118 IPv6 torture corpus is asserted against, at both layers (`X-16`)** — all twelve
  messages from Appendix A, recovered by `scripts/import-rfc5118-corpus.sh` rather than retyped, with
  `--check` re-deriving them from the RFC so the fixtures cannot drift from their source. Asserted in
  `sipx-sip` and, for the three messages carrying a session description, in `sipx-sdp`.
  - **It found a conformance defect, and the defect is recorded rather than hidden.** RFC 5118 §4.10
    requires a parser to tolerate `[2001:db8:::192.0.2.1]` — the three-colon form RFC 3261's ABNF can
    produce, inherited from the obsoleted RFC 2373 — and sipx rejects it. That is an unmet normative
    MUST, so the row was `partial` and not `implemented` — **`S-31`, above, closes it in this same
    release** — and the gap was recorded as one typed entry in
    `rfc5118::DEVIATIONS` saying what the RFC requires, what sipx does, and why it stands.
  - **A new failure cannot be absorbed silently.** The converse assertion requires every message the
    RFC calls valid to parse, and is guarded by hard counts — eleven valid, one recorded deviation, ten
    covered — so admitting a second deviation means editing a number in a diff rather than quietly
    widening a skip. A recorded deviation is also asserted to still reproduce, and prints
    delete-this-entry instructions when it stops.
  - **No source changed.** This story is the measurement; the §4.10 fix is its own story.
  - Two defects in the RFC's own archive are handled and documented: the files are bare-LF terminated
    with no CR anywhere, and all three SDP messages declare a `Content-Length` matching neither the LF
    nor the CRLF body. The corpus stays bit-exact and `Case::wire()` applies three stated
    transformations, with a test proving nothing else moved.

- **A registration can be placed over an Outbound flow, and woken by a push (`S-29`)** — `sipx-ua`'s
  RFC 5626 and 8599 support had no caller above its own tests, which is the eighth instance of the
  same defect: a capability that exists in a crate and cannot be selected from a call. `sipx register`
  now selects both.
  - **`--outbound`** builds the Outbound config, putting `+sip.instance` (§4.1) and `reg-id` (§4.2) on
    the REGISTER's `Contact` and offering the `outbound` option tag, and the command reports whether
    the registrar accepted the flow by reading `Require: outbound` back off the 2xx (§6).
  - **`--push-provider` / `--push-prid`** put RFC 8599 §4.1.2's parameters on the `Contact` URI and
    report whether the registrar named the same service, read out of `Feature-Caps` (§8.2). `--wake`
    drives `UserAgent::woken`, which is §4.1.3's ordering — the binding-refresh REGISTER goes out
    *before* the pending request is expected — as a type rather than as a convention.
  - **RFC 5626 and 8599 return to a `uac` role**, the roles `X-37` demoted for having no caller. Both
    rows stay `partial`, and their notes now separate what has a caller above `sipx-ua` from what is
    implemented and still reached by nothing: `ob` on a dialog-forming Contact (§4.3), §4.4
    keep-alives, and multi-flow independent failure under §4.5 backoff. The CLI registers one flow per
    invocation, so that half has no caller by construction.
  - `sipx-ua`'s stability note narrows to match: **"registering as one Outbound flow"** is Supported;
    `Flows`, `Attempt`, `keepalive_after` and `dialog_contact`'s `ob` are named Experimental under the
    rule the same doc comment already stated, so the crate no longer contradicts itself.
  - Every new flag is refused when malformed rather than accepted and dropped — six unit tests — and
    each flag's value is registered so it can never be misread as the address of record.

### Fixed

- **The answer on the wire and the codec the session is built with can no longer name different
  formats (`M-31`)** — an offer carrying `a=rtpmap:0 PCMU/08000` settled negotiation on µ-law at payload
  type 0 while the answer named only `8`, so sipx sent on a number the answer never offered *and*
  decoded the peer's A-law through a µ-law session: audible garbage rather than silence, with nothing
  in the stack reporting an error. RFC 8866 §6.6 format identity was being decided twice — textually
  where the answer was built, numerically where the codec was chosen.
  - **One reader, in the lower crate.** `sipx-sdp`'s new `rtpmap` module is the only place the question
    is answered: the encoding name case-insensitively, the clock rate and channel count **by value**,
    an omitted channel count meaning one. `answer.rs`'s private rule is deleted rather than reduced to
    a wrapper, and `sipx-call`'s `codec_named` now parses nothing — it asks the same predicate once per
    codec it can run. The direction is forced by the dependency: `answer` builds what goes on the wire
    and cannot call up, so only the lower crate can hold the rule. Nothing above it came down with it —
    which rtpmaps map to a codec sipx has, and which codecs the application selected, both stay in
    `sipx-call`.
  - **Three live disagreements, not the one that was filed.** The reported leading-zero clock rate, the
    same split in the *channel* field, and a **signed** rate that nobody predicted: `u32::from_str`
    accepts `+8000`, so the parsing rule read it as eight thousand where the textual rule did not — the
    same defect reached from the other side. A fourth appears under the `opus` feature, on Opus's own
    rate. Enumerated by instrumenting the table, so this is measured rather than reasoned.
  - **The single rule resolves those two opposite ways** — `08000` is tolerated, `+8000` is refused —
    which is the argument that this is one reader and not a normalisation pass. Agreement comes from
    there being one reader, not from any particular verdict. A rate that is empty, is not a decimal
    digit string, or overflows a `u32` is a typed error and a non-match, never a panic.
  - **Held by a table, not by a comment.** `call.rs:3945` previously claimed "the same rule the answer
    was built with … The two have to agree" while nothing enforced it. The agreement is now a
    biconditional over 17 offers (20 with `opus`): the payload type negotiation settles on must appear
    in the answer, and a stream negotiation refuses must be one the answer rejected with port 0. Both
    halves are reachable defects, which is why it is not a one-way check.
  - `docs/specs/sdp-format-identity.md` is new and normative for the rule, the grammar, everything
    refused, and what is deliberately *not* unified.

- **RFC 5118 §4.10's three-colon IPv6 reference is tolerated (`S-31`)** — `[2001:db8:::192.0.2.1]` was
  rejected, and §4.10 is normative that an implementation "**must** tolerate both of the above
  constructs". This closes the one deviation `X-16` recorded rather than fixed, so RFC 5118 moves from
  `partial` to `implemented`.
  - **The tolerance is exactly one derivation wide.** RFC 4291's own parser is tried first and is
    unchanged for every input it already accepted; only on failure is a single `:::` rewritten to `::`
    and retried through that same parser, and only when the text after it parses as an embedded IPv4
    address. There is no second address grammar — the three-colon form is invalid under RFC 4291 and
    valid under the ABNF RFC 3261 shipped, inherited from the obsoleted RFC 2373.
  - **The narrowness is mechanically enforced, not reviewed once.** A property test enumerates 1764
    references and pins the *entire* beyond-RFC-4291 accepted set as 24 enumerated addresses, so any
    widening shows up as a diff of literal strings rather than as a changed count.
  - The rule is in `docs/specs/sip-parser.md` §4.8 with both RFCs cited, and every row of its
    accept/reject table is asserted against the parser at exact-variant granularity — a spec table that
    makes claims nothing checks is worse than no table.

- **A story declares its own alpha predicate, and the list that could not see its defects is gone
  (`X-42`)** — `scripts/maturity.py` hardcoded each predicate's story list. Predicate 3's read
  `["X-28","X-29","X-34","X-36"]`, all `done`, so it computed as **met** while `X-39`, `X-40` and `X-41`
  were open and each described that predicate failing. The report was one story-close away from claiming
  the whole alpha.
  - **The literal is deleted, not extended.** A story names its predicate in its own `predicate:`
    frontmatter field, so the association lives in the file the filer is already writing when they find
    the defect. A story may declare two — `predicate: [3, 7]` — where a defect falsifies both.
  - **A computed predicate that no story declares now reads `unknown`, never `met`**, so deleting the
    last story naming a predicate cannot look like finishing it. A `predicate:` naming a predicate the
    roadmap does not have, or a malformed one, exits non-zero with a diagnostic rather than being
    silently dropped.
  - **The audit found a second stale list.** Predicate 1's omitted `X-35`, whose own Notes open *"This is
    alpha predicate 1 at the layer the predicate does not currently reach"* — the identical defect,
    invisible only because that story had already closed. Predicates 2, 5 and 7 were clean.
  - `docs/roadmap.md` now says how a predicate's state is read, and predicate 3's own prose no longer
    names stories: that list had gone stale too, which is the same defect one document over.

- **A valued flag given no value is now refused instead of read as absent (`S-30`)** —
  `sipx register sip:alice@example.com --outbound --instance` used to exit 0 having generated an
  instance URN nobody asked for. `Args::value` returned the same `None` for "the flag was last on the
  line" and "the flag was never given", so every caller took its absent-branch and ran on a default.
  - **Fixed once, in the constructor.** `Args::new` is now fallible, so holding an `Args` means every
    valued flag on the line was given a non-empty value — `value`'s `None` therefore means *absent* and
    nothing else. No call site re-checks anything, and the fifteen-odd `value`/`number` call sites are
    untouched. The rejected alternative, returning a `Result` from `value`, would have pushed the
    decision to every caller and let any of them write `.ok().flatten()`.
  - **Every flag in `VALUED_FLAGS` is covered**, with the test iterating that registry rather than an
    enumeration, so a flag added later is protected without a new case.
  - **An empty value is refused too**, for every flag and in both spellings (`--flag=` and `--flag ""`).
    Nothing here has a meaningful empty value, and omitting a flag is already how a caller asks for the
    default — so an empty value can only be the accident a shell produces from an unset variable.
  - The four subcommands ended up *simpler*: they now share one prologue, and each `run` opens with four
    lines where it had five. `--help` is answered before validation, so `sipx dial --help --play` still
    documents the command.

- **A refused early answer now ends the invitation instead of hanging it (`S-25`)** — the one
  place an RFC 4568 §5.1.3 refusal was reported nowhere: an answer arriving in a reliable
  provisional (RFC 3262 §5). `observe`/`adopt_early_answer` now return `Result`, and a refusal
  withdraws the invitation with a CANCEL (RFC 3261 §9.1) and returns `Error::Sdp` naming the
  tag — instead of leaving a caller that never receives a 2xx to time out with no reason.
  - A no-description provisional stays silent, and a guard test answers the 2xx afterwards to
    prove the invitation lived.
  - The registry row for RFC 4568 moves in the same commit, as `AGENTS.md` requires when
    support changes.

- **Reachability now measures *use*, not *paths* — and the caller-check was deliberately not built
  (`X-37`)** — `X-30` and `X-33` both recorded a cross-crate caller check as their successor. This
  story reconsidered it, and the answer was to adjudicate the three named cases by hand and file the
  rest, rather than build a check fitted to the data it was tested on.
  - **RFC 5626 and 8599 are demoted to no roles**, and `docs/compliance.md` moves with it — the
    honest state, not a verdict. Verified by grep: `with_outbound` and `with_push` have **zero
    callers outside `sipx-ua`'s own tests**, the same ICE shape `X-33` suspected and its path check
    could not adjudicate, because both rows satisfied it by citing a genuine caller — `register.rs`
    — for a plain registration. Wiring them back is `S-29`.
  - **Why no check was built.** A syntactic caller-check would be fitted to three rows — wrong in the
    ways macros and re-exports are wrong, and it would quietly stop finding the next shape. The
    accurate version is a dependency plus minutes on the gate. A grep proved the return on either to
    be two honest demotions. Both predecessors named the check a *successor* in prose, after building
    the path check — the one moment building the next check is most tempting and least examined.
  - **Alpha predicate 1 is re-framed.** It now attests the mechanical half (`X-30`, `X-33`, this) and
    defers the rest to `X-38`: ship a real application, after which the reachable-from-a-call surface
    is *defined* as what it uses. That is v1 predicate 3 in other words, and it cannot be gamed by a
    dead-branch citation the way a path check can.

- **The transaction-sequence fuzzer can no longer stop covering something silently (`X-31`,
  alpha predicate 2)** — three of its guards could not catch the thing they were written for, and a
  fuzzer that silently stops covering something is worse than one never written, because the green
  campaign is read as evidence.
  - **The timer table and the `Timer` enum now agree in both directions.** A const assert proved only
    that the table and its count were the same *size*; a fourteenth variant would have been silently
    never-fuzzed. `timer_row` is an exhaustive match, so adding one is now
    `error[E0004]: non-exhaustive patterns`, and a test round-trips every row so the two cannot drift
    on order either — the one drift exhaustiveness cannot see.
  - **An unfalsifiable invariant arm is deleted rather than rescued.** `live > MAX_LIVE_TRANSACTIONS`
    could never fire: the bound equals the vocabulary's key count, so pigeonhole made it decoration in
    a file otherwise careful about exactly this. The two genuinely falsifiable arms carry it.
  - **The corpus check sees additions, not only edits.** CI was `git diff --exit-code`, blind to
    untracked files, so a seed added by hand would have passed. `check-corpus-untouched.sh` checks
    both, as the RFC 4475 check always has; adding a file fails with it named.
  - The RFC 2543 registry item needed nothing: `S-26` had already rewritten the row and deleted the
    stale note in the same commit. Verifying that *was* the item.

- **`respond`'s promise that the response is on the wire is now enforced by the compiler (`X-36`)** —
  `respond_returns_only_once_the_response_has_been_sent` could not detect the thing it was named for.
  Moving the success report ahead of the send left it passing and the whole crate green.
  - **No test can observe that reversal**, which is why it stood. On a `current_thread` runtime,
    sending on a oneshot does not yield, so the send always completed before the waiting task was
    polled — the datagram was out whichever order the two lines were written in.
  - So the guarantee is structural instead: `perform` hands back a `Performed`, and the `Ok` that
    `respond` reports is obtainable only by consuming it. Reversing the statements is now
    `error[E0425]: cannot find value 'performed' in this scope`, verified by doing it. A compile error
    is a stronger pin than a red test and it cannot rot.
  - The 50 ms bound is gone. It bought no detection power at any value, and the argument defending it
    was wrong on its own arithmetic — it rested on a queued send being flushed "within a packet
    interval", which is 20 ms, inside the 50 it was justifying. What remains is a 10 s deadline that
    is a bound on *failure* in `X-29`'s sense.
  - Recorded as a **public guarantee of `respond`** in `docs/designs/sip-transport.md`, because the
    alternative is the failure the code already names at its `NoTransaction` branch: telling an
    application its 200 OK went out while the caller heard nothing.

- **`sips:` is refused rather than sent in the clear (`S-27`)** — `sipx dial sips:alice@host` placed
  the call over **UDP in cleartext**, and `sipx register sips:…` did the same with a digest credential.
  Both commands stripped `sips:` in the same `or_else` as `sip:`, threw the distinction away, and chose
  their transport from one flag (`if args.flag("tcp")`). There is no TLS transport in either path, so
  RFC 3261 §19.1.1 — which makes TLS on every hop the URI's *meaning*, not a hint — was silently
  ignored. Both now refuse, naming the missing capability rather than calling the URI malformed, since
  it is not malformed.
  - **The downgrade was invisible, which is what made it serious**: the call connects, the
    registration succeeds, the audio flows, and only a packet capture shows the promise was broken.
  - **It was two defects, and a fix covering only `dial` was nearly shipped.** `register` has the
    identical shape, and `TransportKind::Tls` appears in that file only inside a *test* of
    `resolve_target` — never in the path a command takes. The refusal now lives in `main.rs` as shared
    policy, because putting it in one command is how the other came to be missed.
  - **Both tests were mutation-checked**: with the call-site disabled they fail `left: 5, right: 2`
    (`Timeout` instead of `Usage`) and take 22 s and 32 s, because the command really does attempt the
    cleartext send. That wall-clock is the defect, not a slow test.
  - The first behavioural test was **vacuous and the mutation caught it** — it passed the URI as the
    only argument, but `Args::positional` skips index 0 as the subcommand, so it asserted the "a URI is
    required" path and passed with the fix disabled. The reason is written at both sites now.
  - Left deliberately: `target_of` still defaults a `sips:` URI to port 5060 rather than 5061. No
    command can reach that code with a `sips:` URI any more, so the wrongness is unreachable, and
    writing a port for a transport this CLI does not have would be inventing a fact. It belongs with
    the `--tls` work.

- **Reachability is now asked of every *selected* capability, not just media ones (`X-33`)** — `X-30`
  made "no claim outlives its caller" mechanical for `layer = "media"`. This widens it on the property
  rather than the string, and each layer was measured before being admitted.
  - `security` is in; **`transport` was measured and declined**, with the reason recorded: it mixes
    capabilities something selects (RFC 7118, 5626, 8599) with plumbing every call runs (3263, 3581),
    and an evidence-path check cannot separate the two.
  - **The `roles`-versus-`status` hole is closed precisely.** `status = "implemented"` does *not* imply
    reachability in general — RTP, SDP and the parser are implemented and selected by nobody — but it
    does at a selection layer. So RFC **6716 and 7587 (Opus) are demoted to `partial`**: they claimed
    `implemented` with no `roles` field at all, which is how they escaped the check entirely while no
    call could select Opus. **The published table now reads 29 implemented / 24 partial where it read
    31/22** — the demotion changes what the artifact says, which is the difference between a demotion
    and a suppression list.
  - **Both escape hatches are shut.** Evidence must now be a `.rs` file in a crate at or above
    `sipx-call`, so `crates/sipx-call/README.md` proves nothing; and `layer` is pinned for any row
    citing `sipx-media`, `sipx-rtp` or `sipx-audio`, so relabelling a media row no longer exits the
    check. One residual is stated rather than hidden: a media capability implemented elsewhere could
    still relabel.
  - Nine rejected rows were corrected and **none suppressed** — 2617, 7616 and 8760 now cite the only
    credential selection above the call layer; 5922 cites a whole call over TLS; 8866, 3550 and 4733
    cite the call layer that runs them.
  - **The four presence rows keep `uas` on a fact nobody had run**: nothing in the workspace receives a
    SUBSCRIBE or PUBLISH off a socket, which is what makes `sipx-ua` the crate that *serves* the role.
    That reason is now a test that goes red the moment anything dispatches on either method, instead of
    a paragraph that would quietly expire.
  - RFC 6665's stale *"no event packages ship yet"* sentence is replaced; `S-17` and `S-18` are done.
  - **Five inherited "facts" failed when actually run**, across this story and its predecessor, and all
    five are corrected in the design: the registry has **117** evidence paths of which **two** are not
    `.rs` (recorded as "80, exactly one"), and a citation to `crates/sipx-cli/tests/cli.rs:116` as
    exercising digest authentication was invented — that line is
    `register_advertises_this_client_in_via_and_contact`, and the tree contains no
    `password`/`401`/`407`/`Authorization` test at all. The conclusions survived; only the evidence was
    fabricated.

- **No test in the workspace now asserts after a fixed sleep (`X-29`, completing what `0.11.0`
  started)** — `X-28` cleared the media path; this is the rest. Twenty-two sites, and the useful
  result is that **three different cures were right**, chosen by what the wait was actually for:
  - **A happens-before already existed — delete the wait.** Five `call.rs` sites, all sleeping after
    `callee.reinvite(…).await`. `reinvite` returns only once the 200 is back, and `on_reinvite`
    applies the direction and records the remote CSeq *before* responding, inside a `handle` call
    across which the pump holds the call's mutex — so `caller.lock().await` on the next line **is**
    the synchronisation. Independently reviewed and mutation-tested four ways, including moving the
    state change 300 ms after the 200 but still inside `handle`, which keeps the test passing and so
    confirms the mutex is what orders it.
  - **An arrival with no ordering to lean on — deadline loop on the condition.** Eleven sites, the
    deadline a bound on failure rather than a window to measure in.
  - **A negative assertion, or a window that is itself the measurement — keep the window and say so
    at the site.** Six sites. A window can only make an empty assertion pass, so the failure mode is
    a missed regression rather than a flake.
  - **The failing-first evidence was not obtained, and the story says so rather than arguing round
    it.** 263 attempts across four sites under 600–1200 single-core spinners produced zero failures.
    The finding is that `X-28`'s method cannot transfer to this family at all: a `tokio` sleep
    **dilates with the load**, because a sleeping task is not competing for the CPU it is denied —
    130 ms of sleeps became 3.0 s under 900 spinners, so the window grows along with the work it was
    supposed to outrun. The original red gate's real trigger was three concurrent compilations, i.e.
    memory and IO pressure, where a process stalls on a major fault without being CPU-starved.
  - **One rationale in this story was false and is retracted in the same breath as shipping it.**
    `udp.rs`'s comment argued the 50 ms bound *is* the assertion. Moving `sent.send(Ok(()))` ahead of
    `perform` in `endpoint.rs` leaves `respond_returns_only_once_the_response_has_been_sent` passing
    and the crate green, so the test cannot detect what its name claims and the bound buys no
    detection power at any value. It also refuted itself on arithmetic, resting on a flush "within a
    packet interval" — 20 ms, inside 50 ms. Filed as `X-36`; the bound stays until there is a test
    that can tell, because removing a clock without one would be the weakening this story forbids.
  - `session.rs`'s loops now name the precondition they lean on — nothing suspends between
    `received.fetch_add` and `note_arrival`'s lock, and an uncontended `Mutex::lock().await` on a
    `current_thread` runtime does not yield. Verified rather than asserted: inserting a 20 ms sleep
    in that gap fails `a_session_reports_the_loss_it_saw` with `left: 9 / right: 10`.

- **The public capability tables stop selling three capabilities no call can reach (`X-35`)** —
  `README.md` describes the compliance table as "a measurement rather than a claim"; the four
  hand-maintained capability tables above it were the opposite, and three of them were wrong.
  - **Opus** was advertised as a stack capability on the README, in `intro.md`, in
    `does-this-fit.md`'s *"It fits if you want to"* list and on the landing page. No call can select
    it: `sipx-call` hardcodes `Capabilities::g711` at six sites, and `Codec::from_payload_type`
    **deliberately** never returns Opus, so even a hand-written peer offer cannot arrive at it. It is
    now scoped to the crates, the way `as-a-library.md` already did it.
  - **Bridging** was sold in five places plus `sipx-call`'s own package description. `Bridge::connect`
    needs an `Arc<MediaSession>` and `Call` lends only `&MediaSession`, with no `into_media`, no
    `Arc` and no `Clone` — so two `Call`s cannot be bridged. `sipx-call`'s description loses the word;
    `sipx-media`'s gains it, because that is where the capability lives. The gap is `C-6`.
  - **DTLS-SRTP** was sold with a workaround — "reachable by building your own capabilities" — that
    cannot be written, because no `MediaSession` can be keyed by DTLS at all: the key types and
    `Config.srtp` never meet, and the handshake cannot share the media port RFC 5764 §5.1.2 requires.
    Both "the two pieces a browser insists on are in place" claims are reduced to one.
  - Four **stale denials** of capabilities that do exist (Outbound, Path, GRUU, push), an RFC count
    off by one, and an under-claim hiding the second interop peer are corrected.
  - **A missing warning is added**: `sipx dial` parses only `--tcp`, so the CLI can never place an
    encrypted call, while the README promised encrypted media beside a `sip:` example. Said plainly in
    `reference/cli.md` without weakening the library's claim, which is true and tested.
  - **The guard is the point.** `X-26` removed the same untruth from `sipx-audio` and it survived at
    `README.md:114` because the check read three strings and the README's crate table was not one of
    them — `--check` exited 0 with a phantom RFC 4733 DTMF claim in place. It now reads **44 front
    doors across all 11 published crates** and asserts crate-table membership equals the set of crates
    without `publish = false`. Still no suppression list, under any name.
  - It also found a defect in its own guard: an earlier pattern for a public item did not allow
    `async`, so `pub async fn play` backed nothing — a crate could have advertised playback with the
    whole feature written in `async fn`s and passed.
  - **Standing risk, recorded rather than left implicit**: the backing synonyms — `digit` for DTMF,
    `refer` for transfer, `flow` for Outbound — are each a real second name, but a synonym added in
    future to turn a red check green would be a suppression list in disguise.

## [0.11.0] — 2026-07-29

### Fixed

- **The compliance table stops claiming roles no call can reach (`X-30`)** — `rfc-report.py --check`
  verified that every cited file exists, never that a claimed capability had a caller. So a feature
  implemented and tested inside one crate, selectable from nowhere above it, read as shipped: RFC 8122,
  8445 and 8839 each claimed both roles on the strength of code no call has ever run. Those three now
  carry **no roles**, RFC 3711 kept both and gained the citations that justify them, and
  `unreachable_role_claims` makes it mechanical — a media row may claim a role only if some cited file
  lives in a crate at or above `sipx-call`. **No suppression list, under any name**, which is the whole
  reason the check is worth having.
  - **Fifth instance of one defect in two days**, alongside ICE (`M-27`), UPDATE (`S-22`), DTLS-SRTP
    (`M-28`) and the SDES answer check (`M-29`). The table is described in `README.md` as "a
    measurement rather than a claim"; for these rows it was neither.
  - **The scope is a choice and now says so.** Measured unscoped at `57857c6` the rule rejects 22 of 29
    role-claiming rows while only 3 rows were over-claiming at all — wrong 19 times out of 22 on the
    question it exists to answer. It is scoped to media because media is where a capability is
    *selected* (`with_srtp`, `with_dtls_srtp`, `start_with_ice`) and selecting nothing is both the
    default and silent: the call still connects and every test in the crate below still passes. Other
    layers cannot fail this way because nothing selects them — there is no `with_transactions`.
    `layer = "media"` is labelled a proxy for selection and held against it by a test.
  - **Two justifications for that scope were shipped before this one and both were false.** The first
    said seven `sipx-ua` rows "cannot satisfy it at any price"; `sipx-cli` sits above both `sipx-call`
    and `sipx-ua`. Its replacement distinguished them from ICE by having a cross-crate caller "which
    `start_with_ice` has none of, in any crate" — `crates/sipx-media/tests/ice.rs:149-150` calls it
    twice, and had that been the criterion 8445 and 8839 would have passed and the correction would
    have collapsed. Twice a crisp-sounding untrue fact stood in for a judgement; the design now names
    that as this story's own failure mode.
  - **Two escape hatches closed or recorded.** The repository-root `tests/` path is gone — it made
    `tests/interop/README.md` proof that a role was reachable — and `layer` being author-set is
    recorded, with the check's own test suite containing the dodge so it stays visible.
  - **Known limit, filed as `X-33`: the gate is on `roles`, not on `status`.** RFC 6716 and 7587 are
    `status = "implemented"`, `layer = "media"`, with no `roles` field at all, so the check never
    interrogates them — while Opus is unreachable from any call, `sipx-call` hardcoding G.711 at six
    sites and `Codec::from_payload_type` *deliberately* never returning it.

- **The DNS TTL test stops racing the scheduler (`X-29`, partial)** — `an_expired_entry_is_not_returned`
  stored an entry with a 50 ms TTL and read it back immediately. Under load the entry expired *before*
  that read, so a gate for a diff that had never opened `sipx-transport` came back red — and a correct
  merge was one command from being reverted for it. The two halves of the test wanted opposite things
  from the clock, so it is now two stores: a TTL the precondition read cannot race, and the real 50 ms
  one for the expiry, waited *for* in a deadline loop where load can only lengthen the wait.
  - Three `quality.rs` drains converted the same way; one left with its fixed window because its
    assertion is negative, where a window can only make it pass.
  - **Roughly 16 of `X-28`'s 20 enumerated sites are untouched and the story stays open** — the
    `sipx-call` half entirely.
  - **A finding worth more than the conversions**: the implementor could not reproduce a flake at the
    `quality.rs` sites — 3/3 passes under 250 spinners pinned to one core — and said so instead of
    dressing it up. Those tests spend most of their window *asleep*, and a sleeping task is not
    starved of a CPU it is not asking for, so they sit far inside their margins. `X-28`'s risk
    ranking for them was too high. `udp.rs:473`, a 50 ms bound on a *positive* socket read, is the
    one plausibly near its edge and is the next site to fix — not the next in list order.

- **A client transaction now gets the key its own responses produce (`S-26`, RFC 3261 §17.1.3)** —
  `from_sent_request` delegated to `from_request`, which is §17.2.3, the **server** rule. For a
  cookieless `Via` that keys on the Request-URI and the `To` tag; a response has no Request-URI at
  all and carries the tag the UAS added rather than the one the request was sent with. So the client
  key never matched any of its own responses — and it did not fail, it retransmitted until Timer F
  with the answer sitting in front of it.
  - Both client derivations now run through one private `legacy_client`, so `from_sent_request` and
    `from_response` agree **field for field by construction** rather than by inspection. The old doc
    comment already claimed this property; now it holds.
  - **`to_tag` is left empty for the client key, and the reason is better than symmetry**: two 200s
    to one forked INVITE carry two different tags and must both reach the single transaction that
    sent it, which RFC 6026's `Accepted` state requires. `from_request` is untouched, so §17.2.3's
    use of the `To` tag to tell one legacy *server* transaction from another still holds.
  - **Reached by an application supplying its own cookieless `Via`, not by an old peer.** A client
    transaction's topmost `Via` is always sipx's own and the transport stamps `z9hG4bK` on it. The
    story was first filed claiming the opposite; corrected before implementation.
  - Found by `X-19`'s fuzzer, whose ignored regression loses its `#[ignore]` here — and the
    campaign's suppression goes with the defect: `KNOWN_DEFECTS` is now empty, `Known` uninhabited,
    and the slot-based masking deleted, so `UnroutableResponse` is reported on every slot. It was
    keyed by slot rather than by cause, so removing it outright is what takes that breadth away.

### Added

- **The gate refuses to report when it cannot be believed (`X-34`)** — five times in one evening a
  full disk produced a red gate that read as a code defect, and a correct merge came one command from
  being reverted for a failure in a crate its diff never opened. Cargo's messages in that state are
  actively misleading: `failed to create file '…/target/debug/examples/canned_program.d': No such file
  or directory` is a vanished `target/`, not a broken build. `./scripts/gate.py` now checks free space
  before it starts and refuses, naming disk and printing both the threshold and the actual figure.
  - **The threshold is measured, not guessed.** A cold worktree was driven through every build step
    with `target/` measured after each — clippy 0.7 GiB, `test` 8.4, examples 0.0, msrv 0.6, feature
    matrix 0.3, docs site 0.5 = 10.6 GiB — plus 10% for cargo's peak while it links. The provenance
    sits at the constant, and a test asserts the threshold covers every size ever measured.
  - **`ENOSPC` and the ENOENT-on-artifact shape are an infrastructure failure, exit code 2, printed
    unlike a red step**: *"NOT A RESULT — the machine stopped this run, not the tree"*, naming the step,
    quoting the evidence, saying why it is not your diff, and stating free space now. Five real
    non-disk failures are asserted **not** to match, because erring toward "it was disk" would hide
    real defects.
  - **A disk failure ends the run**, which contradicts `run()`'s "every step, not up to the first
    failure" rule on purpose: once `target/` is gone every remaining step fails for the same reason,
    and that wall of red is exactly what misled five readers.
  - **A shared `CARGO_TARGET_DIR` was considered and rejected on three grounds**, the strongest being
    that it would promote one worktree's `cargo clean` into everyone's vanished `target/` — occurrence
    4 of that evening — from accident to design feature. An externally set one is still honoured.
  - **The implementor killed its own first design on its own measurement.** It began by crediting an
    existing `target/` against the threshold so a warm gate would not be refused, then found the
    integration worktree had gone 13 GiB → 22 GiB in one evening because nothing there had ever linked
    the integration test binaries. A warm `target/` is no evidence the expensive part is built, so the
    credit would have let precisely that run start with 2 GiB free.
  - Reads with `X-29`: same disease, one layer apart. `X-29` is tests that fail because the machine is
    busy; this is the gate failing because the machine is full. A gate that fails at random trains
    everyone to re-run it instead of believing it.

- **The transaction driver is fuzzed, not only the parser (`X-19`)** — four fuzz targets existed and
  all four stopped at the parser, so the half of the north star about adversarial **timing** had
  nothing at all. A new target drives `TransactionLayer` with a sequence decoded from the fuzzer's
  bytes — incoming messages, application requests and fired timers in any order — with messages
  **built** rather than parsed, so the budget is spent inside the RFC 3261 §17 state machines
  instead of on bytes that do not parse.
  - **The oracle is not "did it panic"**, which finds almost nothing in a state machine. Five
    invariants, each with its own test: no state outside the §17 tables (as amended by RFC 6026),
    no transaction outliving its terminal state, no timer firing for a removed key, a store bounded
    by the vocabulary rather than by the program, and responses that must route.
  - The corpus is seeded from the scenarios the existing FSM table tests already walk — 17 of them —
    the way CI seeds the parser targets from the RFC 4475 corpus, and it runs in the existing fuzz
    smoke job on the same time budget as one parser target.
  - **It found a defect on its first campaign**, which is what the instrument is for:
    `TransactionKey::from_sent_request` derives the *client* key through §17.2.3's **server** rules,
    so a legacy (cookieless) key carries a Request-URI and `To` tag that `from_response` cannot —
    the keys never compare equal and every response is unmatched. Committed as a minimised ignored
    regression and a recorded spec deviation rather than fixed here; the fuzzer is the instrument,
    not the repair. `S-26` fixes it.
  - **Not reachable from the network** — established by independent review, not by the finding
    itself. A client transaction's topmost `Via` is always sipx's own and always carries the magic
    cookie, so this needs an application supplying its own `Via` to `Endpoint::send`. Recorded
    because the first filing said otherwise.
  - `sipx-sip` gained no dependency, no runtime and no clock read: the harness lives in
    `sipx-testkit` and the diff adds zero lines to `sipx-sip/src/`.

## [0.10.0] — 2026-07-29

The release about the difference between *implemented* and *reachable*. RFC 4568's answer check
existed, was tested, and no call ran it — now one does. DTLS-SRTP is implemented in the crates and
reachable from no call at all, so the compliance table stops saying otherwise. And `sipx peers`
answers a question the stack could not answer before: who is there to call.

### Added

- **`sipx peers` — who is there to call (`P-5`)** — the discovery epic's first story, and
  deliberately the one with no protocol in it. A peer book and one command that lists it in both
  the human and machine-readable forms `P-1` set for every other command. **A book that cannot be
  read is an error, not an empty list**: a fresh machine with no book has not told you there is
  nobody to call, and a script that cannot tell those apart calls nobody and reports success.
  - **The format is one peer per line — `name`, whitespace, URI, `#` for comments** — because that
    is the only shape a shell already has verbs for in both directions: `echo "carol sip:carol@host"
    >> "$book"` writes it and `read -r name uri` reads it. Anything structured needs a parser to
    append safely, and none is worth a dependency for two fields. **No dependency was added.**
  - Looked for in `--book`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
    `$HOME/.config/sipx/peers` — the same flag/env/default order `register` already uses.
  - **Every entry carries which source it came from**, though there is only one source today. That
    is what lets `S-24`'s registrar and `T-24`'s local link merge in later without breaking a
    script that already reads the output; a list that flattened them could not be extended.
  - Nothing here consults the network — the module is not even async.

- **`record_at_least`, the counted wait for received audio (`X-28`)** — `MediaSession` and `Call`
  both gain it. `record_until_idle(idle)` spent one duration on two different jobs: how long to
  wait for the stream to *start*, and how long a gap means it has *ended*. Neither is a property of
  the audio — both are properties of how fast the machine is. A caller that knows how much audio
  the far end was given now says so, and `within` is a bound on failure rather than a window to
  measure in.

### Changed

- **The compliance table stops claiming DTLS-SRTP calls sipx cannot place (`M-28`, partial)** — RFC
  5763 and RFC 5764 were `status = "implemented"` with both roles, while `with_dtls_srtp` had no
  caller outside `sipx-sdp`'s own tests. A reader of `docs/compliance.md` would conclude sipx places
  DTLS-SRTP calls; it cannot. Both rows are now `partial`, list **no** roles, and each note opens by
  naming the missing half before describing what the crates genuinely implement — which is a good
  deal, and stays described rather than deleted.
  - **The code half is not here, and the reason is an ordering hazard rather than an estimate.**
    `dial_with` calls `establish()` *before* the ACK, under a stated invariant that from that point
    every path must acknowledge or leave the far end retransmitting its 200 for 32 seconds. A DTLS
    handshake inside `establish()` holds the ACK for the handshake's duration, and a peer that
    starts its own handshake only after the ACK deadlocks with it until timeout — both ends
    waiting, the SDP correct throughout. Keying has to move *after* the acknowledgement, which
    reshapes the 2xx path rather than adding to it. `M-28` stays open for that.
  - A partial selector was deliberately not landed: an offer carrying `UDP/TLS/RTP/SAVP` that this
    side cannot key would connect and carry audio in the clear under a token promising otherwise —
    a worse over-claim than the paperwork one this story exists to fix.

### Fixed

- **The bridge audio test is deterministic under load (`X-28`)** — it recorded **zero of 3200
  samples** while other gates compiled, and zero is not a degraded count. Once the first frame
  lands the rest follow at the packet rate, so a 400 ms idle gap is never reached again: the
  recording is all-or-nothing by construction, and `0 of 3200` means recording never began. Time to
  first frame measured 81 ms idle, 150–273 ms contended, and never under load — 400 ms was never a
  large margin over 81 ms.
  - **The assertion is unchanged, character for character; only the wait moved.** Loosening the
    sample threshold until it passed would have left a test that no longer proves audio crossed the
    bridge, which is the point of it.
  - The sweep classified **46 wall-clock sites**: 30 converted to counted waits, 7 left because they
    assert a recording is *empty* — where a fixed window can only make them pass — 2 left with
    widened gaps, and 2 production sites that were already right. Every one left is annotated with
    why at the site rather than silently.
  - Reproduced by pinning 250 spinners to a **single core**: 10 of 12 failures before, 0 of 12
    after, 0 of 10 at 600 spinners. Saturating all 20 cores did *not* reproduce it — CFS favours a
    sleeper, so starving a `current_thread` runtime takes single-core contention. Worth knowing the
    next time a "CPU load" theory does not reproduce.
  - A second family — a fixed `sleep`, then assert a message arrived — is named but untouched: it
    needs poll-until-condition rather than wait-for-count, and it is `X-29`.

- **A live call now runs the SDES answer check (`M-29`, RFC 4568 §5.1.3)** — `M-26` built the check
  and could not reach `sipx-call`, which a concurrent story held. So the check existed, was tested,
  and no call ran it: `srtp_keys` took two `Option`s, unwrapped both and compared nothing. A call
  whose answer echoed a tag nobody offered connected and keyed media on it.
  - `srtp_keys` now takes the offered attributes as a **slice** and returns `Result`, delegating to
    `SrtpKeys::from_answer`. `establish`, `settle_from` and `dial` propagate the refusal, so the
    call ends through `Error::Sdp` **naming the tag that came back** — and never the key material.
  - **The offerer's check and the answerer's pairing are two functions on purpose.** §5.1.3 binds
    only the offerer; when sipx answers it chose the attribute and echoed its own tag, so there is
    nothing to verify. One function serving both moments would decide at run time which side of the
    exchange it was on.
  - **Behaviour change worth knowing about**: when this side offered a key, an answer carrying no
    usable `a=crypto` is now **refused** rather than placed as a plain call — `docs/specs/srtp.md`
    §5.4, because that is the shape in which "a suite that was never offered" arrives. A peer that
    answered an `RTP/SAVP` offer without a usable crypto attribute previously got an unencrypted
    call; it now gets `Error::Sdp`. Only a call that offered no key at all is still unencrypted.
  - RFC 4568 stays `partial`, deliberately — now for what the RFC defines beyond this exchange (no
    MKI, no key lifetimes, no session parameters, no `RTP/SAVPF`) rather than for a MUST no call
    ran. `docs/specs/srtp.md` §12.3 is closed.
  - Not covered: `Invitation::adopt_early_answer` has no error channel and logs the refusal instead
    of reporting it. Nothing is keyed on the refused answer and the 2xx still ends the call, so the
    loss is the reason rather than the safety — `S-25`.

## [0.9.0] — 2026-07-29

**Secure media in this release does not interoperate with secure media in any earlier one.** The
SRTP authentication key was derived at the wrong length since v0.3.0, so sipx-to-sipx calls agreed
with each other and with nobody else. Fixing it is wire-breaking by necessity: deployments running
sipx at both ends must upgrade both together. See `M-25` under **Fixed**.

### Added

- **The caller gets a handle on its early dialog (`S-22`, RFC 3311)** — everything sipx could
  already do before a call is answered was reachable only from the side that *received* it, because
  `dial_with` awaited the final response inside itself and there was no moment at which the caller
  held anything. `dial_early` now returns a `Dialing` as soon as a provisional establishes a dialog;
  from it a caller can send an UPDATE and receive one, and `Dialing::answered` then waits for the
  call as `dial` would.
  - **`dial`, `dial_once` and `DialOptions` are untouched.** A story that makes the simple case
    harder has traded the wrong thing, so the new handle is a sibling and not a replacement.
  - **§5.1 and §5.2 are lifted into one implementation both roles borrow**, rather than mirrored
    onto the calling side. This is a safety property and not tidiness: RFC 3261 §12.2.2's ordering
    check is what stops a BYE replayed from behind the sequence tearing down a live call — the
    defect fixed in `S-19` — and a second copy of the rules is one refactor away from omitting it.
  - **§5.1's precondition became a type.** `EarlyMedia` is `Offered` until a reliable provisional
    answers our offer (RFC 3262 §5) and `Answered` after, so an UPDATE attempted too early fails
    locally as `NoEarlySession` instead of drawing a 491 from the far end. `Ringing::update` still
    returns `NoDialog`, so nothing an application already matches on changed.
  - **The registry stops over-claiming in the passive voice.** RFC 3311's note opened *"Sent and
    received in an early dialog and a confirmed one"* — role-neutral prose claiming for both ends
    what only the answering end could do. It now names which handle serves which role. The test
    `sipx_sends_an_update_in_an_early_dialog_and_in_a_confirmed_one` is renamed to say `as_uas`; it
    never exercised the UAC path its name promised.
  - Not done, deliberately: `Dialing` exposes no event stream, so an application polls
    `has_early_session` rather than awaiting the answer. That is `C-2`'s to design.

- **The SDES tag is echoed and verified, as RFC 4568 requires twice (`M-26`)** — §5.1.2 and §5.1.3
  are both MUSTs and sipx honoured neither. The answer now carries the tag and suite of the
  attribute actually accepted, built by `Crypto::accepting`, instead of always answering tag `1` —
  which is what `Capabilities::with_srtp` fixed it at. An endpoint that always answers `1`
  interoperates only with peers that happened to offer `1`, and fails undiagnosed at the end that
  is right.
  - **§5.1.3's check is now the only route from an answer to keys.** `Crypto::verify_answer`
    performs all three parts — an offered suite, its accompanying tag, and a key — and returns
    *which* offer the answer accepted, so a caller keys with the half it actually sent.
    `SrtpKeys::from_answer` returns `Result` and not `Option` on purpose: an answer that agreed to
    nothing must be a typed failure, because the two alternatives are both worse. Placing the call
    unencrypted gives a user who asked for secure media an insecure call with nothing said, and
    dropping the stream ends the call with no reason anyone can act on.
  - **Byte-level, off-stack.** RFC 4568 §6.1's published `a=crypto` line is asserted against
    `Crypto::parse` and decodes to the documented 16-octet master key and 14-octet salt. It passed
    on the first run — the parser was already right — which is the usual outcome of a published
    vector and not a reason to have skipped it: nothing distinguished this case from `sipx-rtp`'s
    `n_a` defect beforehand, and that one shipped for six releases.
  - **A live call does not run this check yet, and the registry says so.** `srtp_keys` in
    `sipx-call` still pairs the two halves and compares nothing; the RFC 4568 row names the missing
    wiring under **"Still missing"** rather than claiming the MUSTs hold end to end. `M-29` is the
    remainder.

- **An interop call whose media is actually encrypted (`X-27`)** — the interop harness had never
  exchanged SRTP with a peer that did not come from this repository, which is why `M-25`'s wrong
  authentication key shipped through six releases. `crates/sipx-cli/tests/interop_srtp.rs` now
  places a TLS-signalled call with `RTP/SAVP` against a real peer and asserts three things the
  *far end* observed: that the negotiation chose SAVP (so the case cannot pass by degrading to the
  cleartext call already covered), that the peer logged no authentication failure, and that the
  audio it echoed back is the audio sipx sent.
  - **The falsification was run, not reasoned about.** Reverting `M-25`'s one-line
    `SESSION_AUTH_LEN` fix makes the case fail on the peer's own words — `SRTP unprotect failed …
    because of authentication failure 10`, with the peer's media counters showing `Receive Count
    0`. The defect this coverage exists to catch is one it demonstrably catches.
  - A `media-security` role in `tests/interop/run.sh` runs it per peer, and CI picks it up with no
    workflow change because the matrix already calls `run.sh --peer`. The keying axis reports all
    three outcomes by name — peer cannot, sipx cannot, ran — so a gap is printed on every run
    rather than being an absence.
  - **Only SDES runs; DTLS-SRTP has no sipx side to test.** `dial` hardcodes `.with_srtp(…)` and
    `DialOptions` carries no keying selector, so `sipx_media::dtls` and
    `Capabilities::with_dtls_srtp` have no caller outside their own crate's tests. The harness is
    already shaped for it and says so out loud; closing the gap is `M-28`, which also stops the
    conformance registry claiming RFC 5763 and 5764 in the meantime.

- **The application contract crate and its interpreter (`C-5`)** — a new crate,
  `sipx-app-protocol`, owning the `sipx.app.v1` types and an instruction interpreter that is a
  pure state machine: call events and an instruction program in, typed effects out. The primitive
  every binding drives, remote or in-process. **Experimental**, matching the spec's status.
  - **No third-party dependency, and the library half has no mandatory dependency at all.** The
    wire codec is the crate's own, because this workspace has no serialization framework to
    borrow and taking one would mean two new dependencies plus a proc-macro step to serve one
    leaf crate. The `sipx-call` adapter sits behind an off-by-default `call` feature, so a remote
    SDK wanting only the wire format and the state machine does not inherit a runtime, a socket
    stack and a media session to get them.
  - **The spec's continuation rule is enforced by construction, in both halves.** `Program`'s
    queue is private to its own module and exposes only `replace`/`abandon`/`take_next`/`len` —
    an interpreter that tried to append to a running program would not compile. `Callback` is
    `#[must_use]`, neither `Clone` nor `Copy`, has no public constructor and is consumed by
    value, so answering one delivery twice is not a mistake to detect but a program that does not
    build.
  - The document parser is the crate's one reader of app-supplied input and is bounded as such:
    `MAX_DEPTH` of 32 checked on every recursion, no `unwrap`/`expect`/`panic`/slice-index, and
    strict base64 that refuses rather than repairs.
  - The epic's end-to-end proof — the interpreter driving a real call with no host — runs in the
    gate as step 18 rather than by hand.

- **A caller can give up before sipx answers (`S-23`, RFC 3261 §9.2)** — the UAS half of CANCEL,
  which sipx never implemented. A CANCEL for an invitation still ringing is answered `200` on its
  own transaction and the INVITE it withdraws is answered `487 Request Terminated`; one matching
  no pending INVITE draws `481` rather than being routed somewhere and ignored. Until now sipx
  kept ringing and left the caller's stack to time out.
  - The application is told, and cannot answer afterwards. `EndCause::RemoteCancel` reaches a
    ringing host through the same event vocabulary a talking one uses, because "stop ringing" and
    "hang up" are the same question asked at two different moments.
  - A CANCEL arriving *after* a 2xx is not a teardown — §9.2 is explicit that it has no effect on
    a transaction that has already answered, and BYE is the request for that. Tested as a negative.
  - **Beyond what the story asked**: §9.1's `Call-ID` and `From` tag are checked on top of §9.2's
    transaction match. Every well-formed CANCEL carries them, so the check costs nothing — and a
    `Via` sent-by is the attacker's to write, so without it, observing a branch is enough to stop
    someone else's phone ringing.

- **ICE on the media port (`M-22`, RFC 8445 + 8839)** — the sans-IO agent `M-21` built now has the
  socket and the clock it was written for, so a call can traverse a NAT symmetric RTP cannot.
  Host and server-reflexive gathering off the bound media port, connectivity checks demultiplexed
  from RTP by `dtls::classify` (RFC 5764 §5.1.2), §11 keepalives on selected pairs, and a
  nominated pair replacing symmetric-RTP address learning for the stream that has one. Fourth and
  last of the ICE stories that make it usable; restart is `M-23` and relayed candidates `M-24`.
  - **A peer that offers no ICE gets exactly today's behaviour** — nothing offered, no checks, no
    timers, symmetric RTP. Demonstrated rather than asserted: the `quality`, `srtp`, `bridge`,
    `conference`, `opus` and `dtls_srtp` suites are byte-identical to before. A stack that
    required ICE to place a call would have regressed.
  - **The driver arms only timers the agent asked for.** A driver with a schedule of its own can
    keep an agent that has stopped asking for ticks looking alive — the defect `M-21`'s review
    caught one layer up, in a test that fired the pacing timer by hand.
  - `ice-mismatch` (RFC 8839 §5.3) reported in the answer when the offer's default destination for
    a component matched none of its candidates, with RFC 3264 procedures used for that stream.
  - Corrects `docs/specs/ice.md` §2 — the fourth error found by the fourth story to implement
    against it. `Input::DataSent` named a `PairId`, which a driver cannot produce: the media path
    knows only that a packet went out for a component, and re-deriving the pair would reimplement
    outside the agent the question the agent had just answered.

- **A spec for SRTP and its two keyings (`M-25`)** — `docs/specs/srtp.md` covers the transform and
  its key derivation, SDES, DTLS-SRTP, the profiles, and which keying wins when a peer offers
  both, with byte-level vectors marked derived, reconciled or new. `M-14` and `M-15` shipped
  without the spec AGENTS.md requires; `X-25` found the breach and this closes it. Writing it
  against the RFC rather than against the code's apparent intent is what found the defect above —
  see *Fixed*.

- **The sans-IO ICE agent (`M-21`, RFC 8445)** — gather, prioritise, pair, order, check, resolve
  role conflict and nominate, as a pure function of events: no socket, no clock read, time arriving
  as a fired timer. Third of the six stories `M-16` was cut into; the driver that puts it on a real
  port is `M-22`.
  - §5.1.2.1's priority formula exactly, asserted against the number RFC 8839 prints in its own
    example, and a check carries the peer-reflexive type preference rather than the candidate's own.
  - Role conflict per §7.3.1.1, all seven rows plus the equal-tiebreaker case. **The tiebreaker
    redrawn after a 487 is a fresh random value, not a derivation** — two agents applying the same
    rule to the same value stay equal and oscillate roles forever.
  - **Regular nomination only.** Aggressive nomination is deprecated by §4 and is absent with no
    option to enable it; a controlled agent cannot encode `USE-CANDIDATE` at all.
  - Corrects `docs/specs/ice.md` §6.5, which named only the success-driven unfreeze. Without RFC
    8445 §6.1.4.2 step 2, a foundation whose one unfrozen pair *fails* stays frozen for the rest of
    the session and ICE reports failure for a path it never finished checking.
- **Many calls from one endpoint (`C-4`)** — a dispatcher owns the endpoint's receiver and routes
  each request to the call whose dialog it belongs to, so a host can hold N concurrent calls
  without writing its own demultiplexer. A new INVITE that matches no call surfaces as an
  invitation for the application to answer, ring or reject.
  - **Nothing ends in silence** — `481` for an in-dialog request naming no live call, `405` with
    `Allow` outside a dialog, `482` for a genuinely merged INVITE, `400` for a request carrying no
    `Call-ID` or `From` tag, `503` with `Retry-After` when one call's inbox is full, and a stray
    ACK counted rather than refused. This is `T-19`'s rule carried up to the call layer.
  - **One stalled call cannot stall its siblings**: per-call delivery is bounded and never awaited,
    so a full inbox sheds for that call alone.
  - The route key is `Call-ID` plus the *peer's* tag, never the local one — which is what lets a
    route be reserved from the INVITE alone, before the application has chosen how to answer and
    therefore before a local tag exists.
  - A CANCEL for a routed invitation is delivered, but sipx still has no UAS-side CANCEL handling
    at all; the spec says so plainly and `S-23` will close it.
- **The media design record (`X-25`)** — `docs/designs/media.md` was a 37-line outline headed
  "Stories: _to be cut_" that named no story, no ICE, no DTLS-SRTP, no bridge and no mute, while
  nine delivered stories and all six ICE stories cite it as their design record. It now describes
  the stack as delivered, says up front whether a reader wants a design record or a spec, and
  writes down the sans-IO argument that `docs/specs/ice.md` had been assuming without making.
  Five decisions no evidence could be found for are listed as *unrecorded* rather than given an
  invented rationale.
- **A STUN connectivity check, encoded and answered (`M-20`)** — the codec ICE checks run over, in
  `sipx_media::ice::stun`: the attributes, the credentials and the two integrity values. Second of
  the six stories `M-16` was cut into.
  - **Anchored to IETF-computed bytes, not to its own encoder.** RFC 5769 §2.1 and §2.2 are both
    reproduced byte-for-byte, and §2.2 is keyed with the password §2.1's `USERNAME` resolves to —
    so the inbound direction of the username rule is pinned by published bytes rather than by a
    test mirroring the encoder it is checking.
  - `MESSAGE-INTEGRITY` over the length-adjusted message then `FINGERPRINT` computed last, with a
    constant-time tag comparison. An attribute that would smuggle in its own integrity value is
    refused at encode.
  - **Borrowing twenty bytes of header layout does not cost a TLS stack**: the `sipx-transport`
    edge sets `default-features = false`, and `check-features.sh` now asserts that on the resolved
    graph the way it already did for `sipx-ua`.
  - Corrects `docs/specs/ice.md` §11.1, which required `USERNAME` on a success response — RFC 5389
    §10.1.2 and RFC 8445 §7.2.2 both forbid it.
- **The UPDATE method (`S-19`, RFC 3311)** — renegotiate a session *before* it has been answered,
  which is the only way to change one in an early dialog, and refresh a session with UPDATE where
  RFC 4028 §7.4 recommends it over a re-INVITE. M9's last session-integrity gap.
  - **Three refusals, two status codes, as §5.2 requires** — 491 for glare, and 500 with
    `Retry-After` both for an exchange already in progress and for an offer arriving while this
    side still owes an answer. 491 tells a peer to back off per §14.1; 500 tells it to retry.
  - **A 488 refuses the description without killing the dialog** — the next UPDATE is accepted and
    the call still answers on what it settled.
  - **A session refresh prefers UPDATE when the peer's `Allow` lists it** and falls back to a
    re-INVITE otherwise. A peer that does not advertise UPDATE sees exactly `S-11`'s behaviour.
  - **A confirmed dialog still prefers a re-INVITE for renegotiation.** `Call::update` is opt-in;
    `Call::reinvite` is unchanged.
  - RFC 3311 moves off "syntax only"; RFC 3262 and RFC 4028 lose the notes recording UPDATE as
    missing.

### Fixed

- **SRTP authenticated with a key no conformant peer computes (`M-25`, RFC 3711)** — **this is a
  wire-breaking fix.** A sipx built from this release cannot exchange secure media with a sipx
  built before it, in either direction, for both SRTP and SRTCP; deployments running sipx at both
  ends must upgrade both together. It is still the right change, because nothing else ever
  interoperated either.
  - The session authentication key was sized at **94 octets**. §5.2 and §8.2 fix `n_a` at 160
    bits, and §4.3.1 derives `n = n_a` while stating no length of its own. The 94 comes from
    §B.3, which *posits* an authentication function needing 94 octets so its worked example walks
    the PRF through six AES blocks — a property of the appendix, not of the transform.
  - **A different key, not a weaker one.** HMAC-SHA1's block size is 64, so RFC 2104 reduces any
    longer key to `SHA1` of itself: sipx keyed with `SHA1(the 94-octet block)` where every
    conformant peer keys with that block's first 160 bits. Every tag sipx produced failed
    verification at a correct peer, and every tag a correct peer produced failed at sipx, on the
    first packet. Entropy was capped at the 128-bit master key either way, so no key was weakened
    and no traffic was exposed by it — sipx-to-sipx media was encrypted and authenticated exactly
    as intended, against the wrong constant.
  - **Shipped in v0.3.0 through v0.8.0.** Nothing caught it because nothing could: all 17 SRTP
    tests were round-trips or tamper-negatives, which pass identically whether the key is 20
    octets or 94, since both ends are wrong the same way. The suite was blind to it rather than
    agreeing with it, and the interop harness has never placed a call with encrypted media at
    all. `srtp.rs` now carries a tag vector computed off-stack.
  - The SRTCP index was also incremented *before* each packet rather than after, so the first
    packet carried 1 and index 0 was never emitted (§3.4, a MUST). Not interoperability-breaking
    — the index travels in the trailer — but it selects the SRTCP keystream's counter block.

- **A replayed BYE could end a live call through the early dialog (`S-19`)** — found by review
  before release. The early-dialog path applied no RFC 3261 §12.2.2 ordering check and wrote the
  dialog's remote `CSeq` *backwards*, so a request from behind the sequence was accepted and a
  subsequent replayed BYE tore down a running call. The confirmed-dialog path had always guarded
  this; the ordering rule now lives on `Dialog` itself, so every in-dialog path shares one
  chokepoint rather than each growing its own.

### Changed

- **`sipx-audio` stops advertising what it does not have (`X-26`)** — the package description, the
  crate documentation's summary and the website's "which crate" table all promised G.722,
  resampling and RFC 4733 DTMF. The crate implements none of the three; DTMF lives in `sipx-rtp`,
  and the CLI tells the user to resample the file themselves. All three strings now name what is
  there, including Opus, which the crate *does* have and the description omitted.
  - **The decision, recorded rather than deferred: G.722 is not coming.** `X-25` went looking for
    why it had been dropped and found nothing — no story among twenty-five media stories, no spec,
    no commit that implemented or cut it, only the scaffolding commit that wrote the blurb. The
    stack is specified in the opposite direction: `Codec::from_payload_type(9)` returns `None`,
    `sipx-sdp` answers an offer of G.722 with port 0, and `sipx-call` refuses a call offering
    nothing else, with three tests asserting exactly that. The wideband slot it would have filled
    is Opus's (`M-13`). Written down in `docs/designs/media.md`, closing gap 3 of the record.
  - `scripts/check-audio-claims.py` holds the three front doors against the modules the crate
    exposes, and against each other: a codec needs a module that both encodes and decodes it, a
    capability needs a public item, and an optional codec must be advertised as optional. Wired
    into the gate with its own suite beside it, per `X-22`; the gate is 17 steps.

- `sipx_call::Error` gains `UnacknowledgedProvisional` and `NoEarlySession` (`S-19`) and
  `InvitationCancelled` (`S-23`). Additive, but source-breaking for a downstream exhaustive
  `match`, as previous variants have been. `EndCause` is `#[non_exhaustive]`, so `RemoteCancel`
  joining it breaks nothing.

- **A CANCEL racing an answer is now honoured in full (`S-23`)** — one arriving while `answer` is
  setting media up sends the `487` and returns `Error::InvitationCancelled`, where before it drew
  a bare `200` and the answer succeeded, leaving the caller to notice the 2xx and send a BYE per
  §9.1. Both are defensible; the new one is what the caller actually asked for.

- **RFC 8839's ICE attributes in SDP (`M-19`)** — `sipx_sdp::ice` parses and serialises
  `candidate`, `ice-ufrag`/`ice-pwd`, `ice-options`, `ice-lite`, `ice-mismatch`,
  `remote-candidates` and `ice-pacing`, so the rest of ICE will negotiate over a typed description
  rather than a substring search. Pure parsing — no runtime, socket, clock read or new dependency.
  The first of the six stories `M-16` was cut into.
  - **The priority range check is load-bearing, not defensive.** RFC 8839's grammar is
    `1*10DIGIT`, so `4294967295` parses, and the RFC 8445 §6.1.2.3 pair-priority arithmetic
    overflows `u64` on it. Checked on parse, behind a private field with no public bypass.
  - **Lenient in the right direction**: a candidate line naming an FQDN, an unsupported address
    family or a non-UDP transport is ignored line by line while the rest of the description
    survives byte-identically, and unknown extensions are kept and re-emitted. A parser that
    rejected a whole description over one unusable line would break calls with legal peers.
  - Media-level `ice-ufrag`/`ice-pwd` win over session level and are never mixed across levels.
  - Corrects `docs/specs/ice.md` §6.2, which stated a maximum pair priority that cannot be
    attained and put the overflow threshold one bound too early.

## [0.8.0] — 2026-07-29

### Fixed

- **The interop suite's flake was two runs sharing one machine (`X-23`)** — a call test failed
  about one run in five, and the cause was neither the readiness marker everyone suspected nor a
  timeout that was too tight. `tests/interop/run.sh` had no mutual exclusion while everything a
  run reserves is machine-global: it removes the peer container by fixed name at start-up,
  removes every labelled container at cleanup, and the peer runs on the host network on fixed
  ports. A second run deleted the first run's peer mid-call, which is why *both* call tests
  failed together on their twenty-second timeout.
  - A run now holds an exclusive lock for its whole life and a second run waits its turn.
  - **The timeout is untouched and no retry was added.** The bound was never the problem, and
    widening it would have hidden the cause.
  - Measured rather than asserted: 12 of 16 overlapping runs failed before, 0 of 16 after, and
    0 of 10 run alone. "One run in five" was how often two runs happened to overlap.

### Added

- **A push notification wakes a client that holds no connection (`T-21`, RFC 8599)** — the UA half.
  Every other mechanism in the stack assumes there is something the registrar can route down;
  this is what is left when there is nothing.
  - **`pn-provider`, `pn-param` and `pn-prid` go inside the `Contact` URI's angle brackets**,
    where a registrar's URI parser looks. Outside them a `;` starts a header parameter, which is
    a different field of a different grammar — a registrar would answer 200 and record a binding
    nothing could ever wake.
  - **The ordering RFC 8599 §4.1.3 fixes is a type, not a comment.** `UserAgent::woken` sends the
    binding-refresh REGISTER and only then hands back the permission to expect the request the
    push was sent for. A client that waits for the INVITE instead is waiting on a flow that does
    not exist yet.
  - **555 is surfaced as itself**, not folded into a generic failure — it is the one refusal no
    credential and no retry can fix — but only to a registration that actually named a push
    service, and it keeps the CLI exit code every other refusal uses.
  - **`sip.pns` answers "can this registrar wake me"**, which comes apart from "did the
    registration succeed": a registrar supporting some other push service answers 200 and records
    a good binding that nothing will ring. `sip.pnsreg` and `sip.pnspurr` are read too.
  - **sipx ships no push service implementation and that is deliberate** — it is a trait the
    application adapts to whatever it already uses. A contact that cannot carry the parameters is
    returned unchanged and warned about rather than registered as if it could.
- **The connection pool key is generated from the type that defines it (`X-24`)** —
  `docs/specs/sip-transport.md` §8 said the pool was keyed by `(transport, remote address)`.
  `ConnectionKey` has carried four fields since `T-23`, and the sentence had already been wrong
  once before that: it went stale when the verified TLS identity joined the key and stayed stale
  when the WebSocket resource did.
  - **§8 is now the only place the key is enumerated**, in a region rendered from the struct's
    fields and doc comments by `scripts/check-pool-key.py` — `--check` in the gate, `--update` to
    regenerate. `sip-tls.md` §5 and `sip-quic.md` §6 link to it instead of restating it.
  - **§5 keeps the argument for *why* each field is in the key**, which is the half no generator
    can write and the reason "point at one definition" was not sufficient on its own.
  - A field added to `ConnectionKey` now fails the build before it reaches a reader, and the
    failure names the command that fixes it.
- **A WebSocket target names its own path and port (`T-23`)** — `Target::at_path` says where on a
  server SIP lives. RFC 7118 §5 registers a subprotocol and fixes neither the resource nor the
  port, so a server is entitled to serve SIP at `/ws` on its own HTTP server — and sipx asked for
  `/` on the SIP port unconditionally, which reached one kind of server and none of the others.
  - **The default is unchanged.** A target that names no resource asks for `/`, so every
    arrangement that worked before works identically.
  - **The resource is part of connection identity**, for the reason the verified TLS name already
    is: a socket upgraded at `/ws` was accepted by whatever serves `/ws`, and handing it traffic
    meant for another resource throws away the only thing the target said about where it was
    going. Two resources on one address are two pooled connections.
  - **The interop peer's divergence list is empty for the first time.** The shared WebSocket test
    reads the port and path a peer's profile declares rather than assuming the SIP port and the
    root, and it passes live against both peers — the disagreement `X-17` found is closed, not
    worked around.
- **The gate is a program that checks itself against CI (`X-22`)** — `./scripts/gate.py` replaces
  the command list in `AGENTS.md`, which once omitted a job CI runs: the `msrv` job was red from
  v0.4.0 through v0.7.0 while every documented command passed.
  - `--check` reads `.github/workflows/ci.yml` and fails when the gate and CI disagree — a job
    neither mirrored nor declared CI-only, a flag CI passes that a step drops, or an `msrv` pin
    that differs from the workspace `rust-version`. It runs as a gate step and as a CI job.
  - The MSRV toolchain is derived from the workspace `rust-version` and written nowhere else;
    if it is not installed the step fails and prints the `rustup toolchain install` line — never
    a skip, since a skipped MSRV check is indistinguishable from the defect it exists to catch.
  - Two more omissions surfaced on the way and are steps now: the documented gate never built the
    examples, and it ran without CI's `RUSTFLAGS: -D warnings`.
- **Playback control — queue, stop, interrupt on digit (`M-17`)** — the primitive under "play a
  prompt and collect digits". `MediaSession::start_playback` (mirrored on `Call`) returns a
  `Playback` handle; `Call::play` keeps its signature as the uninterruptible await of one.
  - Stopping takes effect within a stated, tested bound — `Playback::STOP_BOUND_PACKETS`, two
    packet intervals — for `stop` and for interrupt-on-digit alike.
  - Clips queue rather than replace, so stopping is never an implicit side effect of starting;
    the choice and its queue-while-stopping edge are recorded in `docs/designs/app-sdk.md`.
  - A received DTMF digit (RFC 4733) cuts the prompt short without being swallowed: the
    interrupt arms in the receive path only after the digit reaches the application's channel.
  - `CallEvent::PlaybackFinished` now says which playback ended and how — completed, stopped,
    interrupted, or the session ended under it.
- **A second independent interop peer, and the first foreign answer (`X-17`)** — until now every
  interop test ran against one peer, and no implementation sipx did not write had ever answered a
  call it placed. Both are now false.
  - **A peer is a directory, not an edit.** `tests/interop/run.sh` names no image, container or
    configuration directory; a peer is a directory holding a `profile.sh` that declares which roles
    it can play. CI builds its matrix from `run.sh --list`, so adding a peer needs no CI change.
  - **The same test list, unchanged.** The eight non-WebSocket server tests passed against the new
    peer on the first attempt. A test that needed rewording per peer would have been hiding an
    assumption.
  - **The peer shares no ancestry with the first**, so a message leaving sipx is now read by two
    parsers with no common code — chosen against criteria recorded in `tests/interop/README.md`
    rather than by preference.
  - **The media assertion is bytes, not liveness.** In relay mode the whole clip comes back byte
    for byte in both directions, with every packet's payload type checked against the negotiated
    one — `M-3`'s bit-exactness, with a foreign stack in the middle.
  - **The one disagreement is filed, not papered over**: `T-23` records that sipx's WebSocket client
    hardcodes the request path, with the RFC 7118 §5 sentence saying neither path nor port is fixed.
  - The suite stays `#[ignore]`d by default, so `cargo test` still needs no containers.
- **GRUU (`T-20`, RFC 5627)** — one *instance* of a registration becomes separately addressable: a
  URI that routes to this UA and to no other registration of the same address of record. The UA
  offers `Supported: gruu` with its instance ID, keeps the `pub-gruu` and `temp-gruu` the registrar
  issues, publishes one as the `Contact` on dialog-forming and target-refresh requests, and
  recognises a request sent to its own GRUU.
  - **One instance identity, enforced by structure.** Outbound (RFC 5626) and GRUU both name the
    device with `+sip.instance`, so the configuration holds it in a single field rather than two
    that could disagree — a registrar correlating them would otherwise see one device claiming to
    be two.
  - **The address of record is URI-equivalent to the public GRUU (§5.4) and still must not be taken
    for it.** That case and another instance's GRUU are both asserted as negatives, not assumed.
  - **A GRUU is discarded with the binding behind it.** Replaced on every 2xx and cleared when an
    attempt yields none: a stale GRUU is an address that reaches nothing, published in the header a
    peer routes its next request by.
  - Registrar behaviour is deliberately out of scope and the registry says so — RFC 5627 moves to
    *partial*, with roles `uac`/`uas` and a note naming what minting a GRUU would require.

- **The host configuration schema is normative (`A-1`)** — `docs/specs/host-config.md` goes from
  draft to normative: a bounded TOML subset as the concrete syntax, the listener and app schema,
  the failure and grants tables, reload semantics, and thirty vectors `HC-1` … `HC-30`.
  - **The vectors execute.** `crates/sipx-app/src/config/` reads a document, so a normative point
    is pinned by a test rather than by a paragraph. It adds no dependency — the subset is hand-read,
    and §2 defines it narrowly enough that the reader cannot be more permissive than the page.
  - **Coverage is measured against the page, not a copy of it.** The tests parse §3's points and
    §8's vector table out of the spec itself and hold both against what actually runs, in both
    directions — so a normative point added without a vector fails rather than passing quietly.
  - **The failure knobs are the contract's own**, derived from the policy the `A-7` harness already
    tests, so `app-contract.md` §9.2 cannot drift from what a document is able to set.
  - **Secrets are by-name references only**, with a name grammar narrow enough that pasted key
    material does not fit through it, and a vector asserting the reference document is committable.
  - The multi-app-versus-multi-process question is recorded as **explicitly open**, with four
    numbered requirements phase 4 needs preserved either way — the first of them a vector, not prose.
- **The RFC registry's grain is decided, and now enforced (`X-15`)** — the registry stays at one row
  per RFC. Requirement-grain rows were considered against the alternative and declined, with the
  reasoning and the reopen triggers recorded in `docs/designs/rfc-registry-grain.md`.
  - **The key set is closed.** `tomllib` accepts any key, and a checker that reads only the keys it
    knows walks past the rest — so a finer-grained row could land in the source, never reach the
    generated table, and go unmentioned. It now fails `rfc-report.py --check` by name.
  - **`docs/rfc/README.md` states the schema as a contract**, so a downstream registry can inherit
    kernel rows by reference at a pinned version instead of restating the claims.
  - `scripts/test-rfc-report.py` covers the checker itself — previously the thing that verifies
    every compliance claim had no tests of its own.
- **Mute and unmute (`M-18`)** — `Call::mute`, `Call::unmute` and `Call::is_muted` stop a call
  contributing audio to the far end without renegotiating anything. Unlike hold, this is a purely
  local gate: no re-INVITE is sent, the SDP direction is unchanged, and the far end's own hold
  state is untouched.
  - **A muted call sends silence, it does not stop sending.** Packet for packet, with the same
    pacing, sequence numbers and timestamps — the audio is replaced, the stream is not. Stopping
    would close the NAT pinhole, leave the far end's jitter buffer to restart so the first word
    after unmute is clipped, and make "muted" indistinguishable on the wire from "gone away".
  - **The gate sits before the packet is built**, which is what keeps RFC 3550 §6.4.1 honest: a
    mute that dropped the finished datagram instead would overstate this side's own sender-report
    counts and manufacture apparent loss at the far end out of a caller who was merely quiet. The
    far end's `packets_received` and its zero `cumulative_lost` are asserted across a mute.
  - **Keypresses still go through.** A telephone event is generated by the endpoint on purpose,
    the way a keypad tone is on a handset, so a muted caller can still answer an IVR.
  - **Mute survives a media restart** — a far-end re-INVITE that moves the media cannot unmute the
    call behind the application's back — and transitions surface as `CallEvent::Muted` /
    `CallEvent::Unmuted`, emitted only on a real change of state.
- **The deterministic harness (`A-7`)** — `crates/sipx-app`'s first code, and the apparatus every
  later claim about the application host is held to. It drives the host's decision logic with fake
  time, a scripted app and scripted call events: no sockets, no engine, no transport endpoint, no
  clock. The slow app, the flapping app and the absent app are ordinary test cases.
  - **The contract's own vector set runs today.** `AC-1` … `AC-9` from `app-contract.md` §11 are
    expressed as scenarios and pass — before `C-5`'s interpreter exists, which is the reason this
    story came first. What is under test is the actor's logic: delivery and §6.3's alternation
    rule, `seq` and redelivery, the bounded event queue, §6.1's blocking discipline, and §9.2's
    declared failure semantics.
  - **A scenario is data, and so is its expectation** — the app's script with per-reply delays, the
    call events, the redeliveries, and the effects and outcome expected of them. That is what lets
    `A-2` and `A-4` be held to the same twelve failure-semantics scenarios (four §9.2 knobs × three
    declared actions) instead of each restating what `on_5xx: hangup` means.
  - **A scenario needing a real socket or real time cannot be written down.** A binding does not
    wait for an app and return what it said; it declares up front *when* it will answer and with
    what. A real HTTP client cannot answer the second half before making the call. With a clock
    that has no `now()`, that is acceptance enforced by types rather than by review — a minute of
    virtual time costs under 250 ms of real time, and there is a test that says so.

### Changed

- **`sipx-ua` (breaking, pre-1.0)** — `Config` and `Registration` drop the `outbound` field in
  favour of `instance`, `reg_id` and `gruu`, so the device identity Outbound and GRUU share lives
  in one place. `registrar::interpret` now takes the `Registration` rather than three positional
  arguments. `Config::with_outbound` is unchanged, so existing call sites are unaffected.

## [0.7.0] — 2026-07-29

### Changed

- **The timer queue is generic over its instant (`X-21`).** `TimerQueue` documented that "nothing
  here has an opinion about what an instant means" while its field was a `tokio::time::Instant` —
  a type whose only constructors are `now()`, which reads the machine clock, and `from_std`, which
  needs a `std::time::Instant` that has no zero either. A discrete-event simulator on virtual time
  had no instant to hand in and could not build one, so the one caller the queue was generalised
  *for* was the one caller that could not use it.
  - `TimerQueue<K, I = Instant>` with `I: Ord + Copy + Add<Duration, Output = I>` — the minimum the
    queue actually uses: compare deadlines, copy them out, add a `Duration` to get one.
  - **Additive: no existing caller changes.** The default type parameter means `TimerQueue<K>`
    still names exactly what it named before, and a test asserts that rather than leaving it to the
    build to notice.
  - The ordering bounds moved from the key to the instant, because ordering is by deadline alone.

### Documentation

- The website catches up to what has shipped: DTLS-SRTP is keyed on the media path rather than
  "not built yet", the event framework and its three packages are described with the join to live
  dialogs named as the caller's, and ICE is stated as the reason browser interoperability is still
  out of reach rather than SDES keying.

## [0.6.0] — 2026-07-29

### Added

- **A call reports what happens to it as a typed event stream (`C-3`).** A `Call` was only visible
  by calling methods on it at the right moment — `is_on_hold`, `is_ended`, `transfer` — which meant
  a host had to know when to look. Every state change is now also pushed once onto a channel the
  call owns: ringing (with whether the provisional was reliable), answered, a DTMF digit and how
  long it was held, playback and recording finishing, an inbound REFER with its target, transfer
  progress, hold and resume by the far end, and ended with a cause.
  - **`Call::events` hands out the receiver exactly once** and returns `None` after — one consumer
    by construction rather than by convention.
  - **The overflow policy is the part worth knowing.** The channel holds 32, and one slot is
    reserved for `Ended` at construction, before any ordinary event can claim it. Ordinary events
    are dropped rather than queued when the consumer is behind — each carries a snapshot, so a
    consumer that missed one resynchronises from the next. `Ended` is not like that: it is a call's
    last word, and a consumer that never learns a call ended waits forever. So it gets a reserved
    slot rather than a policy, and nothing on the ending path awaits the channel having room. A
    consumer that never reads at all cannot stall a call's teardown, which is tested directly.
  - Events are emitted where the state changes rather than reconstructed afterwards, so the stream
    cannot disagree with the call, and `dial` and `serve` go through the same path. No clock reads
    were added to `sipx-call`.
- **`Call::play` and `Call::record_until_idle`**, which report completion on that stream.
  `PlaybackFinished` carries whether the clip ran to the end or the call cut it off — "the
  announcement finished" and "the caller hung up during it" lead somewhere different, and one flag
  is what keeps them apart. `RecordingFinished` carries how much audio was captured, measured from
  the samples and the negotiated clock rate rather than from how long this side waited: counting
  the idle timeout would describe our own patience rather than the recording.

### Changed

- **`MediaSession::play` returns whether the clip reached the end** instead of `()`. A playback cut
  off by the session stopping was previously indistinguishable from one that finished.
- **`MediaSession::samples_per_packet()` is public.** Callers were passing a literal `160`, which is
  only right for an 8 kHz codec; `Call::play` uses the session's own packet size instead.
- `MediaSession::recv_digit` yields the digit **and how long it was held**, taken from the RFC 4733
  event's own duration field rather than from timing its arrival — the event carries the sender's
  clock, and measuring anything else would make the number depend on jitter rather than on how long
  the key was down. `Call::recv_digit` still yields just the digit.

## [0.5.0] — 2026-07-29

### Added

- **The event notification framework (`S-13`, RFC 6665).** sipx had exactly one subscription: the
  implicit one a REFER creates. Now there is a notifier with a subscription store — establish,
  refresh, unsubscribe, expire, terminate — and packages that register by name.
  - **A terminated subscription stays terminated.** It produces no further notification and cannot
    be refreshed back to life; a subscriber that wants another one starts a new dialog. Terminating
    is not forgetting, either: it stays findable until swept, so a NOTIFY crossing it finds a
    subscription that is *over* rather than one that never existed.
  - The identity is the dialog **and** the package, because §4.4.1 allows several subscriptions in
    one dialog when their `Event` differs — keying on the dialog alone lets the second silently
    replace the first.
  - An unserved package is refused `489 Bad Event` by name, rather than accepted and never
    notified, which a subscriber cannot tell from a slow notifier.
- **`Refer-Sub: false` (RFC 4488)** suppresses the implicit subscription — and needs *both* sides
  to say so. §3 makes it a request and an agreement; a transferor that assumed agreement would stop
  watching for notifications the transferee is still sending.
- **The dialog and registration event packages (`S-17`, RFC 4235 and RFC 3680).** What a busy-lamp
  field on a desk phone actually subscribes to: `dialog-info` documents carrying the five states of
  RFC 4235 §3.7.1, and `reginfo` documents carrying per-contact state with the event that changed
  it.
  - **The first document is `full` and the rest are `partial`.** A watcher that joined mid-call is
    given the whole picture once and told about changes after that; sending only changes from the
    start leaves it inferring a state nobody ever described.
  - **The version counter is scoped per subscription, not per resource.** Two watchers of the same
    dialogs each count from zero — sharing a counter would make one of them see gaps it cannot
    explain. It saturates rather than wraps, because a counter returning to zero looks like a new
    subscription.
  - `expired` and `unregistered` are kept apart, and so are `early` and `confirmed`. Both pairs mean
    roughly one thing to a state machine and two different things to a display: "lost its
    connection" reads differently from "logged out", and a lamp that lights on `early` lights while
    the phone is still ringing.
  - XML metacharacters are escaped. A SIP URI can carry `&` in its parameters, and one unescaped
    makes the whole document unparseable — a watcher then sees nothing at all rather than a
    slightly wrong dialog.
- **Presence, and publishing it (`S-18`, RFC 3856, RFC 3863, RFC 3903).** Nothing in a SIP stack
  knows whether a person is at their desk, so this is the half that lets somebody who does know put
  it in: PUBLISH creates soft state, an entity tag identifies it, and a subscriber to the `presence`
  package is told when it changes. PIDF is a typed document rather than a string template.
  - **A fresh `SIP-ETag` on every acceptance, a refresh included** (RFC 3903 §6 step 6) — which is
    what makes the tag mean anything: a publisher that kept its old one is refused next time.
    Without tags at all, two publishers for one resource overwrite each other and neither can tell.
  - **412 for a tag the compositor does not hold**, including one whose state expired while the
    publisher was not looking. Accepting that refresh as a new publication would resurrect a
    document the server had already forgotten and that nothing has re-sent. Expiry is judged on the
    clock rather than on whether a sweep has run, so the answer is not a race.
  - The three operations are read from what is present (§4.1) rather than dispatched by the caller:
    a tag with no body is a refresh, with a body a modify, with `Expires: 0` a removal.
  - Presence is `open` or `closed` and nothing else (RFC 3863 §4.1.3). The vocabulary people expect
    — busy, away, on the phone — is RFC 4480's, a different document; inventing tokens here would
    put values in a namespace that does not define them.
  - Composition policy is deliberately absent: a second publication for one presentity replaces the
    first. Merging several publishers' documents is a policy question, and a policy belongs to
    whoever has one.

  Both stories stop at the same line, and on purpose: the packages produce documents, and wiring
  them to sipx's *live* dialog store and registration lease is the application's join. A package
  that reached into the call layer would make `sipx-ua` depend on `sipx-call` and reverse the
  dependency direction the workspace is built on.

### Changed

- The implicit REFER subscription reads `Subscription-State` through the event framework instead of
  parsing it a second time. Two parsers for one header eventually disagree about whether a transfer
  has finished.

## [0.4.0] — 2026-07-29

### Added

- **The loopback link the testkit has always promised (`X-14`).** Two full stacks talk in one
  process with no sockets, over a link with seeded loss, duplication, latency and jitter. The same
  seed replays the same trace, so a failure found by varying the loss rate is one that can be
  re-run.
  - **Reordering is not a knob.** Packets overtake because one took longer than another, not
    because a path chose to shuffle them — so jitter produces reordering, and a separate
    probability would model the symptom and permit orderings no real path can produce.
- **The timer queue is generic over its key and no longer reads the clock** — `now` is an argument
  to `set`. It called `Instant::now()` internally, which made it unusable by any driver but the one
  it was written for, and made "when was this scheduled?" a question you could only answer by
  sleeping. Together with the link, a dropped INVITE and the Timer A retransmission that recovers
  from it now cost no wall-clock time at all.

- **A genuine negative DNS answer is cached (`T-17`, RFC 2308 §5).** It was not: an SOA-backed
  NXDOMAIN returned early and was re-queried every time. For a user agent that is one extra lookup
  per call; for a forwarding element resolving for every call it forwards, a domain with no
  `_sips._tcp` record was asked about thousands of times a minute. "Could not ask" is still
  deliberately *not* cached — remembering a network blip as a routing decision keeps a domain
  unreachable long after it has come back.
- `_sip._ws` and `_sips._wss` join the RFC 3263 prefetch, so a WebSocket destination no longer pays
  a serial lookup the other transports avoid.
- `dns::resolve_uri` resolves a URI to a candidate list in one await, for a caller that is not the
  endpoint loop.

### Changed

- A single-flight layer for concurrent identical DNS lookups was written and then **removed**:
  `hickory-resolver` already coalesces them, and the layer was measured to change nothing. The test
  that proves the property stays, so it is a checked fact about the dependency rather than an
  assumption.

- **sipx can issue a digest challenge, not only answer one (`S-16`, RFC 7616 / 8760).**
  `Authenticator` mints a nonce, emits `WWW-Authenticate` or `Proxy-Authenticate`, and verifies the
  credentials that come back. The credential store stays out: `verify` takes the password as an
  argument, so which credential a username maps to is the caller's business.
  - **Nonces are self-describing** — issue time plus an HMAC over it and the realm — so a server
    recognises its own nonce and its expiry without a table of every nonce it ever issued.
  - **A replay and a retransmission are told apart** by the response digest: the same count with
    the same digest is one request seen twice, which is ordinary over UDP and must still
    authenticate; the same count with a different digest is a captured credential.
  - The digest is checked before the clock, so a wrong password on an expired nonce is a rejection
    rather than `stale=true` — which would tell an attacker the only thing wrong with their guess
    was its timing.
  - SHA-256 by default. A server is the only party that can make that choice.

- **The digest primitives can be taken without a runtime (`X-20`).** `sipx-ua` depended on `tokio`
  and `sipx-transport` unconditionally, though only `agent`, `flows` and `error` need either — so
  the caller `S-16` was written for, a proxy or registrar whose decision logic touches no IO, could
  not use the authenticator without linking an async runtime into its core. Its alternative was to
  write digest a second time, and two implementations of one algorithm eventually disagree about
  who is authenticated. A default-on `runtime` feature now carries the two dependencies;
  `default-features = false` leaves `auth`, `challenge`, `outbound` and `registrar` with neither in
  the resolved graph. Nothing changes for anyone who does not ask.
  - The gate asserts on the **resolved dependency graph**, not on whether the build succeeds. A
    runtime-free `sipx-ua` that still pulled `tokio` would compile perfectly and deliver nothing,
    which is precisely the outcome a build check calls success.
  - `outbound::Flow` moved there from `agent`, where it had been sitting for no reason but history:
    it is a pair of the two identifiers `outbound` defines and needs no runtime to be one. `agent`
    re-exports it, so `agent::Flow` still resolves.

- **`Headers` can be edited, not only read (`S-15`).** `remove_first`, `insert` and `retain` — the
  three operations rewriting a message in flight needs. `Via`, `Route`, `Record-Route` and `Path`
  order *is* the routing, so these are exact positions rather than set operations.
  - `insert` past the end appends rather than panicking: this crate parses hostile input, and a
    panic on an index derived from it would be a remote denial of service reachable by arithmetic.
  - The transport's top-`Via` rewrite used to allocate a fresh `Headers` and clone every header to
    change one, on the received-path. It is now two operations that clone nothing.

- **Unmatched responses can be watched (`T-18`).** A response that matches no client transaction was
  logged and dropped — right for a user agent, wrong for anything that forwards: RFC 3261 §16.7
  step 1 requires a stateful proxy that finds no response context to forward the response
  statelessly, which it cannot do if it never sees one. `Handle::watch_unmatched` delivers them.
  - **Opt-in, and that is the design.** Widening `Incoming` into an enum would make every user agent
    handle a case it has no answer for; a second channel out of `bind` would change the signature
    for everyone. An endpoint nobody is watching allocates no channel and behaves exactly as before.

- **Backpressure is visible now (`T-19`).** The endpoint's delivery path ended in
  `let _ = try_send(…)`: a request the application could not take was gone, with nothing logged and
  no counter moved. `Handle::shed()` now reports what was dropped, and both paths log it.
  - **The counter is shared state, not a question asked of the event loop.** The loop is busy in
    exactly the situation this counts, so a metric readable only by asking it would be unavailable
    precisely when it is interesting.
  - **ACKs are counted apart**, because their consequence is different in kind. An ACK cannot be
    refused — SIP has no response to one, and an ACK for a 2xx is a transaction of its own with
    nothing to answer — so nothing retransmits it after Timer H and both ends are left in a dialog
    no timer reaps. A non-zero `ShedCounts::acks` means calls are leaking.

- **DTLS-SRTP (`M-15`, RFC 5764 / 5763 / 8122).** SDES (`M-14`) keys over the signalling path,
  which means every proxy on it has held the key. This keys on the *media* path: the two endpoints
  handshake there, derive SRTP keys from the DTLS master secret, and the SDP carries only a hash of
  the certificate that will appear. It is also the only keying a browser accepts.
  - **The fingerprint check is mandatory and happens where the TLS stack cannot see it.** RFC 8122
    §6.2 requires an endpoint whose peer's certificate does not match to stop; the certificate is
    self-signed, so there is no chain to validate, and what authenticates it arrived in the
    *signalling*. A mismatch yields an error rather than keys, and a peer that sent no fingerprint
    is refused before the handshake runs at all.
  - **Everything the RFC decides is compiled always** — `a=fingerprint`/`a=setup` negotiation,
    §5.1.2's demultiplexing of DTLS from RTP and STUN on one port, §4.2's key derivation. Only the
    handshake sits behind the new **off-by-default `dtls` feature**, which is where OpenSSL lives.
    The default build stays pure Rust.
  - MD5 and MD2 fingerprints are refused at the parser, which is where §5's prohibition on acting
    on them belongs; a digest whose length disagrees with the hash it names is refused too.
  - A session-level `a=fingerprint` is honoured as well as a media-level one — a browser sends only
    the former, and reading just the media level declines a perfectly good offer.

### Fixed

- Three specs linked to `designs/host.md`, which is named `app-host.md`, and the board's epic blurb
  carried a link relative to `docs/designs/` into `docs/stories/`. Both broke the docs build.

### Changed

- **The application host is a workspace crate — `crates/sipx-app` — not a separate product.**
  Reverses the placement 0.3.0 recorded: the contract, its interpreter and its host iterate
  together in one repository with one gate, and the separation's benefits are kept as ground
  rules instead (the host is a leaf no kernel crate depends on; its HTTP stack, serialization
  and future engine stop at its own `Cargo.toml`). The host's planning — designs
  (`app-host`, `embedded-runtime`, `ts-sdk`), four binding specs, and stories `A-1` … `A-7`
  under the new `app-host` epic — moves into `docs/`, and the crate exists as a documented
  stub so the name has its home from day one.

## [0.3.0] — 2026-07-28

### Added

- **The application contract, specified — `sipx.app.v1`** (`docs/specs/app-contract.md`), the
  epic behind it (`app-sdk`), and the six kernel stories it pulls: a call-level event stream
  (`C-3`), multi-call dispatch (`C-4`), the contract crate with its sans-IO interpreter (`C-5`),
  the bridge reachable from a `Call` (`C-6`), playback control (`M-17`) and mute (`M-18`).
  Events carry full call snapshots; instructions are ordered programs with correlated
  completion ids; a response replaces the pending program, which is what makes barge-in
  compose. Experimental until an inbound IVR and an outbound notifier both run against it.
- **Migration guides** — from Kamailio and from Asterisk — written as honest concept maps,
  each opening with a maps-today/not-yet table.
- **Outbound, the client half (`T-15`, RFC 5626).** A `Contact` naming an address behind a NAT is
  unroutable the moment the mapping lapses. Outbound routes down a *flow* the client opened
  instead: `+sip.instance` and `reg-id` on every REGISTER, `outbound` offered, `ob` on a
  dialog-forming `Contact`, and one registration per outbound proxy so that a proxy going away is
  survivable.
  - **`Flows::register` and `Flows::keepalive` return one outcome per flow and no aggregate
    `Result`.** Registering to several proxies exists so one failing is survivable, and a function
    returning a single `Result` cannot help but let one failure stand for all of them. The type is
    the guarantee.
  - **Keep-alives, both techniques.** CRLFCRLF/CRLF for connection-oriented flows (§4.4.1) and STUN
    Binding for UDP (§4.4.2), each over the flow it is testing — a ping on a second connection
    proves a flow nobody is using.
  - **A changed reflexive address is a failed flow**, even when every ping is answered (§4.4.2).
    That is the reason STUN is the UDP technique rather than an `OPTIONS`: the socket still works,
    but the mapping the registrar holds no longer reaches the UA, so a call routed down the flow
    would silently never arrive.
  - §4.5's backoff, with its asymmetric base — 30 seconds when every flow is down, 90 when one is
    still up. A UA that is reachable already has nothing to gain by hurrying.
- **A STUN Binding client (RFC 5389)**, scoped to what the keep-alive needs and no further.
  Decoding is checked against the vectors RFC 5769 publishes — including the 11-byte attribute whose
  padding a decoder must skip to find `XOR-MAPPED-ADDRESS` at all.
- `StreamParser::take_keepalives` counts the CRLFs RFC 3261 §7.5 tells a parser to ignore. It still
  ignores them; RFC 5626 §4.4.1 gives them a meaning, and a transport waiting for a pong has to be
  able to tell one arrived.

- **The registrar's outbound route set, obeyed — `Service-Route` (`T-16`, RFC 3608).** `Path`
  (`T-14`) fixed routing *toward* a UA. This is the other direction: a registration can dictate
  which proxies the UA's own requests must traverse, and a UA that ignores it sends every call
  straight at the destination — arriving at a proxy holding no state for the registration the call
  belongs to.
  - **An absent `Service-Route` clears the stored one.** §6.1's two sentences are one rule, and
    "nothing to say, keep what you had" is the natural mis-implementation: it leaves a UA routing
    through a proxy the registrar has stopped naming.
  - It is **not** attached behind the caller's back. `UserAgent::service_route()` hands it over and
    `DialOptions::with_service_route` takes it, because a `Route` header silently added to every
    request is close to undebuggable from outside.
  - A hop missing the `;lr` that §5 requires is *reported*, not dropped — the registrar is the
    offending party, and a UA that discarded a route set over a missing parameter would be
    unroutable for an invisible reason.

### Changed

- **The documentation site is customer-facing now** (`website/`, Docusaurus), and the internal
  tree under `docs/` is no longer published at all. The book's guarantee survives the move:
  every code sample is a compiled example file, inlined as a generated region the gate refuses
  to let drift (`scripts/sync-website.py --check`). The API reference stays at `/api`.
- `Outcome::Registered` carries a `Registered` struct rather than positional fields. `PathSet` and
  `ServiceRoute` are the same shape and opposite directions, and two interchangeable positions of
  identical type is how they would eventually get swapped.

- **Reliable provisional responses — 100rel and PRACK (`S-12`, RFC 3262).** A `180 Ringing` is
  fire-and-forget over UDP, and some carriers will not accept a call without the option tag at
  all. `100rel` is offered on every INVITE, honoured when a peer requires it, and refused with
  `420 Bad Extension` + `Unsupported: 100rel` when it is switched off locally — refusing plainly,
  because a caller waiting for an `RSeq` that never comes cannot tell that from a dead network.
  - The retransmission schedule doubles from T1 and **deliberately does not cap at T2**, which
    every other retransmission in SIP does. §3 gives the reason: an ACK is resent because a 2xx
    arrived again, but a PRACK is sent once and is not re-triggered by a further 1xx.
  - The `To` tag is chosen when the provisional is sent and reused by the answer. A reliable
    provisional establishes a dialog, so a fresh tag on the 200 would create a *second* one — the
    caller ACKs the dialog it knows while this side retransmits the 200 into a working call.
  - `RSeq` is chosen uniformly in `1..2^31-1` rather than sequentially: it is the only thing an
    off-path attacker would need to forge a PRACK and silence the retransmissions.

## [0.2.1] — 2026-07-28

Documentation and tooling only. **No crate changed**, so the libraries are byte-identical to
0.2.0; this release exists to mark the point where what sipx supports became something you can
check rather than something you have to take on trust.

### Added

- **[A documentation site](https://codewandler.github.io/sipx/)**, built from `docs/` rather
  than from a copy of it — a site with its own content tree is a second copy of the truth, and
  the second copy is the one that rots. `./scripts/build-docs.sh` builds it locally and fails if
  a published page links to something the site does not publish; it found eight such links on
  its first run.
- **An RFC compliance table**, generated from `docs/rfc/registry.toml` and verified in CI: a
  header an entry names must be known to the parser, a cited file must exist, and an entry
  claiming implementation must cite something. 61 RFCs — 22 implemented, 7 partial, 10
  parse-only, 21 not started, 1 superseded.
  *Parse-only is its own status.* sipx parses `RAck` and `RSeq` and does nothing with them, so
  "supports RFC 3262" and "rejects it" are both false, and a three-state table could not say so.
- **An [RFC roadmap](https://codewandler.github.io/sipx/rfc-roadmap.html)** ordering the
  remaining gaps by dependency and by what each changes about where sipx can be deployed.
- A logo: a crab holding a telephone handset.

### Changed

- **The README is for people deciding whether sipx fits**, not for contributors. It had been
  claiming the workspace was still being scaffolded. It now leads with what sipx can and cannot
  do — media is not encrypted, stated in the first table rather than buried — and `AGENTS.md`
  stays the file for contributors and agents.

## [0.2.0] — 2026-07-28

### Added

**Closing the gaps M3 left**

- RFC 4733 DTMF (`M-7`). sipx had been advertising `telephone-event` in every offer since M3
  with nothing able to encode or decode one. The payload type is read from the negotiation
  rather than assumed, since it is dynamic.
- re-INVITE (`M-8`). A call can be renegotiated from either side, including hold and resume. A
  renegotiation that fails is refused with 488 and **leaves the call running**.
- A real DNS client behind the RFC 3263 resolver trait (`T-5`), with a TTL-respecting cache
  that tells "no such record" from "could not ask".
- RTCP sender and receiver reports (`M-6`), with interarrival jitter computed by RFC 3550's
  own recurrence rather than as a variance.

**Milestone M4 — the phone**

- `sipx dial`, `sipx answer` and `sipx register`, with WAV playback and recording, DTMF, a
  `--json` output mode and a distinct exit code per outcome.
- `sipx dial --timeout` bounds the attempt, and **CANCEL** (RFC 3261 §9), which the stack had
  never implemented. Giving up is not just ceasing to wait: without a CANCEL the callee goes
  on ringing, and someone answering afterwards ends up in a call with a party that has left.
  The bound lives in `sipx-call` rather than around it — dropping the call future would
  abandon the exchange after a 200 OK but before the ACK.

**Milestone M5 — depth** (in progress)

- `docs/specs/sip-tls.md` and **SIP over TLS** (`T-6`, `T-7`), with certificate verification
  that cannot be turned off. Trusting a private CA is an addition to the anchor set, not a
  bypass — there is no `insecure` flag, because every stack that ships one eventually finds it
  in production.
- **SIP over WebSocket** (`T-8`, RFC 7118), which is how a browser reaches a SIP network at
  all. The handshake negotiates the `sip` subprotocol and refuses a peer that does not offer
  it; one SIP message per WebSocket message, with anything else closing the connection rather
  than being patched up; and Ping keeps the path open through intermediaries that would
  otherwise close a registered client's socket.
- **Secure WebSocket** (`T-9`), which is the TLS above with the framing above on top — the same
  acceptor, the same connector, the same policy. Not a third set of security rules.
- **Interop for both against Kamailio** (`T-10`). The harness now issues its own certificate
  and asserts three things a fixture test cannot: that a registration over TLS is accepted by an
  implementation that did not learn TLS from sipx, that a certificate for another name is
  refused, and that an unknown issuer is refused. Both refusals must be *immediate* — a test
  that accepted a timeout would also pass against a stack that had simply hung. WebSocket
  registration is proved the same way, against Kamailio's own WebSocket module.

**Load and stability**

- **A load harness** (`X-4`) in `sipx-testkit`, generic over what a call is so it can be pointed
  at sipx or at somebody else's server — a limit found with sipx on both ends cannot be
  attributed to either half. Failures are reported **by cause**, never aggregated, and latency
  as **percentiles**, never a mean: setup latency is a tight cluster with a tail of
  retransmission timeouts, and a mean sits in the empty space between them.
- **A soak assertion** (`X-5`) that tasks, file descriptors and the transaction store are *flat*
  rather than merely bounded, run nightly in CI rather than on every push.

**Media**

- **Opus** (`M-13`, RFC 6716), behind the `opus` feature. Note the one exception in
  `deny.toml`: the FFI shim under it is unmaintained, there is no maintained alternative that
  encodes, and the advisory is excepted with its reasoning and its exit condition written down.
  — off by default, so the stack stays
  pure Rust unless the codec is asked for. Negotiated as a dynamic payload type matched by
  *encoding name*, so an endpoint that numbers Opus differently is still understood, and G.711
  remains the fallback. Being stateful, unlike G.711, it moved the codec into the send and
  receive loops, one each: a stateful codec that cost a lock in the packet path would not be
  worth having.
- **Conferencing** (`M-12`): every party hears every other party and never themselves. The
  mixer saturates rather than wrapping, because wrapping turns the loudest instant of a call —
  the moment two people talk over each other — into a full-scale discontinuity heard as a bang.
  Participants join and leave without interrupting the others.
- **Bridging two calls** (`M-11`). Audio is passed through without decoding when both legs
  agreed on the same codec, and transcoded — visibly, via `Bridge::is_transcoding` — when they
  did not. Dropping a bridge stops it, rather than leaving two tasks forwarding audio between
  calls nobody holds a handle to.
- **Call quality, readable while the call is running** (`M-10`): loss, jitter, round-trip time
  and an estimated MOS, from `MediaSession::quality()` and from `sipx dial --stats`. The round
  trip is computed from the RTCP exchange (RFC 3550 §6.4.1) and is *absent* rather than zero
  when the far end does not speak RTCP — zero would read as "instantaneous", and a script would
  believe it.
- **The media session now binds a control port** and receives RTCP, where before it could only
  send. It also sends **sender** reports once it has sent anything, rather than only receiver
  reports. Without both, no peer could ever have told sipx its round-trip time.
- **An adaptive jitter buffer** (`M-9`). Depth follows observed jitter between a floor and a
  ceiling, growing at the first packet that arrives too late and shrinking only after five
  seconds of clean network — because being too shallow is audible and being too deep is not.
  The fixed buffer remains, as the control: on a trace with recurring 95 ms spikes the constant
  loses 86 packets to lateness and the adaptive one loses 3, and on a clean trace the two behave
  identically. Used by default in `sipx-media`, bounded at 12 packets.

**Transfer**

- **Blind transfer** (`S-9`, RFC 3515). REFER is sent and received in-dialog, the transferee
  places the call, and the outcome comes back as NOTIFY — because a `202 Accepted` means "I will
  try" and nothing more. A transferor that read it as success would report a completed transfer
  to a user whose call had been refused. The implicit subscription is terminated when the
  transfer finishes, either way, rather than left open on both sides.
- **Attended transfer** (`S-10`, RFC 3891). `Replaces` is matched on `Call-ID` *and both tags*,
  and the check is inside `answer_replacing` rather than left to the caller. A `Call-ID` travels
  in every message of a dialog and is visible to every element on the path; the tags are random
  and known only to the two parties. Matching on the `Call-ID` alone would let anyone who had
  seen one message of a call ask to be put in the middle of it — so every mismatch is refused
  with the same 481, which also tells a guesser nothing about how close they got.
- `Call::handle` does not answer a REFER. Whether to place a call on another party's say-so is
  the application's decision, and `accept_referral`/`refuse_referral` are the two answers. A
  `Refer-To` naming nothing usable is the exception: 400, without asking.

### Changed

- **The connection pool keys connections by `(address, transport, verified identity)`**, where
  it used to key by address alone. Two names that resolve to one address are two connections:
  reusing one for the other would send traffic for `a.example.com` over a connection
  authenticated as `b.example.com`, discarding the check that had just been performed. The
  transport is in the key for a related reason — WebSocket and TCP can share a port, and a
  `sips:` request riding a cleartext socket has silently become what it asked not to be.
- `call::contact_for` takes the transport. Over a WebSocket there is no address to advertise,
  and in-dialog requests ignore `Contact` entirely — see the fix below.
- `TrustAnchors::system()` uses the **platform's** trust store rather than a copy of one
  vendor's root list compiled in — so an operator's corporate CA is honoured, and a root
  distrusted after a compromise stops being trusted when the OS says so.
- **The minimum supported Rust version is now 1.88**, raised from 1.85. The DNS client needed
  to clear RUSTSEC-2026-0119 requires it, and the alternative was shipping a known denial of
  service in a parser that reads untrusted network data.

### Security

- Upgraded the DNS client past **RUSTSEC-2026-0119**, a CPU-exhaustion denial of service in
  `hickory-proto`'s name compression. sipx feeds that parser untrusted network data, so this is
  on the path that matters. Caught by `cargo-deny` in CI on the first push after the dependency
  was added — which is the whole reason the gate exists.

### Fixed

**Conformance defects found by reviewing implemented behaviour against the RFCs** (`X-6`).
Deliberately not a gap analysis — a missing feature is visible, a subtly wrong one is not. Every
fix landed with a failing-first test, and the tests that asserted the old behaviour were
rewritten rather than deleted.

- **Timer B fired from `Proceeding`**, so a callee who took longer than 64·T1 to answer was hung
  up on, and `send_to_uri` then dialled the next RFC 3263 candidate while the first phone was
  still ringing. RFC 3261 §17.1.1.2 fires it from `Calling` only; §16.6 item 11 is explicit that
  the INVITE client transaction no longer times out once a provisional has arrived, which is
  precisely why proxies need Timer C.
- **A `sips:` URI with a `transport` parameter resolved to cleartext.** Table 1 and §26.2.2: in a
  SIPS URI the parameter names the transport carried *under* TLS, so `transport=tcp` asks for TLS
  over TCP. The scheme filter lived in the SRV stage, which an IP literal, an explicit port and
  the bare A-record fallback all skip. `sips` over UDP now yields no candidate rather than a
  downgrade, there being no TLS over UDP to offer.
- **RFC 3581 was broken in both halves.** `received` was omitted when the sent-by matched the
  source, though §4 requires it "even if it is identical to the value of the `sent-by`
  component"; and `rport` was consulted only alongside `received`, so a response went to the
  sent-by port a NAT had rewritten. A client on an ephemeral port never got its answers.
- **In-dialog requests carried the route set but were addressed to the remote target**, bypassing
  the record-routing proxy that inserted itself in the dialog in order to be traversed. Where
  that proxy is the only element that can reach the far end, this is the BYE that never arrives —
  with the media still running. §12.2.1.1, now including strict routing and the parameters
  §19.1.1 bars from a Request-URI.
- **The ACK to a 2xx ran inside a transaction**, earning it the retransmission timers of a
  non-INVITE request aimed at a response that never comes; and a *retransmitted* 2xx was never
  acknowledged again, though §13.2.2.4 requires an ACK for each one received.
- **The 200 to a re-INVITE was sent once.** §13.3.1.4 governs the 2xx to any INVITE, and RFC 6026
  has the server transaction absorb the retransmitted requests without answering them, so a
  single lost packet deadlocked hold and resume until the peer's Timer B.
- **§18.1.1's size limit was applied to responses**, which §18.2.2 gives a UAS no transport to
  escape to. A 200 carrying a full SDP answer was refused outright: the caller timed out while
  the callee believed it had answered.
- **A CRLF before a start-line was a fatal framing error**, so the RFC 5626 keepalives that
  mainstream stacks send routinely closed the connection and every dialog riding it. §7.5 makes
  ignoring them a MUST, and only for stream transports.
- **RTCP named both parties SSRC 0** — the report block never learned the peer's synchronisation
  source and the sender field carried the reportee's — so a conforming peer found no block
  matching itself and discarded every loss and jitter figure. Interarrival jitter also used
  non-modular arithmetic, so a 32-bit timestamp wrap (normal, since §5.1 randomises the starting
  timestamp) injected 2³²/16 into the estimate and poisoned it for hundreds of packets.
- **FQDNs in SDP `o=` and `c=` lines were rejected**, failing the whole description — including
  RFC 3264 §10.1's own example offer, which could therefore never be answered.
- **The digest nonce count was global rather than per-nonce**, so a registrar enforcing the replay
  protection that `nc` exists for rejected every fresh nonce answered with a count above one.
  RFC 7616 §3.4.3 counts requests sent *with the nonce in this request*.
- **`sipx register` advertised the registrar's address in its own `Via`** and, on the default
  `--local`, registered a `sip:user@0.0.0.0` binding — so every inbound call to the
  address-of-record was routed nowhere.
- Smaller ones, each with its citation in the story: comma-separated `Contact`/`Route` rows
  rejected (§7.3), case-sensitive SIP-Version (§7.1), `tel:` URIs compared as opaque bytes,
  escaped parameter names not folded (§19.1.4), no target refresh on a re-INVITE (§12.2.2), the
  ordering check applied only to re-INVITEs so a stale BYE ended a live call, `answer()`
  committing a 2xx before it knew a dialog could be formed, weight-0 SRV records unreachable
  (RFC 2782), TLS advertising the cleartext port in `Via`, session-level SDP direction ignored
  and rtpmap matched without its clock rate (RFC 3264 §6.1), DTMF fed to the jitter estimator and
  saturating instead of segmenting (RFC 4733 §2.5.1.3), and UAS final responses without a To tag
  (§8.2.6.2).

- A call hung up while packets were still in the paced send queue, so every call lost its last
  word — or, for DTMF, its last digit. `MediaSession::flush` now drains the queue first.
- The RTCP report block decoder read cumulative loss from byte 4 instead of byte 5, folding the
  loss fraction into the high byte of the count.
- **A `sips:` URI resolved through DNS had its certificate checked against the resolved
  address.** RFC 3263 turns one name into a list of addresses by way of NAPTR and SRV records
  that may name something else entirely, and resolution never attached the name from the URI to
  what it produced. The handshake still succeeded and the check still appeared to run, which is
  the whole failure mode `docs/specs/sip-tls.md` §3.3 exists to prevent: whoever can influence
  DNS chooses which certificate is acceptable. Found while building WSS on top of it.
- **An in-dialog request over a WebSocket was sent to the peer's `Contact`.** A WebSocket client
  has no listening port, so its `Contact` names something that will never resolve (RFC 7118
  §5.2) — every ACK and BYE went nowhere. In-dialog requests now go over the connection the
  dialog was established on, unconditionally, and sipx writes an unresolvable `Contact` of its
  own when it is the WebSocket client.
- **The crate did not compile with the `tls` feature disabled.** `tokio::select!` cannot compile
  a branch out behind a `#[cfg]`, so each optional listener's branch referred to a field that
  was not there. CI only ever built `--all-features`, so nothing noticed. Every optional
  listener now shares one channel and one branch, and each feature combination is checked.
- **A server transaction the application never answered was held for the life of the process.**
  RFC 3261 §17.2 gives one in `Trying` no timer, because its model is that the transaction user
  always responds; an application that ignores a method it does not implement, or that panics in
  a handler, leaves it there and nothing collects it. Found by the new soak run — 300 of them
  for 300 calls, still present two minutes later. The endpoint now abandons one unanswered after
  three minutes and logs it as the application bug it is.
- **A URI carrying the same header name twice was not equivalent to itself.** Each occurrence
  was compared against the *first* header of that name rather than its counterpart, so
  `sip:a?f=a&f=b` failed reflexivity. Headers are now compared as multisets. Found by a
  property test, which is exactly the kind of bug no example test would have reached.
- **`Handle::respond` returned before the response was sent.** It queued a command for the
  endpoint loop and returned, so a process that answered a call and exited could lose the
  response to its own exit — the caller then saw a timeout for a call that had in fact been
  answered or refused. It now returns once the response is on the wire, which is what every
  caller already assumed. Found by a CI-only failure of the `--busy` test.
- **A received CANCEL was absorbed as an INVITE retransmission.** The transaction key folded
  CANCEL to INVITE, but RFC 3261 §17.2.3 folds the method only for ACK — so a CANCEL matched
  the INVITE's own transaction, was swallowed as a duplicate, and nobody was told. Nothing
  could have stopped a ringing phone.
- The DNS client's own response cache is now disabled. Two caches with different TTL policies
  is a source of confusion rather than speed: sipx's exists to cap TTLs and to distinguish
  "no such record" from "could not ask", and neither survives a second layer underneath doing
  its own thing.

## [0.1.0] — 2026-07-28

The first cut. Not published anywhere: no crate is on crates.io and no tag has been pushed.
What this marks is the point at which the bottom four layers of the stack are complete and
verified — a SIP core, transports, a user agent and calls that carry audio.

sipx registers against a real Kamailio over UDP and TCP, answers `OPTIONS`, and places a call
between two of its own endpoints that carries G.711 in both directions. 349 tests, clippy clean
at `-D warnings`, and the whole RFC 4475 torture corpus green.

### Added

**Milestone M0 — workspace**

- Cargo workspace with the ten `sipx-*` crates, shared lints (`unsafe_code = "forbid"`)
  and `MIT OR Apache-2.0` licensing.
- CI: rustfmt, clippy (`-D warnings`), tests, MSRV check, `cargo-deny`, a fuzz smoke run, and
  a provenance gate that fails rather than passing when unconfigured.

**Milestone M1 — the sans-IO SIP core (`sipx-sip`)**

- Specs first: `docs/specs/sip-message.md`, `sip-parser.md` and `sip-transaction.md`, with
  every normative statement either citing an RFC section or marked as a project decision with
  its rationale.
- The RFC 4475 torture corpus, recovered bit-exactly from that RFC's Appendix A archive by
  `scripts/import-rfc4475-corpus.sh` and classified by which layer must object to each
  message. Green across all four layers.
- `Uri`, `Host`/`HostName`, `HeaderName` and parameter lists, with RFC 3261 §19.1.4
  equivalence — deliberately *not* `PartialEq`, since that relation is not transitive.
- A zero-copy message model: parsed messages borrow their bytes and re-serialize byte for
  byte, including original spelling, compact forms, whitespace and line folding.
- One parser for datagram and stream framing, verified identical by splitting every corpus
  message at every byte offset. Fuzz targets for both, seeded from the corpus.
- Typed headers parsed on demand, distinguishing absent from present-and-malformed.
- Message validation returning a list of findings, with `Max-Forwards` marked repairable.
- Builders in which header injection is unrepresentable rather than validated against.
- All four transaction state machines (RFC 3261 §17, amended by RFC 6026), matching with the
  RFC 2543 fallback, and transaction stores with a leak test.

**Milestone M2 — transports and the user agent**

- `docs/specs/sip-transport.md`, settling the connection-reuse and backpressure decisions.
- One event loop per endpoint owning the transaction layer, the timer queue and the sockets;
  no locks in the signalling path.
- UDP with `received`/`rport` (RFC 3581), and the RFC 3261 §18.1.1 datagram size guard.
- TCP with per-connection stream framing and a pool that distinguishes inbound from outbound
  connections, so a response returns the way it came without an inbound connection becoming a
  route for unrelated outbound requests.
- RFC 3263 resolution — NAPTR, SRV with RFC 2782 weighting, A/AAAA — behind a trait, with a
  seeded RNG so the weighted distribution is asserted rather than assumed. No DNS client is
  wired in yet; see `T-5`.
- Digest authentication (RFC 7616): MD5, MD5-sess, SHA-256, SHA-256-sess, verified against
  the digest RFC 2617 publishes for its own worked example.
- Registration as a lease: the registrar's granted expiry wins, refreshes reuse the `Call-ID`
  and advance the `CSeq`, and a rejected password fails once instead of looping.
- `OPTIONS` answered with a real capability list.
- Verified against a real Kamailio, not only against sipx: `./tests/interop/run.sh`.

**Milestone M3 — media and calls**

- `sipx-sdp`: RFC 8866 parsing that keeps unknown lines, and RFC 3264 offer/answer as a pure
  function. Rejected streams keep their place with port 0, codec order is the offerer's, and
  dynamic payload types are matched by encoding name rather than number.
- `sipx-audio`: G.711 µ-law and A-law checked against the ITU algorithm rather than by round
  trip, and WAV for 8 kHz 16-bit mono.
- `sipx-rtp`: packet encode/decode that rejects rather than guesses, and a jitter buffer that
  extends sequence numbers to 64 bits so the 16-bit wrap is ordinary rather than a cliff.
- `sipx-media`: RTP sessions with symmetric RTP, paced by a single clock.
- `sipx-call`: dialogs, `dial`, `answer` and `hang_up`. Two sipx endpoints establish a call,
  play a WAV and record it bit-exact after G.711.

### Fixed

Defects found and fixed before this release — nothing here ever reached a user. They are
recorded because each one is a mistake worth not repeating, and most of them sat directly
beneath a comment asserting the opposite.

- **A 2xx was not retransmitted until acknowledged.** The transaction layer absorbs
  retransmitted requests but does not resend the response; over UDP one lost 200 OK left the
  caller giving up while the answering side held an established call.
- **A 2xx the caller could not use was never acknowledged.** A 200 OK carrying an unusable SDP
  answer made `dial` return an error without an ACK, leaving the far end retransmitting for 32
  seconds and then streaming media at a closed port. It now ACKs and BYEs, per RFC 3261 §15.
- **ACK and BYE went to the address the INVITE was sent to** rather than to the peer's
  `Contact`, so with a redirect or a B2BUA in the path they reached the wrong element.
- **The route set was computed and never sent.** No `Route` header was added to in-dialog
  requests, so a call through a Record-Routing proxy could not be ended.
- **`Record-Route` was read one line at a time**, though it is a comma-separated list header —
  so a UAC's reversal reversed lines rather than routes. A malformed first route also silently
  discarded every later one.
- **An inbound BYE reached nothing**, so the far end hanging up did not stop the local media.
- **A URI with its own parameters was truncated** when its header tag was stripped, producing
  an unterminated angle bracket the far end answers with 400.
- **The RTP timestamp advanced by the configured packet size** rather than the samples actually
  sent, so any other frame size built a timeline at the wrong rate.
- **Unknown RTP payload types were decoded as the negotiated codec.** sipx advertises
  `telephone-event` on 101, so a peer's DTMF was decoded as µ-law and heard as a click.
- **A media session could not be stopped while its consumer was not reading**, leaking the task
  and its socket for the life of the process.
- **A forged RTP packet could silence a call.** Any later packet with a different SSRC was
  admitted to the jitter buffer, where a high sequence number made every genuine packet late.
- **`Contact` carried the socket's local address** rather than the endpoint's advertised one,
  so an endpoint bound to `0.0.0.0` published an unroutable contact.
- An endpoint binding to port 0 could fail with `AddrInUse`: UDP and TCP have independent port
  spaces, so a port the OS handed out for UDP could already be held for TCP. Binding now
  retries for a port free on both, while a *named* port that is taken still fails honestly.

### Not in this release

Stated so nobody has to discover it from a stack trace:

- **No TLS, WebSocket or WSS.** The transport enum names them; only UDP and TCP are
  implemented, and a `sips:` URI resolves to no candidate rather than downgrading.
- **No DNS client.** Every RFC 3263 selection rule is implemented and tested, but the only
  `Resolver` implementations are test fixtures, so a URI naming a domain resolves to nothing at
  runtime. IP literals and explicit `host:port` work today (`T-5`).
- **No re-INVITE.** A call can be established and ended, not modified (`M-8`).
- **No RTCP** (`M-6`) and **no RFC 4733 DTMF** (`M-7`) — the latter matters because the SDP
  already advertises `telephone-event`, so that advertisement is currently a promise sipx does
  not keep.
- **No command-line tool.** `sipx-cli` is a scaffold; `dial`, `answer` and `register` are
  library calls only (milestone M4).
- **Interop is verified against Kamailio only.** A second implementation with different
  opinions — Asterisk, as a B2BUA rather than a proxy — has not been tried.

[Unreleased]: https://github.com/codewandler/sipx/compare/v1.0.0-alpha.4...HEAD
[1.0.0-alpha.4]: https://github.com/codewandler/sipx/compare/v1.0.0-alpha.3...v1.0.0-alpha.4
[1.0.0-alpha.3]: https://github.com/codewandler/sipx/compare/v1.0.0-alpha.2...v1.0.0-alpha.3
[1.0.0-alpha.2]: https://github.com/codewandler/sipx/compare/v1.0.0-alpha.1...v1.0.0-alpha.2
[1.0.0-alpha.1]: https://github.com/codewandler/sipx/compare/v1.0.0-alpha...v1.0.0-alpha.1
[1.0.0-alpha]: https://github.com/codewandler/sipx/compare/v0.12.0...v1.0.0-alpha
[0.12.0]: https://github.com/codewandler/sipx/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/codewandler/sipx/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/codewandler/sipx/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/codewandler/sipx/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/codewandler/sipx/compare/v0.7.0...v0.8.0
[0.1.0]: https://github.com/codewandler/sipx/releases/tag/v0.1.0
