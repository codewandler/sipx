---
id: T-20
title: Implement GRUU
pillar: Signalling
status: done
priority:
design:
epic: conformance
areas: [sipx-sip, sipx-ua]
note: M10 · RFC 5627 · needs T-14's Path and T-15's instance ID
---

# Implement GRUU

## Goal
Make one *instance* of a registration addressable: a URI that routes to this UA and no other
registration of the same address of record.

## Acceptance
- [x] A REGISTER offering GRUU carries `Supported: gruu` and a `+sip.instance` media feature tag —
      RFC 5627 §4.1 requires both, and the instance ID is the same one `T-15`'s Outbound registers
      with. Two mechanisms, one instance identity; a second one would be a bug that only shows up
      under a registrar that correlates them.
- [x] The `pub-gruu` and `temp-gruu` `Contact` header field parameters returned in the REGISTER 2xx
      (§5.2) are parsed and kept with the binding, and are discarded with it when the registration is
      replaced or lapses.
- [x] A dialog-forming or target-refresh request populates `Contact` with the GRUU when one is known
      (§4.4: "A UA SHOULD use a GRUU when populating the Contact header field of dialog-forming and
      target refresh requests and responses"), and with the plain contact when none is.
- [x] The `gr` URI parameter (§7) round-trips through the parser and comparison, and a request
      arriving at a GRUU is accepted for the instance it names.
- [x] The choice between the public and a temporary GRUU is the application's, and the default is
      recorded with its reason. A public GRUU is a stable identifier for the instance; a temporary
      one exists so that, per §5.1, "Given a pair of GRUUs, it MUST be computationally infeasible to
      determine whether they were issued for the same AOR or instance ID or for different AORs" —
      which is a privacy property the caller either wants or does not.
- [x] Registrar behaviour is explicitly **not** in scope, and the registry entry says so in its
      `Roles` column. sipx obtains and uses GRUUs; minting them is a registrar's job.
- [x] The RFC registry entry for RFC 5627 moves off "not started" in the same change.
- [x] Failing-first test: `a_request_to_a_gruu_reaches_the_instance_that_registered_it`.

## Progress
- Implemented for the **UA side**, both roles: obtaining GRUUs and using them. The registrar half —
  minting them, §5.4's temporary-GRUU construction, §5.2's 480 after a binding lapses — is not here
  and the registry entry says so. RFC 5627 moves from `none` to `partial` for exactly that reason.
- Split the way `T-15` split RFC 5626. `sipx-sip/src/gruu.rs` is the URI level: `gr` recognition
  (§4.5), its opaque value, and the comparison. `sipx-ua/src/gruu.rs` is the registration level:
  the option tag, the `pub-gruu`/`temp-gruu` pair, and which of the two to publish.
- **§5.4 is why the comparison is its own function rather than `Uri::equivalent`.** "A public GRUU
  will always be equivalent to the AOR based on URI equality rules", because RFC 3261 §19.1.4
  ignores a parameter present in only one of two URIs. `gruu::addressed_to` requires `gr` on *both*
  sides; without that, a UA answers requests aimed at the address of record — which names every
  device the user registered — as though they had been aimed at this one. The unit test asserts the
  RFC's own observation alongside the behaviour, so the day `Uri::equivalent` changes, it says so.
- **One instance identity, enforced by there being one field.** `Registration` and `Config` lost
  `outbound: Option<Flow>` and gained `instance` / `reg_id` / `gruu`. RFC 5626 §4.1 and RFC 5627
  §4.1 name the same `+sip.instance` tag, and two fields would eventually hold two URNs — a fault
  that surfaces at a registrar that correlates the mechanisms, not here. `Config::with_outbound` is
  unchanged as a builder; `Config::with_gruu(instance, kind)` takes the identity explicitly.
- **Asking for a temporary GRUU never quietly yields the public one.** `Gruus::preferred` returns
  `None` rather than substituting, and `dialog_contact` then falls back to the contact (with `ob`
  where RFC 5626 §4.3 wants it) and logs. §5.4's unlinkability is not something the stable
  identifier can stand in for, and a caller told otherwise has been told the opposite of the truth
  about the address it just published.
- GRUUs are read off the `Contact` row whose `+sip.instance` matches ours, not the first row: RFC
  3261 §10.3 has a 2xx list every binding for the AOR, other devices' included. They are replaced
  wholesale on each 2xx and cleared on a rejection or a failed challenge, which is §4.2's
  discard-on-`Call-ID`-change rule expressed as "the set is only ever as old as the binding".
- A `pub-gruu` or `temp-gruu` arriving without `gr` is dropped rather than kept: it would be the
  address of record, and publishing it as this instance's `Contact` fans a mid-dialog request out
  to every device the user has.
- `interpret` now takes the whole `&Registration` instead of `(expires, contact)`. Almost
  everything read out of a 2xx is read relative to what this client sent, and the argument list was
  about to grow a third such field.

## Notes
- The temporary GRUU is the half that is usually skipped, and skipping it silently is worse than not
  offering GRUU at all: a caller that believes it has an unlinkable address and does not is worse off
  than one that knows it is using a stable one.
- The instance ID is a URN and must be stable across restarts, or every restart looks like a new
  device to the registrar. Where it is persisted is `T-15`'s decision; this story consumes it.
