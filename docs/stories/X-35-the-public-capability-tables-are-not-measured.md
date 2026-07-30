---
id: X-35
title: Make the public capability tables measured, not hand-maintained
pillar: Build
status: done
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs, tests]
predicate: 1
note: two independent read-only sweeps found the front page advertising Opus, bridging and a DTLS-SRTP workaround that no call can reach — the same shape X-30 removed from the registry, in the four hand-maintained tables no script reads
---

# Make the public capability tables measured, not hand-maintained

## Goal
Stop `README.md` and `website/` selling capabilities that exist in a crate and are reachable from
nothing. `README.md:38` calls the compliance table "a measurement rather than a claim"; the
capability tables above it are the opposite, and three of them are wrong today.

## Acceptance

### The over-claims — each is a capability real in a crate, unreachable from `sipx-call` or the CLI
- [x] **Opus is advertised as a stack capability and no call can select it.** `README.md:26`,
      `website/docs/intro.md:21`, `website/docs/guides/does-this-fit.md:17` (under *"It fits if you
      want to"*), `website/src/pages/index.js:14`. Two independent locks: `sipx-call` hardcodes
      `Capabilities::g711` at `call.rs:606,752,955,1728,2860,3161` so payload type 111 is never
      offered, and `Codec::from_payload_type` (`sipx-media/src/session.rs:115-124`) *deliberately*
      never returns Opus — so even a hand-written peer offer cannot arrive at it. `with_opus`
      (`sipx-sdp/src/answer.rs:85`) has no caller outside `sipx-sdp`'s own tests, and no `sipx-call`
      entry point accepts caller-supplied `Capabilities` (`dial`, `dial_early`, `answer`,
      `answer_ringing`, `answer_early`, and `DialOptions`' four builders). `crates/sipx-call/Cargo.toml`
      has no `[features]` block at all, so the `opus` feature cannot be reached through it.
      **`website/docs/guides/as-a-library.md:104` already has the honest wording — copy it.**
- [x] **"Bridges and conferences" is sold as a phone capability in five places; two `Call`s cannot be
      bridged.** `README.md:19`, `README.md:31`, `website/docs/intro.md:14-15` and `:26`,
      `does-this-fit.md:8-9`, `website/src/pages/index.js:9`, plus
      `crates/sipx-call/Cargo.toml:3`'s description. `Bridge::connect` needs
      `Arc<MediaSession>` (`bridge.rs:39`) and `Conference::join` the same (`conference.rs:93`);
      `Call` holds `media: MediaSession` by value (`call.rs:62`) and lends only `&MediaSession`
      (`call.rs:127-129`) — no `into_media`, no `Arc`, no `Clone`. `grep -rn "Bridge" crates/sipx-call/src`
      → zero hits. **The doc set already knows**: `website/docs/migrate/from-asterisk.md:25` says
      "bridge from the public API is being finished" and `website/docs/sdk/overview.md:43-44` lists it
      as designed-and-tracked. Adopt that wording; the gap is `C-6`.
- [x] **DTLS-SRTP is described as reachable "by building your own capabilities", and the workaround
      does not exist.** `website/docs/intro.md:43-45`, `website/docs/whats-new.md:36-38`. The gap is
      **wider than `docs/compliance.md:107-109` states** — that row says no role is reachable from
      `sipx-call`; in fact no `MediaSession` can be keyed by DTLS at all. `dtls::Keys` carries
      pre-built `srtp::Context`s (`dtls/mod.rs:116-121`) while `Config.srtp` takes `SrtpKeys` — master
      key and salt per direction (`session.rs:264,375-381`) — and the two types never meet. The
      handshake also cannot run on the media port RFC 5764 §5.1.2 requires it to share:
      `MediaPort.socket` is private with no accessor (`session.rs:801-810`) while
      `dtls/openssl.rs:165-169` needs an owned `std::net::UdpSocket` it connects itself.
      Also fix the two "both browser pieces are in place" claims — `does-this-fit.md:37-38`,
      `whats-new.md:47-48` — to one, and `does-this-fit.md:56-57` to "keyed by SDES", full stop.
      Feed the type-boundary finding to `M-28`.
- [x] **`X-26`'s removed untruth survives in the fourth front door its guard does not read.**
      `README.md:114` claims `sipx-audio` ships "RFC 4733 DTMF"; `sipx-audio/src/lib.rs:17-18` says it
      "is not here either, and never was". `check-audio-claims.py --check` exits 0 because
      `front_doors()` (`:191-218`) reads exactly three strings and the README's crate table is not one
      of them. Same row also says bare "Opus" where the script's own `names_the_feature` rule
      (`:251-258`) would demand "behind the `opus` feature".

### The stale denials — capabilities that exist and are denied
- [x] `README.md:30` — "No Outbound, Path, GRUU or push yet". All four exist with public entry points
      and integration tests: `sipx_ua::Config::with_outbound` (`agent.rs:109`), `with_gruu` (:130),
      `with_push` (:156), `UserAgent::gruus` (:340), `UserAgent::woken` (:421); Path is ✅ at
      `compliance.md:51`. The README contradicts its own site (`intro.md:25`).
- [x] `website/docs/intro.md:25` ("No GRUU, push or ICE yet" — ICE is right, the other two are not)
      and `does-this-fit.md:36` ("a sleeping client cannot be woken", against `UserAgent::woken`,
      exercised at `sipx-ua/tests/push.rs:216`).
- [x] `website/docs/reference/compliance.md:8` says "69 RFCs"; the registry has **70**
      (`docs/compliance.md:15`, `README.md:9`, `grep -c '^\[\[rfc\]\]'`). `rfc-report.py --check`
      passes because it does not read the website page.
- [x] The interop claim names one peer and there are two — `README.md:35`,
      `website/docs/intro.md:29`, `does-this-fit.md:66-68` all say "Against Kamailio", but
      `tests/interop/asterisk/profile.sh:14` declares `PEER_ROLES="server user-agent media-security"`
      and `tests/interop/README.md:7-8` says "Plural, since `X-17`". An **under**-claim, and the
      strongest evidence the project has.

### The missing warning
- [x] **The CLI cannot reach a secure transport and nothing says so.** `sipx-cli/src/dial.rs:32` and
      `register.rs:26` parse only `--tcp`, and `reference/cli.md:26,62` document only `--tcp` — so
      `sipx dial` can never produce an encrypted call, while `README.md:28`, `README.md:29` and
      `intro.md:40-41` promise encrypted media beside a CLI section showing `sip:` URIs. One sentence
      in `reference/cli.md` closes it. (Encrypted media on a WSS call is real and tested —
      `sipx-call/tests/secure_media.rs:61,176` — so the claim is true of the library, not the binary.)

### The guard, so this cannot drift a fifth time
- [x] **Generalise `check-audio-claims.py` from codecs to front doors.** Every finding above is one
      shape: a capability word in a hand-maintained string with no code behind it, in a string no
      script reads. The check should hold the manifest `description`, the `lib.rs`/`main.rs` summary,
      the README crate row and the website crate row to each other — and assert crate-table
      membership equals the set of crates without `publish = false`. Adding `README.md` to
      `front_doors()` is the one-line start; the script's own docstring (`:8-10`) argues for the
      general version, because a fourth hand correction leaves the arrangement that produced the
      first three.
- [x] The check runs in `./scripts/gate.py` and in CI, per `X-22`'s parity rule.
- [x] **No suppression list**, under any name — `X-30` held that line and it is why its check is
      worth having.
- [x] Failing-first test: `README.md:114`'s "RFC 4733 DTMF" passes
      `check-audio-claims.py --check` today at exit 0. Name the test that makes it fail.

## Progress
- Filed from two independent read-only sweeps that never met: one over `README.md` +
  `website/` (14 pages, `index.js`, `docusaurus.config.js`), one over all 11 publishable crates'
  manifests and rustdoc. They agreed on bridging and on the README DTMF row from opposite directions.
- **2026-07-29 — done.** Every finding was spot-checked against the code before being fixed, and
  all of them held. Three commits: the guard, the corrections the guard demanded, the prose.

**What the guard became, and why that shape.** `check-audio-claims.py` keeps its filename — a
rename is an edit to `gate.py` and `ci.yml` in disguise, and both already run it, so the
generalised check inherited the wiring and `X-22`'s parity rule for free. It now reads four front
doors for each of the 11 published crates (44 doors) under three rules:

1. **Membership** — both crate tables name exactly the crates that publish. This is the rule that
   forced the table membership the Notes had left to `A-8`: `README.md` was missing `sipx-app` and
   `sipx-app-protocol` and was offering `sipx-testkit`, which is `publish = false`.
2. **Restatement** — no door may claim a capability the crate's own manifest description omits,
   and the two tables must claim the same set as each other. Deliberately *containment* against
   the description rather than equality across four doors: `Sans-IO SIP core.` is a good first line
   and a bad capability list, and a rule demanding a summary enumerate everything would turn crate
   front pages into keywords and be switched off by whoever hit it.
3. **Backing** — a capability in any door needs an item of that crate named for it. Codecs stay
   scoped to `sipx-audio` for the reason the old docstring gave. A capability may be named after
   what it does rather than after its RFC — `send_digits` backs DTMF — because otherwise the
   check calls `sipx-call`'s true DTMF claim an over-claim, which is the false-positive the old
   docstring predicted. The synonyms are other true names, not an escape hatch: nothing in
   `sipx-call` is called `couple` either, so bridging still has nothing behind it there.

Three reader bugs were found while building it, each of which had made the check *weaker* than it
read: `pub async fn play` was not an item (so playback was backed by nothing), test function names
were items (and this project names tests as sentences, so most of English backed every crate), and
a binary crate has no `pub` anything (so `sipx-cli` had no vocabulary at all). All three are now
tested directly.

**What is deliberately not here.** `A-8`'s description findings other than bridging —
`sipx-sip`'s and `sipx-ua`'s "dialogs", `sipx-app`'s crate-doc summary — are untouched, and the
vocabulary is the reason rather than a suppression list: it is the capability words the front
page's own table sells, and `dialogs`, `transactions`, `parser` and `registration` are how sipx is
built rather than capabilities a reader shops for. Add one of those words to `CAPABILITIES` and
the check will report them; that is `A-8`'s to do under this guard, which is the intended order.

`sipx_call::Error` and `#[non_exhaustive]` are also still `A-8`'s — untouched here.

## Notes
- **This is alpha predicate 1 at the layer the predicate does not currently reach.** `X-30` made "no
  claim outlives its caller" mechanical for the *registry*; `X-33` generalises it past `layer =
  "media"`. Neither touches the four capability tables a user actually reads first — and those tables
  are where Opus, bridging and the DTLS-SRTP workaround are being sold right now.
- **The pattern is now five for five.** ICE (`M-27`), UPDATE (`S-22`), DTLS-SRTP (`M-28`), the SDES
  answer check (`M-29`), RFC 8122 — every one was a capability implemented in a crate with no caller
  above it, reading as shipped. Opus is the sixth, and it is the first one found in the *public
  docs* rather than in the registry.
- **What makes the Opus case worse than the others**: `Codec::from_payload_type` refuses Opus
  *deliberately*, with a comment saying so. This is not an unfinished wire — it is a closed door the
  front page advertises as open.
- The sweeps also confirmed a large amount of correctness that should keep these fixes narrow: the
  CLI surface matches the code exactly (four commands, every flag, both env vars, the `--book`
  lookup order, the exit codes), `sync-website.py --check` reports 4 regions in sync, every
  hand-written snippet in `place-a-call.md` resolves to a real item, and hold/resume, both transfer
  flavours, session timers, DTMF, playback, recording and MOS are all real and reachable. ICE is
  honestly declared absent in all three places it appears, and the SDK pages are correctly labelled
  preview.
- Adjacent, and deliberately left to `A-8`: `sipx_call::Error` is not `#[non_exhaustive]`
  (`sipx-call/src/error.rs:6-7`) while its neighbours `CallEvent`, `EndCause`, `Dispatched` and
  `DispatchCounts` are; it went 13 → 16 variants between v0.8.0 and v0.9.0, breaking any downstream
  exhaustive match, and `place-a-call.md:128-133` teaches readers to write one. Only
  `whats-new.md:8` says sipx is pre-1.0.
- Adjacent, also `A-8`: the README crate table omits `sipx-app` and `sipx-app-protocol` (both
  publish) and includes `sipx-testkit` (`publish = false`), so its membership is inverted from
  "published" before a single guarantee is written.
- Neither sweep could verify what crates.io and docs.rs actually render, or that the deployed site
  matches this tree — both read `website/`, not `gh-pages`. A stale deploy would carry claims older
  than these.
