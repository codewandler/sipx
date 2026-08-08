# Interop tests

Every other test in this repo is sipx agreeing with itself. That is worth a great deal for
correctness against the specs, and worth nothing against a wrong shared assumption: if the
parser and the builder both misread the same sentence of RFC 3261, they agree perfectly and
interoperate with nothing.

These tests run sipx against real SIP implementations. Plural, since `X-17`: one peer narrows
that risk and does not close it, because one peer is one more reading of the RFCs, not a
consensus.

## Running

```sh
./tests/interop/run.sh                   # every peer
./tests/interop/run.sh --peer asterisk   # one
./tests/interop/run.sh --list            # what peers exist
./tests/interop/run.sh -- some_test_name # extra arguments for cargo test
```

Needs Docker. For each peer it issues a fixture certificate, starts the container with that
peer's configuration, waits for it, runs the test list and removes the container.
`SIPX_KEEP_SERVER=1` leaves it running for poking at.

The certificate is generated per run by `cargo run -p sipx-testkit --example issue-certs`, from
the same fixture authority the unit tests use. Not committed: a certificate in the repository is
a private key in the repository, and one with a fixed expiry is a test that starts failing on a
date nobody chose.

The tests are `#[ignore]`d, so a normal `cargo test` skips them and does not require Docker.

## Adding a peer

A peer is a directory beside `run.sh` containing a `profile.sh`. Nothing in `run.sh` names an
image, a container or a configuration directory, and adding a peer must not need to change it.
A profile declares:

| | |
|---|---|
| `PEER_TITLE` | one line, for the banner |
| `PEER_IMAGE` | pinned, with an environment override |
| `PEER_CONTAINER` | the container name |
| `PEER_ROLES` | `server`, `user-agent`, `media-security`, `opus-audio`, or any combination — see below |
| `PEER_KEYINGS` | `sdes` and/or `dtls`, for a `media-security` peer — which SRTP keyings it can do |
| `PEER_READY_MARKER` | the log line that means *listening*, not merely *started* |
| `PEER_ENV` | environment the tests need to find this peer |
| `PEER_DIVERGES_ON` | `test_name:STORY-ID` for a measured, filed disagreement |
| `peer_prepare` | generate anything that cannot be committed |
| `peer_mounts` | the `-v` arguments |
| `peer_check` | guards against a peer that started but cannot do the thing under test |

**A profile does not choose its tests.** `run.sh` owns the list, and a peer says only which
*roles* it can play:

- **`server`** — registration, digest, a refresh, a wrong password, `OPTIONS`, TLS and its two
  refusals, WebSocket. Every peer runs this list, identically.
- **`user-agent`** — a call sipx placed and a call sipx answered, with SDP negotiated, audio
  flowing and a BYE ending it. A proxy has no dialplan and cannot answer a call, so it does not
  claim this role; that is a property of the peer, not a per-peer wording of a test.
- **`media-security`** — a call whose media is encrypted, keyed the way `PEER_KEYINGS` declares.
  Added by `X-27`, because until then `grep -i "srtp\|savp\|dtls\|sdes"` over this directory
  matched nothing: the harness had placed calls against real peers since `X-17` and had never
  once done it with encrypted media. That is why `M-25`'s defect survived six releases — all 17
  SRTP unit tests were round trips, and a round trip between two ends that are wrong the same way
  is a round trip that works.
- **`opus-audio`** — exact Opus-only calls in both offer/answer roles. Each sends a distinct
  48 kHz signal through the peer's decoder and encoder and requires recognisable, non-silent
  samples from sipx's decoder. It is separate from `user-agent` because Opus is an optional
  capability and the role must fail closed when the peer's codec module is absent.

SRTP's two keyings share no code path, so this role is run per keying and a peer declares which
it can do. **Three different things can stop a keying from being exercised, and `run.sh` prints
which one applies on every run:** the peer does not support it, sipx does not offer it, or it
ran. The middle case is kept separate from the first on purpose — asterisk does DTLS-SRTP
perfectly well; what is missing is on our side, and recording it against the peer would file our
gap as theirs.

Both declared keyings now have a named test. The harness enables `sipx-cli`'s `dtls` feature for
the media-security role; the call still selects DTLS-SRTP explicitly, so enabling the build feature
does not alter the SDES case or any ordinary call.

The list is the contract. A test that is softened until every peer passes it measures the
intersection of the peers, which is the one thing an interop suite must not do — so a
disagreement is recorded in `PEER_DIVERGES_ON` with the story that settles it, printed loudly on
every run, and never edited into the test.

## How a second peer was chosen

Not by preference. The criteria, in the order they eliminate candidates:

1. **Independent lineage.** The point of a second peer is that it shares no code and no reading
   of the RFCs with the first. The two peers here — Kamailio and Asterisk — are separate
   projects with separate authorship, and, the part that actually matters, their SIP message
   handling comes from different code bases entirely: Kamailio parses SIP itself, in a lineage
   descended from a SIP router; Asterisk's `chan_pjsip` delegates the protocol to PJSIP, a
   general-purpose SIP library maintained by a third party. So a message that leaves sipx is
   read by two parsers with no common ancestor. A fork, a rewrite of the same code, or a second
   product wrapping the same library would have failed this criterion, and most candidates do.
