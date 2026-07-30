---
title: Security
description: What sipx protects today, how the CLI differs from the Rust libraries, and what remains out of reach.
---

# Security

sipx separates signalling security from media security. TLS and secure WebSocket protect a
signalling hop. SRTP protects RTP and RTCP. One does not imply the other unless the application
selects a call path that provides both.

## Capability matrix

| Capability | `sipx` CLI | Rust libraries |
|---|---|---|
| UDP and TCP signalling | Yes | Yes, through `sipx-transport` |
| TLS and secure WebSocket (WSS) | Yes, selected explicitly with `--transport tls` or `--transport wss` | Yes. `sipx-transport` supports TLS and WSS when their Cargo features are enabled |
| Certificate verification | Mandatory. Platform roots, additional PEM roots, service identity and optional mutual-TLS identity are configurable; verification cannot be disabled | Mandatory for outgoing TLS and WSS; there is no skip-verification option |
| SDES-keyed SRTP | Yes. A CLI call over TLS or WSS selects the call layer's protected-signalling path | Yes. `sipx-call` negotiates SDES-keyed SRTP when the selected signalling transport is secure |
| DTLS-SRTP | No | The SDP, fingerprint checking, handshake, and SRTP context pieces exist in `sipx-sdp` and `sipx-media`, but no `sipx-call` API can select or complete a DTLS-keyed call |
| Signalling capture | `--capture <FILE>` writes a redacted pcapng file | `sipx-transport::CaptureConfig` enables capture; redaction is on by default |

For a protected command-line call, select TLS or WSS and provide any private trust root explicitly;
the result reports both requested and negotiated transport. Media remains SDES-keyed SRTP until the
call-level DTLS story lands.

## TLS and WSS protect one hop

A `sips:` URI restricts sipx to TLS-capable transports. It does not promise end-to-end signalling
confidentiality: each SIP intermediary may terminate one TLS connection and open another, as
described by [RFC 3261 §26.2.2](https://www.rfc-editor.org/rfc/rfc3261#section-26.2.2). WSS applies
the same TLS policy before the WebSocket upgrade; it is not a weaker, separate certificate path.

sipx validates the certificate chain, expiry, signature, and peer identity. The identity is the
SIP domain sipx set out to reach, not an address returned by DNS. A mismatch, expired certificate,
or unknown issuer ends the connection; sipx does not retry over cleartext. Applications may choose
trust anchors and may configure a client certificate, but cannot disable verification. TLS 1.2 is
the minimum and TLS 1.3 is preferred, following [RFC 8996](https://www.rfc-editor.org/rfc/rfc8996).

## SDES and SRTP

SRTP encrypts the RTP payload and authenticates the packet. sipx currently reaches SRTP from the
call layer through SDES: the master key is carried in SDP, so sipx offers it only when signalling
uses TLS or WSS. A TLS-terminating intermediary can still read that SDP key. This is a property of
SDES's threat model, not end-to-end media keying; see
[RFC 4568 §7.1](https://www.rfc-editor.org/rfc/rfc4568#section-7.1).

The implemented SRTP transform is AES counter mode with a 128-bit key and HMAC-SHA1 with an 80-bit
authentication tag. Rekeying and the other SRTP transforms are not implemented. The
[RFC compliance table](compliance.md) records these limits alongside the supported portions of
RFC 3711 and RFC 4568.

## DTLS-SRTP is not reachable from a call

DTLS-SRTP performs its handshake on the media path and carries only the certificate fingerprint
in signalling. That prevents a signalling intermediary from learning the media key merely by
terminating TLS.

sipx implements the component pieces: SDP fingerprints and setup roles, certificate fingerprint
verification, the DTLS handshake behind the `sipx-media` `dtls` feature, and derivation of SRTP
contexts. Those pieces are not wired into `sipx-call`: no dial or answer option selects them, and
no CLI command can reach them. Enabling the Cargo feature does **not** turn an ordinary call into a
DTLS-keyed call.

## Captures remain sensitive

The CLI's `--capture` option records SIP signalling, not media. It redacts digest responses,
opaque authorization tokens, SDES keys, SDP `k=` values, push tokens, and instance identifiers.
TLS and WSS traffic is captured after decryption so that the signalling remains diagnosable.

Redaction does not anonymize a call. Names, SIP addresses, network addresses, timing, routes, and
other call metadata remain. Treat every capture as sensitive data: store it with restricted
permissions, share it only with intended recipients, and delete it when the investigation ends.
The library can disable capture redaction for tightly controlled diagnostics; the CLI deliberately
does not expose that option.

For operational symptoms and capture commands, see [Troubleshooting](../guides/troubleshooting.md).
