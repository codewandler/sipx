# Archived: the vendor-neutral public site

**Decision:** the public site names no third-party software.
**Held:** `X-47` (2025-11) to `X-71` (2026-08-04).
**Superseded by:** `X-71`, which permits comparison subjects inside a defined path scope.

## What the decision was

`X-47` deleted `website/docs/migrate/from-<product>.md` pages — per-product migration guides with
"in your deployment / goes to / status" tables — and replaced them with one vendor-neutral guide,
`website/docs/guides/integrate-existing-system.md`, framed as a table of **SIP roles** (user agent
client and server, registration client, proxy, registrar and location service, call application)
citing RFC 3261. Its Acceptance stated the rule directly: *no prior-art project names left in the
README or public site.*

The rule was enforced by convention rather than by a check. Nothing in `sync-website.py` matched
product names, and `check-provenance.sh` only ever fired on the private denylist. It held because
people knew about it.

## Why it was made

Three reasons, and they are not equally durable.

1. **Per-product migration guides rot faster than anything else on a docs site.** They encode
   another project's configuration surface, which changes without notice and which nobody here
   tracks. The deleted pages were already partly wrong when they were deleted.
2. **A role-based framing is more useful to more readers.** "You need a registration client" routes
   correctly whatever the reader is migrating from; "here is how your dialplan maps" only helps one
   audience and teaches the wrong mental model to the rest.
3. **Distance from prior art.** sipx is written from RFCs, and the repository's non-negotiable 1
   keeps design rationale citing primary sources. Keeping product names off the public surface was
   read, at the time, as part of the same posture.

Reasons 1 and 2 are about documentation quality and remain true. Reason 3 conflated two different
things, and that conflation is what `X-71` unpicks.

## Why it changed

The conflation: **naming a project as prior art and naming it as a comparison subject are different
speech acts.** Non-negotiable 1 targets the first — it exists so design rationale cites RFCs rather
than another implementation, and so the repository does not advertise a derivation. A comparison
page makes no claim about where sipx's design came from. It answers the question a reader arrives
with, which `does-this-fit.md` was never structured to answer: not "is this for me" but "why this
and not that".

Two supporting facts made the old rule look more principled than it was:

- The peers named throughout `tests/interop/` — as executed test subjects — were **never** on the
  denylist, and are named in some thirty tracked files. The site's neutrality was a product
  decision, not a legal or provenance constraint. Those are easy to confuse and were confused.
- An anonymised comparison is not a workable compromise. Every observation has to cite evidence
  that can stop being true, and the evidence is a URL that carries the subject's name. Anonymity
  and falsifiability are mutually exclusive here, so a page that tried for both would have had to
  give up the property that makes it trustworthy.

## What did not change

- `docs/vision.md` principle 5. **Design rationale still cites primary sources** — RFCs and our own
  specs — never another implementation.
- The role-based framing of `integrate-existing-system.md`, which stays as the integration guide.
  `X-71` does not bring the per-product migration pages back, and if they ever return that should
  be a decision with its own story rather than drift from this one.
- Commit messages. `check-provenance.sh --history` has no exception at all.
- The rot problem that motivated reason 1. The comparison page answers it differently rather than
  ignoring it: the data is generated and checked, every observation carries a pinned version and a
  date, and staleness past a policy window fails the gate.
