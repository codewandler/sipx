#!/usr/bin/env python3
"""Install one exact archive, compile a clean consumer and prove native browser calls."""

import argparse, functools, hashlib, http.server, importlib.util, json, os, pathlib, shutil, signal, socket, subprocess, threading, time, traceback

ROOT = pathlib.Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "browser_audio_driver", ROOT / "tests/browser-audio/driver.py"
)
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def media_address():
    # A UDP connect selects the host route without sending any packet to the documentation address.
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
        route.connect(("192.0.2.1", 9))
        return route.getsockname()[0]


def stop(process):
    if process and process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def checked(command, cwd, log):
    with log.open("w") as output:
        run = subprocess.run(
            command, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, timeout=120
        )
    log.with_suffix(".exit").write_text(str(run.returncode) + "\n")
    if run.returncode:
        raise RuntimeError(f"{command[0]} failed: {log}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument(
        "--native",
        type=pathlib.Path,
        default=ROOT / "target/debug/examples/browser_sdk_proof",
    )
    parser.add_argument("--chrome", default="/opt/google/chrome/chrome")
    parser.add_argument(
        "--driver",
        default="/home/timo/.cache/sipx-chromedriver-150/chromedriver-linux64/chromedriver",
    )
    parser.add_argument(
        "--cases",
        default="positive-offerer,positive-answerer,wrong-fingerprint,insecure-signalling,missing-ice,weaker-media,oversized-signalling,cancel-during-setup",
    )
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    consumer = out / "consumer"
    consumer.mkdir()
    (consumer / "package.json").write_text('{"private":true,"type":"module"}\n')
    archive = args.archive.resolve()
    archive_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
    checked(
        [
            "npm",
            "install",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--offline",
            str(archive),
        ],
        consumer,
        out / "install.log",
    )
    for source in ["consumer.ts", "peer.html", "peer.mjs"]:
        shutil.copyfile(ROOT / "tests/browser-sdk" / source, consumer / source)
    checked(
        [
            "tsc",
            "--strict",
            "--target",
            "ES2022",
            "--module",
            "NodeNext",
            "--moduleResolution",
            "NodeNext",
            "--lib",
            "ES2022,DOM",
            "--outDir",
            "dist",
            "consumer.ts",
        ],
        consumer,
        out / "typecheck-build.log",
    )
    package = consumer / "node_modules/@sipx/browser"
    for line in (package / "SHA256SUMS").read_text().splitlines():
        expected, name = line.split("  ", 1)
        if hashlib.sha256((package / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"packed checksum mismatch: {name}")
    if any("node:" in p.read_text() for p in (package / "src").glob("*.mjs")):
        raise RuntimeError("runtime Node import")
    checked(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            str(out / "key.pem"),
            "-out",
            str(out / "cert.pem"),
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ],
        out,
        out / "certificate.log",
    )
    pin = driver.spki_pin((out / "cert.pem").read_bytes(), "PEM")
    active_result = [None]

    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/native-status":
                path = active_result[0]
                data = path.read_bytes() if path and path.exists() else b"{}"
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(data)
            else:
                super().do_GET()

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(Handler, directory=str(consumer))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        driver_port = reserve.getsockname()[1]
    driver_log = (out / "driver.log").open("w")
    driver_process = subprocess.Popen(
        [args.driver, f"--port={driver_port}"],
        stdout=driver_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    wd = driver.WebDriver(f"http://127.0.0.1:{driver_port}")
    cases = []
    try:
        driver.wait_webdriver(wd.base, 10)
        for name in args.cases.split(","):
            role = (
                "browser-answerer" if name == "positive-answerer" else "browser-offerer"
            )
            case = "positive" if name.startswith("positive-") else name
            directory = out / name
            directory.mkdir()
            native = None
            native_output = (directory / "native.log").open("w")
            blackhole = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            blackhole.bind(("0.0.0.0", 0))
            blackhole.settimeout(0.1)
            blackhole_packets = []
            blackhole_done = threading.Event()

            def drain():
                while not blackhole_done.is_set():
                    try:
                        data, _ = blackhole.recvfrom(65536)
                        blackhole_packets.append(len(data))
                    except socket.timeout:
                        pass
                    except OSError:
                        break

            thread = threading.Thread(target=drain, daemon=True)
            thread.start()
            try:
                active_result[0] = directory / "native-result.json"
                native = subprocess.Popen(
                    [
                        str(args.native.resolve()),
                        str(out / "cert.pem"),
                        str(out / "key.pem"),
                        role,
                        str(directory / "native-result.json"),
                    ],
                    stdout=native_output,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                    env={
                        **os.environ,
                        "SIPX_SDK_PROOF_CASE": case,
                        "SIPX_SDK_PROOF_BLACKHOLE_PORT": str(
                            blackhole.getsockname()[1]
                        ),
                        "SIPX_SDK_PROOF_MEDIA_ADDRESS": media_address(),
                    },
                )
                address = driver.wait_listening(directory / "native.log", 10)
                port = int(address.rsplit(":", 1)[1])
                caps = {
                    "browserName": "chrome",
                    "goog:loggingPrefs": {"browser": "ALL"},
                    "acceptInsecureCerts": False,
                    "goog:chromeOptions": {
                        "binary": args.chrome,
                        "args": [
                            "--headless=new",
                            "--no-sandbox",
                            "--disable-dev-shm-usage",
                            "--use-fake-device-for-media-stream",
                            "--use-fake-ui-for-media-stream",
                            "--autoplay-policy=no-user-gesture-required",
                            "--disable-features=WebRtcHideLocalIpsWithMdns",
                            f"--ignore-certificate-errors-spki-list={pin}",
                        ],
                    },
                }
                wd.start(caps, pin, 20)
                wd.request(
                    "POST",
                    f"/session/{wd.session}/url",
                    {"url": f"http://127.0.0.1:{server.server_port}/peer.html"},
                )
                wd.request("POST", f"/session/{wd.session}/timeouts", {"script": 40000})
                result = wd.request(
                    "POST",
                    f"/session/{wd.session}/execute/async",
                    {
                        "script": "const done=arguments[arguments.length-1];window.runProof(arguments[0]).then(proof=>done({proof})).catch(e=>done({proof:{fixtureError:String(e.stack)}}));",
                        "args": [
                            {
                                "port": port,
                                "role": role,
                                "case": case,
                                "blackholePort": blackhole.getsockname()[1],
                                "mediaAddress": media_address(),
                            }
                        ],
                    },
                    timeout=45,
                )
                result = result["proof"]
                (directory / "console.json").write_text(
                    json.dumps(
                        wd.request(
                            "POST", f"/session/{wd.session}/log", {"type": "browser"}
                        ),
                        indent=2,
                    )
                    + "\n"
                )
                result["browser"] = wd.peer
                result["archive_sha256"] = archive_hash
                result["blackhole_packets"] = len(blackhole_packets)
                (directory / "browser-result.json").write_text(
                    json.dumps(result, indent=2) + "\n"
                )
                if result.get("fixtureError"):
                    raise RuntimeError(result["fixtureError"])
                if any(result["resources"].values()):
                    raise RuntimeError(f'resources leaked: {result["resources"]}')
                if case == "positive":
                    if result["error"]:
                        raise RuntimeError(
                            f'positive browser failure: {result["error"]}'
                        )
                    native.wait(timeout=10)
                    if native.returncode:
                        raise RuntimeError("native positive process failed")
                    native_result = json.loads(
                        (directory / "native-result.json").read_text()
                    )
                    if (
                        result["error"]
                        or result["established"] != 1
                        or result["remotePeak"] <= 0.001
                        or native_result["received_peak"] <= 0
                    ):
                        raise RuntimeError(f"positive media failed: {result}")
                else:
                    expected = {
                        "wrong-fingerprint": "SipxMediaError",
                        "insecure-signalling": "SipxTransportError",
                        "missing-ice": "SipxMediaError",
                        "weaker-media": "SipxMediaError",
                        "oversized-signalling": "SipxTransportError",
                        "cancel-during-setup": "SipxCancelled",
                    }[case]
                    if (
                        result["established"]
                        or result.get("error", {}).get("name") != expected
                        or not result["fault"]
                    ):
                        raise RuntimeError(f"negative failed its contract: {result}")
                    if case == "missing-ice" and not blackhole_packets:
                        raise RuntimeError(
                            "missing ICE fixture was not independently activated"
                        )
                record = {"case": name, "passed": True, "browser": wd.peer}
                cases.append(record)
                print(f"PASS {name}", flush=True)
            except Exception as error:
                cases.append({"case": name, "passed": False, "error": str(error)})
                print(f"FAIL {name}: {error}", flush=True)
            finally:
                wd.close()
                stop(native)
                native_output.close()
                blackhole_done.set()
                blackhole.close()
                thread.join(timeout=1)
                (directory / "native.exit").write_text(
                    str(native.returncode if native else "not-started") + "\n"
                )
    finally:
        wd.close()
        stop(driver_process)
        driver_log.close()
        server.shutdown()
        server.server_close()
    result = {
        "archive": str(archive),
        "sha256": archive_hash,
        "source": json.loads((package / "PROVENANCE.json").read_text())["source"],
        "cases": cases,
        "unsupported": ["Firefox: not executed", "WebKit: not executed"],
    }
    (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    failed = sum(not c["passed"] for c in cases)
    print(
        f"packed browser SDK: {len(cases)} executed, {len(cases)-failed} passed, {failed} failed"
    )
    return int(failed > 0)


if __name__ == "__main__":
    raise SystemExit(main())