2. **A different role.** Kamailio is a proxy and registrar; it never answers a call. The gap this
   suite had was that no independent implementation had ever answered a call sipx placed, and
   only a user agent can close it. Asterisk answers as a back-to-back user agent, which is a
   third reading of the RFCs on top of a second parser.
3. **Scriptable without interaction.** No GUI, no manual step, no account to create. The
   [vision](../../docs/vision.md)'s "testable from a shell" applies to the harness too, and a
   peer that needs a person is not a peer for this purpose however good it is.
4. **Obtainable in CI.** A pinned public container image that a runner can pull without
   credentials, and that starts in seconds rather than minutes.
5. **A licence that permits it.** Both peers are run as separate processes over a network
   socket; nothing here links against either, and no configuration or code is copied from
   either project.

## What is covered

Both peers run this list, unchanged:

| Test | What it proves |
|---|---|
| `registers_against_a_real_server_over_udp` | Digest authentication is right, against an implementation that did not learn it from us |
| `registers_against_a_real_server_over_tcp` | The same over a stream transport, including connection reuse for the response |
| `a_refresh_is_accepted_by_a_real_registrar` | The refresh is seen as a refresh — this is where a reused `CSeq` or a changed `Call-ID` shows up |
| `a_real_server_refuses_a_wrong_password` | The success above means something. A server that accepted anything would pass the others too |
| `a_real_server_answers_our_options_ping` | An `OPTIONS` we send is understood by a real element |
| `registers_against_a_real_server_over_tls` | The handshake and the certificate check work against a TLS stack that is not ours |
| `refuses_a_real_server_presenting_the_wrong_name` | …and a genuine server with a genuine certificate for *another* name is still refused |
| `a_real_server_is_refused_when_its_issuer_is_unknown` | …and so is one whose issuer we do not know |
| `registers_against_a_real_server_over_websocket` | RFC 7118 framing and the `sip` subprotocol are understood by a real WebSocket module |
| `registers_against_a_real_server_over_secure_websocket` | The same upgrade succeeds only after certificate verification on the peer's HTTPS path |

The user agent peer additionally runs:

| Test | What it proves |
|---|---|
| `an_independent_user_agent_answers_a_call_sipx_placed` | sipx's offer is read by a foreign answerer, its answer is read back, audio flows and a BYE sipx sends ends it |
| `an_independent_user_agent_places_a_call_sipx_answers` | The other half of RFC 3264: sipx reads a foreign *offer* and writes the answer |
| `a_real_peer_accepts_media_sipx_encrypted_with_sdes` | A peer that derived the SRTP session keys by its own reading of RFC 3711 authenticates sipx's packets — and says so when it cannot. Reverting `M-25`'s `SESSION_AUTH_LEN` makes this fail with the peer's own `SRTP unprotect failed`, which is the measure of whether it would have caught the defect it exists for |
| `opus_audio_peer_answers_sipx_offer_and_echoes_real_audio` | The peer answers an Opus-only offer, decodes sipx's signal and re-encodes it; sipx requires a dynamic payload type, the 48 kHz RTP clock, and the recovered signal rather than silence |
| `opus_audio_peer_offers_and_sipx_answers_with_real_audio` | The peer creates the Opus-only offer and the same decoded-signal proof exercises the inverse offer/answer role |

The two TLS refusals assert that the failure is **immediate**, not merely that no lease was
granted. A test that accepted a timeout would pass just as happily against a stack that had
hung, or against a server that never started.

The G.711 call tests assert on audio, not on "a session was set up". They run the media in relay
mode and compare the µ-law bytes the peer echoed against the µ-law bytes sipx sent, because a
decode step on the way in would be sipx's opinion of those bytes rather than the bytes. What is
observed is the whole 600 ms clip returned byte for byte in both directions — `M-3`'s bit-exactness
with a foreign implementation in the middle, relaxed not at all. Confirmed non-vacuous by pointing
the same test at an extension that answers and stays silent: it fails on "a session was set up and
nothing was heard", which is exactly the failure a weaker assertion would have hidden.

Opus cannot use byte equality: it is lossy, and its encoder and decoder retain state. Its pair of
tests therefore sends different 48 kHz tones in the two call roles and measures the recovered
waveform after the peer has decoded and re-encoded it. The assertions require a negotiated dynamic
payload type, a 48 kHz RTP clock, substantial non-silent output, strong correlation with the sent
tone, and weak correlation with an unrelated tone. Both endpoint configurations permit only Opus,
so G.711 cannot turn a codec failure into a pass.

## What the second peer disagreed with

One thing, and it is now fixed rather than filed: `T-23`. Both peers pass the whole shared list,
and neither profile declares a divergence.

sipx's WebSocket client used to request `/` on the SIP port, unconditionally. The second peer
serves SIP over WebSocket from its own HTTP server, at `/ws`, on that server's own port:

