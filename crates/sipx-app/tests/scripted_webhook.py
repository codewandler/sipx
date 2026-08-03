#!/usr/bin/env python3
"""A loopback document app for the A-2 phase-1 shell proof."""

import argparse
import json
import math
import pathlib
import struct
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def prompt(path: pathlib.Path) -> None:
    samples = [int(9000 * math.sin(2 * math.pi * 440 * index / 8000)) for index in range(6000)]
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(8000)
        output.writeframes(b"".join(struct.pack("<h", sample) for sample in samples))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prompt", type=pathlib.Path, required=True)
    parser.add_argument("--outcome", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    prompt(arguments.prompt)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:  # noqa: N802 - HTTP method spelling
            length = int(self.headers.get("Content-Length", "0"))
            envelope = json.loads(self.rfile.read(length))
            event = envelope["event"]
            event_type = event["type"]
            if event_type == "call.incoming":
                document = {
                    "contract": "sipx.app.v1",
                    "instructions": [
                        {"id": "answer", "do": "answer"},
                        {
                            "id": "pin",
                            "do": "gather",
                            "min": 1,
                            "max": 1,
                            "terminators": "#",
                            "timeout_ms": 3000,
                            "prompt": {"file": str(arguments.prompt)},
                        },
                    ],
                }
            elif event_type == "call.gather.finished":
                arguments.outcome.write_text(event["digits"] + "\n", encoding="utf-8")
                document = {
                    "contract": "sipx.app.v1",
                    "instructions": [
                        {"id": "done", "do": "hangup", "cause": "hangup"}
                    ],
                }
            else:
                document = {"contract": "sipx.app.v1", "instructions": []}

            body = json.dumps(document, separators=(",", ":")).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, _format: str, *_arguments: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"READY http://127.0.0.1:{server.server_port}/hook", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
