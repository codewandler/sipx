# Design: demand-led capability work

**Status:** accepted · **Pillar:** Media · **Epic:** `demand` · **Stories:** S-36, M-42, M-43,
M-44, M-45, M-46, T-28, T-29, T-30, P-14

## Why

sipx's backlog has been derived from RFCs, from our own review findings, and from what the design
implies is missing. None of those sources says what people actually hit when they try to ship a
voice product. A 2026-08-04 survey of ~376 public issues and 17 discussions across a comparable
open-source SIP ecosystem supplies that missing input, and it disagrees with our priorities in
three ways worth acting on.

**First, the loudest unmet need is not a protocol feature.** It is NAT and address advertisement —
controlling the address that appears in `Contact`, `Via` and the SDP `c=` line independently of the
bind address, and latching RTP to the source when the peer's SDP lies. sipx has ICE, which solves a
superset of this for peers that also do ICE; most of the demand is from people not doing ICE at
all. Whether sipx has a good non-ICE answer is the question `M-42` settles.

**Second, several requests that look like codec requests are one request for an unopinionated PCM
boundary.** Users bridging calls to real-time voice services need 16-bit linear PCM in and out at
arbitrary sample rates, with resampling and no hardcoded bit depth. Reading those as "add an AI
integration" would build the wrong thing; reading them as "stop assuming 8 kHz 8-bit µ-law
everywhere" builds the right one and serves four separate reported use cases at once.

**Third, the negative signal is as useful as the positive.** Across that corpus, **QUIC and SCTP
drew zero requests**, while G.722, linear PCM and a jitter buffer each drew named, still-open
requesters. G.729, AMR, iLBC, T.38, video, answering-machine detection, voice activity detection,
CDR and OpenTelemetry are all at or near zero. Conferencing demand is thin — three mentions, all in
one thread — which recalibrates how urgent reaching the mixer from a call actually is.

None of this overrides the roadmap. It is one more input, and a lagging one: it measures what an
existing user base asked an existing project for, not what sipx's own users will need. It is
recorded here so the reasoning is inspectable rather than absorbed as a hunch.

## Approach

Work splits into two shapes, and conflating them wastes effort.

**Verify-first.** Several reported failures are places sipx may already be correct. Each becomes a
test that either passes — pinning behaviour we did not know we relied on — or fails and reveals a
defect. Cheap, high information per unit effort, and they are grouped into `S-36` rather than
spread across a story each, because the work is one pass through the same subsystems:

- transfer must report success on the `NOTIFY` carrying `sipfrag` `200`, never on the `202` that
  merely accepted the REFER (RFC 3515 §2.4.6);
- hold must accept a re-INVITE with an **empty** body and must answer `a=sendonly` with
  `a=recvonly` rather than `sendrecv` (RFC 3264 §6.1);
- registration must refresh against the **granted** `Expires` from the 200, not the requested one,
  tolerate a `100 Trying` from a registrar, survive a single failed refresh without tearing the
  registration down, and answer a `stale=true` re-challenge (RFC 3261 §10.2.4, §22.4);
- in-dialog requests must be able to answer a challenge on `BYE` and `MESSAGE`;
- offer/answer must handle a dynamic payload type where the local and remote numbers differ
  (RFC 3264 §6.1).

**Build.** The rest are genuine gaps, ordered by evidence strength rather than by how interesting
they are. Two of them — `a=rtcp-mux` (RFC 5761) and DTLS `a=setup:actpass` (RFC 4145, RFC 5763) —
are hard blockers for anyone bridging to a browser, which makes them prerequisites for `M-38`
rather than peers of it.

Everything here stays inside sipx's stated scope. Proxy, registrar-server and PBX asks — a large
share of the surveyed demand — belong to the separate platform and are not filed here.

## Alternatives considered

- **File the whole survey as stories.** Rejected: much of it is out of scope, and a backlog that
  records another project's issue tracker stops being a plan.
- **Treat the voice-service requests as an integration feature.** Rejected. Building
  answering-machine detection, voice activity detection or a speech integration answers a question
  nobody asked; the four reported cases all resolve to raw PCM I/O and resampling, which is smaller
  and composes with any provider.
- **Prioritise conferencing because we already have a mixer.** Rejected as sunk-cost reasoning —
  the demand evidence is thin, and `C-6` should be scheduled on its own merits.
- **Extend QUIC on the strength of it being a differentiator.** Rejected: zero requests in the
  surveyed corpus. It stays experimental and `T-13` remains the only QUIC work queued.

## Risks and open questions

- **The demand signal is from another project's user base**, filtered by what that project already
  did well. It under-reports needs its users never expected it to meet, and one systematic
  third-party audit batch inflates several themes — those were counted as a single voice, but the
  correction is a judgment, not a measurement.
- **`M-42` may find the capability already present.** That is a good outcome and the story must be
  allowed to close as "verified, documented, no code" rather than inventing work to justify itself.
- Whether resampling belongs in `sipx-audio` or behind a feature flag with a dependency is left to
  `M-43`; the crate currently documents the absence of resampling as deliberate, and that note has
  to be revisited rather than contradicted.
