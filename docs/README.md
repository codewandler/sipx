# sipx docs

Start here to find anything inside the repository. These are the **internal contributor
docs**: vision, roadmap, story status, specs, design records, and notes. The public user
documentation lives in [`../website`](../website) and is deployed to
[codewandler.github.io/sipx](https://codewandler.github.io/sipx/) — nothing under `docs/` is
published there. Work is tracked with the **track** framework — see
[AGENTS.md](../AGENTS.md) → **"Start here"** for the working loop.

## Map

| If you want… | Read |
|---|---|
| Why the project exists; the principles | [vision.md](vision.md) |
| Status + what's next; the epics | [roadmap.md](roadmap.md) |
| **What to work on right now** | [stories/README.md](stories/README.md) — the backlog/status board |
| The detail of a specific story | `stories/<ID>-<slug>.md` |
| The implementable contract for a subsystem | [specs/](specs/) |
| Design records for non-trivial work | [designs/](designs/) |
| Finished / superseded material | [archive/](archive/) |
| Released history | [../CHANGELOG.md](../CHANGELOG.md) |

## Specs vs. designs

They are not the same thing and the distinction matters here.

- A **spec** (`specs/`) is the normative contract for a subsystem: RFC citations, types,
  state tables, timers, and byte-level test vectors. It is written *before* the code and is
  what the tests are derived from. It says what the software must do.
- A **design** (`designs/`) is the record of a decision: why this approach, what was
  rejected, what is still open. It says why the software is shaped the way it is.

An epic normally has both — a design that frames it, and one or more specs it implements.

## Working here

Every contributor — human or agent — starts at [AGENTS.md](../AGENTS.md) → **"Start here"**:
open the [board](stories/README.md), take the top `ready` story by priority, follow the loop,
keep the gate green. New or unscoped work? Create a story first (`/track:story`) so the next
agent inherits the context. After any change to a story's status/priority/title/epic/note, run
`/track:board`. Optional story `areas` are query-only subsystem tags matching crate names, so
`/track:next sipx-rtp` selects media-layer work.
