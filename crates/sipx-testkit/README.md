# sipx-testkit

Deterministic support for tests built on sipx. `CallHarness` invokes the real `sipx-call`
`DialOptions`/`dial`/`answer` application path over bounded in-process SIP signalling and returns
both established `Call` values only after their ACK completes. The calls still own ordinary media
ports; it is the signalling path that opens no socket.

The crate also carries the protocol torture corpora and certificate fixtures used by the sipx
workspace. `TransactionHarness`, `Link` and nanosecond `Virtual` provide the lower-level seeded
fault and chronological-time surface. Neither harness claims network interoperability.

See the [public test guide](https://codewandler.github.io/sipx/docs/guides/test-a-call).

Licensed under either of Apache-2.0 or MIT, at your option.
