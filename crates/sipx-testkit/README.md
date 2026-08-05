# sipx-testkit

Deterministic support for tests built on sipx. `CallHarness` invokes the real `sipx-call`
`DialOptions`/`dial`/`answer` application path over bounded in-process SIP signalling and returns
both established `Call` values only after their ACK completes. The calls still own ordinary media
ports; it is the signalling path that opens no socket.

The crate also carries the protocol torture corpora and certificate fixtures used by the sipx
workspace. `TransactionHarness`, `Link` and nanosecond `Virtual` provide the lower-level seeded
fault and chronological-time surface. `RtpEcho` is a bounded PCMU test peer for checking a media
boundary from a shell. `RealtimePeer` is a deterministic loopback WebSocket peer for the realtime
bridge vectors, including authentication, audio, cancellation and failure scripts. Neither fixture
is a SIP user agent or production service, and neither claims network interoperability.

See the [call test guide](https://codewandler.github.io/sipx/docs/guides/test-a-call) and the
[RTP echo guide](https://codewandler.github.io/sipx/docs/guides/rtp-echo).

Licensed under either of Apache-2.0 or MIT, at your option.
