# Measurements

Runs of `crates/sipx-call/examples/capacity_test.rs`, committed so a change can be compared against
what came before rather than against a memory of it.

**These are not deployment sizing figures and not a comparison with any other stack.** Both ends
share one process, one kernel and one scheduler, so the client's cost is charged to the same CPU as
the server's. A ceiling recorded here is this machine's ceiling for this shape of traffic. What the
files are good for is a diff: run the same ramp before and after a change and read the difference.

## The files

| file | run |
|---|---|
| `capacity-none.json` | ramp to 1000 held calls, **no media negotiated at all** — no SDP, no RTP socket |
| `capacity-idle.json` | a negotiated session that sends no audio |
| `capacity-full.json` | audio flowing both ways |

Reproduce any of them with:

```sh
cargo build --release -p sipx-call --example capacity_test
./target/release/examples/capacity_test --ramp 50,200,250,500 --dwell 10 --media full \
  --record docs/measurements/capacity-full.json
```

## Reading them

`audio_observed` is the field to check first. When it is `false`, `loss`, `jitter_ms` and `mos` are
`null` rather than numbers — because they are computed from a receive stream, and over an empty one
they read as a perfect score for a call that carried nothing. The first run of this example reported
exactly that for a thousand calls, and the guard exists so it cannot happen silently again.

`drift_s` is the run's wall clock minus what the schedule said it should be: one `--dwell` per step
and nothing else. It is the surplus the machine could not absorb, and it grows before calls start
failing. **It measures the machine, not sipx** — a box doing anything else is charged here too, which
is why these three were taken back to back on an idle host and the earlier pair were not.

## What the recorded runs show

All three reached **1000 concurrent calls with no ceiling** on a 20-core development box. At that
point:

| | RSS | per call | setup p50 | drift |
|---|---|---|---|---|
| no media | 51 MB | 52 KB | 17.5 ms | 0.07 s |
| idle media | 119 MB | 121 KB | 36.5 ms | 0.73 s |
| audio flowing | 218 MB | 223 KB | 34.7 ms | 0.74 s |

**Media costs about 4× the memory of plain signalling, and most of the first doubling is the stack
rather than the audio.** Negotiating a session and binding a socket takes 51 MB to 119 MB before a
single packet is sent; carrying audio then takes it to 218 MB. So a deployment that puts media
elsewhere and runs sipx as signalling only is not saving a fraction — it is working at roughly a
quarter of the footprint.

**Setup latency and drift tell the same story in a sharper way.** The `none` column is not merely
faster, it is an order of magnitude steadier: 0.07 s of drift against 0.73 s, and a setup median of
17.5 ms against ~35 ms. Carrying audio on top of an already-negotiated session adds essentially
nothing to either — the cost is in having the media path at all, not in feeding it.

Neither of the two media runs was close to saturation by the clock, and the sampling probe stayed at
0.000 s throughout, so the runtime always had headroom to read a thousand calls' counters. The
limit, wherever it is, is past 1000 on this machine for all three shapes.
