# Interop tests

Every other test in this repo is sipx agreeing with itself. That is worth a great deal for
correctness against the specs, and worth nothing against a wrong shared assumption: if the
parser and the builder both misread the same sentence of RFC 3261, they agree perfectly and
interoperate with nothing.

These tests run sipx against a real SIP server.

## Running

```sh
./tests/interop/run.sh
```

Needs Docker. It issues a fixture certificate, starts Kamailio with a small registrar
configuration, waits for it, runs the tests and removes the container. `SIPX_KEEP_SERVER=1`
leaves it running for poking at.

The certificate is generated per run by `cargo run -p sipx-testkit --example issue-certs`, from
the same fixture authority the unit tests use. Not committed: a certificate in the repository is
a private key in the repository, and one with a fixed expiry is a test that starts failing on a
date nobody chose.

The tests are `#[ignore]`d, so a normal `cargo test` skips them and does not require Docker.

## What is covered

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
| `registers_against_a_real_server_over_websocket` | RFC 7118 framing and the `sip` subprotocol are understood by Kamailio's own WebSocket module |

The two TLS refusals assert that the failure is **immediate**, not merely that no lease was
granted. A test that accepted a timeout would pass just as happily against a stack that had
hung, or against a server that never started.

## Traps worth knowing

Three of these cost real time, and all three share a shape: something that looks like a bug in
sipx is a bug in the harness.

**A table that fails to load authenticates nothing.** Kamailio's `db_text` refuses a table with
a null column, and a table it cannot load authenticates *nothing* while the server still answers
normally. The first run of these tests failed exactly that way and looked like a digest bug in
sipx. `run.sh` generates the `ha1` columns and checks the log for a load failure before running
anything.

**Kamailio's default `tls_method` refuses TLS 1.3 outright.** It pins the OpenSSL *method* to
1.2, which rejects a ClientHello offering 1.3 rather than negotiating down to 1.2 — `openssl
s_client` fails against it too. The configuration here sets `TLSv1.2+`. sipx offers 1.3 and 1.2
and will not stop offering 1.3 to accommodate a server that is configured this way.

**`grep -q` in a `pipefail` script reports failure on a match.** `docker logs | grep -q pattern`
exits non-zero when the pattern *is* found: `grep -q` stops at the first match, `docker logs`
takes SIGPIPE, and `pipefail` reports the pipeline by that. Every log guard here would have
fired precisely when the thing it looked for was present. The log is read into a variable once
instead.

## Still to do

Asterisk, for a second implementation with different opinions — Kamailio is a proxy/registrar,
and a B2BUA exercises different parts. Filed as its own story.

WSS. Kamailio serves it, but the module wants its own TLS domain configuration, and the
certificate policy it would exercise is the same code `registers_against_a_real_server_over_tls`
already proves against a third party. Worth adding, not urgent.
