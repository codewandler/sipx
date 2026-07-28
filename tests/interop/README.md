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

Needs Docker. It starts Kamailio with a small registrar configuration, waits for it, runs the
tests and removes the container. `SIPX_KEEP_SERVER=1` leaves it running for poking at.

The tests are `#[ignore]`d, so a normal `cargo test` skips them and does not require Docker.

## What is covered

| Test | What it proves |
|---|---|
| `registers_against_a_real_server_over_udp` | Digest authentication is right, against an implementation that did not learn it from us |
| `registers_against_a_real_server_over_tcp` | The same over a stream transport, including connection reuse for the response |
| `a_refresh_is_accepted_by_a_real_registrar` | The refresh is seen as a refresh — this is where a reused `CSeq` or a changed `Call-ID` shows up |
| `a_real_server_refuses_a_wrong_password` | The success above means something. A server that accepted anything would pass the others too |
| `a_real_server_answers_our_options_ping` | An `OPTIONS` we send is understood by a real element |

## A trap worth knowing

Kamailio's `db_text` refuses to load a table with a null column, and a table it cannot load
authenticates *nothing* while the server still answers normally. The first run of these tests
failed exactly that way and looked like a digest bug in sipx. `run.sh` therefore generates the
`ha1` columns and checks the log for a load failure before running anything.

## Still to do

Asterisk, for a second implementation with different opinions — Kamailio is a proxy/registrar,
and a B2BUA exercises different parts. Filed as its own story.
