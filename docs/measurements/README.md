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
| `capacity-media.json` | ramp to 1000 held calls, audio flowing both ways |
| `capacity-idle.json` | the same ramp with a negotiated session that sends no audio |

Reproduce either with:

```sh
cargo build --release -p sipx-call --example capacity_test
./target/release/examples/capacity_test --ramp 50,200,250,500 --dwell 10 --media full \
  --record docs/measurements/capacity-media.json
```

## Reading them

`audio_observed` is the field to check first. When it is `false`, `loss`, `jitter_ms` and `mos` are
`null` rather than numbers — because they are computed from a receive stream, and over an empty one
they read as a perfect score for a call that carried nothing. The first run of this example reported
exactly that for a thousand calls, and the guard exists so it cannot happen silently again.

`drift_s` is the run's wall clock minus what the schedule said it should be: one `--dwell` per step
and nothing else. It is the surplus the machine could not absorb, and it grows before calls start
failing. **It measures the machine, not sipx** — a box doing anything else is charged here too.

## What the recorded runs show

On a 20-core development box, both ramps reached **1000 concurrent calls with no ceiling**. Two
things separate the audio run from the idle one at that point, and both are the media path:

* **memory roughly doubles** — 231 MB against 120 MB, or 237 KB against 122 KB per call;
* **setup latency under a 500-call burst is about eight times worse** — 300 ms against 39 ms at the
  median, while jitter climbs from 0.32 ms at 50 calls to 16.22 ms at 1000.

Neither run was close to saturation by the clock: drift was 1.79 s and 0.75 s against a 40 s
schedule, and the sampling probe stayed at 0.000 s throughout, so the runtime always had headroom to
read a thousand calls' counters. The limit, wherever it is, is past 1000 on this machine.
