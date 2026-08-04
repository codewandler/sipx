#!/usr/bin/env python3
"""Statically hold the approved beta workflow to the release orchestration contract."""

from __future__ import annotations

import pathlib
import re
import sys
from collections.abc import Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "crates-io.yml"
SPEC = ROOT / "docs" / "specs" / "release-workflow.md"


def required(text: str, label: str, pattern: str, problems: list[str]) -> None:
    """Append one named structural defect when ``pattern`` is absent."""

    if re.search(pattern, text, re.MULTILINE | re.DOTALL) is None:
        problems.append(label)


def workflow_problems(text: str) -> list[str]:
    """Return release-contract defects found in workflow source."""

    problems: list[str] = []
    checks = (
        ("no version-tag push entry", r"push:\s*\n\s+tags:\s*\n\s+- [\"']v\*[\"']"),
        ("no manual resume entry", r"workflow_dispatch:\s*\n\s+inputs:\s*\n\s+tag:"),
        ("manual tag input is not required", r"tag:.*?required:\s*true"),
        ("release tag is not derived from the selected ref", r"RELEASE_TAG:\s*\$\{\{\s*github\.ref_name\s*\}\}"),
        ("manual confirmation is not captured separately", r"REQUESTED_RELEASE_TAG:\s*\$\{\{\s*inputs\.tag\s*\}\}"),
        ("release runs outside the approved environment", r"environment:\s*\n\s+name:\s*release"),
        ("release job has no finite timeout", r"\n\s*release:\s*\n(?:(?!\n\s*github_release:).)*?timeout-minutes:\s*[1-9][0-9]*"),
        ("release concurrency can cancel a publication", r"cancel-in-progress:\s*false"),
        ("release concurrency is not keyed by tag", r"group:.*(?:inputs\.tag|RELEASE_TAG|ref_name)"),
        ("workflow cannot read Actions evidence", r"actions:\s*read"),
        ("workflow permissions are not read-only", r"permissions:\s*\n\s+actions:\s*read\s*\n\s+contents:\s*read\s*\n\s+pages:\s*read"),
        ("GitHub prerelease is not a dependent job", r"\n\s*github_release:\s*\n.*?needs:\s*release"),
        ("GitHub prerelease lacks job-scoped write authority", r"\n\s*github_release:\s*\n.*?permissions:\s*\n\s+contents:\s*write"),
        ("workflow cannot read Pages evidence", r"pages:\s*read"),
        ("Cargo secret does not use the repository convention", r"CARGO_REGISTRY_TOKEN:\s*\$\{\{\s*secrets\.CARGO_REGISTRY_TOKEN\s*\}\}"),
        ("empty Cargo secret is not refused", r"-z [\"']?\$CARGO_REGISTRY_TOKEN"),
        ("checkout does not select the release tag", r"ref:\s*\$\{\{\s*env\.RELEASE_TAG\s*\}\}"),
        ("checkout does not fetch tag history", r"fetch-depth:\s*0"),
        ("release checkout persists a credential", r"Check out the exact tag(?:(?!Check out the reviewed release record).)*?persist-credentials:\s*false"),
        ("workflow does not require the selected ref to be the release tag", r"GITHUB_REF_TYPE.*!= tag.*?GITHUB_REF.*refs/tags/\$RELEASE_TAG.*?GITHUB_REF_NAME.*\$RELEASE_TAG"),
        ("manual confirmation need not equal the selected tag", r"GITHUB_EVENT_NAME[^\n]*workflow_dispatch[^\n]*REQUESTED_RELEASE_TAG[^\n]*!=[^\n]*RELEASE_TAG"),
        ("workspace version is not matched to the tag", r"RELEASE_TAG.*v\$version"),
        ("lightweight tags are not refused", r"git cat-file -t .*refs/tags/\$RELEASE_TAG.*!= tag"),
        ("tag is not peeled to its commit", r"git rev-parse .*refs/tags/\$RELEASE_TAG\^\{commit\}"),
        ("event and workflow SHAs are not bound to the release commit", r"GITHUB_SHA[^\n]*release_sha[^\n]*GITHUB_WORKFLOW_SHA[^\n]*release_sha"),
        ("workflow source is not required from the release tag", r"expected_workflow_ref=[^\n]*refs/tags/\$RELEASE_TAG.*?GITHUB_WORKFLOW_REF[^\n]*!=[^\n]*expected_workflow_ref"),
        ("another tag may share the release commit", r"git tag --points-at HEAD"),
        ("dirty or untracked release files are not refused", r"git status --porcelain=v1 --untracked-files=all"),
        ("release commit is not required on main", r"git merge-base --is-ancestor .*origin/main"),
        ("reviewed versioned release record is not required", r"docs/releases/\$version\.md"),
        ("complete gate is absent", r"\./scripts/gate\.py(?:\s|$)"),
        ("locked publication rehearsal is absent", r"\./scripts/release\.py --dry-run"),
        ("publication bypasses exact tag confirmation", r"--publish.*?--confirm-publish [\"']\$RELEASE_TAG[\"']"),
        ("publication bypasses exact CI tag and commit authorization", r"--publish.*?--authorize-ci-publish [\"']\$RELEASE_TAG@\$RELEASE_SHA[\"']"),
        ("publication does not use a finite visibility bound", r"--registry-wait-seconds\s+[1-9][0-9]*"),
        ("frontier loop is not bounded by public package count", r"max_invocations=\$\(\(public_count \+ 1\)\).*?invocation <= max_invocations"),
        ("frontier loop does not require the all-visible observation", r"all public packages are already registry-visible"),
        ("exact registry consumer proof is absent", r"--verify-consumer"),
        ("consumer command has no finite bound", r"--consumer-timeout-seconds\s+[1-9][0-9]*"),
        ("Pages run is not selected by release head SHA", r"actions/workflows/ci\.yml/runs\?.*head_sha=\$RELEASE_SHA"),
        ("returned Pages run is not checked against release head SHA", r"\.head_sha == env\.RELEASE_SHA"),
        ("Pages evidence does not require the deployment job", r"deploy docs site.*conclusion == [\"']success[\"']"),
        ("GitHub read token is not scoped to the Pages step", r"Verify Pages deployment from the release commit\s*\n\s+env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
        ("public guide is not probed", r"https://codewandler\.github\.io/sipx/docs/getting-started"),
        ("public API is not probed", r"https://codewandler\.github\.io/sipx/api/sipx_call/index\.html"),
        ("GitHub prerelease checkout persists a credential", r"Check out the reviewed release record.*?persist-credentials:\s*false"),
        ("GitHub write token is not scoped to the prerelease step", r"Create or verify the GitHub prerelease\s*\n\s+env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
        ("GitHub prerelease does not verify the existing tag", r"gh release create .*?--verify-tag"),
        ("GitHub Release is not a prerelease", r"gh release create .*?--prerelease"),
        ("GitHub prerelease does not consume reviewed notes", r"gh release create .*?--notes-file [\"']\$RELEASE_NOTES[\"']"),
        ("resume does not verify the existing GitHub prerelease", r"gh release view .*?record\.get\([\"']prerelease[\"']\).*?reviewed notes differ"),
    )
    for label, pattern in checks:
        required(text, label, pattern, problems)

    if re.search(r"(?m)^\s*cargo\s+publish\b", text):
        problems.append("workflow calls cargo publish directly instead of the release helper")

    if re.search(r"(?m)^  announce:\s*$", text):
        problems.append("workflow contains an announcement job")
    posting_patterns = (
        r"\bgh\s+(?:issue|pr)\s+(?:create|comment)\b",
        r"\bgh\s+api\b[^\n]*(?:--method|-X)\s+POST\b",
        r"\bcurl\b[^\n]*(?:--request|-X)\s+POST\b",
        r"\brepository_dispatch\b",
    )
    if any(re.search(pattern, text, re.IGNORECASE) for pattern in posting_patterns):
        problems.append("workflow contains an external announcement or posting side effect")

    release_job = re.search(r"(?ms)^  release:\s*$.*?(?=^  github_release:\s*$)", text)
    if release_job is not None and re.search(
        r"(?m)^\s+contents:\s*write\s*$", release_job.group()
    ):
        problems.append("publication job can write repository contents")
    if re.search(r"(?m)^      (?:GH_TOKEN|CARGO_REGISTRY_TOKEN):", text):
        problems.append("a release credential is exposed at job scope")

    ordered = (
        ("Publish dependency-ready frontiers", "publication"),
        ("Verify the exact registry consumer", "consumer proof"),
        ("Verify Pages deployment", "Pages proof"),
        ("Create or verify the GitHub prerelease", "GitHub prerelease"),
    )
    positions = [(text.find(marker), label) for marker, label in ordered]
    if all(position >= 0 for position, _label in positions):
        if [position for position, _label in positions] != sorted(
            position for position, _label in positions
        ):
            problems.append("publication, consumer, Pages and GitHub prerelease steps are out of order")

    return problems


def specification_problems(text: str) -> list[str]:
    """Require the normative contract to retain the no-announcement boundary."""

    problems: list[str] = []
    required(
        text,
        "specification does not separate the GitHub prerelease from broader publicity",
        r"MUST NOT post broader publicity",
        problems,
    )
    return problems


def check(root: pathlib.Path = ROOT) -> list[str]:
    """Check the real workflow and its normative specification."""

    workflow = root / ".github" / "workflows" / "crates-io.yml"
    spec = root / "docs" / "specs" / "release-workflow.md"
    problems = []
    if not workflow.is_file():
        problems.append(f"missing {workflow.relative_to(root)}")
        return problems
    if not spec.is_file():
        problems.append(f"missing {spec.relative_to(root)}")
    else:
        problems.extend(specification_problems(spec.read_text(encoding="utf-8")))
    problems.extend(workflow_problems(workflow.read_text(encoding="utf-8")))
    return problems


def main(argv: Sequence[str] | None = None) -> int:
    args = tuple(sys.argv[1:] if argv is None else argv)
    if args != ("--check",):
        print("usage: check-release-workflow.py --check", file=sys.stderr)
        return 2
    problems = check()
    for problem in problems:
        print(f"release workflow: {problem}", file=sys.stderr)
    if not problems:
        print("release workflow: approved tag, bounded registry, Pages and resumable GitHub prerelease")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
