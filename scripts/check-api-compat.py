#!/usr/bin/env python3
"""Check or deliberately write the v1 Supported Rust API baseline."""

import argparse
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import api_compat


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--check", action="store_true", help="compare the candidate with v1")
    action.add_argument("--update", action="store_true", help="write the reviewed v1 candidate")
    args = parser.parse_args()
    try:
        candidate = api_compat.extract_candidate()
        if args.update:
            if api_compat.BASELINE.exists():
                existing = json.loads(api_compat.BASELINE.read_text(encoding="utf-8"))
                problems = api_compat.baseline_update_problems(existing, candidate)
                if problems:
                    print(
                        "api compatibility: refusing to overwrite v1 commitments:",
                        file=sys.stderr,
                    )
                    for problem in problems:
                        print(f"  {problem}", file=sys.stderr)
                    return 1
            document = api_compat.baseline_document(candidate)
            api_compat.BASELINE.parent.mkdir(parents=True, exist_ok=True)
            api_compat.BASELINE.write_text(
                json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            print(
                f"api compatibility: recorded {sum(len(p['items']) for p in document['packages'].values())} "
                f"Supported paths in {api_compat.BASELINE.relative_to(api_compat.ROOT)}"
            )
            return 0
        if not api_compat.BASELINE.exists():
            print(
                f"api compatibility: baseline missing: {api_compat.BASELINE.relative_to(api_compat.ROOT)}; "
                "cut it from the final release candidate with --update",
                file=sys.stderr,
            )
            return 1
        baseline = json.loads(api_compat.BASELINE.read_text(encoding="utf-8"))
        problems = api_compat.document_problems(baseline, candidate)
        if problems:
            print("Supported Rust API compatibility failed:", file=sys.stderr)
            for problem in problems:
                print(f"  {problem}", file=sys.stderr)
            return 1
        count = sum(len(p["items"]) for p in baseline["packages"].values())
        print(f"api compatibility: {count} Supported paths are source-compatible with v1")
        return 0
    except (api_compat.ApiCompatError, OSError, json.JSONDecodeError) as error:
        print(f"api compatibility: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
