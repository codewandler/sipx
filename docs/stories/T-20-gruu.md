---
id: T-20
title: Implement GRUU
pillar: Signalling
status: backlog
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
- [ ] A REGISTER offering GRUU carries `Supported: gruu` and a `+sip.instance` media feature tag —
      RFC 5627 §4.1 requires both, and the instance ID is the same one `T-15`'s Outbound registers
      with. Two mechanisms, one instance identity; a second one would be a bug that only shows up
      under a registrar that correlates them.
- [ ] The `pub-gruu` and `temp-gruu` `Contact` header field parameters returned in the REGISTER 2xx
      (§5.2) are parsed and kept with the binding, and are discarded with it when the registration is
      replaced or lapses.
- [ ] A dialog-forming or target-refresh request populates `Contact` with the GRUU when one is known
      (§4.4: "A UA SHOULD use a GRUU when populating the Contact header field of dialog-forming and
      target refresh requests and responses"), and with the plain contact when none is.
- [ ] The `gr` URI parameter (§7) round-trips through the parser and comparison, and a request
      arriving at a GRUU is accepted for the instance it names.
- [ ] The choice between the public and a temporary GRUU is the application's, and the default is
      recorded with its reason. A public GRUU is a stable identifier for the instance; a temporary
      one exists so that, per §5.1, "Given a pair of GRUUs, it MUST be computationally infeasible to
      determine whether they were issued for the same AOR or instance ID or for different AORs" —
      which is a privacy property the caller either wants or does not.
- [ ] Registrar behaviour is explicitly **not** in scope, and the registry entry says so in its
      `Roles` column. sipx obtains and uses GRUUs; minting them is a registrar's job.
- [ ] The RFC registry entry for RFC 5627 moves off "not started" in the same change.
- [ ] Failing-first test: `a_request_to_a_gruu_reaches_the_instance_that_registered_it`.

## Progress
- Not started. `T-14` recorded RFC 5627 as gated on `Path` and Outbound; `Path` is done and Outbound
  is `T-15` in M6.

## Notes
- The temporary GRUU is the half that is usually skipped, and skipping it silently is worse than not
  offering GRUU at all: a caller that believes it has an unlinkable address and does not is worse off
  than one that knows it is using a stable one.
- The instance ID is a URN and must be stable across restarts, or every restart looks like a new
  device to the registrar. Where it is persisted is `T-15`'s decision; this story consumes it.
