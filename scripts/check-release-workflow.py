#!/usr/bin/env python3
"""Statically hold ordinary and recovery workflows to the release contracts."""

from __future__ import annotations

import pathlib
import re
import sys
from collections.abc import Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "crates-io.yml"
RESUME_WORKFLOW = ROOT / ".github" / "workflows" / "crates-io-resume.yml"
SPEC = ROOT / "docs" / "specs" / "release-workflow.md"

#: Steps of the ordinary release job that X-93's speed work is not allowed to move, skip or make
#: conditional. Named here because every rule below that talks about the preflight or the artifact
#: cache is positional relative to one of them, and a name spelled twice is a name that drifts.
PREFLIGHT_STEP = "Require exact-SHA main CI and Pages deployment evidence"
CACHE_KEY_STEP = "Derive the release build cache key"
CACHE_KEY_STEP_ID = "build_cache_key"
CACHE_RESTORE_STEP = "Restore the Actions-managed Rust artifact cache"
CACHE_SAVE_STEP = "Save the Actions-managed Rust artifact cache"
TIMINGS_STEP = "Record the gate's cold or warm timings"
TAG_VALIDATION_STEP = "Validate the immutable annotated tag"
GATE_STEP = "Run the complete release gate"

#: Every step that carries a normative release claim. None of them may be skipped, reordered behind
#: a cache, or conditioned on anything: a cache is an optimisation, and an optimisation that can
#: decide whether a proof runs is not an optimisation any more.
NORMATIVE_STEPS = (
    TAG_VALIDATION_STEP,
    PREFLIGHT_STEP,
    GATE_STEP,
    "Rehearse the locked registry packages",
    "Publish dependency-ready frontiers under a finite bound",
    "Verify the exact registry consumer and installed CLI",
    "Verify Pages deployment from the release commit",
)

#: Exactly what the release job's artifact cache may hold. An allow-list rather than a deny-list:
#: the claim it must not weaken is the *isolated* one — `release.py` gives package rehearsal, the
#: resume proof and the registry consumer their own `CARGO_HOME` and `--target-dir` under the
#: runner temp, and a cache that reached any of those would turn a fresh registry fetch into a
#: replay of bytes this run already had.
CACHEABLE_PATHS = (
    "~/.cargo/registry/index",
    "~/.cargo/registry/cache",
    "~/.cargo/git/db",
    "target",
)

#: What the release build cache key must be derived from, and the token that proves each input was
#: read rather than assumed. Anything absent here makes the key coarser than the artifacts it
#: names, which is the failure mode that serves a stale build to a release gate.
CACHE_KEY_INPUTS = (
    ("runner image", r"\$RUNNER_OS-\$RUNNER_ARCH-\$\{ImageOS"),
    ("the workspace lockfile", r"sha256sum Cargo\.lock"),
    ("the CI flags the gate runs under", r"ci-flags=[^\n]*\.github/workflows/ci\.yml"),
    ("the stable toolchain", r"rustc \+stable -vV"),
    ("the MSRV toolchain", r"rustc \+\"\$msrv\" -vV"),
    ("the native feature libraries", r"pkg-config --modversion opus openssl alsa"),
)


def required(text: str, label: str, pattern: str, problems: list[str]) -> None:
    """Append one named structural defect when ``pattern`` is absent."""

    if re.search(pattern, text, re.MULTILINE | re.DOTALL) is None:
        problems.append(label)


def step_body(text: str, name: str) -> str:
    """One named step of the release job, up to the next step, or `""` when there is no such step.

    Sliced rather than matched across the whole file on purpose. Both the preflight and the
    post-consumer Pages proof query the same Actions endpoints, so a whole-file `re.search` for
    either one's evidence is answered by the other's copy — and "the preflight proved it" versus
    "something later proved it" is the entire distinction these rules exist to hold.
    """

    match = re.search(
        rf"(?ms)^      - name: {re.escape(name)}$.*?(?=^      - (?:name|uses|id):|\Z)", text
    )
    return "" if match is None else match.group()


def _step_position(text: str, name: str) -> int:
    """Where one named step starts, or `-1`."""

    return text.find(f"      - name: {name}")


