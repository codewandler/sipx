---
id: M-73
title: Align the DTLS profile names with the IANA registry
pillar: Media
status: in-progress
priority: 30
design: docs/designs/media-security-profiles.md
epic: media-security-profiles
areas: [sipx-media, docs]
predicate:
announcement:
note: the counter-mode DTLS profile carries OpenSSL's spelling rather than the registry's; M-41 added registry-correct names beside it
---

# Align the DTLS profile names with the IANA registry

## Goal

Make the DTLS-SRTP protection profile names in the tree the ones the registry defines, so the two
spellings now sitting side by side do not become a permanent inconsistency.

## Acceptance

- [x] The counter-mode profile is named as the IANA registry names it, matching the AEAD profiles
      `M-41` added from RFC 7714 §14.2.
- [x] `docs/specs/srtp.md` §12.4 no longer records the divergence as open.
- [x] A failing-first test pins the wire-visible name, and the change is stated as a behaviour
      change for the `dtls` feature in `CHANGELOG.md` with migration guidance.
- [ ] `./scripts/gate.py` green, and the interop suite is run or its absence stated.

## Progress

- 2026-08-08: filed from `M-41`'s adjacent findings. It deliberately did not widen §12.4 — the two
  names it added are RFC 7714's own — and recorded that renaming the counter-mode row is a
  behaviour change belonging to whoever owns that section.

- 2026-08-08: implemented. `dtls::Profile::as_str` returns RFC 5764 §4.1.2's
  `SRTP_AES128_CM_HMAC_SHA1_80` for `0x0001`, so all three offered profiles now carry the registry's
  spelling. §12.4 took the first of the two options it recorded — rename and translate at the call
  site — because the second leaves one row of a public enum asserting a library's vocabulary and the
  two beside it asserting IANA's, with nothing to tell a reader which is which.

  **The translation is two-way and lives only in `dtls::openssl`.** `openssl_name` maps `0x0001` to
  the shorter spelling on the way out and `from_openssl_name` inverts it on the way back;
  `Profile::parse` is deliberately *not* lenient, so a library's name is `None` there and refuses
  rather than guessing at a transform. The failure this had to avoid is not a downgrade:
  `set_tlsext_use_srtp` rejects the **whole** profile list on one unknown name, which would leave
  every DTLS-SRTP call with no protection profile at all, in both roles.

  Failing-first: `the_counter_mode_profile_is_spelled_as_the_registry_spells_it`
  (`crates/sipx-media/src/dtls/mod.rs`), which asserted `left: "SRTP_AES128_CM_SHA1_80"` against
  `right: "SRTP_AES128_CM_HMAC_SHA1_80"` at the merge base. It asserts `id() == 0x0001` beside the
  name, which is the whole compatibility argument: **the wire did not move.**

  Interop: **the third-party evidence was not produced.**
  `two_endpoints_key_srtp_by_handshaking_on_the_media_path`
  (`crates/sipx-media/tests/dtls_srtp.rs`) keys a real handshake between two real sockets locally,
  and `openssl_accepts_every_name_in_the_offered_list` asserts the offered list against OpenSSL's
  own table rather than a transcribed one — but both ends are sipx.
  `a_real_peer_accepts_media_sipx_encrypted_with_dtls_srtp`
  (`crates/sipx-cli/tests/interop_srtp.rs`) is `#[ignore]`d and needs a container peer, so no
  independent implementation has confirmed the renamed profile since the change.

- **For `CHANGELOG.md`** (the coordinator writes it; this story must not):

  > **The DTLS-SRTP protection profile names are the IANA registry's.** `dtls::Profile::as_str`
  > returns `SRTP_AES128_CM_HMAC_SHA1_80` for `0x0001`, as RFC 5764 §4.1.2 and the *DTLS-SRTP
  > Protection Profiles* registry spell it, rather than the shorter spelling OpenSSL's option table
  > uses; the two AEAD names `M-41` added were already RFC 7714 §14.2's own, so the three now agree.
  > **Nothing on the wire changes** — RFC 5764 §4.1.1 carries the identifier, and `0x0001` is
  > unchanged and asserted — and no call negotiates differently: the shorter spelling is translated
  > at the OpenSSL boundary in both directions.
  >
  > *Behaviour change, `dtls` feature, source-compatible but not behaviour-compatible for one case.*
  > `Profile::parse` now accepts the registry's spelling only. Code that fed a DTLS library's own
  > name to it — which only a third-party implementor of the public `Handshake` trait can be doing —
  > gets `None`, and therefore a refused call rather than a mis-keyed one. **Migration:** match on
  > the `Profile` variant rather than on the string, or keep your library's names in your own
  > `Handshake` implementation and translate there — `dtls::openssl` does exactly that internally,
  > in both directions. Callers that only display `as_str()` see the new spelling.
