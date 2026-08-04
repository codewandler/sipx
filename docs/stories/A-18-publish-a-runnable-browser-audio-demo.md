---
id: A-18
title: Publish a runnable browser-audio demo
pillar: Application
status: backlog
priority: 18
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, website, example, audio, m15]
predicate:
announcement:
note: after A-17 · static public demo for register, dial, answer and non-silent audio
---

# Publish a runnable browser-audio demo

## Goal

Put a working executable example on the public documentation site so a reader can exercise the
packaged SDK instead of reconstructing its lifecycle from snippets.

## Acceptance

- [ ] A static demo imports the packed SDK and supports configured WSS registration, dial, answer,
      hangup, mute and microphone/output selection without a second SIP or media library.
- [ ] The UI reports connection, registration, call, permission and negotiated-security state with
      actionable typed failures; it never prints credentials or full authorization headers.
- [ ] It displays the audio-only, relay-limited support boundary before connection and does not offer
      video or data-channel controls.
- [ ] Local development uses explicit configuration and documented certificate/origin requirements;
      the public deployment has a restrictive content security policy and no embedded credentials.
- [ ] The public guide explains the complete lifecycle and inlines code from the example rather than
      maintaining a second snippet.
- [ ] A bounded browser test loads the built public page, completes both call roles with fake media,
      proves non-silent audio and observes track/socket cleanup after hangup.
- [ ] `build-docs.sh`, link checks and the full gate are green.

## Progress

- Backlog. Depends on A-17.
