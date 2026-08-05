---
title: Export signalling and call quality
description: Send redacted signalling to a HEP3 collector and feed RTCP quality samples into application-owned telemetry.
---

# Export signalling and call quality

sipx exposes the observations and leaves storage, dashboards, labels and retention to the
application. It does not bundle a metrics backend. That boundary keeps a collector outage or slow
backend out of the call path and lets an embedding application use the telemetry system it already
operates.

## Send signalling to a HEP3 collector

HEP3 is an optional sink on the existing `sipx-transport` capture path. Configure the local
pcapng file first, then attach the collector:

```rust
use sipx_transport::{CaptureConfig, Config, HepConfig};

let mut transport = Config::new("0.0.0.0:5060".parse()?);
transport.capture = Some(
    CaptureConfig::new("/var/lib/sipx/signalling.pcapng").with_hep(HepConfig::new(
        "192.0.2.40:9060".parse()?,
        42,
    )),
);
```

Export is off by default. When enabled, the transport stamps the message once, applies the same
redaction used by the local capture, and hands the record to one bounded writer queue. The HEP UDP
socket is non-blocking. A full queue or unavailable collector drops and counts the export instead
of delaying a SIP timer or failing a call; inspect `Handle::counters().capture.hep_dropped` and
`hep_records`.

**Redaction is mandatory for HEP export.** Digest responses, opaque authorization credentials,
SDP key material, push tokens and instance identifiers are removed before the HEP payload leaves
the process. Combining HEP with `CaptureConfig::without_redaction()` is rejected before the
endpoint binds. Redaction does not anonymize a call: identities, addresses, routes and timing
remain, so use a trusted network path to the collector and apply the collector's access controls.

The pcapng file is retained as the reliable local diagnostic record. HEP uses best-effort UDP and
sipx does not retry, acknowledge or create an unbounded delivery queue for it.

## Feed RTCP quality into your telemetry

Install a callback on a running call. The callback receives the peer's report for this RTP stream:
interval loss, cumulative loss, jitter as a `Duration`, and an optional round-trip time.

```rust
use sipx_media::RtcpQualityHook;

let (quality_tx, mut quality_rx) = tokio::sync::mpsc::channel(64);
call.set_rtcp_quality_hook(Some(RtcpQualityHook::new(move |sample| {
    // Never block the RTCP worker. The application owns this queue and its drop policy.
    let _ = quality_tx.try_send(sample);
})));

while let Some(sample) = quality_rx.recv().await {
    record_quality_in_your_backend(sample).await;
}
```

The callback runs only after an RTCP packet has passed authentication when SRTCP is active and has
parsed successfully. A report about another SSRC is ignored. Round trip remains `None` until the
peer echoes a sender report; sipx never substitutes zero for missing evidence.

The hook belongs to the logical call. It remains installed across an ordinary re-INVITE, a
re-INVITE that replaces the media session, and an ICE restart. A callback must return promptly;
put blocking aggregation or network work behind a bounded application queue as in the example.
If application callback code panics, sipx logs it and keeps the RTCP worker alive.
