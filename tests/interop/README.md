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
| `PEER_ROLES` | `server`, `user-agent`, or both — see below |
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

The user agent peer additionally runs:

| Test | What it proves |
|---|---|
| `an_independent_user_agent_answers_a_call_sipx_placed` | sipx's offer is read by a foreign answerer, its answer is read back, audio flows and a BYE sipx sends ends it |
| `an_independent_user_agent_places_a_call_sipx_answers` | The other half of RFC 3264: sipx reads a foreign *offer* and writes the answer |

The two TLS refusals assert that the failure is **immediate**, not merely that no lease was
granted. A test that accepted a timeout would pass just as happily against a stack that had
hung, or against a server that never started.

The call tests assert on audio, not on "a session was set up". They run the media in relay mode
and compare the µ-law bytes the peer echoed against the µ-law bytes sipx sent, because a decode
step on the way in would be sipx's opinion of those bytes rather than the bytes. What is
observed is the whole 600 ms clip returned byte for byte in both directions — `M-3`'s
bit-exactness with a foreign implementation in the middle, relaxed not at all. Confirmed
non-vacuous by pointing the same test at an extension that answers and stays silent: it fails on
"a session was set up and nothing was heard", which is exactly the failure a weaker assertion
would have hidden.

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

## Still to do

WSS. Both peers serve it, and plain WebSocket now passes against both. For the first, the module
wants its own TLS domain configuration; for the second, the HTTP server needs its own TLS
binding. The certificate policy it would exercise is the same code
`registers_against_a_real_server_over_tls` already proves against a third party. Worth adding,
not urgent.

A third peer with a different implementation language would be worth more than a fourth C one.
Both peers here are C, and a whole class of assumption — about integer widths, about what a
string is — is shared by construction.
