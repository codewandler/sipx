# Spec: SRTP, SDES and DTLS-SRTP

**Status:** normative, and **written after the code**. `M-14` built the SRTP transform and SDES,
`M-15` built DTLS-SRTP, and neither wrote the spec [AGENTS.md](../../AGENTS.md) non-negotiable 4
requires of a non-trivial subsystem; `X-25` found the breach and `M-25` is this document. The order
is inverted and it cost something, which §12 records rather than smooths over: writing this found
five places where the code and the RFC disagree — two fixed by `M-25`, a third by `M-26` down to
the one wiring change §12.3 names, two left open with an owner — and the first of them was fatal to
interoperating with anything that is not sipx. That is the
argument for the rule, made backwards. · **Crates:** `sipx-rtp` (the
transform), `sipx-sdp` (SDES, the fingerprint and the offer/answer), `sipx-media` (DTLS-SRTP and the
session) · **Stories:** [M-14](../stories/M-14-secure-media.md),
[M-15](../stories/M-15-dtls-srtp.md), [M-25](../stories/M-25-srtp-spec.md),
[M-26](../stories/M-26-sdes-tag-neither-echoed-nor-verified.md),
[M-28](../stories/M-28-dtls-srtp-unreachable-from-a-call.md),
[M-41](../stories/M-41-negotiate-aead-srtp-protection-profiles.md) · **Design:**
[media](../designs/media.md), [media security profiles](../designs/media-security-profiles.md)

Where this document and the code disagree, this document is right until somebody changes it
deliberately. §12 lists the places they currently disagree and says which story each belongs to.

## 1. Normative references

- **RFC 3711** — SRTP. §3.1 (the SRTP packet), §3.2 (the cryptographic context), §3.3 (packet
  processing), §3.3.1 (the packet index and rollover), §3.3.2 (replay), §3.4 (SRTCP), §4.1.1
  (AES-CM), §4.2 (authentication), §4.2.1 (HMAC-SHA1), §4.3.1 … §4.3.3 (key derivation and the
  PRF), §5.1 … §5.3 (the default transforms and their parameters), §8.2 (the parameter table), §9.2
  (key lifetime), Appendix B (test vectors).
- **RFC 7714** — AES-GCM Authenticated Encryption in SRTP. §7.1 (why the separate authentication
  tag goes away), §8.1 (the SRTP IV), §8.2 (Associated Data, Plaintext and Raw Data in an SRTP
  packet), §8.4 (IV reuse), §9.1 (the SRTCP IV), §9.2/§9.3 (the encrypted and unencrypted SRTCP
  layouts), §10 (the parameter constraints), §11 (which KDF each profile uses), §12 (Tables 1–3),
  §13.2 (why the tag may not be truncated), §14.1 (the SDES crypto-suite names), §14.2 (the
  DTLS-SRTP protection profiles and their identifiers), §16/§17 (the test vectors).
- **RFC 6188** — The Use of AES-192 and AES-256 in SRTP. §2 and §7.1, for `AES_256_CM_PRF`, which
  RFC 7714 §11 requires as the KDF for `AEAD_AES_256_GCM`. Only the PRF is in scope; RFC 6188's own
  counter-mode-plus-HMAC suites are not implemented.
- **RFC 4568** — SDP Security Descriptions (SDES). §4 (the attribute), §5.1.1 … §5.1.3 (offer,
  answer, and processing the answer), §6.1 (`AES_CM_128_HMAC_SHA1_80` and the `inline` parameter),
  §7.1 (why a secure signalling path is a condition of use), §9.1/§9.2 (the ABNF and the examples).
- **RFC 5764** — DTLS Extension to Establish Keys for SRTP. §4.1.1 (`use_srtp`), §4.1.2 (the
  protection profiles and their parameters), §4.2 (the exporter and the key split), §5.1.2
  (demultiplexing on one port), §8 (the `UDP/TLS/RTP/SAVP` token).