```text
GET /ws  on 127.0.0.1:8088 → upgraded, subprotocol sip
GET /    on 127.0.0.1:8088 → HTTP/1.1 404 Not Found
```

RFC 7118 §5 fixes neither the path nor the port, so both readings are legal — this was a gap in
what a sipx `Target` could express, not a defect in either implementation. The first peer accepts
the upgrade on any path, which is precisely why one peer could not have found it.

A `Target` now names both (`Target::at_path`, defaulting to `/`), and each peer declares where it
serves SIP over WebSocket through `SIPX_INTEROP_WS_PORT` and `SIPX_INTEROP_WS_PATH` in its
`PEER_ENV` — the defaults being the SIP port and `/`, which is why the first peer's profile says
nothing. Where a peer puts its WebSocket is a fact about the peer, so it belongs in the profile
and not in the test.

Everything else the second peer agreed with on the first attempt, including the offer/answer
exchange that had never met a foreign answerer.

## Traps worth knowing

All of these cost real time, and they share a shape: something that looks like a bug in sipx is a
bug in the harness, or in a peer's defaults.

**A table that fails to load authenticates nothing.** Kamailio's `db_text` refuses a table with a
null column, and a table it cannot load authenticates *nothing* while the server still answers
normally. The first run of these tests failed exactly that way and looked like a digest bug in
sipx. Its profile generates the `ha1` columns and checks the log for a load failure before
running anything.

**Both peers ship a TLS default that no current client will talk to.** Kamailio pins the OpenSSL
*method* to 1.2, which rejects a ClientHello offering 1.3 rather than negotiating down.
Asterisk's `res_pjsip` defaults to TLS 1.0, which modern OpenSSL refuses outright — `openssl
s_client -tls1_2` fails against it too, with `unsupported protocol`. Both configurations here set
a floor rather than a version. sipx offers 1.3 and 1.2, per `sip-tls.md` §3.5, and will not stop
offering 1.3 to accommodate a server configured this way.

**A peer that is up is not a peer that works.** Each profile's `peer_check` runs after the
readiness marker and before the tests: a subscriber that did not load, or a TLS transport that
did not start, leaves a peer answering happily on UDP while every result that depends on it reads
as a bug in sipx.

**`grep -q` in a `pipefail` script reports failure on a match.** `docker logs | grep -q pattern`
exits non-zero when the pattern *is* found: `grep -q` stops at the first match, `docker logs`
takes SIGPIPE, and `pipefail` reports the pipeline by that. Every log guard here would have fired
precisely when the thing it looked for was present. The log is read into a variable once instead.

## What the media-security role does *not* prove — the AEAD suites

`PEER_KEYINGS` says how a peer keys SRTP; it says nothing about which transform the keying settles
on, and the two are not the same claim. Both keyings run here in counter mode, because the pinned
peer cannot do anything else: **it is built without AEAD-GCM support entirely.** Measured rather
than assumed, twice, at `andrius/asterisk:20.20.1-alpine-3.24`:

- Its SRTP module references `srtp_crypto_policy_set_aes_cm_*` and no `srtp_crypto_policy_set_aes_gcm_*`,
  though the `libsrtp2` 2.7.0 beside it exports them and is built against OpenSSL. Its RTP module
  contains the strings `SRTP_AES128_CM_SHA1_80` and `SRTP_AES128_CM_SHA1_32`, and no
  `SRTP_AEAD_*`, so it cannot offer a GCM profile in a DTLS handshake either.
- Offered a single `a=crypto` line by hand, to the same endpoint on the same run: with
  `AES_CM_128_HMAC_SHA1_80` it answers `200 OK`; with `AEAD_AES_256_GCM` and with
  `AEAD_AES_128_GCM` it answers `488 Not Acceptable Here`, logging `Couldn't negotiate stream
  0:audio`. The control is what makes the two refusals mean the suite and not the probe.

So `a_real_peer_accepts_media_sipx_encrypted_with_sdes` passing is a fact about RFC 3711's key
derivation, which a published vector already pins — not about RFC 7714's, which none does. Nothing
in this directory currently exercises the AEAD derivation, and a reader who took the
`media-security` role at face value would think otherwise. What does exercise it, over DTLS-SRTP
and for `AEAD_AES_256_GCM` only, is the native-browser proof in `../browser-audio/`; see
[`docs/specs/srtp.md`](../../docs/specs/srtp.md) §12.10 for what that settles and what it leaves
open.

Closing the rest needs a SIP peer built with AEAD-GCM, pinned and public, that can be made to
*require* a GCM suite so a silent fall back to counter mode fails the run rather than passing it.
That is a peer this harness does not have, not a gap in the harness.

## Still to do

The five released signalling transports run against both profiles. A profile can put WS and WSS on
different ports and paths; the shared tests read those facts from the profile and do not assume the
two upgrades share an HTTP listener.

A third peer with a different implementation language would be worth more than a fourth C one.
Both peers here are C, and a whole class of assumption — about integer widths, about what a
string is — is shared by construction.
