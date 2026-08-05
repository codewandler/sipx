# sipx-testkit

Deterministic support for tests built on sipx. `CallHarness` places and answers SIP signalling
between two real transaction layers in one process, over a seeded link and explicit virtual time.
It opens no socket, starts no runtime and sleeps for no wall-clock duration.

The crate also carries the protocol torture corpora and certificate fixtures used by the sipx
workspace. The call harness is the deliberately small supported downstream surface; it stops at
answered INVITE signalling and does not emulate media or claim network interoperability.

See the [public test guide](https://codewandler.github.io/sipx/docs/guides/test-a-call).

Licensed under either of Apache-2.0 or MIT, at your option.
