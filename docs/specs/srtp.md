# Spec: SRTP, SDES and DTLS-SRTP

**Status:** normative, and **written after the code**. `M-14` built the SRTP transform and SDES,
`M-15` built DTLS-SRTP, and neither wrote the spec [AGENTS.md](../../AGENTS.md) non-negotiable 4
requires of a non-trivial subsystem; `X-25` found the breach and `M-25` is this document. The order
is inverted and it cost something, which §12 records rather than smooths over: writing this found
five places where the code and the RFC disagree — two fixed by `M-25`, three left open with an owner
— and the first of them was fatal to interoperating with anything that is not sipx. That is the
argument for the rule, made backwards. · **Crates:** `sipx-rtp` (the
transform), `sipx-sdp` (SDES, the fingerprint and the offer/answer), `sipx-media` (DTLS-SRTP and the
session) · **Stories:** [M-14](../stories/M-14-secure-media.md),
[M-15](../stories/M-15-dtls-srtp.md), [M-25](../stories/M-25-srtp-spec.md) · **Design:**
[media](../designs/media.md)

Where this document and the code disagree, this document is right until somebody changes it
deliberately. §12 lists the places they currently disagree and says which story each belongs to.

## 1. Normative references

- **RFC 3711** — SRTP. §3.1 (the SRTP packet), §3.2 (the cryptographic context), §3.3 (packet
  processing), §3.3.1 (the packet index and rollover), §3.3.2 (replay), §3.4 (SRTCP), §4.1.1
  (AES-CM), §4.2 (authentication), §4.2.1 (HMAC-SHA1), §4.3.1 … §4.3.3 (key derivation and the
  PRF), §5.1 … §5.3 (the default transforms and their parameters), §8.2 (the parameter table), §9.2
  (key lifetime), Appendix B (test vectors).
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

- **Every transform but one.** AES-192 and AES-256, AES-f8 (RFC 3711 §4.1.2), the NULL cipher, the
  AEAD suites of RFC 7714, and the 32-bit tag. §5 says why a short list is a promise rather than a
  limitation.
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
| `srtp::Context` | `session`, `roc`, `highest_seq`, `replay`, `rtcp_index` | `sipx-rtp` | §3.2's cryptographic context, one direction |
| `srtp::SrtpError` | `KeyLength`, `TooShort`, `NotAuthentic`, `Replayed` | `sipx-rtp` | §3.3 step 5, §4.2 |
| `crypto::Suite` | `AesCm128HmacSha1_80` | `sipx-sdp` | RFC 4568 §6.1 |
| `crypto::Crypto` | `tag`, `suite`, `key_and_salt` | `sipx-sdp` | RFC 4568 §4, §9.2 |
| `fingerprint::HashFunc` | `Sha1`, `Sha224`, `Sha256`, `Sha384`, `Sha512` | `sipx-sdp` | RFC 8122 §5 |
| `fingerprint::Fingerprint` | `func`, `digest` | `sipx-sdp` | RFC 8122 §5 |
| `fingerprint::Setup` | `Active`, `Passive`, `ActPass`, `HoldConn` | `sipx-sdp` | RFC 4145 §4 |
| `dtls::Arriving` | `Stun`, `Dtls`, `Rtp`, `Unknown` | `sipx-media` | RFC 5764 §5.1.2 |
| `dtls::Profile` | `Aes128CmHmacSha1_80` | `sipx-media` | RFC 5764 §4.1.2 |
| `dtls::Role` | `Client`, `Server` | `sipx-media` | RFC 5764 §4.2 |
| `dtls::Keys` | `outbound`, `inbound` | `sipx-media` | RFC 5764 §4.2 |
| `dtls::Handshake` | trait: `run`, `peer_certificate`, `profile`, `export` | `sipx-media` | the seam at RFC 5764 §4.1 |

**A `Context` is one direction of one stream, and MUST stay that way.** RFC 3711 §3.2 keys each
direction separately. A context shared between the two would give two senders one replay window and
one rollover counter, and the replay window would then reject the far end's traffic as a replay of
this end's.

**Key material never reaches a `Debug` output.** `Context`, `Session` and `Crypto` all implement
`Debug` by hand. For a key that arrives in signalling, a log line is the likeliest way it escapes.

## 4. The SRTP transform (RFC 3711)

### 4.1 Parameters

The default transform, and the only one. Every value is RFC 3711 §5's default and §8.2's
mandatory-to-support value; none of them is sipx's choice.

| Parameter | Value | Source |
|---|---|---|
| Encryption transform | AES-128 counter mode | §4.1.1, §5.1 |
| Authentication transform | HMAC-SHA1 | §4.2.1, §5.2 |
| Master key length | 128 bits (16 octets) | §5.3, §8.2 |
| Master salt length | 112 bits (14 octets) | §5.3, §8.2 |
| `n_e` — session encryption key | 128 bits (16 octets) | §8.2 |
| `n_a` — session authentication key | **160 bits (20 octets)** | §5.2, §8.2 |
| `n_s` — session salt | 112 bits (14 octets) | §5.3, §8.2 |
| `n_tag` — authentication tag | 80 bits (10 octets) | §5.2, §8.2 |
| `SRTP_PREFIX_LENGTH` | 0 | §4.2.1, §5.2 |
| Key derivation rate | 0 | §8.2, and §4.3 below |
| MKI | absent, length 0 | §8.2 |
| Replay window | 64 | §3.3.2's minimum |

