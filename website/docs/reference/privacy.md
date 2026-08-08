---
title: Speech privacy
description: What a speech provider may do with a call's audio and text in sipx, what the defaults are, and which opt-ins a host has to write down.
---

# Speech privacy

sipx defines two substitutable speech provider contracts — recognition and synthesis — and this
page is the rule about what a provider may do with the audio and text a call gives it.

**No speech provider ships yet.** What exists today is the contract, its selection policy, the
per-call session driver and the privacy rule below; there is no bundled recogniser or synthesiser,
no model, and no accelerator dependency. The rule is written first on purpose: retention is the kind
of behaviour that is impossible to remove once one implementation has assumed it.

## The default

An endpoint that configures nothing gets a policy that is **local, offline and keeps nothing**:

- a provider may be selected only if it declares that no audio, text or derived data leaves the
  machine, and that it requires no network egress;
- data that came from the call is retained for the live operation that produced it, and no longer.

Retention is not a default you turn off. It is something a host asks for, one opt-in at a time, and
each request is refused at selection until it is written down.

## What is classified as the call's data

| Class | What it is | Whose it is | Kept for |
|---|---|---|---|
| Call audio | the PCM a recognition session receives, and the PCM a synthesis session produces | the call's | the live operation |
| Transcript | recognition text, at any revision | the call's | the live operation |
| Synthesis input | the text an application asked the call to speak | the call's | the live operation |
| Derived cache | anything computed from that audio or text — adaptation state, embeddings, indexes | the call's | the live operation |
| Model state | weights and warmed engine state | the provider's | while the provider is loaded |
| Credentials | material a provider needs to reach whatever it reaches | the provider's | while the provider is loaded |

"The live operation" is named per class: a recognition result ends at the utterance's terminal
event, a synthesized chunk ends when it is handed to the call, and everything a session holds ends
when the session stops.

## The three opt-ins

Each of these is host configuration, and each is refused at selection until it is present. No
provider, no fallback step and no default turns one on.

| Opt-in | What it admits |
|---|---|
| Debug capture | call audio, transcripts or synthesis input written to a durable destination for diagnosis |
| Persistent derived data | a cache derived from the call's audio or text surviving the session |
| Off-host processing | the call's data leaving this machine, and the network egress that carries it |

Two things make this checkable rather than aspirational. A provider **declares** each property in
discovery, so an operator can read what is installed without running it. And a session **reports**
what it was admitted to do on the speech event stream before it produces anything — the provider
identity, the contract kind, what the provider declared, and which opt-ins are in force for that
call. An opt-in a host permitted but no provider uses is not reported as in force.

A provider that declares an opt-in the host has not configured is not selected, and selection says
which half refused it: a provider that would send the call elsewhere and one that would keep it are
different problems with different answers.

## What ordinary logs contain

sipx installs no logger; a host owns its subscriber, filtering and destination — see
[Logging](logging.md). What the speech contract guarantees is what its records can and cannot say.

A record may name the provider identity, the contract kind, the lifecycle transition, the configured
limits and the typed failure or cancellation cause. It never contains audio samples, transcript
text, synthesis input, model paths or credentials.

The guarantee is in the types rather than in a review rule: every value that carries the call's data
renders as its class and its size, so a record written the ordinary way is already redacted, and a
provider's credentials and model paths have a carrier whose only rendering is a redaction. Size and
class stay visible on purpose — "an utterance of 33 octets was cancelled because the call ended" is
what makes an incident diagnosable, and it is not a transcript.

## One call cannot reach another

Every queue is per session and every session is per call, including two sessions of the same
provider on two different calls. A call has its own bounded input queue, its own output queue, its
own limits, its own cancellation and its own provider state:

- one call's stalled consumer loses that call's frames and names the loss to that call's session;
- one call's cancellation stops that call's session and no other;
- utterance and request identities start again for every session and never carry across.

## When a session stops

A session stops on a flush, a cancellation, the call ending, a provider failure, provider loss, or
the drain deadline expiring against a provider that will not stop itself. In every one of those
cases the driver erases the unconsumed audio it is holding, releases the provider — with whatever
engine, model state and device allocation it held — releases its tap on the call's audio, and only
then closes the session's output stream.

Terminal and lifecycle events are not erased. They are the outcome, and a consumer that never learns
why a session ended has been told less than nothing.

## Operational limits worth knowing

- **Erasure means no copy survives and nothing can still reach one.** It does not mean a freed
  memory block was overwritten; an allocator may already have copied it, and a promise nothing can
  check is not one worth making.
- **Speech failing is never the call failing.** Warm-up failure, provider loss and fallback leave
  the call established; ending a call because speech failed is an application decision.
- **A slow speech consumer never slows the call.** It loses named frames instead, and the session is
  told exactly what was lost.
- **Nothing here is speaker identification, translation, conversation memory or model training**,
  and no network-backed provider is ever selected implicitly.

For the transport and media protections around all of this, see [Security](security.md).