def preflight_problems(text: str) -> list[str]:
    """Hold the cheap read-only evidence check in front of the gate, without letting it become one.

    X-93. The gate is the expensive half of a release and it ran first, so a run whose exact-SHA
    `main` CI evidence never existed still paid twelve minutes to find that out. Reading the
    evidence first is free. What it must never become is a *substitute*: CI succeeding on the same
    commit is not the release gate, and the deployment job answering the API is not the deployed
    page answering HTTP.
    """

    problems: list[str] = []
    preflight = step_body(text, PREFLIGHT_STEP)
    if not preflight:
        problems.append("read-only exact-SHA CI and Pages preflight is absent")
        return problems

    gate = _step_position(text, GATE_STEP)
    validation = _step_position(text, TAG_VALIDATION_STEP)
    position = _step_position(text, PREFLIGHT_STEP)
    if validation < 0 or not validation < position:
        problems.append("preflight reads Actions evidence before the tag is validated")
    if gate < 0 or not position < gate:
        problems.append("preflight does not run before the expensive gate")

    for label, pattern in (
        ("preflight does not select the CI run by release head SHA", r"actions/workflows/ci\.yml/runs\?[^\n]*head_sha=\$RELEASE_SHA"),
        ("preflight does not recheck the returned run against the release head SHA", r"\.head_sha == env\.RELEASE_SHA"),
        ("preflight does not require the successful Pages deployment job", r"deploy docs site[^\n]*conclusion == [\"']success[\"']"),
        ("preflight accepts a missing exact-SHA CI run", r"no successful main CI run has head_sha \$RELEASE_SHA.*?exit 1"),
        ("preflight accepts a missing Pages deployment job", r"has no successful deploy docs site job.*?exit 1"),
        ("preflight GitHub read token is not scoped to the preflight step", r"env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
    ):
        required(preflight, label, pattern, problems)

    # The preflight is evidence *about* a deployment, never the deployment answering. Both public
    # probes stay where the state machine puts them: after the registry consumer proof.
    if re.search(r"(?m)^\s*curl\b", preflight):
        problems.append("preflight probes the public site instead of leaving that to the Pages proof")
    for label, pattern in (
        ("public guide probe is no longer the single post-consumer proof", r"https://codewandler\.github\.io/sipx/docs/getting-started"),
        ("public API probe is no longer the single post-consumer proof", r"https://codewandler\.github\.io/sipx/api/sipx_call/index\.html"),
    ):
        if len(re.findall(pattern, text)) != 1:
            problems.append(label)

    # Counted, not merely present. The preflight queries the same two endpoints as the
    # post-consumer proof, so a presence check on either sentence is answered by whichever copy
    # survives — and one copy is exactly the substitution this story must not ship.
    for label, pattern in (
        ("Pages run is not selected by release head SHA", r"actions/workflows/ci\.yml/runs\?[^\n]*head_sha=\$RELEASE_SHA"),
        ("returned Pages run is not checked against release head SHA", r"\.head_sha == env\.RELEASE_SHA"),
        ("Pages evidence does not require the deployment job", r"deploy docs site[^\n]*conclusion == [\"']success[\"']"),
    ):
        if len(re.findall(pattern, text)) != 2:
            problems.append(label)

    return problems


def _cache_paths(step: str) -> list[str]:
    """The literal paths one cache step names, in order."""

    match = re.search(r"(?ms)^          path:\s*\|\s*\n(.*?)(?=^          \S|^      - |\Z)", step)
    if match is None:
        return []
    return [line.strip() for line in match.group(1).splitlines() if line.strip()]


def build_cache_problems(text: str) -> list[str]:
    """Hold the artifact cache to being an optimisation and nothing else.

    X-93 asks for a faster release without a weaker one, and every way a cache weakens a release is
    a placement question rather than a correctness one: restored before the tag is validated it is
    an input nobody authorised; reaching the isolated consumer roots it replays bytes the release is
    supposed to fetch fresh; conditioning a step it decides whether a proof runs; and a shared
    `CARGO_TARGET_DIR` hands the release gate a directory some other run owns. So the rules here are
    positional and exhaustive, and the retention decision is left to recorded timings.
    """

    problems: list[str] = []
    restore = step_body(text, CACHE_RESTORE_STEP)
    save = step_body(text, CACHE_SAVE_STEP)
    key_step = step_body(text, CACHE_KEY_STEP)
    timings = step_body(text, TIMINGS_STEP)

    # A shared build directory is refused outright rather than checked for where it points: X-34
    # recorded the decision against one, and a release job is the last place to reopen it.
    if "CARGO_TARGET_DIR" in text:
        problems.append("release workflow sets a shared CARGO_TARGET_DIR")

    # A proof that runs only on a cache miss, or only on a cache hit, is not a proof. The one place
    # a cache result may decide anything is the cache's own save: re-archiving a ten-gigabyte build
    # directory that was restored byte-for-byte would make a hit *slower* than a miss, which is the
    # opposite of what this story is for.
    if re.search(r"if:[^\n]*cache-hit", text.replace(save, "", 1) if save else text):
        problems.append("a release step is conditioned on a cache hit")
    for name in NORMATIVE_STEPS:
        body = step_body(text, name)
        if body and re.search(r"(?m)^        if:", body):
            problems.append(f"normative release step {name!r} is conditional")

    if not restore:
        problems.append("Actions-managed Rust artifact cache is never restored")
    if not save:
        problems.append("Actions-managed Rust artifact cache is never saved")
    if not key_step:
        problems.append("release build cache key is not derived from named inputs")
    if not restore or not save or not key_step:
        return problems

    validation = _step_position(text, TAG_VALIDATION_STEP)
    restored_at = _step_position(text, CACHE_RESTORE_STEP)
    keyed_at = _step_position(text, CACHE_KEY_STEP)
    gate_at = _step_position(text, GATE_STEP)
    saved_at = _step_position(text, CACHE_SAVE_STEP)
    if validation < 0 or not validation < keyed_at < restored_at:
        problems.append("artifact cache is restored before the immutable tag is validated")
    if gate_at < 0 or not restored_at < gate_at < saved_at:
        problems.append("artifact cache is not restored before and saved after the complete gate")

    for label, pattern in (
        ("artifact cache is not the first-party Actions cache", r"uses:\s*actions/cache/restore@v4"),
        ("artifact cache save is not the first-party Actions cache", r"uses:\s*actions/cache/save@v4"),
    ):
        required(restore + save, label, pattern, problems)

    # A cache that can end a release is worse than no cache: a corrupt or half-written entry is an
    # infrastructure fact, and the gate behind it still runs every step either way.
    for name, step in ((CACHE_RESTORE_STEP, restore), (CACHE_SAVE_STEP, save)):
        if re.search(r"(?m)^        continue-on-error:\s*true", step) is None:
            problems.append(f"a failed cache step can stop the release: {name!r}")
    if re.search(r"(?m)^          restore-keys:", restore) is not None:
        problems.append("artifact cache accepts an inexact restore key")
    if re.search(r"(?m)^        if:[^\n]*cache-hit != 'true'", save) is None:
        problems.append("artifact cache is re-archived even when the exact entry was restored")

    for label, pattern in CACHE_KEY_INPUTS:
        required(key_step, f"release build cache key does not cover {label}", pattern, problems)
    if re.search(r"key:\s*[^\n]*steps\." + CACHE_KEY_STEP_ID + r"\.outputs\.key", restore) is None:
        problems.append("artifact cache key is not the derived key")

    for name, step in ((CACHE_RESTORE_STEP, restore), (CACHE_SAVE_STEP, save)):
        paths = _cache_paths(step)
        if list(CACHEABLE_PATHS) != paths:
            problems.append(f"{name!r} does not cache exactly the workspace build and registry downloads")

    # The gate's own clock is the only source of a cold-versus-warm figure, so a cache that is not
    # measured cannot be retained or dropped on evidence. Recording it is part of the mechanism.
    required(text, "the gate run records no timings", r"\./scripts/gate\.py --timings [\"']\$GATE_TIMINGS[\"']", problems)
    if not timings:
        problems.append("the gate's cold or warm timings are never recorded")
    else:
        for label, pattern in (
            ("recorded timings do not state whether the build directory was warm", r"cache"),
            ("recorded timings are not preserved as run evidence", r"GITHUB_STEP_SUMMARY"),
            ("recorded timings are skipped when the gate fails", r"(?m)^        if:\s*always\(\)"),
        ):
            required(timings, label, pattern, problems)

    # The Node dependency cache. `build-docs.sh` installs only when `website/node_modules` is
    # absent, so an exact-lock cache of that directory can skip installation and nothing else —
    # but only while no cache reaches the site output or the rustdoc build it also produces.
    required(
        text,
        "the Node dependency cache is not keyed on the exact website lockfile",
        r"cache:\s*npm\s*\n\s+cache-dependency-path:\s*website/package-lock\.json",
        problems,
    )
    for forbidden in ("website/build", "target/doc"):
        if any(forbidden in path for path in _cache_paths(restore) + _cache_paths(save)):
            problems.append(f"a cache holds {forbidden}, which would skip a build rather than an install")

    return problems


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
        ("release job has no finite timeout", r"\n  release:\s*\n\s+name:[^\n]*\n\s+runs-on:[^\n]*\n\s+timeout-minutes:\s*[1-9][0-9]*"),
        ("release concurrency can cancel a publication", r"cancel-in-progress:\s*false"),
        ("release concurrency is not keyed by tag", r"group:.*(?:inputs\.tag|RELEASE_TAG|ref_name)"),
        ("workflow cannot read Actions evidence", r"actions:\s*read"),
        ("workflow permissions are not read-only", r"permissions:\s*\n\s+actions:\s*read\s*\n\s+contents:\s*read\s*\n\s+pages:\s*read"),
        ("GitHub release does not depend on registry and artifact evidence", r"\n\s*github_release:\s*\n.*?needs:\s*\[release, cli_artifact_set\]"),
        ("GitHub release lacks job-scoped write authority", r"\n\s*github_release:\s*\n.*?permissions:\s*\n\s+contents:\s*write"),
        ("workflow cannot read Pages evidence", r"pages:\s*read"),
        ("Cargo secret does not use the repository convention", r"CARGO_REGISTRY_TOKEN:\s*\$\{\{\s*secrets\.CARGO_REGISTRY_TOKEN\s*\}\}"),
        ("empty Cargo secret is not refused", r"-z [\"']?\$CARGO_REGISTRY_TOKEN"),
        ("checkout does not select the release tag", r"ref:\s*\$\{\{\s*env\.RELEASE_TAG\s*\}\}"),
        ("checkout does not fetch tag history", r"fetch-depth:\s*0"),
        ("release checkout persists a credential", r"Check out the exact tag\s*\n(?:(?!\n\s+- name:).)*?persist-credentials:\s*false"),
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
        ("GitHub read token is not scoped to the Pages step", r"Verify Pages deployment from the release commit\s*\n\s+env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
        ("public guide is not probed", r"https://codewandler\.github\.io/sipx/docs/getting-started"),
        ("public API is not probed", r"https://codewandler\.github\.io/sipx/api/sipx_call/index\.html"),
        ("GitHub release checkout persists a credential", r"Check out the reviewed release record.*?persist-credentials:\s*false"),
        ("GitHub write token is not scoped to the release step", r"Create or verify the GitHub release and assets\s*\n\s+env:\s*\n\s+GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}"),
        ("GitHub release does not verify the existing tag", r"gh release create .*?--verify-tag"),
        ("stable and prerelease kinds are not selected from the version", r"RELEASE_VERSION[\"']? == \*-\*.*?prerelease=true.*?release_flag=--prerelease"),
        ("stable GitHub Release is not marked latest", r"release_flag=--latest"),
        ("GitHub release does not consume reviewed notes", r"gh release create .*?--notes-file [\"']\$RELEASE_NOTES[\"']"),
        ("existing GitHub release kind is not verified", r"record\.get\([\"']prerelease[\"']\) is not prerelease"),
        ("resume does not verify the existing GitHub release", r"gh release view .*?record\.get\([\"']prerelease[\"']\).*?reviewed notes differ"),
        ("release commit timestamp is not exported for deterministic artifacts", r"release_epoch=.*?git show -s --format=%ct.*?echo [\"']epoch=\$release_epoch"),
        ("portable artifact matrix job is absent", r"\n\s*cli_artifacts:\s*\n.*?needs:\s*release"),
        ("artifact matrix is not bounded", r"\n\s*cli_artifacts:\s*\n.*?timeout-minutes:\s*[1-9][0-9]*.*?fail-fast:\s*false"),
        ("artifact source is not the validated release tag", r"Check out the immutable artifact source.*?ref:\s*\$\{\{\s*needs\.release\.outputs\.release_tag\s*\}\}.*?persist-credentials:\s*false"),
        ("artifact toolchain is not selected from workspace rust-version", r"rust-version.*?RUSTUP_TOOLCHAIN=\$rust_version"),
        ("artifact build does not use the locked no-feature target command", r"cargo build --locked --release -p sipx-cli --target [\"']\$TARGET[\"'] --no-default-features"),
        ("portable device-audio compile check is absent", r"runner\.os == [\"']macOS[\"'] \|\| runner\.os == [\"']Windows[\"'].*?cargo check --locked -p sipx-cli --target [\"']\$TARGET[\"'] --all-targets --features device-audio"),
        ("native artifact proof does not invoke the packager", r"release-artifacts\.py package.*?--binary [\"']\$BINARY[\"'].*?--source-date-epoch [\"']\$RELEASE_EPOCH[\"']"),
        ("artifact aggregation does not require every matrix job", r"\n\s*cli_artifact_set:\s*\n.*?needs:\s*\[release, cli_artifacts\]"),
        ("artifact aggregation does not invoke the exact-set validator", r"release-artifacts\.py aggregate.*?--input .*?--release-sha [\"']\$RELEASE_SHA[\"'].*?--source-date-epoch [\"']\$RELEASE_EPOCH[\"']"),
        ("GitHub release does not download the verified asset set", r"Download the verified release asset set.*?name:\s*cli-release-assets"),
        ("existing release assets are not byte-checked before upload", r"release-artifacts\.py compare.*?--allow-missing.*?gh release upload"),
        ("completed release assets are not byte-checked", r"gh release download [\"']\$RELEASE_TAG[\"'] --dir [\"']\$verified[\"'].*?release-artifacts\.py compare.*?--actual [\"']\$verified[\"']"),
    )
    for label, pattern in checks:
        required(text, label, pattern, problems)

    problems.extend(preflight_problems(text))
    problems.extend(build_cache_problems(text))

    if re.search(r"(?m)^\s*cargo\s+publish\b", text):
        problems.append("workflow calls cargo publish directly instead of the release helper")
    for target, runner in (
        ("x86_64-unknown-linux-musl", "ubuntu-24.04"),
        ("aarch64-unknown-linux-musl", "ubuntu-24.04-arm"),
        ("x86_64-apple-darwin", "macos-15-intel"),
        ("aarch64-apple-darwin", "macos-15"),
        ("x86_64-pc-windows-msvc", "windows-2025"),
    ):
        if len(re.findall(rf"target:\s*{re.escape(target)}\s*\n\s+runner:\s*{re.escape(runner)}", text)) != 1:
            problems.append(f"artifact matrix does not contain exactly one {target} on {runner}")
    if "--clobber" in text:
        problems.append("release assets may be overwritten")

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
        ("Rehearse the locked registry packages", "locked rehearsal"),
        ("Publish dependency-ready frontiers", "publication"),
        ("Verify the exact registry consumer", "consumer proof"),
        ("Verify Pages deployment", "Pages proof"),
        ("Create or verify the GitHub release and assets", "GitHub release"),
    )
    positions = [(text.find(marker), label) for marker, label in ordered]
    if all(position >= 0 for position, _label in positions):
        if [position for position, _label in positions] != sorted(
            position for position, _label in positions
        ):
            problems.append(
                "locked rehearsal, publication, consumer, Pages and GitHub release steps "
                "are out of order"
            )

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
        "specification does not distinguish stable and prerelease records",
        r"prerelease suffix creates a prerelease; a\s+stable version creates a non-prerelease release",
        problems,
    )
    required(
        text,
        "specification permits existing release assets to be replaced",
        r"no existing\s+asset may be overwritten or deleted",
        problems,
    )
    # X-93's clauses, each matched across line wraps: the specification is prose the checker greps,
    # and a rule that breaks when a paragraph is rewrapped teaches people to stop rewrapping.
    for label, words in (
        (
            "specification does not put a read-only preflight in front of the gate",
            "Missing or wrong-SHA evidence MUST stop the release before the gate",
        ),
        (
            "specification lets the preflight replace the post-consumer Pages proof",
            "It MUST NOT probe the public site",
        ),
        (
            "specification lets CI success substitute for a normative proof",
            "MUST NOT be substituted for the complete gate or for any other normative proof",
        ),
        (
            "specification does not place the artifact cache after immutable-tag validation",
            "MAY be restored, and only after the immutable-tag facts of §3 are established",
        ),
        (
            "specification does not keep the isolated helper roots out of the cache",
            "no cache may hold the isolated `CARGO_HOME` or target directories",
        ),
        (
            "specification does not require every gate step to run on a cache miss",
            "MUST still run every one of the gate's steps",
        ),
        (
            "specification states no retention rule for the artifact cache",
            "retained only if the recorded cold and warm figures differ by at least 60 seconds",
        ),
        (
            "specification lets a Node dependency cache skip more than installation",
            "MAY skip installation and MUST NOT skip the site, anchor or rustdoc builds",
        ),
    ):
        required(text, label, r"\s+".join(re.escape(word) for word in words.split()), problems)
    return problems


def check(root: pathlib.Path = ROOT) -> list[str]:
    """Check the real workflows and their normative specification."""

    workflow = root / ".github" / "workflows" / "crates-io.yml"
    resume_workflow = root / ".github" / "workflows" / "crates-io-resume.yml"
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
            "release workflow: approved tag, bounded registry, portable artifacts, Pages, "
            "resumable GitHub release and failed-run recovery"
        )
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
