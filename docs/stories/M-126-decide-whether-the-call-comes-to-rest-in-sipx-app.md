---
id: M-126
title: Decide whether the call comes to rest in sipx-app
pillar: Media
status: in-progress
priority: 3
design:
epic: media
areas: [sipx-app, diagnostics, redaction]
predicate:
announcement:
note: after M-117 · WssMessage::Binary derives its Debug and sipx-app is in neither redaction scope
---

# Decide whether the call comes to rest in sipx-app

## Goal

Answer, for `sipx-app`, the question `M-117` answered for `sipx-app-protocol` and `sipx-testkit`: is
this a crate where a call comes to rest, and therefore one whose byte buffers must not print
themselves?

## Context

`M-117` gave `check-audio-claims.py` a second scope — the crates where a call comes to rest, as
distinct from the relay path it passes through — and redacted six carriers inside it. It found
`sipx_app::WssMessage::Binary(Bytes)` deriving its `Debug` and left it alone, deliberately and with
a reason: under the realtime contract that audio travels base64-encoded in *text* frames, so a
binary frame is not one of the four carriers the story was about.

That is a good answer to "is this carrier audio" and not an answer to "is this crate a place where
the call rests". The second is a scope decision, and the `M-117` implementor was right that widening
a checker constant is not something to do inside another story's fence — the constant is the
statement, and moving it silently is how a scope stops meaning anything.

So this story decides it. Either `sipx-app` joins the at-rest scope and its carriers are redacted or
argued at their sites, or it does not and the checker records why — the same way the existing scopes
record their own boundaries. What it must not do is stay unexamined, because "nobody looked" and
"we looked and it is fine" are indistinguishable from outside.

## Acceptance

- [ ] `sipx-app` is either in the at-rest scope or explicitly outside it, and the reason is in
      `check-audio-claims.py` beside the scope rather than in a commit message.
- [ ] If in scope: every reachable public carrier is redacted with a bounded record or argued at its
      site, each with a behavioural test — a checker cannot read what an implementation prints.
- [ ] If out of scope: a test pins the property that makes it so, so the decision fails when the
      property stops holding rather than when someone notices.
- [ ] `WssMessage::Binary` is resolved either way and not left as the one carrier nobody classified.
- [ ] The gate is green.

## Progress

- 2026-08-10: selected in the five-story rc.22 wave.

- Filed 2026-08-10 by the `M-117` implementor, who found the carrier and declined to widen a scope
  constant from inside another story.
