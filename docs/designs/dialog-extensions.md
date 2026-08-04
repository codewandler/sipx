# Design: application-owned dialog extensions

**Status:** accepted · **Pillar:** Signalling · **Epic:** `dialog-extensions` · **Stories:** S-40

## Why

Calls already specialize the methods whose semantics the stack owns: BYE, re-INVITE, UPDATE,
REFER, NOTIFY and OPTIONS. INFO, MESSAGE and extension methods currently fall out of dispatch, so an
application cannot implement a negotiated package or a private dialog operation without forking the
call layer. Treating every method as generic would be equally wrong because it would let an
application bypass offer/answer, transfer and dialog sequencing rules.

## Approach

Expose one typed application-owned request path in both directions. Inbound requests carry the
validated method, headers and body plus a transaction-backed response capability that can finish
exactly once. Outbound requests always use the dialog's remote target, route set, CSeq and existing
authentication machinery. INFO, MESSAGE and one `Method::Other` value prove the generality; methods
with stack-owned semantics remain on their specialized paths.

The response capability has a finite lifetime. A dropped or expired capability produces a defined
refusal and releases transaction state, so an application that disappears cannot retain peer-driven
work. No API accepts a prebuilt request whose dialog identifiers, branch or sequence can disagree
with the live dialog.

## Exit

An application can send and answer INFO, MESSAGE and an admitted extension method on a live dialog;
both directions survive an authentication challenge; invalid state and unsupported body semantics
are typed failures; and the existing specialized methods cannot be intercepted through the escape
hatch.
