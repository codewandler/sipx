---
title: RFC compliance
description: What sipx implements, what it only parses, and what it has not started — measured by a CI-checked registry, not asserted.
---

# RFC compliance

sipx tracks its own standards coverage as a **measurement, not a claim**: a registry lists 65
RFCs, each marked with one of five statuses, and CI regenerates the published table from the
registry and fails the build when a claim does not hold — a header the table says sipx parses
must actually be known to the parser, and a file an entry cites must exist.

**[The full table lives in the repository](https://github.com/codewandler/sipx/blob/main/docs/compliance.md)**,
where it is regenerated on every change. What the statuses mean:

| Status | Meaning |
|---|---|
| ✅ implemented | Behaviour present and tested for the roles listed |
| 🟡 partial | Some of the normative behaviour; the note says which part is missing |
| 🔤 syntax only | The parser represents it losslessly; nothing acts on it |
| ⬜ not started | Tracked as a target, not started |
| — superseded | Obsoleted by a later RFC that is tracked instead |

Two of those deserve a word:

**Syntax only is a feature, not an apology.** A parsed message borrows the bytes it arrived in,
so a header sipx has no behaviour for survives intact and re-serializes byte for byte. The
statuses exist to keep that honest: passing something through unharmed is not the same as
supporting it, and the table refuses to blur the two.

**Partial entries say which part.** RFC 3711 (SRTP) is *partial* because there is one transform,
no rekeying, and SDES keying places the key where a TLS-terminating intermediary can read it.
The point of the note is that you learn this here, not in production.

The order in which the remaining gaps close — and the reasoning — is the
[RFC roadmap](https://github.com/codewandler/sipx/blob/main/docs/rfc-roadmap.md).
