#!/usr/bin/env python3
"""Drive and validate the independent native-browser audio peer for M-51."""

from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import ipaddress
import json
import pathlib
import socket
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


CONTRACT = "sipx.browser-audio.v1"
MAX_EVIDENCE_BYTES = 1024 * 1024
ROLES = ("browser-offerer", "browser-answerer")
MAX_IDENTIFIER_CHARS = 256
MAX_ERROR_CHARS = 4096


class ProofError(RuntimeError):
    """A harness or evidence boundary failed closed."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProofError(message)


def pin_bytes(pin: str) -> bytes:
    try:
        decoded = base64.b64decode(pin, validate=True)
    except ValueError as error:
        raise ProofError("WSS SPKI pin is not canonical base64") from error
    require(len(decoded) == 32, "WSS SPKI pin must contain one SHA-256 digest")
    require(any(decoded), "WSS SPKI pin must not be all zero")
    return decoded


def spki_pin(certificate: bytes, inform: str) -> str:
    public_key = subprocess.run(
        ["openssl", "x509", "-inform", inform, "-pubkey", "-noout"],
        input=certificate,
        check=True,
        capture_output=True,
    ).stdout
    encoded = subprocess.run(
        ["openssl", "pkey", "-pubin", "-outform", "DER"],
        input=public_key,
        check=True,
        capture_output=True,
    ).stdout
    return base64.b64encode(hashlib.sha256(encoded).digest()).decode("ascii")


def preflight_certificate(path: pathlib.Path, hostname: str, expected_pin: str) -> None:
    pin_bytes(expected_pin)
    certificate = path.read_bytes()
    checked = subprocess.run(
        ["openssl", "x509", "-checkhost", hostname, "-noout"],
        input=certificate,
        capture_output=True,
    )
    require(checked.returncode == 0, f"WSS certificate does not cover {hostname}")
    require(
        spki_pin(certificate, "PEM") == expected_pin,
        "WSS certificate public-key pin differs from the expected pin",
    )


def preflight_wss(url: str, ca: pathlib.Path, expected_pin: str, timeout: float) -> None:
    parsed = urllib.parse.urlparse(url)
    require(parsed.scheme == "wss", "browser signalling URL must use wss")
    require(bool(parsed.hostname), "browser signalling URL must name a DNS identity")
    require(not _is_ip_literal(parsed.hostname or ""), "WSS identity must not be an IP literal")
    port = parsed.port or 443
    context = ssl.create_default_context(cafile=str(ca))
    deadline = time.monotonic() + timeout
    last_error: OSError | ssl.SSLError | None = None
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((parsed.hostname, port), timeout=1.0) as raw:
                with context.wrap_socket(raw, server_hostname=parsed.hostname) as secured:
                    certificate = secured.getpeercert(binary_form=True)
                    require(
                        spki_pin(certificate, "DER") == expected_pin,
                        "live WSS public-key pin differs from the issued identity",
                    )
                    return
        except (OSError, ssl.SSLError) as error:
            last_error = error
            time.sleep(0.05)  # poll interval: the socket/TLS handshake is the readiness condition
    raise ProofError(f"WSS identity preflight did not connect before its deadline: {last_error}")


def _is_ip_literal(value: str) -> bool:
    try:
        import ipaddress

        ipaddress.ip_address(value)
        return True
    except ValueError:
        return False


def load_json(path: pathlib.Path) -> Any:
    require(path.stat().st_size <= MAX_EVIDENCE_BYTES, f"evidence exceeds {MAX_EVIDENCE_BYTES} bytes")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ProofError(f"malformed JSON evidence in {path}") from error


def _mapping(value: Any, name: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{name} must be an object")
    return value


def _positive_number(value: Any, name: str) -> float:
    require(isinstance(value, (int, float)) and not isinstance(value, bool), f"{name} must be numeric")
    require(value > 0, f"{name} must be positive")
    return float(value)


def _bounded_string(value: Any, name: str, maximum: int = MAX_IDENTIFIER_CHARS) -> str:
    require(isinstance(value, str) and bool(value), f"{name} must be a non-empty string")
    require(len(value) <= maximum, f"{name} exceeds {maximum} characters")
    return value


def _socket_address(value: Any, name: str) -> tuple[ipaddress.IPv4Address | ipaddress.IPv6Address, int]:
    rendered = _bounded_string(value, name)
    host: str
    port_text: str
    if rendered.startswith("["):
        closing = rendered.find("]")
        require(closing > 1 and rendered[closing + 1 : closing + 2] == ":", f"{name} is malformed")
        host, port_text = rendered[1:closing], rendered[closing + 2 :]
    else:
        host, separator, port_text = rendered.rpartition(":")
        require(separator == ":", f"{name} is malformed")
    try:
        address = ipaddress.ip_address(host)
        port = int(port_text)
    except ValueError as error:
        raise ProofError(f"{name} is malformed") from error
    require(0 < port <= 65535, f"{name} port is invalid")
    return address, port


def validate_browser_result(value: Any, role: str, expected_pin: str) -> dict[str, Any]:
    result = _mapping(value, "browser result")
    require(result.get("contract") == CONTRACT, "browser result contract is missing or wrong")
    require(result.get("type") == "proof.result", "browser did not emit a terminal proof result")
    require(result.get("role") == role, "browser result names the wrong SIP role")

    codec = _mapping(result.get("codec"), "codec evidence")
    require(str(codec.get("mime_type", "")).lower() == "audio/opus", "selected codec is not Opus")
    require(codec.get("clock_rate") == 48000, "selected Opus clock is not 48 kHz")
    _positive_number(codec.get("payload_type"), "Opus payload type")

    security = _mapping(result.get("security"), "security evidence")
    require(security.get("wss_spki_sha256") == expected_pin, "browser did not report the pinned WSS identity")
    require(security.get("dtls_state") == "connected", "DTLS is not connected")
    require(security.get("setup_role") in ("active", "passive"), "DTLS setup role is unresolved")
    require(bool(security.get("srtp_profile")), "SRTP profile/cipher evidence is absent")

    pair = _mapping(result.get("candidate_pair"), "candidate-pair evidence")
    _bounded_string(pair.get("id"), "candidate-pair identifier")
    require(pair.get("selected") is True, "candidate pair is not selected")
    require(pair.get("nominated") is True, "candidate pair is not nominated")
    require(pair.get("state") == "succeeded", "candidate pair did not succeed")
    require(pair.get("component") == 1, "browser-audio selected another ICE component")
    for side in ("local", "remote"):
        candidate = _mapping(pair.get(side), f"{side} candidate")
        require(candidate.get("candidate_type") in ("host", "srflx"), f"{side} candidate is outside the proven topology")
        _bounded_string(candidate.get("address"), f"{side} candidate address")
        _positive_number(candidate.get("port"), f"{side} candidate port")

    media = _mapping(result.get("media"), "media evidence")
    for name in ("inbound_packets", "outbound_packets", "inbound_bytes", "outbound_bytes", "oscillator_frames"):
        _positive_number(media.get(name), name)
    _positive_number(media.get("received_audio_energy"), "received audio energy")

    sip = _mapping(result.get("sip"), "SIP evidence")
    order = sip.get("order")
    require(isinstance(order, list), "SIP order evidence must be an array")
    expected = ["invite", "final", "ack", "bye", "bye-final"]
    require(all(name in order for name in expected), "SIP lifecycle evidence is incomplete")
    require([order.index(name) for name in expected] == sorted(order.index(name) for name in expected), "SIP lifecycle is out of order")
    return result


def validate_sipx_result(value: Any, role: str) -> dict[str, Any]:
    result = _mapping(value, "sipx result")
    require(result.get("status") == "answered", "sipx did not report an answered call")
    require(result.get("media_profile") == "browser-audio", "sipx did not report browser-audio")
    require(str(result.get("negotiated_codec", "")).lower() == "opus", "sipx did not report Opus")
    _positive_number(result.get("negotiated_payload_type"), "sipx Opus payload type")
    require(result.get("negotiated_clock_rate") == 48000, "sipx did not report a 48 kHz Opus clock")
    require(result.get("negotiated_keying") == "dtls-srtp", "sipx did not report DTLS-SRTP")
    require(result.get("browser_role") == role, "sipx result names the wrong browser role")
    require(result.get("ice_component") == 1, "sipx selected another ICE component")
    require(bool(result.get("nominated_local")) and bool(result.get("nominated_remote")), "sipx nominated-pair evidence is absent")
    require(result.get("media_state") == "running", "sipx media component never reached Running")
    _positive_number(result.get("packets_sent"), "sipx sent packet count")
    _positive_number(result.get("packets_received"), "sipx received packet count")
    _positive_number(result.get("received_audio_peak"), "sipx received audio peak")
    return result


def proof_digest(browser: dict[str, Any], sipx: dict[str, Any]) -> str:
    encoded = json.dumps(
        {"browser": browser, "sipx": sipx},
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def validate_negative(value: Any, expected: str, roles: dict[str, Any]) -> None:
    result = _mapping(value, f"{expected} negative")
    role = result.get("positive_role")
    required_role = "browser-offerer" if expected == "FingerprintMismatch" else "browser-answerer"
    require(role == required_role, f"{expected} must run in {required_role}")
    require(role in roles, f"{expected} negative names no validated positive role")
    positive = roles[role]
    require(
        result.get("positive_sha256") == proof_digest(positive["browser"], positive["sipx"]),
        f"{expected} negative is not bound to its validated positive evidence",
    )
    sipx = _mapping(result.get("sipx"), f"{expected} sipx refusal")
    require(sipx.get("error") == expected, f"negative failed at {sipx.get('error')!r}, not {expected}")
    browser = _mapping(result.get("browser"), f"{expected} browser observation")
    require(browser.get("contract") == CONTRACT, f"{expected} browser contract is wrong")
    require(browser.get("type") == "proof.negative-browser", f"{expected} browser run did not fail")
    require(browser.get("role") == role, f"{expected} browser role differs from its positive")
    require(browser.get("mutation") == expected, f"{expected} browser applied another mutation")
    _bounded_string(browser.get("error"), f"{expected} browser error", MAX_ERROR_CHARS)
    facts = _mapping(browser.get("facts"), f"{expected} browser facts")
    require(facts.get("rtp_packets") == 0, f"{expected} negative carried RTP")
    if expected == "FingerprintMismatch":
        require(facts.get("selected_pair") is True, "fingerprint negative selected no ICE pair")
        require(facts.get("nominated") is True, "fingerprint negative nominated no ICE pair")
        require(facts.get("dtls_state") in ("failed", "closed"), "fingerprint negative did not observe DTLS failure")
    elif expected == "NoNominatedPair":
        require(facts.get("ice_started") is True, "nomination negative never started ICE")
        require(facts.get("nominated") is False, "nomination negative reports a nominated pair")
        require(facts.get("selected_pair") is False, "nomination negative selected a pair")
        require(facts.get("ice_state") in ("failed", "closed"), "nomination negative did not observe ICE failure/closure")
        require(facts.get("dtls_state") in ("new", "not-started", "closed"), "nomination negative started DTLS")
    elif expected == "WeakerMedia":
        require(facts.get("ice_started") is False, "weaker-media negative started ICE")
        require(facts.get("dtls_state") in ("new", "not-started"), "weaker-media negative started DTLS")
        require(facts.get("fallback_attempted") is False, "weaker-media negative attempted fallback")


def validate_proof(directory: pathlib.Path, expected_pin: str) -> dict[str, Any]:
    roles: dict[str, Any] = {}
    for role in ROLES:
        role_dir = directory / role
        browser = validate_browser_result(load_json(role_dir / "browser.json"), role, expected_pin)
        sipx = validate_sipx_result(load_json(role_dir / "sipx.json"), role)
        browser_pair = _mapping(browser.get("candidate_pair"), "browser candidate pair")
        browser_local = _mapping(browser_pair.get("local"), "browser local candidate")
        browser_remote = _mapping(browser_pair.get("remote"), "browser remote candidate")
        require(
            (ipaddress.ip_address(browser_local["address"]), int(browser_local["port"]))
            == _socket_address(sipx.get("nominated_remote"), "sipx nominated remote"),
            "browser local candidate differs from sipx nominated remote",
        )
        require(
            (ipaddress.ip_address(browser_remote["address"]), int(browser_remote["port"]))
            == _socket_address(sipx.get("nominated_local"), "sipx nominated local"),
            "browser remote candidate differs from sipx nominated local",
        )
        roles[role] = {"browser": browser, "sipx": sipx}
    for name in ("FingerprintMismatch", "NoNominatedPair", "WeakerMedia"):
        validate_negative(load_json(directory / "negatives" / f"{name}.json"), name, roles)
    return {"contract": CONTRACT, "type": "proof.complete", "roles": roles}


class WebDriver:
    def __init__(self, base: str):
        self.base = base.rstrip("/")
        self.session: str | None = None

    def request(self, method: str, path: str, payload: Any | None = None) -> Any:
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{self.base}{path}", data=data, method=method, headers={"Content-Type": "application/json"}
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                encoded = response.read(MAX_EVIDENCE_BYTES + 1)
        except urllib.error.HTTPError as error:
            encoded = error.read(MAX_EVIDENCE_BYTES + 1)
            require(len(encoded) <= MAX_EVIDENCE_BYTES, "WebDriver error exceeds the evidence cap")
            detail = encoded.decode("utf-8", errors="replace")
            try:
                error_value = json.loads(detail)
                if isinstance(error_value, dict):
                    error_value = error_value.get("value", error_value)
                if isinstance(error_value, dict):
                    kind = str(error_value.get("error", "request failed"))
                    message = str(error_value.get("message", detail))
                    detail = f"{kind}: {message}"
            except json.JSONDecodeError:
                pass
            raise ProofError(f"WebDriver HTTP {error.code}: {detail}") from error
        require(len(encoded) <= MAX_EVIDENCE_BYTES, "WebDriver response exceeds the evidence cap")
        value = json.loads(encoded.decode("utf-8"))
        if isinstance(value, dict) and "value" in value:
            if isinstance(value["value"], dict) and value["value"].get("error"):
                raise ProofError(f"WebDriver: {value['value']['error']}: {value['value'].get('message', '')}")
            return value["value"]
        return value

    def start(self, capabilities: dict[str, Any], expected_pin: str) -> None:
        require(capabilities.get("acceptInsecureCerts") is not True, "acceptInsecureCerts would bypass WSS identity")
        require(expected_pin in json.dumps(capabilities, sort_keys=True), "browser capabilities do not enforce the WSS SPKI pin")
        value = self.request("POST", "/session", {"capabilities": {"alwaysMatch": capabilities}})
        require(isinstance(value, dict), "WebDriver session response is not an object")
        self.session = value.get("sessionId")
        require(bool(self.session), "WebDriver did not return a session id")

    def close(self) -> None:
        if self.session:
            try:
                self.request("DELETE", f"/session/{self.session}")
            finally:
                self.session = None

    def run(self, page: pathlib.Path, config: dict[str, Any], timeout: int) -> Any:
        require(self.session is not None, "WebDriver session has not started")
        self.request("POST", f"/session/{self.session}/url", {"url": page.resolve().as_uri()})
        self.request("POST", f"/session/{self.session}/timeouts", {"script": timeout * 1000})
        script = """
            const config = arguments[0];
            const done = arguments[arguments.length - 1];
            if (!window.sipxBrowserAudio) {
                done({contract: 'sipx.browser-audio.v1', type: 'proof.error', error: 'peer page did not load'});
                return;
            }
            window.sipxBrowserAudio.run(config).then(done).catch((error) => {
                window.sipxBrowserAudio.failure(config, error).then(done).catch((failureError) => done({
                    contract: 'sipx.browser-audio.v1', type: 'proof.error',
                    error: String(failureError && failureError.stack || failureError)
                }));
            });
        """
        return self.request("POST", f"/session/{self.session}/execute/async", {"script": script, "args": [config]})


def wait_webdriver(url: str, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            value = WebDriver(url).request("GET", "/status")
            if isinstance(value, dict) and value.get("ready", True):
                return
        except Exception as error:  # the service is expected not to exist yet
            last_error = error
        time.sleep(0.05)  # poll interval: WebDriver's ready fact is the completion condition
    raise ProofError(f"WebDriver did not become ready: {last_error}")


def wait_listening(path: pathlib.Path, timeout: float) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            require(path.stat().st_size <= MAX_EVIDENCE_BYTES, "sipx listening output exceeded its cap")
            for line in path.read_text(encoding="utf-8").splitlines():
                try:
                    value = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(value, dict) and value.get("status") == "listening":
                    address = str(value.get("address", ""))
                    host, separator, port = address.rpartition(":")
                    require(separator == ":" and host in ("127.0.0.1", "[::1]"), "sipx proof listener is not loopback")
                    require(port.isdigit() and 0 < int(port) <= 65535, "sipx proof listener port is invalid")
                    return address
        time.sleep(0.05)  # poll interval: the structured listening fact is the completion condition
    raise ProofError("sipx proof endpoint did not report its WSS listener")


def prepare_config(source: pathlib.Path, output: pathlib.Path, role: str, wss_url: str) -> None:
    config = _mapping(load_json(source), "browser config template")
    require(config.get("role") == role, "browser config template names the wrong role")
    config["wssUrl"] = wss_url
    output.write_text(json.dumps(config, separators=(",", ":")) + "\n", encoding="utf-8")


def prepare_capabilities(source: pathlib.Path, output: pathlib.Path, pin: str) -> None:
    pin_bytes(pin)
    encoded = json.dumps(load_json(source), separators=(",", ":"))
    require("__SIPX_SPKI_PIN__" in encoded, "capabilities template has no SPKI pin placeholder")
    output.write_text(encoded.replace("__SIPX_SPKI_PIN__", pin) + "\n", encoding="utf-8")


def combine_negative(
    positive_directory: pathlib.Path,
    browser_path: pathlib.Path,
    sipx_path: pathlib.Path,
    expected: str,
    role: str,
    expected_pin: str,
    output: pathlib.Path,
) -> None:
    browser_positive = validate_browser_result(
        load_json(positive_directory / "browser.json"), role, expected_pin
    )
    sipx_positive = validate_sipx_result(load_json(positive_directory / "sipx.json"), role)
    combined = {
        "positive_role": role,
        "positive_sha256": proof_digest(browser_positive, sipx_positive),
        "browser": load_json(browser_path),
        "sipx": load_json(sipx_path),
    }
    validate_negative(combined, expected, {role: {"browser": browser_positive, "sipx": sipx_positive}})
    output.write_text(json.dumps(combined, separators=(",", ":")) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)

    cert = commands.add_parser("preflight-cert")
    cert.add_argument("--cert", type=pathlib.Path, required=True)
    cert.add_argument("--host", required=True)
    cert.add_argument("--pin", required=True)

    pin = commands.add_parser("print-pin")
    pin.add_argument("--cert", type=pathlib.Path, required=True)

    wss = commands.add_parser("preflight-wss")
    wss.add_argument("--url", required=True)
    wss.add_argument("--ca", type=pathlib.Path, required=True)
    wss.add_argument("--pin", required=True)
    wss.add_argument("--timeout", type=float, default=10)

    wait = commands.add_parser("wait-webdriver")
    wait.add_argument("--url", required=True)
    wait.add_argument("--timeout", type=float, default=10)

    listening = commands.add_parser("wait-listening")
    listening.add_argument("--input", type=pathlib.Path, required=True)
    listening.add_argument("--timeout", type=float, default=10)

    config = commands.add_parser("prepare-config")
    config.add_argument("--input", type=pathlib.Path, required=True)
    config.add_argument("--output", type=pathlib.Path, required=True)
    config.add_argument("--role", choices=ROLES, required=True)
    config.add_argument("--wss-url", required=True)

    capabilities = commands.add_parser("prepare-capabilities")
    capabilities.add_argument("--input", type=pathlib.Path, required=True)
    capabilities.add_argument("--output", type=pathlib.Path, required=True)
    capabilities.add_argument("--pin", required=True)

    run = commands.add_parser("run-role")
    run.add_argument("--url", required=True)
    run.add_argument("--page", type=pathlib.Path, required=True)
    run.add_argument("--config", type=pathlib.Path, required=True)
    run.add_argument("--capabilities", type=pathlib.Path, required=True)
    run.add_argument("--role", choices=ROLES, required=True)
    run.add_argument("--pin", required=True)
    run.add_argument("--output", type=pathlib.Path, required=True)
    run.add_argument("--timeout", type=int, default=120)

    negative = commands.add_parser("run-negative")
    negative.add_argument("--url", required=True)
    negative.add_argument("--page", type=pathlib.Path, required=True)
    negative.add_argument("--config", type=pathlib.Path, required=True)
    negative.add_argument("--capabilities", type=pathlib.Path, required=True)
    negative.add_argument("--role", choices=ROLES, required=True)
    negative.add_argument("--mutation", choices=("FingerprintMismatch", "NoNominatedPair", "WeakerMedia"), required=True)
    negative.add_argument("--pin", required=True)
    negative.add_argument("--output", type=pathlib.Path, required=True)
    negative.add_argument("--timeout", type=int, default=120)

    validate = commands.add_parser("validate-proof")
    validate.add_argument("--directory", type=pathlib.Path, required=True)
    validate.add_argument("--pin", required=True)

    combine = commands.add_parser("combine-negative")
    combine.add_argument("--positive-directory", type=pathlib.Path, required=True)
    combine.add_argument("--browser", type=pathlib.Path, required=True)
    combine.add_argument("--sipx", type=pathlib.Path, required=True)
    combine.add_argument("--error", choices=("FingerprintMismatch", "NoNominatedPair", "WeakerMedia"), required=True)
    combine.add_argument("--role", choices=ROLES, required=True)
    combine.add_argument("--pin", required=True)
    combine.add_argument("--output", type=pathlib.Path, required=True)

    args = parser.parse_args()
    if args.command == "preflight-cert":
        preflight_certificate(args.cert, args.host, args.pin)
    elif args.command == "print-pin":
        print(spki_pin(args.cert.read_bytes(), "PEM"))
    elif args.command == "preflight-wss":
        preflight_wss(args.url, args.ca, args.pin, args.timeout)
    elif args.command == "wait-webdriver":
        wait_webdriver(args.url, args.timeout)
    elif args.command == "wait-listening":
        print(wait_listening(args.input, args.timeout))
    elif args.command == "prepare-config":
        prepare_config(args.input, args.output, args.role, args.wss_url)
    elif args.command == "prepare-capabilities":
        prepare_capabilities(args.input, args.output, args.pin)
    elif args.command == "run-role":
        config = load_json(args.config)
        require(config.get("role") == args.role, "browser config names the wrong role")
        config["wssSpkiSha256"] = args.pin
        driver = WebDriver(args.url)
        try:
            driver.start(load_json(args.capabilities), args.pin)
            result = driver.run(args.page, config, args.timeout)
            validate_browser_result(result, args.role, args.pin)
            args.output.write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
        finally:
            driver.close()
    elif args.command == "run-negative":
        config = load_json(args.config)
        require(config.get("role") == args.role, "browser config names the wrong role")
        config["wssSpkiSha256"] = args.pin
        config["mutation"] = args.mutation
        driver = WebDriver(args.url)
        try:
            driver.start(load_json(args.capabilities), args.pin)
            result = driver.run(args.page, config, args.timeout)
            require(result.get("contract") == CONTRACT, "negative browser result contract is wrong")
            require(result.get("type") == "proof.negative-browser", "negative browser run did not fail")
            require(result.get("mutation") == args.mutation, "negative browser result names the wrong mutation")
            args.output.write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
        finally:
            driver.close()
    elif args.command == "validate-proof":
        print(json.dumps(validate_proof(args.directory, args.pin), separators=(",", ":")))
    elif args.command == "combine-negative":
        combine_negative(
            args.positive_directory,
            args.browser,
            args.sipx,
            args.error,
            args.role,
            args.pin,
            args.output,
        )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ProofError, OSError, subprocess.SubprocessError, urllib.error.URLError, http.client.HTTPException) as error:
        print(f"browser-audio proof: {error}", file=sys.stderr)
        raise SystemExit(1) from error
