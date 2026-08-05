# Design: comparative signalling load

**Status:** accepted · **Pillar:** Build · **Epic:** `comparative-load` · **Stories:** X-98, P-15,
X-99

## Why

The existing load scheduler, bounded CLI load command and soak checks measure sipx well, but they do
not make a fair cross-process comparison. The current call driver negotiates media, while the smallest
common endpoint workload is signalling-only. A useful result needs one neutral workload, immutable
builds, qualification before capacity, bounded supervision and raw evidence; it does not need to
declare a winner.

## Approach

`X-98` writes `docs/specs/comparative-load.md` first. The common flow is a finite
`INVITE -> 2xx -> ACK -> BYE -> 2xx` dialog with no SDP or media, deterministic identifiers, exact
timeouts and a stable result schema. The supervisor waits for readiness, bounds logs and every phase,
owns a process group, terminates it on EXIT/INT/TERM, and waits for the whole group. It records
unsupported resource measurements as absent, never zero.

`P-15` adds the missing bounded answering surface. It reuses the existing endpoint and reporting
types, announces readiness in machine-readable form, requires count or duration plus concurrency and
cleanup bounds, and checks ACK/CANCEL/BYE outcomes. Signalling-only is the baseline; generated media
is an explicit separate mode.

`X-99` qualifies immutable builds at low rate before measuring them in both caller directions. A
finite fixed-rate ladder uses five repetitions with warm-up, measurement and drain phases; correctness
failure is reported as not measured, and overlapping uncertainty is inconclusive. Subject-specific
names, revision pins, commands and results stay in the comparison data directory; generic code,
specification and stories stay subject-neutral.

## Exit

The neutral driver demonstrates at least twice the tested ceiling without becoming the bottleneck.
At each reported capacity point at least 99.9% of offered dialogs complete, no invalid messages or
crashes occur, loopback p99 setup is at most 250 ms, every process drains, and post-warm-up memory
growth stays inside one preregistered tolerance applied identically. Raw per-run JSON, hashes,
environment, seeds, commands and limitations generate the published summary.