- **RFC 5763** — Framework for Establishing an SRTP Security Context Using DTLS. §5 (the offer,
  the answer, and `a=setup`), §6.6 (session modification), §6.7 (the answerer's obligations).
- **RFC 8122** — Connection-Oriented Media Transport over TLS in SDP. §5 (`a=fingerprint`, the
  grammar, and the prohibition on MD5/MD2), §6.2 (the certificate check), §7 (what the mechanism
  does and does not guarantee).
- **RFC 4145** — `a=setup`, borrowed by RFC 5763 §5 to decide who opens the DTLS connection. §4,
  §4.1 (answering a role).
- **RFC 2104** — HMAC. The tag in §4.6 is HMAC-SHA1 as defined there; RFC 3711 §4.2.1 only fixes the
  key, the message and the truncation.
- RFC 3550 §6.1 — the compound-packet rule SRTCP inherits (§4.7).
- RFC 4648 — base64, for the `inline` parameter. RFC 4568 §6.1 cites RFC 3548, which RFC 4648
  obsoletes without changing the alphabet.

**Out of scope, deliberately:**

- **Every transform but three.** AES-f8 (RFC 3711 §4.1.2), the NULL cipher, the 32-bit tag, and
  RFC 6188's counter-mode AES-192 and AES-256 suites. What *is* implemented is RFC 3711's
  `AES_CM_128_HMAC_SHA1_80` and RFC 7714's `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` (§4.10). §5
  says why a short list is a promise rather than a limitation, and why it is still short.
- **RFC 6904 header-extension encryption.** RFC 7714 §8.3 says what an AEAD profile would have to
  do if it were in use; sipx does not encrypt header extensions under any profile, so both the
  encrypted and the unencrypted forms are authenticated Associated Data and nothing more.
- **Truncated AEAD tags.** RFC 7714 §13.2 forbids them and there is nothing to decide.
- **Rekeying, MKI and key lifetimes.** The key derivation rate is fixed at zero (§4.3), so one
  master key produces one set of session keys for the life of the stream. §4.9 states the limits
  that follow and what happens when they are reached.
- **MIKEY (RFC 3830) and any `key-mgmt` key parameter.** SDES's `inline` is the only key parameter
  sipx reads; anything else is refused rather than half-understood (§5.4).
- **Being a relay, an SBC or a mixer that re-keys.** Everything here is endpoint behaviour. A stack
  that terminates SRTP on behalf of somebody else has a different threat model and is not this.
- **`a=connection` and TCP media transport** (RFC 4145's other half). sipx carries media over UDP;
  only `a=setup` is borrowed.

## 2. What SRTP protects, and what it does not

SRTP encrypts the RTP **payload** and authenticates the **whole packet**, header included
(RFC 3711 §3.1). The sequence number, timestamp, SSRC and CSRC list therefore travel in the clear
and cannot be altered without detection. That split is deliberate and load-bearing: a relay has to
read the header to do its job, and the jitter buffer has to read it before the payload is
trustworthy.

What follows from it, and is easy to state wrongly:

- **Traffic analysis is not defended against.** Packet sizes, timing and SSRCs are visible. Silence
  suppression is visible. Who is talking to whom, on a path that can see the media, is visible.
- **Authentication of the *endpoint* is not SRTP's job.** SRTP proves that whoever holds the key
  sent the packet. Which human or certificate that is comes from the keying: from the signalling
  path for SDES (§5), from a certificate fingerprint that also arrived over the signalling path for
  DTLS-SRTP (§6.6). RFC 8122 §7 is explicit that the guarantee is only as good as the integrity of
  the signalling in both cases; DTLS-SRTP improves *confidentiality* against an intermediary, not
  *authentication*.
- **A keyed stream is not a secure call.** The signalling can still be read unless it is over TLS,
  and the SDP still names the addresses.

## 3. Types

| Type | Fields / variants | Crate | Source |
|---|---|---|---|
| `srtp::Profile` | `AesCm128HmacSha1_80`, `AeadAes128Gcm`, `AeadAes256Gcm` | `sipx-rtp` | RFC 3711 §5, RFC 7714 §12 |
| `srtp::Context` | `profile`, `session`, `roc`, `highest_seq`, `replay`, `rtcp_index`, `highest_rtcp_index`, `rtcp_replay` | `sipx-rtp` | §3.2's cryptographic context, one direction |
| `srtp::SrtpError` | `KeyLength`, `TooShort`, `NotAuthentic`, `Replayed`, `ReplayedRtcp` | `sipx-rtp` | §3.3 step 5, §3.4, §4.2 |
| `crypto::Suite` | `AesCm128HmacSha1_80`, `AeadAes128Gcm`, `AeadAes256Gcm` | `sipx-sdp` | RFC 4568 §6.1, RFC 7714 §14.1 |
| `crypto::Crypto` | `tag`, `suite`, `key_and_salt` | `sipx-sdp` | RFC 4568 §4, §9.2 |
| `fingerprint::HashFunc` | `Sha1`, `Sha224`, `Sha256`, `Sha384`, `Sha512` | `sipx-sdp` | RFC 8122 §5 |
| `fingerprint::Fingerprint` | `func`, `digest` | `sipx-sdp` | RFC 8122 §5 |
| `fingerprint::Setup` | `Active`, `Passive`, `ActPass`, `HoldConn` | `sipx-sdp` | RFC 4145 §4 |
| `dtls::Arriving` | `Stun`, `Dtls`, `Rtp`, `Unknown` | `sipx-media` | RFC 5764 §5.1.2 |
| `dtls::Profile` | `Aes128CmHmacSha1_80`, `AeadAes128Gcm`, `AeadAes256Gcm` | `sipx-media` | RFC 5764 §4.1.2, RFC 7714 §14.2 |
| `SrtpKeys` | `profile`, `local`, `remote` | `sipx-media` | the keying seam — §4.1, §5.4, §6.4 |
| `dtls::Role` | `Client`, `Server` | `sipx-media` | RFC 5764 §4.2 |
| `dtls::Keys` | `outbound`, `inbound` | `sipx-media` | RFC 5764 §4.2 |
| `dtls::Handshake` | trait: `run`, `peer_certificate`, `profile`, `export` | `sipx-media` | the seam at RFC 5764 §4.1 |

**A `Context` is one direction of one stream, and MUST stay that way.** RFC 3711 §3.2 keys each
direction separately. A context shared between the two would give two senders one replay window and
one rollover counter, and the replay window would then reject the far end's traffic as a replay of
this end's.

**Key material never reaches a `Debug` output.** `Context`, `Session` and `Crypto` all implement
`Debug` by hand. For a key that arrives in signalling, a log line is the likeliest way it escapes.

**The protection profile is carried, never inferred.** `srtp::Profile` is an argument to
`Context::new`, a field of `SrtpKeys`, and the single value both keying paths map onto —
`crypto::Suite` through `sipx_media::transform_of`, `dtls::Profile` through `Profile::transform`.
Nothing anywhere reconstructs a transform from how many octets of key arrived. Two profiles can
agree on a key length and disagree on everything the key is used for, so a guess is not a guess with
a small blast radius: it is a stream protected by something the far end never agreed to, which is
what [media-runtime-safety](../designs/media-runtime-safety.md)'s negotiation-truth rule forbids
(§12.9).

## 4. The SRTP transforms (RFC 3711, RFC 7714)

### 4.1 Parameters

Three profiles, and **every length below is read off the profile** rather than from a constant.
`Context::new` refuses a master key or salt that is not the length the named profile requires; §12.9
records why that refusal is the load-bearing part.

| Parameter | `AES_CM_128_HMAC_SHA1_80` | `AEAD_AES_128_GCM` | `AEAD_AES_256_GCM` |
|---|---|---|---|
| Encryption transform | AES-128 counter mode | AES-128-GCM | AES-256-GCM |
| Authentication transform | HMAC-SHA1 | the AEAD tag | the AEAD tag |
| Master key length | 128 bits (16 octets) | 128 bits (16 octets) | 256 bits (32 octets) |
| Master salt length | **112 bits (14 octets)** | **96 bits (12 octets)** | **96 bits (12 octets)** |
| `n_e` — session encryption key | 128 bits | 128 bits | 256 bits |
| `n_a` — session authentication key | **160 bits (20 octets)** | not derived | not derived |
| `n_s` — session salt | 112 bits (14 octets) | 96 bits (12 octets) | 96 bits (12 octets) |
| `n_tag` — authentication tag | 80 bits (10 octets) | 128 bits (16 octets) | 128 bits (16 octets) |
| Key derivation function | `AES_CM_PRF` | `AES_CM_PRF` | `AES_256_CM_PRF` |
| `SRTP_PREFIX_LENGTH` | 0 | 0 | 0 |
| Key derivation rate | 0 | 0 | 0 |
| MKI | absent, length 0 | absent, length 0 | absent, length 0 |
| Replay window | 64 | 64 | 64 |
| Sources | RFC 3711 §5.1–§5.3, §8.2 | RFC 7714 §12 Table 2, §14.2 | RFC 7714 §12 Table 3, §14.2 |

`n_a` is bold because it is the value this stack got wrong for two releases; §12.1 records how, and
§10.2 is the vector that now holds it. The **master salt lengths** are bold because they are the
numbers most easily carried across from the profile beside them, and a salt of the wrong length
produces a key schedule that decrypts nothing with no error anywhere to say why.

`n_a` is "not derived" and not "zero" for the AEAD profiles: RFC 7714 §7.1 makes the AEAD tag "the
primary message authentication mechanism", so labels 0x01 and 0x04 (§4.3) derive nothing at all
under them. A key that is never derived cannot leak.

**Ranking, and where it is decided.** `AEAD_AES_256_GCM` > `AEAD_AES_128_GCM` >
`AES_CM_128_HMAC_SHA1_80`. The order lives in `srtp::Profile::strength` and both keying paths defer
to it, so an SDES crypto-suite and a DTLS-SRTP profile naming the same transform cannot be ranked
differently. §7 rule 8 states what the order is *for*.

**`AES_CM_128_HMAC_SHA1_80` is never dropped.** RFC 5764 §4.1.2 makes its DTLS-SRTP counterpart
mandatory to implement, and it is what most of the telephone network can do. It is the floor, not
the default: it is offered last and selected only when nothing better is common.

**Why `AEAD_AES_256_GCM` is implemented and not only the 128-bit one.** RFC 7714 §12 is explicit —
"Any implementation of AES-GCM SRTP MUST support both `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM`" —
so shipping one of the two would be a conformance claim this document could not make. The marginal
cost is one branch on the key length in the KDF and in the AEAD invocation, which is smaller than
the paragraph that would be needed to explain the omission (`M-41`).

### 4.2 Packet layout

RFC 3711's counter-mode layout:

```
 SRTP:  | RTP header | encrypted payload            | tag (10) |
        |<-------------- authenticated ------------>|
                     |<-- encrypted -->|

 SRTCP: | RTCP hdr (8) | encrypted payload | E|index (4) | tag (10) |
        |<---------------- authenticated ------------->|
                       |<-- encrypted --->|
```

RFC 7714's AEAD layout, which is **not the same shape** for SRTCP:

```
 SRTP:  | RTP header | ciphertext + tag (16)        |          (§8.2)
        |<--- AAD -->|<------ encrypted ---------->|

 SRTCP: | RTCP hdr (8) | ciphertext + tag (16) | E|index (4) |   (§9.2, E = 1)
        |<---- AAD --->|                       |<--- AAD --->|

 SRTCP: | RTCP hdr (8) | plaintext body | tag (16) | E|index (4) |  (§9.3, E = 0)
        |<-------------- AAD ---------->|          |<--- AAD --->|
```

**The ESRTCP word moves.** RFC 3711 §3.4 puts the encryption flag and index *before* the
authentication tag; RFC 7714 §9.2 puts them **after** the cipher, because under AEAD the tag is the
last of the ciphertext and the word is Associated Data. A transform that kept RFC 3711's order under
an AEAD profile round-trips against itself and against nothing else — which is why §10.7's vector
asserts the position by offset rather than by round trip.

**Under AEAD every SRTP packet is encrypted** (RFC 7714 §8.2, "All SRTP packets MUST be both
authenticated and encrypted"). SRTCP may be tagged without being encrypted (§9.3); sipx never
*sends* that form and does read it, because a peer may send one and §9.3's associated-data rule for
it differs from §9.2's.

The RTP header is 12 octets plus 4 per CSRC plus, if the X bit is set, 4 octets of extension header
and the extension body it declares. The encrypted portion begins after all of that: the header
extension is authenticated and **not** encrypted (§3.1). Measuring the header from the wrong offset
encrypts part of the header and leaves part of the audio in the clear, and the packet still
round-trips against an implementation that makes the same mistake.

The length declared by the extension is read **fallibly**. This function is handed whatever arrived
on a UDP socket; a declared length longer than the buffer is a typed error and a dropped datagram,
never an index (§10).

### 4.3 Key derivation (§4.3.1, §4.3.2)

One master key and master salt produce six session keys, three for SRTP and three for SRTCP:

| Label | Key | Length |
|---|---|---|
| 0x00 | SRTP encryption `k_e` | `n_e` |
| 0x01 | SRTP authentication `k_a` | `n_a` |
| 0x02 | SRTP salt `k_s` | `n_s` |
| 0x03 | SRTCP encryption | `n_e` |
| 0x04 | SRTCP authentication | `n_a` |
| 0x05 | SRTCP salt | `n_s` |

The derivation, exactly as §4.3.1 states it:

```
r      = index DIV key_derivation_rate          ; 0, since kdr = 0
key_id = <label> || r                           ; 7 octets
x      = key_id XOR master_salt                 ; right-aligned: label lands on octet 7
PRF    = AES-CTR(master_key, x * 2^16)          ; §4.3.3: two null octets on the right
k      = the first n octets of PRF              ; n is n_e, n_a or n_s
```

**The AEAD profiles use the same derivation** (RFC 7714 §11): `AEAD_AES_128_GCM` "MUST use the
(128-bit) `AES_CM` PRF KDF described in [RFC3711]" and `AEAD_AES_256_GCM` "MUST use the
`AES_256_CM_PRF` KDF described in [RFC6188]". RFC 6188 §2 makes that the identical construction with
AES-256 under the counter, so the cipher follows the master key length and nothing else changes.
Labels 0x01 and 0x04 derive nothing under an AEAD profile (§4.1).

**One AEAD parameter is not pinned by any published number, and this is where it is recorded.**
RFC 3711 §4.3.1 words the label placement as right-alignment against a **112-bit** master salt,
which puts the label on octet 7 of the 16-octet PRF input block. RFC 7714 shortens the salt to 96
bits and says nothing about moving anything, and neither RFC publishes a KDF vector for the AEAD
profiles. sipx keeps the block layout fixed: the master salt occupies octets `0..n_s`, the label
exclusive-ors into octet **7**, `r` into octets 8..14, and octets 14 and 15 are null. That is the
reading that changes one thing at a time when the salt shortens. It is the only value in §4 with no
external check behind it; `the_256_bit_kdf_reads_the_whole_master_key` pins the half of it that
*can* be checked without a vector — that both 128-bit halves of a 256-bit master key reach the key
schedule — and §12.10 records the rest as an open interoperability risk rather than a settled fact.

Three details, each of which produces keys that are self-consistent and interoperate with nothing:

1. **Right alignment.** `key_id` is 56 bits and the master salt is 112, so the label exclusive-ors
   into octet **7** of the 14-octet salt and `r` into octets 8..14. Aligning left puts the label on
   octet 0.
2. **`x * 2^16`.** The 14-octet `x` becomes a 16-octet AES input block by padding **on the right**
   with two null octets, not on the left.
3. **`n = n_a`, not 94.** §4.3.1 derives as many octets as the parameter asks for; §5.2 fixes `n_a`
   at 160 bits. §B.3's worked example derives 94 octets because that appendix posits an
   authentication function needing 94, so the PRF is walked through six AES blocks. HMAC accepts a
   key of any length (RFC 2104), so taking 94 for `n_a` raises no error anywhere — see §12.1.

**The key derivation rate is zero, and that is a decision, not a default taken by accident.**
§4.3.1: "for a key_derivation_rate of 0, the application of the key derivation SHALL take place
exactly once." Rekeying mid-stream buys nothing until there is a way to signal it: SDES's `inline`
lifetime field is parsed and not acted on (§5.4), and DTLS-SRTP fixes the rate at zero itself
(RFC 5764 §4.1.2, "the Key Derivation Rate (KDR) is equal to zero"). §4.9 states the limit this
leaves.

### 4.4 The keystream (§4.1.1)

```
IV = (k_s * 2^16) XOR (SSRC * 2^64) XOR (i * 2^16)
```

with `i` the 48-bit packet index (§4.5). Every term is shifted left by at least two octets, so the
low 16 bits of the IV are always zero, and a plain 128-bit big-endian counter is therefore correct:
within one packet the counter cannot carry out of those low 16 bits. RFC 3711 §B.2's own keystream
segment is 65282 blocks, which is inside that bound and demonstrates it.

Laid out over the 16-octet block: `k_s` occupies octets 0..14, the SSRC exclusive-ors into octets
4..8, and the low 48 bits of `i` into octets 8..14.

**Two distinct packets MUST use distinct counter blocks under one key** (§9.1). That is why the
index is 48 bits and why SRTP and SRTCP use different session keys — an SRTCP index of 5 and an SRTP
index of 5 would otherwise produce the same keystream under the same key.

### 4.5 The packet index and the rollover counter (§3.3.1)

The index is 48 bits: a 32-bit rollover counter above the 16-bit RTP sequence number,
`i = 2^16 * ROC + SEQ`. **It is not transmitted.** Both ends infer it, and two ends that infer
differently decrypt to noise from the first wrap onwards — about twenty minutes into a call at
speech packet rates.

**Sender.** The ROC increments when the sequence number wraps past 65535.

**Receiver**, with `s_l` the highest sequence number accepted so far and `v` the ROC to use for this
packet:

| Condition | `v` |
|---|---|
| `s_l < 32768` and `SEQ − s_l > 32768` | `(ROC − 1) mod 2^32` |
| `s_l ≥ 32768` and `s_l − SEQ > 32768` | `(ROC + 1) mod 2^32` |
| otherwise | `ROC` |

`ROC` and `s_l` are updated only when the packet authenticates **and** `i` is greater than the
current index — a packet from the previous cycle, or an old one inside the window, changes neither.

**The subtraction is signed.** §3.3.1 writes `SEQ - s_l > 32768` over two 16-bit values and means
ordinary arithmetic that may go negative. Read as wrapping `u16` subtraction, a packet arriving one
place out of order looks 65535 ahead, is taken for the previous cycle, and fails authentication:
every out-of-order packet in every call, dropped, with no error that says why. `M-14`'s
`## Progress` records getting this wrong first time and three tests catching it.

### 4.6 The authentication tag (§4.2, §4.2.1)

`tag = HMAC-SHA1(k_a, M)` truncated to the leftmost `n_tag` = 80 bits, where

- **SRTP:** `M` = the authenticated portion (the whole packet, header included, after encryption)
  `|| ROC`. The ROC is appended precisely because it is not transmitted: without it a packet from
  before a wrap could be replayed after one.
- **SRTCP:** `M` = the authenticated portion only — the RTCP packet, the E flag and the SRTCP index.
  No ROC; SRTCP's index travels in the packet.

**Authentication happens before decryption and before the replay window is touched** (§3.3 step 5).
A packet that fails it MUST NOT change any state in the context, which is what stops an attacker
advancing the window with forgeries.

**The comparison is constant-time.** The tag is 80 bits and an attacker who can send packets and
observe timing gets a byte-at-a-time oracle otherwise.

**The error says only that the packet is not authentic.** A caller that could tell "wrong key" from
"altered packet" would be an oracle for which of the two an attacker had achieved.

### 4.7 SRTCP (§3.4)

SRTCP appends three mandatory fields to the RTCP compound packet: the E flag (1 bit), the SRTCP
index (31 bits) and the authentication tag. The first eight octets — the RTCP header and the sender
SSRC — are not encrypted; the encrypted portion starts at the ninth octet.

- **The index starts at zero.** §3.4: it "MUST be set to zero before the first SRTCP packet is sent,
  and MUST be incremented by one, modulo 2^31, **after** each SRTCP packet is sent." Incrementing
  first means index 0 is never emitted and every packet's counter block is one off from what a
  reader of the RFC expects. After a re-key it MUST NOT be reset.
- **The E flag says whether the payload is encrypted.** sipx sets it on everything it sends and
  honours it on receipt — a compound packet split per RFC 3550 §9.1 into an encrypted half and a
  cleartext half is legal, and a receiver that ignores the flag decrypts the cleartext half into
  noise.
- **The authentication tag is REQUIRED**, and §5.2 forbids the pre-defined HMAC-SHA1 from being
  applied to SRTCP with an `n_tag` or `n_a` below the defaults. SRTCP therefore has no short-tag
  variant even where SRTP would tolerate one.
- **The RTCP encryption prefix of RFC 3550 §6.1 MUST NOT be used.**
- **The receiver does not estimate the index**; it is explicit.

**Replay protection applies to SRTCP too**, "as defined in Section 3.3.2, but using the SRTCP index
as the index `i` and a separate Replay List that is specific to the SRTCP stream" (§3.4). Separate
is the operative word: SRTP and SRTCP indices are unrelated counters, and one shared list would
reject valid packets of both. `M-47` implements the separate 64-entry list; §12.2 records its
closure and named boundary tests.

### 4.8 Replay (§3.3.2)

Each receiving context keeps a replay list as a sliding window of at least
**SRTP-WINDOW-SIZE = 64** indices, most recent at bit 0.

| Incoming index vs. the window | Outcome |
|---|---|
| ahead of the window | accept; shift the window; record it |
| inside the window, not yet recorded | accept; record it |
| inside the window, already recorded | `Replayed` |
| more than the window behind | `Replayed` |

The last row is the one worth stating: a packet too old to judge is **refused**, not accepted.
Accepting it would make the window a speed bump an attacker only has to wait out.

The list is updated **after** authentication (§3.3.2, "after the packet has been authenticated (if
necessary the window is first moved ahead), the replay list SHALL be updated"), which is the same
ordering §4.6 states from the other side.

### 4.9 Key lifetime (§9.2, §8.2)

One master key protects at most **2^48 SRTP packets** and **2^31 SRTCP packets**. Past those, the
48-bit index or the 31-bit SRTCP index wraps and two packets share a counter block under one key,
which is a two-time pad on the audio.

sipx does not count packets against these limits and has no way to re-key if it did: the derivation
rate is zero (§4.3) and neither keying mechanism sipx implements can signal a new key mid-session
without a new offer/answer. **This is a stated gap, not an oversight.** 2^31 SRTCP packets at one
report every five seconds is longer than any call; 2^48 SRTP packets at 50 packets a second is
about 178 000 years. A story that adds re-keying owes this section a paragraph; nothing before then
does.

### 4.10 The AEAD transforms (RFC 7714)

AES-GCM replaces §4.4's keystream and §4.6's tag with one construction. Nothing else in §4 changes:
the packet index (§4.5), the replay window (§4.8), the SRTCP index (§4.7) and the key lifetime (§4.9)
are profile-independent, and the tests that hold them run over every profile rather than over the one
they were written for.

**The IV is derived from the packet, never drawn** (§8.1 for SRTP, §9.1 for SRTCP):

```
SRTP:   0x0000 || SSRC (4) || ROC (4) || SEQ (2)     XOR session salt (12)
SRTCP:  0x0000 || SSRC (4) || 0x0000  || 0 || index (31 bits)   XOR session salt (12)
```

Two consequences, both stated because they are easy to get wrong:

1. **The SRTCP IV carries the index without the E-flag.** Folding the flag in would give the
   encrypted and unencrypted forms of one index two different keystreams, and neither would be the
   RFC's.
2. **Uniqueness is a property of the sequence numbering, not of a random draw.** RFC 7714 §8.4:
   "the (ROC,SEQ,SSRC) triple is never used twice with the same master key", and reusing one
   "compromises the authentication mechanism" — it is worse than the confidentiality loss the same
   mistake costs counter mode. §4.9's limits are therefore harder under AEAD than under §4, not
   softer, and sipx still does not count against them.

**The associated-data boundary is the header** (§8.2). Version through SSRC, the CSRC list, and any
header extension are authenticated and not encrypted; the payload is the plaintext. That is the same
offset §4.2 measures for counter mode, so `rtp_header_len` is one function with one failure mode
rather than two — and reading it short would hand a CSRC to GCM as plaintext and hide four octets a
relay has to see.

**Authenticate-before-decrypt is held by the construction.** §4.6's ordering rule does not have to be
remembered here: AES-GCM releases no plaintext to a caller whose tag did not verify. The replay
window is still consulted only after that, and still updated only after *it* passes, so a forged
packet moves no state — the same property, proved a different way, and the existing replay tests are
what prove it (§10.7).

**Nothing about the transform is hand-rolled.** AES-GCM comes from the same RustCrypto family as
§4's AES-CM and HMAC (`aes-gcm`, pure Rust, no C dependency). The arithmetic is not the hard part of
SRTP; the framing, the IV and the associated-data boundary are, and those are the parts §10.7
checks against the RFC's own numbers.

## 5. SDES (RFC 4568, RFC 7714 §14.1)

### 5.1 The attribute

```abnf
crypto-attribute = "crypto:" tag SP crypto-suite SP key-params *(SP session-param)
tag              = 1*9DIGIT
key-params       = key-param *(";" key-param)
key-param        = "inline:" <key||salt> ["|" lifetime] ["|" MKI ":" length]
```

`key||salt` is the master key concatenated with the master salt and base64-encoded. **The length is
the suite's**, and a decoded length that is not the one the named suite requires is refused: a short
key that was accepted would be padded or truncated somewhere further down, and both produce a stream
that fails to decrypt for no stated reason.

| Suite | key + salt | base64 characters | Source |
|---|---|---|---|
| `AES_CM_128_HMAC_SHA1_80` | 16 + 14 = 30 | 40 | RFC 4568 §6.1 |
| `AEAD_AES_128_GCM` | 16 + 12 = 28 | 40 | RFC 7714 §12 Table 2, §14.1 |
| `AEAD_AES_256_GCM` | 32 + 12 = 44 | 60 | RFC 7714 §12 Table 3, §14.1 |

The first two encode to the same number of base64 characters and decode to different lengths, which
is exactly why the length is checked against the **named suite** and never used to identify it.

**The suite token is case-sensitive.** RFC 4568 §9.2 defines `AES_CM_128_HMAC_SHA1_80` as a fixed
spelling and RFC 7714 §14.1 registers `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` the same way; a peer
sending another case is not offering that suite.

**Only `inline:` is read.** A `key-param` naming a key-management protocol — `keymgmt:mikey` — is
not a key sipx can use, and an offer carrying only those is refused rather than answered with a
suite that cannot be performed.

### 5.2 Offering

sipx offers **one `a=crypto` line per suite, strongest first**, on an `m=` line whose protocol token
is `RTP/SAVP`:

```
a=crypto:1 AEAD_AES_256_GCM inline:<44 octets>
a=crypto:2 AEAD_AES_128_GCM inline:<28 octets>
a=crypto:3 AES_CM_128_HMAC_SHA1_80 inline:<30 octets>
```

Tags are `1` upward in that order. RFC 4568 §5.1.1 allows several attributes and reads their order
as preference, so ours agrees with the rule §5.3 applies whether or not the far end honours it.

**A single-suite offer is an ultimatum**, which is what this replaces (`M-41`): a peer that cannot
perform the one suite named has to decline the stream, and before this change that peer was every
AEAD-only endpoint.

**Each line carries its own key.** RFC 4568 §6.1's `inline` parameter is per attribute, and reusing
one key across two suites would mean a peer that accepted the weaker one had also been handed the
master secret of the stronger.

**A key is generated only over a secure signalling path.** This is rule 1 of §7 and RFC 4568 §7.1's
condition of use, and it is enforced by the type rather than documented: `Crypto::offer` takes
whether the signalling is secure and returns `None` when it is not, so no caller can publish a key
by forgetting a check. It gates every line, so a cleartext offer carries none of them.

**Every offer gets its own key, from a cryptographic random source.** A generator seeded once, or a
key reused between calls, encrypts and authenticates perfectly and protects nothing.

A deployment that has to narrow the list — a peer that mis-parses an unknown suite token, a policy
that will not carry counter mode — uses `Capabilities::with_srtp_suites`, which takes the suites to
offer and keeps them in strength order regardless of the order they are passed in.

### 5.3 Answering (RFC 4568 §5.1.2)

The answerer MUST accept **exactly one** of the offered crypto attributes or reject the stream —
there is no third option, and in particular there is no answering an `RTP/SAVP` offer in the clear.

The accepted attribute in the answer MUST carry:

- **the tag and crypto-suite from the accepted attribute in the offer.** The suite must be the same
  in both directions. *(Implemented by `M-26` as `Crypto::accepting`, which takes the tag and the
  suite from the accepted offer and keeps this side's own key; §12.3. Asserted by
  `the_answer_echoes_the_tag_of_the_accepted_offer` and
  `the_answer_echoes_the_tag_of_the_suite_it_actually_accepted` in
  [`srtp_negotiation`](../../crates/sipx-sdp/tests/srtp_negotiation.rs).)*
- **the answerer's own key** — the one it will use for media it sends. A key MUST be present
  whatever the direction attributes say.

Where several are offered, RFC 4568 §5.1.2 leaves the choice to the answerer. **sipx selects by
strength and never by the order the offer listed them in** — see §7 rule 8 — over the intersection
of what was offered and what this side generated a key for. Ties, which arise only between two lines
naming the same suite, go to the earlier line, so the peer's order still decides where strength does
not. A stream whose two lists share nothing is declined (rule 4), not answered in the clear.

Accepting a suite this side generated no key for would produce an answer naming a transform and
carrying nothing usable under it, which is why the ranking runs over the intersection rather than
over the offer alone.

### 5.4 Processing the answer (RFC 4568 §5.1.3)

The offerer MUST verify that one of the crypto suites it offered **and its accompanying tag** were
echoed in the answer, and that the answer carries a key. "If any of the above fails, the negotiation
MUST fail." *(Implemented by `M-26` as `Crypto::verify_answer`, which returns the offered attribute
the answer accepted so the caller keys with the half it sent; `SrtpKeys::from_answer` is the only
route from an answer to keys. On the call path since `M-29`: `settle_answer` runs it for both
places an answer can reach a caller — the 200 and the reliable provisional — and `establish`
propagates the refusal; §12.3.)*

**The check belongs to the offerer, and only to the offerer.** Answering is the other half: this
side chose the attribute and echoed its tag (§5.3), so there is nothing there to verify and the two
moments do not share a function. `sipx-call` keeps them apart as `srtp_keys` and
`srtp_keys_answering`, because one that served both would have to decide at run time which side of
the exchange it was on.

**A failed check is an error, not an unkeyed call.** The verification returns
[`SdpError`](../../crates/sipx-sdp/src/lib.rs) rather than `None`, and that choice is the whole
value of the check: the two ways of "failing" that are not an error are both worse than one. A
stream that drops to unencrypted because a tag disagreed hands the user an insecure call presented
as a secure one, and a stream dropped without a reason ends the call with nothing anyone can act on.

**An answer naming a suite that was never offered** reaches this side as an `a=crypto` carrying
nothing sipx can perform, because `Crypto::parse` refuses a suite it cannot key (§5.1). So it
arrives as *no usable attribute at all* rather than as a recognisable wrong one, and the check must
treat an absent attribute as a failure and not as a plain call. That is why `verify_answer` takes an
`Option` and refuses `None`.

**The error never names key material.** It names the tag and the suite. An error string is a log
line waiting to happen (§3), and for a key that arrives in signalling that is the likeliest way it
escapes.

**Both halves or neither.** A session is keyed only when both our key and theirs are present. A
stream keyed at one end connects and carries silence, which is worse than one that fails to
connect — the user hears nothing and no error is raised anywhere.

**The half this side keys with is the one whose suite was accepted.** Since §5.2 offers several,
"our key" is no longer "the one we sent" — it is the one carrying the suite the answer echoed.
`Crypto::verify_answer` returns that attribute rather than a boolean for exactly this reason, and
`Crypto::accepting` refuses a local key whose suite differs from the offered one even when the
lengths happen to agree. **The tag alone is not enough**: matching on it would accept an answer that
kept a number this side recognised and renamed the transform under it, which becomes reachable the
moment an offer carries more than one suite.

**The transform travels with the keys.** `SrtpKeys::from_answer` records the negotiated suite's
transform in `SrtpKeys::profile`, and the media session installs *that*. Before `M-41` the profile
was discarded here and the session had one cipher to fall back on; with three, falling back is
installing a cipher nobody agreed to (§12.9).

### 5.5 What SDES does not do, and what is not implemented

- **The key is in the SDP.** Every element that reads the signalling has held it: every proxy, every
  session border controller that terminates the TLS. RFC 4568 §7.1 treats a secure signalling path
  as a condition of use for exactly this reason, and it is still only hop-by-hop. This is the
  argument for DTLS-SRTP (§6), not a defect in SDES.
- **No MKI.** Parsed past, never generated. An MKI identifies which of several master keys a packet
  used, which is only meaningful with re-keying (§4.9).
- **No key lifetime.** The `|2^20|` field is parsed past and not acted on, for the same reason.
- **No session parameters.** `UNENCRYPTED_SRTP`, `UNAUTHENTICATED_SRTP`, `UNENCRYPTED_SRTCP`,
  `FEC_ORDER`, `KDR`, `WSH`. None is offered and none is honoured. Two of them switch off protection
  this document requires, so honouring them is a decision to be taken deliberately, in a story, and
  not inherited from a peer's offer.
- **No `RTP/SAVPF`.** RFC 5124's feedback profile is a separate token and sipx does not offer or
  answer it.

## 6. DTLS-SRTP (RFC 5763, RFC 5764, RFC 8122)

The two endpoints handshake **on the media path** and derive the SRTP keys from the DTLS master
secret. The signalling carries only a hash of the certificate that will appear. A proxy that
terminates the TLS therefore learns nothing it can decrypt with — though it can still substitute a
fingerprint of its own (RFC 8122 §7), which is why this is a confidentiality improvement over SDES
and not an authentication one.

### 6.1 The SDP half (RFC 5763 §5, RFC 8122 §5)

| Attribute | Level | Meaning |
|---|---|---|
| `m=… UDP/TLS/RTP/SAVP` | media | DTLS-SRTP is the keying (RFC 5764 §8) |
| `a=fingerprint:<hash-func> SP <2UHEX *(":" 2UHEX)>` | session or media | the certificate that will be presented |
| `a=setup:<active\|passive\|actpass\|holdconn>` | media | who opens the DTLS connection (RFC 4145 §4) |

- **A fingerprint is looked for at media level and then at session level.** RFC 8122 §5 permits
  either, and a session-level value applies to every stream that does not override it. A browser
  puts one at the top and none on the `m=` lines, so reading only the media level refuses a
  perfectly good offer.
- **Several fingerprints may be offered**, under different hash functions. The first one sipx may
  act on is taken.
- **MD5 and MD2 are refused at the parser.** §5: implementations "MUST NOT use the MD2 and MD5 hash
  functions to calculate fingerprints or to verify received fingerprints". The grammar admits them;
  returning one would hand a caller a value it is forbidden to act on, so `None` is how the
  prohibition is expressed.
- **A digest whose length does not match the hash named is refused.** A truncated digest that
  compared equal against a prefix would be a check that verifies almost nothing.
- **Hex is uppercase on the way out.** §5's `UHEX` is `DIGIT / %x41-46`. The `hash-func` tokens are
  ABNF string literals and therefore case-insensitive (RFC 5234 §2.3) — §5's own figure writes
  `SHA-256` while the rule spells `sha-256` — so they are parsed case-insensitively and written
  lowercase.

### 6.2 Roles (RFC 5763 §5, RFC 4145 §4.1)

| Offered `a=setup` | Answer | This side is |
|---|---|---|
| `actpass` | `active` | DTLS client |
| `passive` | `active` | DTLS client |
| `active` | `passive` | DTLS server |
| `holdconn` | `holdconn` | neither; no connection is formed |
| absent | `active` | DTLS client, treating the offer as `actpass` |

**An offerer MUST send `actpass`** (RFC 5763 §5). One that sends nothing is not conformant, and
reading its silence as `actpass` — rather than refusing the stream — is what lets the answer name a
role it can hold.

**The role is answered, never copied.** Two endpoints that both say `active` both send a
`ClientHello` and neither answers one; two that both say `passive` wait for each other until the
call times out. Answering `actpass` with `active` is RFC 5763 §5's recommendation and not merely a
preference: the *answerer* starting the handshake means its own `ClientHello` opens the NAT it sits
behind, rather than the offerer sending to an address it has only just learned.

`Role::Client` ⇔ `Setup::Active`, `Role::Server` ⇔ `Setup::Passive`. The mapping decides nothing
about the handshake and everything about the keys (§6.4).

### 6.3 Demultiplexing one port (RFC 5764 §5.1.2)

Three protocols share the media port and are told apart by the **first byte alone**:

| First byte | Protocol |
|---|---|
| 0 … 1 | STUN |
| 20 … 63 | DTLS |
| 128 … 191 | RTP or RTCP |
| anything else | none of the three — dropped by name |

The ranges do not overlap: RTP's version-2 header puts `10` in the top two bits, DTLS content types
are 20–63, and STUN's first two bits are zero. An empty datagram is not RTP. A byte outside all
three ranges is given no meaning by §5.1.2, so it is dropped rather than fed to whichever parser
happens to be first — silently treating a stray datagram as RTP corrupts the sequence state.

This is the same classification ICE relies on ([ice.md](ice.md) §1); it is implemented once, here.

### 6.4 Protection profiles and key derivation (RFC 5764 §4.1.2, §4.2)

sipx offers three profiles in `use_srtp`, **strongest first** (RFC 5764 §4.1.1: the client sends
them "in preference order"):

| | `SRTP_AEAD_AES_256_GCM` | `SRTP_AEAD_AES_128_GCM` | `SRTP_AES128_CM_SHA1_80` |
|---|---|---|---|
| Wire value | `{0x00, 0x08}` | `{0x00, 0x07}` | `{0x00, 0x01}` |
| cipher | AES_256_GCM | AES_128_GCM | AES_128_CM |
| cipher_key_length | 256 bits — 32 octets | 128 bits — 16 octets | 128 bits — 16 octets |
| cipher_salt_length | **96 bits — 12 octets** | **96 bits — 12 octets** | **112 bits — 14 octets**, not 16 |
| auth_function | NULL — the AEAD tag | NULL — the AEAD tag | HMAC-SHA1 / 160 bits |
| `aead_auth_tag_length` | 16 octets | 16 octets | — (80-bit `auth_tag_length`) |
| exported octets | 88 | 56 | 60 |
| maximum_lifetime | 2^31 SRTCP / 2^48 SRTP | 2^31 SRTCP / 2^48 SRTP | 2^31 packets (§4.9) |
| Key Derivation Rate | 0 | 0 | 0 |
| Source | RFC 7714 §14.2 | RFC 7714 §14.2 | RFC 5764 §4.1.2 |

The remaining profiles §4.1.2 defines are not offered: the two NULL profiles encrypt nothing, and
`SRTP_AES128_CM_HMAC_SHA1_32` needs a 32-bit tag the transform in §4 does not implement. **A profile
list is a promise**, so the list holds exactly what `srtp::Profile` can key — derived from that type
rather than written out, and checked by
`every_offered_profile_maps_to_a_transform_that_can_be_keyed`. *(The counter-mode name sipx uses is
OpenSSL's spelling, which is not RFC 5764's; see §12.4.)*

`SRTP_AES128_CM_SHA1_80` stays in the list because §4.1.2 makes it mandatory to implement. It is
offered **last**, which is the whole of the change: last means "if nothing better is common", not
"not offered".

**Key derivation.** The DTLS exporter (RFC 5705) produces
`2 * (master_key_len + master_salt_len)` octets under the label `"EXTRACTOR-dtls_srtp"`, with an
**empty** context value — 60, 56 or 88 depending on the profile agreed. The label is not a choice:
a different one derives different keys and the failure is silent on both sides. **The length is
read off the negotiated profile**, so exporting for one profile and keying for another is not
expressible.

The octets are assigned, in this order (shown for the 60-octet counter-mode case):

```
 0..16   client_write_SRTP_master_key
16..32   server_write_SRTP_master_key
32..46   client_write_SRTP_master_salt
46..60   server_write_SRTP_master_salt
```

**Keys first, then salts** — not key-and-salt per side, which is the natural way to read it. Each
end protects with its own pair and unprotects with the other's; RFC 5764 §4.2 says the peer "MUST
only use these keys to decrypt and to check the authenticity of inbound packets".

`M-15` records why this needs a byte-offset assertion rather than a round trip: the split is applied
identically at both ends, so *any* consistent permutation of the block passes a test that says "what
the client protects, the server unprotects" — including a wrong one. That mutation survived the
whole suite, including a real two-socket handshake, because sipx was talking to sipx. §11.5 is the
vector that replaced it.

### 6.5 The seam (`Handshake`)

sipx does not implement DTLS. Everything RFC 5764 and RFC 8122 *decide* — the fingerprint check, the
profile, the key split, the demultiplexing, the `a=setup` negotiation — is compiled always. Only the
record layer and the handshake sit behind the `Handshake` trait and the off-by-default `dtls`
feature, which is where OpenSSL lives. The default build stays pure Rust.

The trait exposes `peer_certificate()` rather than leaving verification to the implementation,
because §6.6's check is against a value that arrived in the **signalling**, which a DTLS library has
no way to see.

A pure-Rust DTLS was considered and rejected in `M-15`: a hand-rolled handshake for a
security-critical protocol is the liability this project declines elsewhere — the same reasoning
that has SRTP's AES come from an audited implementation rather than from here.

### 6.6 The certificate check (RFC 8122 §6.2)

Ordered, and the order is normative:

1. **No fingerprint from the peer → refuse, before the handshake runs.** An unverified DTLS
   handshake with a self-signed certificate authenticates nobody, and discovering that afterwards
   means having established a channel to an unknown party.
2. Run the handshake in the negotiated role.
3. **No certificate presented → refuse.**
4. **Hash the presented certificate and compare, in constant time, against the fingerprint from the
   SDP.** §6.2 requires an endpoint whose peer's certificate does not match to "terminate the media
   connection with a `bad_certificate` error".
5. **No agreed profile → refuse.**
6. Export, split (§6.4), and only now return keys.

A mismatch returns an error, never a pair of contexts a caller might use anyway. The certificate
chain is deliberately **not** validated by the TLS stack: §5 expects a self-signed certificate, so
there is no chain, and what authenticates it is the fingerprint.

The comparison is constant-time even though the digest is public. It is not about protecting the
digest; it is about not handing an attacker who can offer certificates a byte-at-a-time oracle for
how far a forged one matched.

## 7. Choosing a keying — the eight rules

These are the rules `M-14` and `M-15` settled and then left in two closed story files, plus the one
`M-41` added when there was more than one transform to choose between. They are normative here. Each
names the RFC it comes from and the failure it exists to prevent.

**1. SDES is offered only over a secure signalling path** (RFC 4568 §7.1). The master key *is* the
SDP, so an offer over cleartext SIP publishes it to everyone on the path. Enforced by the signature
of `Crypto::offer`, which returns nothing without the flag — the difference between a stack that has
a rule and one that has a comment.

**2. The `m=` protocol token decides the keying, not the attributes present.** `UDP/TLS/RTP/SAVP` is
DTLS-SRTP (RFC 5764 §8), a bare `RTP/SAVP` is SDES (RFC 4568 §5.1.1), anything else is plain RTP.
The token is what tells the far end which keying to expect; an `RTP/SAVP` line carrying only
`a=fingerprint` describes a stream nobody can key.

**This is also the answer to "which keying wins when both are offered".** One `m=` line carries one
`proto` token (RFC 8866 §5.14), so a stream cannot be *offered* under both mechanisms; what a peer
can do is put both sets of attributes on one stream. Then: the token decides, the other mechanism's
attributes are ignored, and in particular a `UDP/TLS/RTP/SAVP` stream carrying an `a=crypto` line is
keyed by DTLS-SRTP and the `a=crypto` is not read. Offering the same stream twice under two tokens
is SDP capability negotiation (RFC 5939), which sipx neither offers nor answers; a peer that tries
it gets each `m=` line answered on its own terms, and the ones sipx cannot key are rejected with
port 0.

**3. A stream is never offered under both mechanisms.** Selecting DTLS-SRTP clears any `a=crypto`
this side would have offered. Leaving a stale one in place would put a master key in an SDP whose
entire purpose is not to carry one.

**4. A secure offer that cannot be keyed is declined, never answered in the clear.** An `RTP/SAVP`
or `UDP/TLS/RTP/SAVP` stream that sipx cannot key — no usable `a=crypto`, no fingerprint, no local
capability — is answered with port 0. Answering it as `RTP/AVP` would be a downgrade *this side*
chose; answering it secure without a key would negotiate encryption neither end can perform.
RFC 4568 §5.1.2 leaves rejecting the stream as the option, and it is the right one.

**5. A plain offer is answered plainly.** `RTP/AVP` is never answered with `a=crypto`, however much
this side would have preferred a key. That is how a stream ends up encrypted at one end only.

**6. Both halves or neither.** A keyed session needs our key *and* theirs. One key is not a session:
the stream connects, carries silence, and reports no error — worse than a call that fails to
connect, because nothing tells anyone.

**7. A session expecting SRTP refuses plain RTP.** Once a stream is keyed, an unprotected packet
arriving on it is dropped rather than delivered. Accepting it would let an attacker downgrade a call
with one unencrypted packet, which is the whole of the attack.

**8. The transform is chosen by strength, never by peer order** (`M-41`). Both keying paths rank the
profiles both ends named and take the strongest, and both offer their own list strongest first.
This is a deliberate departure from the specified behaviour on the SDES side — RFC 4568 §5.1.1 reads
the offer's order as preference and §5.1.2 leaves the choice to the answerer — and it is the same
departure, for the same reason, that `sipx_sip::auth::strongest` makes from RFC 8760 §2.4 for digest
algorithms. **An `a=crypto` list is not integrity protected before the media is keyed.** An on-path
attacker who reorders the lines picks the cipher, and an answerer that honours the order complies
with him; ranking by strength removes the lever entirely. What it gives up is a preference the peer
has already said it can live without, by offering the alternative at all.

Two corollaries the rule needs to be worth anything:

- **The strongest common profile, not the strongest offered.** Ranking runs over the intersection,
  so a suite this side generated no key for cannot be selected.
- **A weaker profile offered first must not win**, which is the property under test rather than
  the implementation's own opinion of itself:
  `a_weaker_suite_offered_first_does_not_win` in
  [`srtp_negotiation`](../../crates/sipx-sdp/tests/srtp_negotiation.rs).

Rules 1, 5, 6 and 7 come from `M-14`; rule 3 from `M-15`; rules 2 and 4 are the negotiation `M-14`
built and `M-15` extended; rule 8 from `M-41`. All eight have tests, and `M-14` and `M-15` each
record mutation-testing
them: offering a key over cleartext, accepting plain RTP on an encrypted session, skipping the
replay window, skipping the fingerprint check, splitting the exported block per side, ignoring the
`a=setup` role, changing the exporter label, answering a DTLS offer that carried no fingerprint, and
copying the role instead of answering it each fail a test.

## 8. State

The per-direction SRTP context (§3.2) holds exactly this, and nothing here is optional:

| Field | Meaning | Changed by |
|---|---|---|
| six session keys | derived once from the master key (§4.3) | never, after construction |
| `ROC` | the high 32 bits of the packet index (§4.5) | a sender's wrap; a receiver accepting a packet ahead of `s_l` |
| `s_l` | highest sequence number accepted | as above |
| replay list | 64-bit window over accepted indices (§4.8) | a receiver accepting an authenticated packet |
| SRTCP index | 31 bits, this side's SRTCP counter (§4.7) | incremented after each SRTCP packet sent |

**Nothing in this table may be changed by an unauthenticated packet.** That single sentence is what
stops an off-path attacker who can reach the media port from desynchronising a call by spraying
forgeries: no rollover inference, no window advance, no index update happens before the tag
verifies.

The negotiation state above it is not stored in the context. Which keying a stream uses, and the
keys it was given, are settled once by the offer/answer (§5, §6) and handed to the media session; a
change of keying requires a new offer/answer.

## 9. What must not happen

Everything in §4 and §6.3 eats unauthenticated datagrams from whoever can reach the media port.
The workspace lints (AGENTS.md non-negotiable 3) are the floor, not the ceiling:

- No `unwrap`, no `expect`, no raw indexing, no length arithmetic that can wrap. A malformed packet
  is a typed error and a dropped datagram, never a panic — a stack that panics on a 4-octet
  datagram has a remote denial of service, not a parsing bug.
- A declared length is never trusted against the buffer that carries it (§4.2).
- Key material never reaches `Debug`, a log, or an error message (§3).
- Tag and fingerprint comparisons are constant-time (§4.6, §6.6).
- An error must not distinguish *why* authentication failed (§4.6).

## 10. Test vectors

Tests are derived from these, not from the implementation. Each row says where the vector comes
from and — because this spec was written after the code — whether the existing test was **derived**
from it or **reconciled** with it by `M-25`.

| # | Vector | Source | Test | Provenance |
|---|---|---|---|---|
| 1 | AES-CM keystream, three blocks | RFC 3711 §B.2 | `the_keystream_matches_the_rfc` | derived — the RFC's session key, its pre-shifted salt, and its three published keystream blocks |
| 2 | Key derivation: cipher key, cipher salt, 94-octet auth block | RFC 3711 §B.3 | `key_derivation_matches_the_rfc` | derived — reconciled by `M-25` only in what it claims: it tests the PRF, not `n_a` |
| 3 | `n_a` = the first 160 bits of §B.3's block | RFC 3711 §5.2, §8.2, §B.3 | `the_session_authentication_key_is_the_160_bits_the_rfc_fixes` | **new** (`M-25`) — the vector §12.1 was found by |
| 4 | HMAC-SHA1 tag, both forms of `M` | RFC 3711 §4.2.1 + §B.3's key + §B.1's header and ROC; HMAC computed off-stack | `the_authentication_tag_is_hmac_sha1_over_the_packet_and_the_roc` | **new** (`M-25`) |
| 5 | The exported block splits keys before salts | RFC 5764 §4.2's stated offsets | `the_exported_block_splits_keys_before_salts`, `the_server_protects_with_what_the_client_unprotects_with`, `the_two_roles_do_not_send_with_the_same_key` | derived — asserted by position, after a per-side split survived a round-trip test |
| 6 | Profile parameters: 16/14 octets, 60 exported, id 0x0001 | RFC 5764 §4.1.2 | `the_profile_asks_for_the_key_and_salt_sizes_the_rfc_states` | derived |
| 7 | Exporter label `EXTRACTOR-dtls_srtp` | RFC 5764 §4.2 | `the_exporter_label_is_the_one_the_rfc_fixes` | derived |
| 8 | §5.1.2's demultiplexing ranges, at every boundary | RFC 5764 §5.1.2 | `one_port_tells_stun_dtls_and_rtp_apart_by_the_first_byte` | derived |
| 9 | base64 of the `inline` parameter | RFC 4648 §10 | `base64_matches_the_published_vectors` | derived |
| 10 | The first SRTCP packet carries index 0 | RFC 3711 §3.4 | `the_first_srtcp_packet_carries_index_zero` | **new** (`M-25`) — the vector §12.5 was found by |
| 11 | The published `a=crypto` line and the 16 + 14 octets it decodes to | RFC 4568 §6.1, §9.1 (§10.4 below) | `the_published_crypto_line_parses_to_the_published_key_and_salt`, `the_other_published_inline_parameters_are_read` | **new** (`M-26`) — the first thing `Crypto::parse` has been held against that sipx did not write |
| 12 | AES-GCM SRTP: encryption, decryption and a flipped bit, under both key sizes | RFC 7714 §16.1.1, §16.1.2, §16.2.1, §16.2.2 (§10.7 below) | `the_srtp_encryption_vectors_are_reproduced`, `the_srtp_decryption_vectors_are_reproduced`, `an_altered_rfc_packet_does_not_verify` | **new** (`M-41`) — recovered from the RFC, not transcribed |
| 13 | AES-GCM SRTCP, both encrypted and tagged-only, including the ESRTCP word's position | RFC 7714 §17.1 … §17.4 | `the_srtcp_encryption_vector_is_reproduced`, `the_srtcp_decryption_vector_is_reproduced`, `the_srtcp_tagging_only_vectors_are_reproduced` | **new** (`M-41`) |
| 14 | AEAD profile parameters: 16/12 and 32/12 octets, 16-octet tag, ids 0x0007 and 0x0008 | RFC 7714 §12, §14.2 | `the_aead_profiles_carry_the_lengths_the_rfc_tabulates`, `the_aead_profiles_carry_the_names_and_ids_rfc_7714_registers` | **new** (`M-41`) |
| 15 | A key or salt of another profile's length is refused by name | RFC 7714 §12, RFC 3711 §8.2 | `a_key_or_salt_of_another_profiles_length_is_refused_by_name` | **new** (`M-41`) — §12.9 |

The numbers themselves, so a reader can check a test without opening an RFC.

### 10.1 RFC 3711 §B.2 — AES-CM keystream

```
session key   2B7E151628AED2A6ABF7158809CF4F3C
session salt  F0F1F2F3F4F5F6F7F8F9FAFBFCFD          (SSRC and index both zero)
counter       F0F1F2F3F4F5F6F7F8F9FAFBFCFD0000  ->  E03EAD0935C95E80E166B16DD92B4EB4
              F0F1F2F3F4F5F6F7F8F9FAFBFCFD0001  ->  D23513162B02D0F72A43A2FE4A5F97AB
              F0F1F2F3F4F5F6F7F8F9FAFBFCFD0002  ->  41E95B3BB0A2E8DD477901E4FCA894C0
```

The RFC gives the salt already shifted, so with SSRC and index zero the IV *is* that offset — which
is what makes this a usable vector for §4.4's IV construction and not only for AES.

### 10.2 RFC 3711 §B.3 — key derivation

```
master key    E1F97A0D3E018BE0D64FA32C06DE4139
master salt   0EC675AD498AFEEBB6960B3AABE6

label 0x00 -> cipher key   C61E7A93744F39EE10734AFE3FF7A087
label 0x02 -> cipher salt  30CBBC08863D8C85D49DB34A9AE1
label 0x01 -> CEBE321F6FF7716B6FD4AB49AF256A15 6D38BAA48F0A0ACF3C34E2359E6CDBCE
              E049646C43D9327AD175578EF7227098 6371C10C9A369AC2F94A8C5FBCDDDC25
              6D6E919A48B610EF17C2041E47403576 6B68642C59BBFC2F34DB60DBDFB2
```

The label-0x01 block is 94 octets because §B.3 says so about its own example. **`k_a` is its first
20**: `CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4`. Both facts are asserted, separately, and the
distinction between them is §12.1.

### 10.3 RFC 3711 §4.2.1 — the tag

With `k_a` from §10.2, `M` = §B.1's published RTP header `806E5CBA50681DE55C621599` and §B.1's
published ROC `D462564A`:

```
SRTP  (M || ROC)  2E19C5351B7F99278F33
SRTCP (M alone)   66126DD7550B7E7C90A4
```

Both are HMAC-SHA1 truncated to `n_tag`, computed with an implementation outside this repository.
A tag that agrees only with `authenticate` would prove nothing about either.

### 10.4 RFC 4568 §6.1 and §9.1 — the `inline` parameter

The published example line, and what it decodes to:

```
a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj|2^20|1:4

master key   774466766726542B2978473740666235      (16 octets)
master salt  6A552C5261417D5C7C7030252A23          (14 octets)
```

Two more from the same RFC, both 30 octets and both legal input:
`PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR` (§4) and
`YUJDZGVmZ2hpSktMbW9QUXJzVHVWd3l6MTIzNDU2|1066:4` (§6.1, showing the lifetime field's default form).

**The `|2^20|1:4` suffix must not stop the key being read** (§5.1): the lifetime and MKI are parsed
past, and a parser that treats them as part of the base64 sees a key of the wrong length and refuses
a valid offer. *(Asserted by `M-26`: `the_published_crypto_line_parses_to_the_published_key_and_salt`
and `the_other_published_inline_parameters_are_read` in
[`crypto`](../../crates/sipx-sdp/src/crypto.rs). `Crypto::parse` agreed with the published octets
already — the point is that until then nothing said so. §10.6 is still unasserted; see §12.6.)*

### 10.5 RFC 5764 §4.2 — the exported block

60 octets, split at 0/16/32/46. Asserted by position, because a per-side split is structurally valid
and decrypts nothing.

### 10.6 RFC 8122 §5 — a fingerprint line

```
a=fingerprint:SHA-256 12:DF:3E:5D:49:6B:19:E5:7C:AB:4A:AD:B9:B1:3F:82:
                      18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:4A:AD
a=fingerprint:SHA-1   4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB
```

32 octets and 20 octets respectively, matching the hash each names, with the `hash-func` in the
case §5's own figure uses rather than the case its rule spells. *(Not yet asserted against
`Fingerprint::parse`; see §12.6.)*

### 10.7 RFC 7714 §16 and §17 — the AES-GCM vectors

**These are the only vectors in this document that are not written out here, and that is the
point.** RFC 7714's worked examples run to nine hundred lines of hex; transcribing a subset would
put the same class of error into the fixture that the fixture exists to catch. Instead
`scripts/import-rfc7714-corpus.sh` fetches the RFC and slices §16 and §17 into
`crates/sipx-testkit/corpus/rfc7714/`, stripping the running page header and footer and nothing
else, and `crates/sipx-rtp/src/srtp/rfc7714_vectors.rs` reads the RFC's own labelled lines out of
those files. This is what `import-rfc4475-corpus.sh` does for the SIP torture corpus, adapted to a
document with no embedded archive to recover.

**What proves the vectors were not hand-edited** is `--check`: it re-slices from the RFC editor and
diffs against the tree, and it is a gate step, so a fixture nudged into agreement with an
implementation that disagreed with it fails before a story can be called done. Unreachable network
exits `EX_TEMPFAIL`, which the gate reads as a step disclaiming its own run rather than as a finding
— a provenance check that *passed* when it could not read the RFC would be worse than none.

What the vectors pin, and could not be pinned any other way:

- **The IV formation** (§8.1, §9.1) and the **associated-data boundary** (§8.2, §9.2, §9.3). Both
  are self-consistent when wrong: two endpoints running the same wrong code protect and unprotect
  each other's packets perfectly, and every round-trip test in the crate passes.
- **The ESRTCP word's position** (§9.2), asserted by offset. See §4.2 for why it moved.
- That the tag is load-bearing: a flipped bit anywhere in the RFC's own encrypted packet — header,
  ciphertext or tag — must not authenticate.

What they do **not** pin is §11's key derivation, because RFC 7714 §16 publishes session keys and no
master key. §4.3 says what sipx does there and §12.10 records it as the one open parameter.

## 11. Where the code goes

| Piece | Crate | Why there |
|---|---|---|
| The transforms (§4) | `sipx-rtp`, [`srtp`](../../crates/sipx-rtp/src/srtp/mod.rs) | Pure bytes over a packet. No clock, no socket. |
| The RFC 7714 vectors (§10.7) | `sipx-rtp`, [`srtp::rfc7714_vectors`](../../crates/sipx-rtp/src/srtp/rfc7714_vectors.rs) | Reaching past key derivation is a thing a test may do and an API may not |
| SDES (§5) | `sipx-sdp`, [`crypto`](../../crates/sipx-sdp/src/crypto.rs) | An SDP attribute is parsing |
| Fingerprint and `a=setup` (§6.1, §6.2) | `sipx-sdp`, [`fingerprint`](../../crates/sipx-sdp/src/fingerprint.rs) | Likewise |
| Which keying, per stream (§7) | `sipx-sdp`, [`answer`](../../crates/sipx-sdp/src/answer.rs) | Offer/answer is a pure function |
| Demultiplexing, profiles, key split, the check (§6.3–§6.6) | `sipx-media`, [`dtls`](../../crates/sipx-media/src/dtls/mod.rs) | Where the media socket is |
| The DTLS handshake | `sipx-media`, `dtls::openssl`, behind the `dtls` feature | The only part that needs a C library |
| Keying a live session, both halves or neither (rule 6) | `sipx-call` | Where the offer and the answer meet |

The last row is where policy becomes a live stream. §12.8 records the boundary `M-28` closed:
`sipx-call` now selects either mechanism explicitly, retains the selected mechanism's keying state,
and starts the session only after both directional keys exist.

The split follows [vision.md](../vision.md) principle 1 and the same line ICE draws
([ice.md](ice.md) §15): grammar and pure transforms below, sockets and handshakes above.

## 12. Where the code and this document currently disagree

Writing a spec after the implementation is worth doing mostly for this section. Each entry says what
the RFC requires, what the code does, and which story owns closing the gap.

### 12.1 The session authentication key was 94 octets — **fixed by `M-25`**

`SESSION_AUTH_LEN` was 94. RFC 3711 §5.2 fixes `n_a` at 160 bits and §8.2 lists it as both
mandatory-to-support and the default; §4.3.1 derives `n = n_a` and states no length of its own. The
94 is §B.3's, which posits an authentication function needing 94 octets so its worked example walks
the PRF through six AES blocks.

The misreading is silent in every direction. HMAC-SHA1 accepts a key of any length (RFC 2104), so
nothing errors; both ends of a sipx-to-sipx call derive the same wrong key, so every round-trip
test, every mutation test and the real two-socket DTLS handshake all pass; and no conformant peer
authenticates a single packet in either direction, which looks exactly like a network problem.

`M-14`'s `## Progress` warns about precisely this failure mode — "a key derivation that is wrong but
self-consistent gives two endpoints that interoperate perfectly with each other and with nothing
else in the world" — and then reproduced §B.3's vector correctly while drawing the wrong conclusion
from it. Reproducing a vector is not the same as reading the parameter it was published under.

Fixed in `M-25` with §10.2's and §10.3's vectors. **Wire-visible:** a sipx built after that commit
does not interoperate with one built before it. Neither interoperated with anything else.

### 12.2 SRTCP replay list — **fixed by `M-47`; closed**

§4.7 and RFC 3711 §3.4 require replay protection over the SRTCP index, with a **separate** list from
SRTP's. `unprotect_rtcp` now authenticates first, refuses a repeated or too-old explicit SRTCP
index, decrypts, and only then advances that separate window. A forged high index therefore cannot
move trusted state, and SRTP sequence traffic cannot consume an SRTCP replay bit.

It is RECOMMENDED rather than MUST, and the exposure is smaller than SRTP's — an attacker replays
reception statistics, not audio, though a replayed receiver report can drive a congestion-control
response. `an_authenticated_srtcp_packet_is_accepted_once` pins the primary refusal;
`srtp_and_srtcp_have_separate_replay_windows`, the forged-index, too-old and wrap tests pin the
state boundaries. The typed result is `SrtpError::ReplayedRtcp(index)`.

### 12.3 The SDES tag is neither echoed nor verified — **echo fixed by `M-26`, check wired by `M-29`; closed**

RFC 4568 §5.1.2: an accepted crypto attribute in the answer "MUST contain … the tag and
crypto-suite from the accepted crypto attribute in the offer". §5.1.3: the offerer "MUST verify that
one of the initially offered crypto suites and its accompanying tag were accepted and echoed in the
answer … If any of the above fails, the negotiation MUST fail."

sipx did neither. The answerer emitted its **own** `Crypto`, whose tag `Capabilities::with_srtp`
fixes at 1, so an offer of `a=crypto:2 …` was answered `a=crypto:1 …`; and the offerer read the
answer's crypto without comparing tags at all.

The visible failure is one-sided and easy to misread: a conformant peer that offers any tag but 1
MUST fail the negotiation on sipx's answer, and calls to peers that happen to use tag 1 — which is
most of them — work. Two MUSTs, one interop bug, one missing check.

**§5.1.2 is closed.** `answer()` now takes the tag and suite from the attribute it accepted
(§5.3). **Wire-visible**, and in the direction that fixes rather than breaks: a peer that offered
tag 2 and was answered tag 1 was failing the call at its end.

**§5.1.3 is closed too.** `Crypto::verify_answer` (§5.4) and `SrtpKeys::from_answer` implement the
check; `M-29` moved `sipx-call`'s `srtp_keys`
([`offer_answer.rs`](../../crates/sipx-call/src/call/offer_answer.rs)) onto
it. It now takes the offered attributes as a **slice** and the answered one as an `Option`, and
returns `Result` — the shape the check has, rather than a pair of `Option`s that unwrapped both and
compared nothing. `Ok(None)` survives for exactly one case, a call that offered no key at all; an
answer to an offer that *did* carry one is refused unless its tag and suite are ours and it carries
a key, and the refusal reaches the application as `Error::Sdp` naming the tag that came back.

*(Recorded by `M-26`, 2026-07-29, which implemented both halves in `sipx-sdp` and `sipx-media` and
could not reach `sipx-call`: that crate was outside its write set and held by a concurrent story.
Closed by `M-29`, whose failing-first test is
`an_answer_echoing_a_tag_that_was_never_offered_fails_the_call` in
[`secure_media`](../../crates/sipx-call/tests/secure_media.rs) — a whole call over WSS, answered by
a peer that echoes tag 9 with a perfectly well-formed key. It connected before that change, which
is the point: nothing about that answer is malformed except that it agrees to an offer nobody made,
and a check on the key material alone sees nothing wrong with it.)*

The gap this pair of stories leaves behind is not about SDES. `M-26` shipped a check whose only
caller was its own test suite, and `docs/compliance.md` could not tell that from a shipped one —
which is `M-28`'s pattern, and the reason the registry note for RFC 4568 carried a **"Still
missing"** sentence until this landed rather than claiming the MUSTs end to end.

### 12.4 The protection profile is named in OpenSSL's spelling — open

`Profile::as_str()` returns `SRTP_AES128_CM_SHA1_80` and its documentation says that is "the name as
the IANA registry and every DTLS API spell it". The IANA *DTLS-SRTP Protection Profiles* registry and
RFC 5764 §4.1.2 both spell 0x0001 `SRTP_AES128_CM_HMAC_SHA1_80`; the string in the code is OpenSSL's.

Nothing on the wire is wrong — the wire carries `id()`, which is `0x0001` — and the string is
correct for the one implementation that consumes it. But `Handshake` is a public trait whose reason
for existing is a second DTLS implementation, and an implementor reading that doc comment and
looking the name up in the registry will not find a match. **Owner: a new story**, in `sipx-media`:
either rename to the registry's spelling and map to OpenSSL's at the call site, or keep the string
and stop the doc claiming it is IANA's.

`M-41` did **not** close this and deliberately did not widen it. The two AEAD names it added,
`SRTP_AEAD_AES_128_GCM` and `SRTP_AEAD_AES_256_GCM`, are RFC 7714 §14.2's own spellings and happen
to be what the DTLS library uses as well, so the discrepancy is now confined to the one
counter-mode row rather than being a pattern. Renaming that row is still a behaviour change for the
`dtls` feature and belongs to the story that owns it.

### 12.5 The SRTCP index started at 1 — **fixed by `M-25`**

§3.4 states the order as a MUST: zero before the first packet, incremented **after** each one.
`protect_rtcp` incremented first, so the first packet carried 1 and index 0 was never emitted. No
interoperability effect — the index travels explicitly in the trailer — but it selects the SRTCP
keystream's counter block, so which packet uses which is not a free choice. Fixed in `M-25` with
§10's vector 10.

### 12.6 Two published SDP vectors are stated here and not yet asserted — **half closed by `M-26`**

§10.4's `a=crypto` lines and §10.6's `a=fingerprint` lines are published, byte-level, and would test
`Crypto::parse` and `Fingerprint::parse` against something other than their own output. Both parsers
were tested only against values this stack generated — round trips, plus negative cases.

**§10.4 is asserted** (`M-26`, vector 11). `Crypto::parse` reproduces the published key and salt
exactly, so the finding is that the parser was right — which is the outcome this kind of test has
most of the time and is not a reason to skip it: §12.1 is what the same blind spot cost in
`sipx-rtp`, and nothing distinguished the two cases beforehand.

**§10.6 is still unasserted.** `Fingerprint::parse` is tested only against its own output.
**Owner: a new story**, in `sipx-sdp`.

### 12.7 Everything else was checked and agrees

So the list above is a finding and not a sample. Checked against the RFC text rather than against
the code's stated intent, and found correct: the key-derivation label assignment and right
alignment (§4.3); the `x * 2^16` padding; the IV construction and the claim that a 128-bit counter
cannot carry within a packet (§4.4); the signed rollover inference and its update rule (§4.5); the
authenticated-then-decrypted ordering and the ROC's place in `M` (§4.6); the SRTCP authenticated and
encrypted portions and the E-flag handling (§4.7); the 64-entry replay window including the refusal
of anything older than it (§4.8); the header-length arithmetic across CSRCs and the extension
(§4.2); §5.1.2's demultiplexing boundaries (§6.3); the profile's 16/14 octet lengths, its 60-octet
export and its `0x0001` id (§6.4); the exporter label and empty context (§6.4); the exported block's
offsets (§6.4); the `a=setup` answer table including `holdconn` (§6.2); RFC 8122 §5's MD5/MD2
prohibition, its digest-length rule, its uppercase-hex output and its case-insensitive `hash-func`
(§6.1); §6.2's ordering, including refusing a peer with no fingerprint before the handshake runs
(§6.6); and all seven rules of §7 against RFC 4568 §5.1.2, §7.1 and RFC 5764 §8.

### 12.8 DTLS-SRTP reaches an initial call — closed by `M-28`

`Keying` is the application choice shared by `DialOptions` and the answering `MediaPolicy`.
`Keying::Sdes` is the default and preserves the old wire behaviour: SDES over protected signalling,
plain RTP over clear signalling. `Keying::DtlsSrtp` emits `UDP/TLS/RTP/SAVP`, a fresh per-call
SHA-256 certificate fingerprint and `a=setup`; it never appears merely because a Cargo feature is
enabled. A build without the `dtls` feature returns `Error::DtlsUnavailable` for that selection and
does not substitute SDES or plain RTP.

**Which keying a call uses is the application's decision and cannot be inferred.** §5.2's rule 1
makes SDES conditional on secure signalling because the master key *is* the SDP; DTLS-SRTP carries
only a hash and is therefore the keying that survives a path sipx does not control. A stack that
picked for the application would be picking between two different threat models on its behalf.

The call retains the identity that produced its offer or answer until keying. `dtls::Keys` retains
the exported directional master key and salt pairs as well as its public contexts, so the verified
result moves directly into `Config::srtp`; there is no second export or unchecked re-split.
`MediaPort::key_with_dtls` duplicates the descriptor for the bounded blocking handshake and returns
the original descriptor to Tokio before media workers start. Thus DTLS and RTP use the exact port
the SDP named, without a bind-drop-rebind race.

The final-answer ordering is normative. A UAS sends its 200 before beginning the handshake. A UAC
validates the answer, sends ACK, and only then begins the handshake. This permits a peer to defer
media setup until the SIP exchange completes without deadlocking: neither ClientHello nor the wait
for one can hold the ACK. A failed UAC handshake tears down the acknowledged dialog with BYE.

The implementation closes the four boundaries the earlier audit found:

1. application policy and a stable default;
2. feature-gated per-call identity with a typed refusal in builds that lack it;
3. verified exported keys entering the session on the already-bound socket; and
4. final response and ACK preceding the handshake.

Reliable early media remains deliberately unavailable for DTLS-SRTP and returns
`Error::DtlsEarlyMedia` before emitting an offer or answer. Its PRACK ordering is the same class of
problem as the ACK fixed here and needs a separate state-machine change, not a path that keys before
the provisional leaves. DTLS-SRTP plus ICE is likewise refused until the selected candidate, rather
than the provisional SDP address, can be the connected handshake peer. Neither refusal falls back.

### 12.9 The profile was discarded at the keying seam — **fixed by `M-41`**

Before `M-41` there was half a seam. `crypto::Suite` and `dtls::Profile` were both single-variant
enums that already carried `key_and_salt_len()`, so the shape of a negotiated profile existed — and
then `SrtpKeys` kept only two pairs of bytes, and `sipx-rtp::srtp` had no notion of a profile at
all: its cipher was a type alias, its lengths were `pub const`, and `derive()` took fixed-size
arrays. With one transform this was invisible. With three it is the failure the whole story is
about, because it is the one that produces a *working* stream: a context keyed for the wrong
transform under the right negotiated name protects and unprotects its own traffic perfectly.

What now holds instead:

1. **`srtp::Profile` is an argument to `Context::new`**, and the key and salt are measured against
   it. `a_key_or_salt_of_another_profiles_length_is_refused_by_name` runs every profile against
   every other profile's key length and every other profile's salt length and requires each to be
   refused **by name**.
2. **`SrtpKeys::profile` carries the negotiated transform** from either keying path into the
   session, and `srtp_context` takes the whole `SrtpKeys` rather than a bare key pair, so the
   profile and the material it belongs to cannot be separated at a call site.
3. **Two functions, one per keying path**, are the only places a name becomes a cipher:
   `sipx_media::transform_of` for an RFC 4568 crypto-suite and `dtls::Profile::transform` for an
   RFC 5764 profile. `every_offered_profile_maps_to_a_transform_that_can_be_keyed` checks that the
   DTLS list round-trips through its transforms, agrees with them on lengths, and can actually be
   keyed.
4. **`Crypto::accepting` refuses a local key whose suite differs from the offered one**, and
   §5.1.3's check is on tag *and* suite. `AES_CM_128_HMAC_SHA1_80` and `AEAD_AES_128_GCM` decode
   from `inline` parameters of the same base64 length (§5.1), so length is not identity.

No `pub const` describing one profile's lengths is load-bearing any more. `MASTER_KEY_LEN`,
`MASTER_SALT_LEN` and `TAG_LEN` remain exported and documented as belonging to
`AES_CM_128_HMAC_SHA1_80` specifically, with the profile accessor named as the thing to prefer;
`MAX_TAG_LEN` is new, for a caller sizing a buffer that must hold any profile's packet.

### 12.10 The AEAD key derivation has no published vector — open, and stated rather than hidden

RFC 7714 §16 publishes *session* keys, so §11's KDF is the one part of the AEAD profiles nothing
external pins. §4.3 states exactly what sipx does — master salt left-aligned in the 16-octet PRF
input block, label on octet 7, AES-256 under the counter for `AEAD_AES_256_GCM` — and why that
reading was chosen. The consequence is bounded and worth naming: **if it is wrong, two sipx
endpoints interoperate with each other and neither interoperates with anybody else**, which is the
same failure shape §12.1 describes and is invisible to every round-trip test.

What is checked today: `the_256_bit_kdf_reads_the_whole_master_key` proves both halves of a 256-bit
master key reach the key schedule, which rules out an AES-128 PRF silently applied to the first
sixteen octets. What is not: the salt alignment. **Owner: a new story**, and its evidence has to be
an interoperability run against an independent AEAD implementation rather than another test in this
repository — `crates/sipx-cli/tests/interop_srtp.rs` is where that would land, and it is
`#[ignore]`d out of the local gate for the usual reason.

### 12.11 The MTU refusal was re-derived and is not affected — `M-41`

`M-41`'s design named this as a risk: AEAD changes the relationship between plaintext and
ciphertext length, so every path that sizes a buffer from a payload length needed auditing and the
MTU refusal in `crates/sipx-transport/src/endpoint.rs` had to be re-derived rather than assumed to
still hold. The audit was done and the answer is that **it is orthogonal**, which is recorded here
because "we looked and there was nothing" is a finding and a silent absence is not.

- The refusal is `UNKNOWN_PATH_MTU_REQUEST_LIMIT` (1300) and `PATH_MTU_HEADROOM` (200), and both
  come from **RFC 3261 §18.1.1** — a limit on *SIP requests over UDP*, on the signalling socket.
  Nothing in its derivation reads an SRTP figure, and media does not pass through it.
- Nothing outside `sipx-rtp` referenced `TAG_LEN`, `MASTER_KEY_LEN` or `MASTER_SALT_LEN` at all.
- `protect` and `protect_rtcp` return a freshly grown `Vec`; no caller writes a tag into a buffer it
  sized itself, under any profile.
- The receive buffers are 2048 octets (`session.rs`, `browser.rs`, `rtp_echo.rs`). A 20 ms G.711
  SRTP packet is `12 + 160 + 10 = 182` octets under counter mode and 188 under AEAD; the browser
  component's `MAX_DATAGRAM` admission ceiling is 2048 and its outbound 1200-octet bound applies to
  DTLS handshake records, which SRTP does not pass through.

The six-octet difference is therefore real on the wire and reaches no bound anywhere. The two places
that *did* hardcode the 80-bit tag were assertions in `crates/sipx-media/tests/srtp.rs`, and both
now read `profile.tag_len()` and run over every profile.
