#!/usr/bin/env python3
"""Statically hold ordinary and recovery beta workflows to the release contract."""

from __future__ import annotations

import pathlib
import re
import sys
from collections.abc import Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "crates-io.yml"
RESUME_WORKFLOW = ROOT / ".github" / "workflows" / "crates-io-resume.yml"
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
        (
            "complete gate does not receive the provenance denylist secret",
            r"Run the complete release gate\s*\n\s+env:\s*\n\s+SIPX_DENYLIST:\s*\$\{\{\s*secrets\.SIPX_DENYLIST\s*\}\}\s*\n\s+run:\s*\|.*?\./scripts/gate\.py",
        ),
        (
            "empty provenance denylist is not refused before the gate",
            r"Run the complete release gate.*?-z [\"']?\$SIPX_DENYLIST.*?exit 1.*?\./scripts/gate\.py",
        ),
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
    if len(re.findall(r"\$\{\{\s*secrets\.SIPX_DENYLIST\s*\}\}", text)) != 1:
        problems.append("provenance denylist secret is not confined to the gate step")

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


def resume_workflow_problems(text: str) -> list[str]:
    """Return authority and recovery-contract defects in the protected resume workflow."""

    problems: list[str] = []
    checks = (
        ("recovery has no required exact tag input", r"workflow_dispatch:\s*\n\s+inputs:\s*\n\s+tag:.*?required:\s*true"),
        ("recovery has no required failed run input", r"failed_run_id:.*?required:\s*true"),
        ("recovery permissions are not read-only", r"permissions:\s*\n\s+actions:\s*read\s*\n\s+contents:\s*read\s*\n\s+pages:\s*read"),
        ("recovery does not serialize with publication by tag", r"group:\s*crates-io-\$\{\{\s*inputs\.tag\s*\}\}"),
        ("recovery concurrency can cancel a publication", r"cancel-in-progress:\s*false"),
        ("recovery runs outside the protected release environment", r"\n\s*recover:\s*\n.*?environment:\s*\n\s+name:\s*release"),
        ("recovery job has no finite timeout", r"\n\s*recover:\s*\n(?:(?!\n\s*github_release:).)*?timeout-minutes:\s*[1-9][0-9]*"),
        ("failed run input is not passed to the helper convention", r"SIPX_FAILED_RELEASE_RUN_ID:\s*\$\{\{\s*inputs\.failed_run_id\s*\}\}"),
        ("recovery does not define separate controller and release roots", r"CONTROLLER_ROOT:.*?/controller\s*\n\s+SIPX_RELEASE_ROOT:.*?/release"),
        ("recovery does not pin the beta tag object", r"EXPECTED_RELEASE_TAG_OBJECT:\s*04a19dff6a7d7b6c072c98d18ad4b42407955d4b"),
        ("recovery tag object is not passed to the dependent job", r"release_tag_object:\s*\$\{\{\s*steps\.release_facts\.outputs\.tag_object\s*\}\}.*?RELEASE_TAG_OBJECT:\s*\$\{\{\s*needs\.recover\.outputs\.release_tag_object\s*\}\}"),
        ("recovery does not pin the original packager toolchain", r"RUSTUP_TOOLCHAIN:\s*1\.97\.1"),
        (
            "fixed controller checkout is absent or mutable",
            r"Check out the fixed recovery controller\s*\n(?:(?!\n\s+- name:).)*?uses:\s*actions/checkout@v4(?:(?!\n\s+- name:).)*?ref:\s*\$\{\{\s*github\.sha\s*\}\}(?:(?!\n\s+- name:).)*?path:\s*controller(?:(?!\n\s+- name:).)*?fetch-depth:\s*0(?:(?!\n\s+- name:).)*?persist-credentials:\s*false",
        ),
        (
            "immutable release checkout is absent or not separate",
            r"Check out the immutable release tag separately\s*\n(?:(?!\n\s+- name:).)*?uses:\s*actions/checkout@v4(?:(?!\n\s+- name:).)*?ref:\s*refs/tags/\$\{\{\s*inputs\.tag\s*\}\}(?:(?!\n\s+- name:).)*?path:\s*release(?:(?!\n\s+- name:).)*?fetch-depth:\s*0(?:(?!\n\s+- name:).)*?persist-credentials:\s*false",
        ),
        (
            "recovery workflow source is not required from exact main",
            r"expected_workflow_ref=.*?\.github/workflows/crates-io-resume\.yml@refs/heads/main.*?GITHUB_WORKFLOW_REF.*?expected_workflow_ref",
        ),
        ("recovery event is not required on main", r"GITHUB_REF.*?refs/heads/main.*?GITHUB_REF_NAME.*?main"),
        (
            "controller checkout is not bound to event and workflow SHAs",
            r"GITHUB_SHA.*?GITHUB_WORKFLOW_SHA.*?git -C [\"']?\$CONTROLLER_ROOT[\"']? rev-parse HEAD.*?GITHUB_SHA",
        ),
        ("controller cleanliness is not required", r"git -C [\"']?\$CONTROLLER_ROOT[\"']? status --porcelain=v1 --untracked-files=all"),
        ("failed run ID is not required to be positive numeric", r"SIPX_FAILED_RELEASE_RUN_ID.*?\^\[1-9\]\[0-9\]\*\$"),
        ("recovery does not require an annotated tag", r"git -C [\"']?\$SIPX_RELEASE_ROOT[\"']? cat-file -t .*?refs/tags/\$RELEASE_TAG.*?!= tag"),
        (
            "recovery does not bind local and remote annotated tag objects",
            r"local_tag_object=.*?rev-parse .*?refs/tags/\$RELEASE_TAG.*?remote_tag_object=.*?ls-remote --refs --tags origin .*?refs/tags/\$RELEASE_TAG.*?local_tag_object.*?EXPECTED_RELEASE_TAG_OBJECT.*?remote_tag_object.*?EXPECTED_RELEASE_TAG_OBJECT",
        ),
        ("recovery does not peel the tag to its commit", r"git -C [\"']?\$SIPX_RELEASE_ROOT[\"']? rev-parse .*?refs/tags/\$RELEASE_TAG\^\{commit\}"),
        ("recovery tag is not matched to workspace version", r"RELEASE_TAG.*?v\$version"),
        ("recovery permits another tag on the release commit", r"git -C [\"']?\$SIPX_RELEASE_ROOT[\"']? tag --points-at HEAD"),
        ("release checkout cleanliness is not required", r"git -C [\"']?\$SIPX_RELEASE_ROOT[\"']? status --porcelain=v1 --untracked-files=all"),
        ("recovery release commit is not required on main", r"git -C [\"']?\$SIPX_RELEASE_ROOT[\"']? merge-base --is-ancestor .*?origin/main"),
        ("failed release run record is not queried", r"actions/runs/\$SIPX_FAILED_RELEASE_RUN_ID[\"']?"),
        ("failed release jobs are not queried", r"actions/runs/\$SIPX_FAILED_RELEASE_RUN_ID/jobs\?filter=latest&per_page=100"),
        ("failed run is not bound to the ordinary workflow", r"run\.get\([\"']path[\"']\)\s*!=\s*[\"']\.github/workflows/crates-io\.yml[\"']"),
        ("failed run is not bound to tag and release SHA", r"run\.get\([\"']head_sha[\"']\)\s*!=\s*sha\s+or\s+run\.get\([\"']head_branch[\"']\)\s*!=\s*tag"),
        ("recovery accepts a run that did not fail", r"run\.get\([\"']status[\"']\)\s*!=\s*[\"']completed[\"']\s+or\s+run\.get\([\"']conclusion[\"']\)\s*!=\s*[\"']failure[\"']"),
        ("recovery does not require the complete gate to have succeeded", r"Run the complete release gate[\"']:\s*[\"']success"),
        ("recovery does not require rehearsal to have succeeded", r"Rehearse the locked registry packages[\"']:\s*[\"']success"),
        ("recovery does not require publication to have failed", r"Publish dependency-ready frontiers under a finite bound[\"']:\s*[\"']failure"),
        ("failed-run evidence is not checked in step order", r"ordered\s*=.*?Validate the immutable annotated tag.*?Run the complete release gate.*?Rehearse the locked registry packages.*?Publish dependency-ready frontiers under a finite bound.*?numbers.*?sorted\(numbers\)"),
        ("recovery accepts downstream consumer evidence from the failed run", r"Verify the exact registry consumer and installed CLI[\"']:\s*[\"']skipped"),
        ("recovery accepts downstream Pages evidence from the failed run", r"Verify Pages deployment from the release commit[\"']:\s*[\"']skipped"),
        ("recovery accepts an earlier GitHub prerelease", r"publish or verify GitHub prerelease.*?conclusion.*?skipped"),
        ("Cargo secret does not use the repository convention in recovery", r"CARGO_REGISTRY_TOKEN:\s*\$\{\{\s*secrets\.CARGO_REGISTRY_TOKEN\s*\}\}"),
        ("empty Cargo secret is not refused in recovery", r"-z [\"']?\$CARGO_REGISTRY_TOKEN"),
        ("recovery publication does not use the fixed controller", r"working-directory:\s*controller\s*\n\s+run:\s*\|.*?\./scripts/release\.py"),
        ("recovery does not install the pinned packager toolchain", r"rustup toolchain install [\"']\$RUSTUP_TOOLCHAIN[\"'] --profile minimal"),
        ("recovery publication does not name the immutable release root", r"Resume dependency-ready frontiers.*?\./scripts/release\.py.*?--release-root [\"']\$SIPX_RELEASE_ROOT[\"'].*?--publish"),
        (
            "recovery does not recheck the remote tag object before every helper write",
            r"for \(\(invocation = 1; invocation <= max_invocations; invocation\+\+\)\); do.*?ls-remote --refs --tags origin .*?refs/tags/\$RELEASE_TAG.*?remote_tag_object.*?EXPECTED_RELEASE_TAG_OBJECT.*?\./scripts/release\.py.*?--publish",
        ),
        ("recovery authorization is not bound to tag, release SHA and failed run", r"--authorize-ci-recovery [\"']\$RELEASE_TAG@\$RELEASE_SHA@\$SIPX_FAILED_RELEASE_RUN_ID[\"']"),
        ("recovery publication does not use a finite visibility bound", r"--registry-wait-seconds\s+[1-9][0-9]*"),
        ("recovery frontier loop is not bounded by public package count", r"max_invocations=\$\(\(public_count \+ 1\)\).*?invocation <= max_invocations"),
        ("recovery frontier does not require the all-visible observation", r"all public packages are already registry-visible"),
        ("recovery exact consumer proof is absent", r"- name:\s*Verify the exact registry consumer and installed CLI.*?\./scripts/release\.py.*?--release-root [\"']\$SIPX_RELEASE_ROOT[\"'].*?--verify-consumer"),
        ("recovery consumer command has no finite bound", r"--consumer-timeout-seconds\s+[1-9][0-9]*"),
        ("recovery Pages run is not selected by release SHA", r"actions/workflows/ci\.yml/runs\?.*?head_sha=\$RELEASE_SHA"),
        ("recovery Pages result is not checked against release SHA", r"\.head_sha == env\.RELEASE_SHA"),
        ("recovery Pages evidence omits the deployment job", r"deploy docs site.*?conclusion == [\"']success[\"']"),
        ("recovery public guide is not probed", r"https://codewandler\.github\.io/sipx/docs/getting-started"),
        ("recovery public API is not probed", r"https://codewandler\.github\.io/sipx/api/sipx_call/index\.html"),
        ("recovery GitHub prerelease is not dependent", r"\n\s*github_release:\s*\n.*?needs:\s*recover"),
        ("recovery GitHub prerelease lacks least-privilege write authority", r"\n\s*github_release:\s*\n.*?permissions:\s*\n\s+contents:\s*write\s*\n\s+env:"),
        ("recovery prerelease checkout cannot prove the annotated tag object", r"Check out the recovered release record.*?fetch-depth:\s*0.*?persist-credentials:\s*false"),
        ("recovery write token is not scoped to prerelease step", r"Create or verify the recovered GitHub prerelease\s*\n\s+env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
        ("recovery GitHub prerelease does not verify the tag", r"gh release create .*?--verify-tag"),
        ("recovery GitHub Release is not a prerelease", r"gh release create .*?--prerelease"),
        ("recovery GitHub prerelease is not bound to release SHA", r"gh release create .*?--target [\"']\$RELEASE_SHA[\"']"),
        (
            "recovery does not recheck the tag object before GitHub prerelease handling",
            r"Create or verify the recovered GitHub prerelease.*?local_tag_object=.*?rev-parse .*?refs/tags/\$RELEASE_TAG.*?remote_tag_object=.*?ls-remote --refs --tags origin .*?refs/tags/\$RELEASE_TAG.*?local_tag_object.*?RELEASE_TAG_OBJECT.*?remote_tag_object.*?RELEASE_TAG_OBJECT.*?gh release view",
        ),
        ("recovery GitHub prerelease does not consume reviewed notes", r"gh release create .*?--notes-file [\"']\$RELEASE_NOTES[\"']"),
        ("recovery does not verify an existing prerelease", r"gh release view .*?record\.get\([\"']prerelease[\"']\).*?reviewed notes differ"),
    )
    for label, pattern in checks:
        required(text, label, pattern, problems)

    if re.search(r"(?m)^  (?:push|pull_request|schedule):", text):
        problems.append("recovery has an automatic entry")
    if re.search(r"(?m)^\s*cargo\s+publish\b", text):
        problems.append("recovery calls cargo publish directly")
    recover_job = re.search(r"(?ms)^  recover:\s*$.*?(?=^  github_release:\s*$)", text)
    if recover_job is not None and re.search(r"(?m)^\s+contents:\s*write\s*$", recover_job.group()):
        problems.append("recovery publication job can write repository contents")
    if re.search(r"(?m)^      (?:GH_TOKEN|CARGO_REGISTRY_TOKEN):", text):
        problems.append("a recovery credential is exposed at job scope")

    posting_patterns = (
        r"\bgh\s+(?:issue|pr)\s+(?:create|comment)\b",
        r"\bgh\s+api\b[^\n]*(?:--method|-X)\s+POST\b",
        r"\bcurl\b[^\n]*(?:--request|-X)\s+POST\b",
        r"\brepository_dispatch\b",
    )
    if any(re.search(pattern, text, re.IGNORECASE) for pattern in posting_patterns):
        problems.append("recovery contains an external announcement or posting side effect")

    ordered = (
        "- name: Authorize recovery from the failed release evidence",
        "- name: Require the approved Cargo credential",
        "- name: Resume dependency-ready frontiers",
        "- name: Verify the exact registry consumer",
        "- name: Verify Pages deployment",
        "- name: Create or verify the recovered GitHub prerelease",
    )
    positions = [text.find(marker) for marker in ordered]
    if all(position >= 0 for position in positions) and positions != sorted(positions):
        problems.append("recovery evidence, credential, publication and downstream proofs are out of order")

    return problems


def beta1_replay_workflow_problems(text: str) -> list[str]:
    """Return defects in the hard-bound historical beta.1 first-publication replay."""

    problems: list[str] = []
    checks = (
        ("beta.1 replay permissions are not read-only", r"permissions:\s*\n\s+actions:\s*read\s*\n\s+contents:\s*read\s*\n\s+pages:\s*read"),
        ("beta.1 replay does not serialize with beta.1 publication", r"group:\s*crates-io-v1\.0\.0-beta\.1"),
        ("beta.1 replay concurrency can cancel publication", r"cancel-in-progress:\s*false"),
        ("beta.1 replay runs outside the protected release environment", r"\n\s*replay:\s*\n.*?environment:\s*\n\s+name:\s*release"),
        ("beta.1 replay job has no finite timeout", r"\n\s*replay:\s*\n(?:(?!\n\s*github_release:).)*?timeout-minutes:\s*[1-9][0-9]*"),
        ("beta.1 replay tag constant differs", r"RELEASE_TAG:\s*v1\.0\.0-beta\.1"),
        ("beta.1 replay commit constant differs", r"RELEASE_SHA:\s*3ab81709c7a235831638c62eba5fe73ce9eb7773"),
        ("beta.1 replay failed-run constant differs", r"SIPX_FAILED_RELEASE_RUN_ID:\s*30906820031"),
        ("beta.1 replay tag-object constant differs", r"EXPECTED_RELEASE_TAG_OBJECT:\s*b0bcadcc2a69a5824ec4a9549f7800c88c4f13fa"),
        ("beta.1 replay Pages run constant differs", r"EXPECTED_PAGES_RUN_ID:\s*30906258443"),
        ("beta.1 replay Pages artifact constant differs", r"EXPECTED_PAGES_ARTIFACT_ID:\s*8891214271"),
        ("beta.1 replay does not define separate controller and release roots", r"CONTROLLER_ROOT:.*?/controller\s*\n\s+SIPX_RELEASE_ROOT:.*?/release"),
        (
            "beta.1 replay controller checkout is absent or mutable",
            r"Check out the fixed beta\.1 replay controller\s*\n(?:(?!\n\s+- name:).)*?uses:\s*actions/checkout@v4(?:(?!\n\s+- name:).)*?ref:\s*\$\{\{\s*github\.sha\s*\}\}(?:(?!\n\s+- name:).)*?path:\s*controller(?:(?!\n\s+- name:).)*?fetch-depth:\s*0(?:(?!\n\s+- name:).)*?persist-credentials:\s*false",
        ),
        (
            "beta.1 replay immutable checkout is absent or not separate",
            r"Check out the immutable beta\.1 tag separately\s*\n(?:(?!\n\s+- name:).)*?uses:\s*actions/checkout@v4(?:(?!\n\s+- name:).)*?ref:\s*refs/tags/v1\.0\.0-beta\.1(?:(?!\n\s+- name:).)*?path:\s*release(?:(?!\n\s+- name:).)*?fetch-depth:\s*0(?:(?!\n\s+- name:).)*?persist-credentials:\s*false",
        ),
        ("beta.1 replay workflow source is not exact main", r"expected_workflow_ref=.*?\.github/workflows/crates-io-beta1-replay\.yml@refs/heads/main.*?GITHUB_WORKFLOW_REF.*?expected_workflow_ref"),
        ("beta.1 replay controller is not bound to event/workflow SHA", r"GITHUB_SHA.*?GITHUB_WORKFLOW_SHA.*?controller_sha.*?GITHUB_SHA"),
        ("beta.1 replay controller cleanliness is not required", r"CONTROLLER_ROOT.*?status --porcelain=v1 --untracked-files=all"),
        ("beta.1 replay does not require the annotated tag", r"SIPX_RELEASE_ROOT.*?cat-file -t .*?refs/tags/\$RELEASE_TAG.*?!= tag"),
        ("beta.1 replay does not bind local and remote tag objects", r"local_tag_object=.*?refs/tags/\$RELEASE_TAG.*?remote_tag_object=.*?ls-remote --refs --tags origin .*?EXPECTED_RELEASE_TAG_OBJECT"),
        ("beta.1 replay does not bind the peeled commit", r"release_sha=.*?refs/tags/\$RELEASE_TAG\^\{commit\}.*?RELEASE_SHA"),
        ("beta.1 replay failed run is not queried", r"actions/runs/\$SIPX_FAILED_RELEASE_RUN_ID"),
        ("beta.1 replay failed jobs are not queried", r"actions/runs/\$SIPX_FAILED_RELEASE_RUN_ID/jobs\?filter=latest&per_page=100"),
        ("beta.1 replay does not require the original gate failure", r"Run the complete release gate[\"']:\s*[\"']failure"),
        ("beta.1 replay does not require original rehearsal to be skipped", r"Rehearse the locked registry packages[\"']:\s*[\"']skipped"),
        ("beta.1 replay does not require original publication to be skipped", r"Publish dependency-ready frontiers under a finite bound[\"']:\s*[\"']skipped"),
        ("beta.1 replay does not require original consumer to be skipped", r"Verify the exact registry consumer and installed CLI[\"']:\s*[\"']skipped"),
        ("beta.1 replay does not require original Pages proof to be skipped", r"Verify Pages deployment from the release commit[\"']:\s*[\"']skipped"),
        ("beta.1 replay does not rerun the complete gate", r"- name:\s*Rerun the complete beta\.1 gate.*?working-directory:\s*release.*?\./scripts/gate\.py"),
        ("beta.1 replay gate lacks the provenance input", r"Rerun the complete beta\.1 gate\s*\n\s+env:\s*\n\s+SIPX_DENYLIST:\s*\$\{\{\s*secrets\.SIPX_DENYLIST\s*\}\}"),
        ("beta.1 replay does not rerun locked rehearsal", r"- name:\s*Rehearse the immutable beta\.1 packages.*?working-directory:\s*controller.*?--release-root [\"']\$SIPX_RELEASE_ROOT[\"'].*?--dry-run"),
        ("beta.1 replay Cargo secret does not use the repository convention", r"CARGO_REGISTRY_TOKEN:\s*\$\{\{\s*secrets\.CARGO_REGISTRY_TOKEN\s*\}\}"),
        ("beta.1 replay does not refuse an empty Cargo secret", r"-z [\"']?\$CARGO_REGISTRY_TOKEN"),
        ("beta.1 replay does not use its distinct helper authority", r"--authorize-ci-beta1-replay [\"']\$RELEASE_TAG@\$RELEASE_SHA@\$SIPX_FAILED_RELEASE_RUN_ID[\"']"),
        ("beta.1 replay does not recheck remote tag before publication", r"for \(\(invocation = 1; invocation <= max_invocations; invocation\+\+\)\); do.*?ls-remote --refs --tags origin .*?EXPECTED_RELEASE_TAG_OBJECT.*?--authorize-ci-beta1-replay"),
        ("beta.1 replay frontier loop is not bounded", r"max_invocations=\$\(\(public_count \+ 1\)\).*?invocation <= max_invocations"),
        ("beta.1 replay does not require all-visible observation", r"all public packages are already registry-visible"),
        ("beta.1 replay exact consumer proof is absent", r"Verify the exact beta\.1 registry consumer and installed CLI.*?--release-root [\"']\$SIPX_RELEASE_ROOT[\"'].*?--verify-consumer"),
        ("beta.1 replay does not inspect both archived Pages surfaces", r"\./docs/getting-started\.html\s+\./api/sipx_call/index\.html"),
        ("beta.1 replay does not download the exact Pages artifact", r"actions/artifacts/\$EXPECTED_PAGES_ARTIFACT_ID/zip"),
        ("beta.1 replay does not keep live docs on beta.2", r"codewandler\.github\.io/sipx/docs/getting-started.*?1\.0\.0-beta\.2"),
        ("beta.1 replay GitHub prerelease is not dependent", r"\n\s*github_release:\s*\n.*?needs:\s*replay"),
        ("beta.1 replay GitHub prerelease lacks write authority", r"\n\s*github_release:\s*\n.*?permissions:\s*\n\s+contents:\s*write"),
        ("beta.1 replay does not recheck tag before GitHub Release", r"Create or verify the replayed beta\.1 GitHub prerelease.*?local_tag_object=.*?remote_tag_object=.*?RELEASE_TAG_OBJECT.*?gh release view"),
        ("beta.1 replay GitHub Release is not a prerelease", r"gh release create .*?--prerelease"),
        ("beta.1 replay may become latest", r"gh release create .*?--latest=false"),
        ("beta.1 replay does not consume reviewed notes", r"gh release create .*?--notes-file [\"']\$RELEASE_NOTES[\"']"),
        ("beta.1 replay does not verify an existing release", r"gh release view .*?target_commitish.*?reviewed replay notes differ"),
    )
    for label, pattern in checks:
        required(text, label, pattern, problems)

    if re.search(r"(?m)^  (?:push|pull_request|schedule):", text):
        problems.append("beta.1 replay has an automatic entry")
    if re.search(r"(?m)^\s*cargo\s+publish\b", text):
        problems.append("beta.1 replay calls cargo publish directly")
    replay_job = re.search(r"(?ms)^  replay:\s*$.*?(?=^  github_release:\s*$)", text)
    if replay_job is not None and re.search(r"(?m)^\s+contents:\s*write\s*$", replay_job.group()):
        problems.append("beta.1 replay publication job can write repository contents")
    if re.search(r"(?m)^      (?:GH_TOKEN|CARGO_REGISTRY_TOKEN|SIPX_DENYLIST):", text):
        problems.append("a beta.1 replay credential is exposed at job scope")

    posting_patterns = (
        r"\bgh\s+(?:issue|pr)\s+(?:create|comment)\b",
        r"\bgh\s+api\b[^\n]*(?:--method|-X)\s+POST\b",
        r"\bcurl\b[^\n]*(?:--request|-X)\s+POST\b",
        r"\brepository_dispatch\b",
    )
    if any(re.search(pattern, text, re.IGNORECASE) for pattern in posting_patterns):
        problems.append("beta.1 replay contains an external announcement or posting side effect")

    ordered = (
        "- name: Authorize beta.1 replay from the failed release evidence",
        "- name: Rerun the complete beta.1 gate",
        "- name: Rehearse the immutable beta.1 packages",
        "- name: Require the approved Cargo credential for beta.1 replay",
        "- name: Replay dependency-ready beta.1 frontiers",
        "- name: Verify the exact beta.1 registry consumer",
        "- name: Verify the archived beta.1 Pages artifact",
        "- name: Create or verify the replayed beta.1 GitHub prerelease",
    )
    positions = [text.find(marker) for marker in ordered]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        problems.append("beta.1 replay credential precedes evidence, gate or rehearsal")
    return problems


def specification_problems(text: str) -> list[str]:
    """Require the normative contract to retain its authority boundaries."""

    problems: list[str] = []
    required(
        text,
        "specification does not separate the GitHub prerelease from broader publicity",
        r"MUST NOT post broader publicity",
        problems,
    )
    required(
        text,
        "specification does not confine the provenance denylist to the gate step",
        r"`SIPX_DENYLIST`.*?MUST be exposed only to the complete-gate step",
        problems,
    )
    required(
        text,
        "specification does not bind recovery to failed-run evidence before credentials",
        r"recovery workflow MUST verify the named failed run through the Actions API before exposing the\s+Cargo credential",
        problems,
    )
    required(
        text,
        "specification does not separate recovery tooling from release bytes",
        r"Recovery tooling and\s+release bytes live in separate checkouts",
        problems,
    )
    required(
        text,
        "specification does not hard-bind the beta.1 replay incident",
        r"beta\.1 replay workflow MUST verify failed run `30906820031`.*?b0bcadcc2a69a5824ec4a9549f7800c88c4f13fa.*?3ab81709c7a235831638c62eba5fe73ce9eb7773",
        problems,
    )
    required(
        text,
        "specification does not keep beta.1 superseded",
        r"beta\.1 replay takes its reviewed body.*?superseded status.*?beta\.2 as current",
        problems,
    )
    return problems


def check(root: pathlib.Path = ROOT) -> list[str]:
    """Check the real workflows and their normative specification."""

    workflow = root / ".github" / "workflows" / "crates-io.yml"
    resume_workflow = root / ".github" / "workflows" / "crates-io-resume.yml"
    beta1_replay_workflow = root / ".github" / "workflows" / "crates-io-beta1-replay.yml"
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
    if not resume_workflow.is_file():
        problems.append(f"missing {resume_workflow.relative_to(root)}")
    else:
        problems.extend(resume_workflow_problems(resume_workflow.read_text(encoding="utf-8")))
    if not beta1_replay_workflow.is_file():
        problems.append(f"missing {beta1_replay_workflow.relative_to(root)}")
    else:
        problems.extend(
            beta1_replay_workflow_problems(
                beta1_replay_workflow.read_text(encoding="utf-8")
            )
        )
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
        print(
            "release workflow: approved tag, partial recovery and fixed beta.1 replay, "
            "bounded registry, Pages and resumable GitHub prerelease"
        )
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
