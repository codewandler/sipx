# Browser-audio proof harness

This directory owns the independent native-browser proof specified in
[`docs/specs/browser-audio-proof.md`](../../docs/specs/browser-audio-proof.md). The page uses only
browser APIs. It does not contain sipx protocol or media code.

`run.sh` owns every background process as a process group, caps each stdout/stderr file at 1 MiB,
and applies a five-minute outer failure bound. It receives an ephemeral localhost certificate and
key, the exact SPKI pin, a WebDriver command, the compiled `browser_audio_proof` example, and an
evidence directory through the `SIPX_BROWSER_AUDIO_*` environment variables used by `ci.sh`.

The example is a real public-API consumer in `sipx-call`. For browser-offerer it answers the native
INVITE normally. For browser-answerer the browser first sends OPTIONS as a bounded readiness fact;
the endpoint responds, then dials over that exact authenticated inbound WSS connection. Each run
uses an ephemeral WSS port parsed from the example's bounded listening object and injected into the
browser configuration. No fixed port or shell-evaluated command string is involved.

CI runs the native browser in both roles, repeats the offerer role with one bounded unused RTCP
fallback candidate in its browser-authored offer, and runs three more native sessions for
fingerprint, nomination and weaker-media failures. The compatibility run must still report one
nominated component and protected audio in both directions. `scripts/test-browser-audio-proof.py`
separately reverses the measuring instrument's identity, structured-evidence, completeness,
output-cap and process-tree boundaries. The real proof and self-test are different claims, and both
must pass.

For the terminating browser-to-ordinary-SIP seam, run `run.sh --coupling-only` with the same
identity, browser, proof-binary, media-address and evidence-directory inputs. This mode does not
need the perturbed-KDF binary. It runs a native browser offer without `100rel`, couples it to a
separate UDP/G.711 peer, and validates audio at both far endpoints. One case hangs up from the
browser, the other from the ordinary peer. Each case retains `browser.json`, `sipx.json` and
`proof.json`; the ordinary peer's PCM peak is explicitly identified as the audio observation.
A third case corrupts the offered fingerprint and requires post-answer BYE on the browser
dialog, target teardown without audio, and the typed fingerprint error.