`n_a` is bold because it is the value this stack got wrong for two releases; §12.1 records how, and
§11.2 is the vector that now holds it.

### 4.2 Packet layout

```
 SRTP:  | RTP header | encrypted payload            | tag (10) |
        |<-------------- authenticated ------------>|
                     |<-- encrypted -->|

 SRTCP: | RTCP hdr (8) | encrypted payload | E|index (4) | tag (10) |
        |<---------------- authenticated ------------->|
                       |<-- encrypted --->|
```

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
PRF    = AES-128-CTR(master_key, x * 2^16)      ; §4.3.3: two null octets on the right
k      = the first n octets of PRF              ; n is n_e, n_a or n_s
```

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
reject valid packets of both. *(Not implemented; see §12.2.)*

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

## 5. SDES (RFC 4568)

### 5.1 The attribute

```abnf
crypto-attribute = "crypto:" tag SP crypto-suite SP key-params *(SP session-param)
tag              = 1*9DIGIT
key-params       = key-param *(";" key-param)
key-param        = "inline:" <key||salt> ["|" lifetime] ["|" MKI ":" length]
```

`key||salt` is the master key concatenated with the master salt and base64-encoded — for
`AES_CM_128_HMAC_SHA1_80`, 16 + 14 = 30 octets, which base64 expands to 40 characters. A decoded
length that is not 30 is refused: a short key that was accepted would be padded or truncated
somewhere further down, and both produce a stream that fails to decrypt for no stated reason.

**The suite token is case-sensitive.** RFC 4568 §9.2 defines `AES_CM_128_HMAC_SHA1_80` as a fixed
spelling; a peer sending another case is not offering this suite.

**Only `inline:` is read.** A `key-param` naming a key-management protocol — `keymgmt:mikey` — is
not a key sipx can use, and an offer carrying only those is refused rather than answered with a
suite that cannot be performed.

### 5.2 Offering

sipx offers exactly one `a=crypto` line, with `tag = 1` and suite `AES_CM_128_HMAC_SHA1_80`, on an
`m=` line whose protocol token is `RTP/SAVP`.

**A key is generated only over a secure signalling path.** This is rule 1 of §7 and RFC 4568 §7.1's
condition of use, and it is enforced by the type rather than documented: `Crypto::offer` takes
whether the signalling is secure and returns `None` when it is not, so no caller can publish a key
by forgetting a check.

**Every offer gets its own key, from a cryptographic random source.** A generator seeded once, or a
key reused between calls, encrypts and authenticates perfectly and protects nothing.

### 5.3 Answering (RFC 4568 §5.1.2)

The answerer MUST accept **exactly one** of the offered crypto attributes or reject the stream —
there is no third option, and in particular there is no answering an `RTP/SAVP` offer in the clear.

The accepted attribute in the answer MUST carry:

- **the tag and crypto-suite from the accepted attribute in the offer.** The suite must be the same
  in both directions. *(The tag is not echoed today; see §12.3.)*
- **the answerer's own key** — the one it will use for media it sends. A key MUST be present
  whatever the direction attributes say.

Where several are offered, the answerer selects the first valid one it supports. sipx supports one
suite, so "the first valid one" is the first `a=crypto` naming `AES_CM_128_HMAC_SHA1_80` with a
30-octet `inline` key.

### 5.4 Processing the answer (RFC 4568 §5.1.3)

The offerer MUST verify that one of the crypto suites it offered **and its accompanying tag** were
echoed in the answer, and that the answer carries a key. "If any of the above fails, the negotiation
MUST fail." *(The tag is not verified today; see §12.3.)*

**Both halves or neither.** A session is keyed only when both our key and theirs are present. A
stream keyed at one end connects and carries silence, which is worse than one that fails to
connect — the user hears nothing and no error is raised anywhere.

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

sipx offers exactly one profile in `use_srtp`:

| | Value |
|---|---|
| IANA / RFC 5764 §4.1.2 name | `SRTP_AES128_CM_HMAC_SHA1_80` |
| Wire value | `{0x00, 0x01}` |
| cipher | AES_128_CM |
| cipher_key_length | 128 bits — 16 octets |
| cipher_salt_length | **112 bits — 14 octets**, not 16 |
| auth_function / auth_key_length | HMAC-SHA1 / 160 bits |
| auth_tag_length | 80 bits |
| maximum_lifetime | 2^31 packets (§4.9) |
| Key Derivation Rate | 0 |

The other three profiles §4.1.2 defines are not offered: the two NULL profiles encrypt nothing, and
`SRTP_AES128_CM_HMAC_SHA1_32` needs a 32-bit tag the transform in §4 does not implement. **A profile
list is a promise**, so the list is short. *(The name sipx uses is OpenSSL's spelling, which is not
this one; see §12.4.)*

**Key derivation.** The DTLS exporter (RFC 5705) produces
`2 * (master_key_len + master_salt_len)` = **60** octets under the label `"EXTRACTOR-dtls_srtp"`,
with an **empty** context value. The label is not a choice: a different one derives different keys
and the failure is silent on both sides.

The 60 octets are assigned, in this order:

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

## 7. Choosing a keying — the seven rules

These are the rules `M-14` and `M-15` settled and then left in two closed story files. They are
normative here. Each names the RFC it comes from and the failure it exists to prevent.

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

Rules 1, 5, 6 and 7 come from `M-14`; rule 3 from `M-15`; rules 2 and 4 are the negotiation `M-14`
built and `M-15` extended. All seven have tests, and `M-14` and `M-15` each record mutation-testing
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
a valid offer. *(These lines are not yet asserted against `Crypto::parse`; see §12.6.)*

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

## 11. Where the code goes

| Piece | Crate | Why there |
|---|---|---|
| The transform (§4) | `sipx-rtp`, [`srtp`](../../crates/sipx-rtp/src/srtp.rs) | Pure bytes over a packet. No clock, no socket. |
| SDES (§5) | `sipx-sdp`, [`crypto`](../../crates/sipx-sdp/src/crypto.rs) | An SDP attribute is parsing |
| Fingerprint and `a=setup` (§6.1, §6.2) | `sipx-sdp`, [`fingerprint`](../../crates/sipx-sdp/src/fingerprint.rs) | Likewise |
| Which keying, per stream (§7) | `sipx-sdp`, [`answer`](../../crates/sipx-sdp/src/answer.rs) | Offer/answer is a pure function |
| Demultiplexing, profiles, key split, the check (§6.3–§6.6) | `sipx-media`, [`dtls`](../../crates/sipx-media/src/dtls/mod.rs) | Where the media socket is |
| The DTLS handshake | `sipx-media`, `dtls::openssl`, behind the `dtls` feature | The only part that needs a C library |
| Keying a live session, both halves or neither (rule 6) | `sipx-call` | Where the offer and the answer meet |

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

### 12.2 SRTCP has no replay list — open

§4.7 and RFC 3711 §3.4 require replay protection over the SRTCP index, with a **separate** list from
SRTP's. `unprotect_rtcp` authenticates and decrypts and keeps no list, so a captured SRTCP packet can
be replayed for as long as the key lives.

It is RECOMMENDED rather than MUST, and the exposure is smaller than SRTP's — an attacker replays
reception statistics, not audio, though a replayed receiver report can drive a congestion-control
response. It is a behaviour change to a public method (`unprotect_rtcp` gains a `Replayed` return),
so it is a story rather than something to fold into this one. **Owner: a new story.**

### 12.3 The SDES tag is neither echoed nor verified — open

RFC 4568 §5.1.2: an accepted crypto attribute in the answer "MUST contain … the tag and
crypto-suite from the accepted crypto attribute in the offer". §5.1.3: the offerer "MUST verify that
one of the initially offered crypto suites and its accompanying tag were accepted and echoed in the
answer … If any of the above fails, the negotiation MUST fail."

sipx does neither. The answerer emits its **own** `Crypto`, whose tag `Capabilities::with_srtp`
fixes at 1, so an offer of `a=crypto:2 …` is answered `a=crypto:1 …`; and the offerer reads the
answer's crypto without comparing tags at all.

The visible failure is one-sided and easy to misread: a conformant peer that offers any tag but 1
MUST fail the negotiation on sipx's answer, and calls to peers that happen to use tag 1 — which is
most of them — work. Two MUSTs, one interop bug, one missing check. **Owner: a new story**, in
`sipx-sdp` and `sipx-call`.

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

### 12.5 The SRTCP index started at 1 — **fixed by `M-25`**

§3.4 states the order as a MUST: zero before the first packet, incremented **after** each one.
`protect_rtcp` incremented first, so the first packet carried 1 and index 0 was never emitted. No
interoperability effect — the index travels explicitly in the trailer — but it selects the SRTCP
keystream's counter block, so which packet uses which is not a free choice. Fixed in `M-25` with
§10's vector 10.

### 12.6 Two published SDP vectors are stated here and not yet asserted — open

§10.4's `a=crypto` lines and §10.6's `a=fingerprint` lines are published, byte-level, and would test
`Crypto::parse` and `Fingerprint::parse` against something other than their own output. Today both
parsers are tested only against values this stack generated — round trips, plus negative cases.
`M-25`'s write set did not extend to `sipx-sdp`, so they are recorded here as the vectors a test
should use rather than added. **Owner: a new story**, in `sipx-sdp`.

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
