#!/usr/bin/env python3
"""Render docs/comparison.md from docs/comparison/, and check that it is not lying.

A comparison table is the `docs/compliance.md` failure mode with a larger blast radius, because
half its claims are about software this repository does not control and cannot test. So the same
two rules apply — generated, and checked — and the asymmetry is handled by a visible confidence
ladder rather than by being careful.

Four tiers, each with an obligation this script enforces. `generated` is computed from this
repository at render time and is restricted to the one stack marked `is_self`: the value is
substituted into a `{rule}` placeholder, so it is never typed and cannot be hand-edited without
the byte-compare noticing. `measured` must carry a `reproduce` command that re-derives the finding
from the subject at the version named. `documented` points at the subject's own published
material. `assessed` is reviewer judgment and must say what the judgment rests on.

What it deliberately does *not* do is judge whether an `assessed` rationale is fair, or whether
the evidence a row cites has anything to do with the question the dimension asks. Only a reader
can. What it can do is stop a claim outliving its evidence, and stop the whole page outliving its
subjects — an observation past the age limit fails, and the failure names the way to refresh it.

There is no list of claims this script agrees not to look at, under any name. The only way past a
rule is demotion to a lower tier or removal of the row, because both change what the published
page says.

The script names no comparison subject. Subjects live in `docs/comparison/stacks.json`, which is
inside the provenance scope this file is not — see `COMPARISON_SCOPE` in
`scripts/check-provenance.sh`.
"""

import argparse
import datetime
import importlib.util
import json
import os.path
import pathlib
import re
import statistics
import subprocess
import sys
import tempfile
import tomllib
from collections import Counter
from urllib.parse import urlparse

ROOT = pathlib.Path(__file__).resolve().parent.parent
COMPARISON = ROOT / "docs" / "comparison"
DIMENSIONS = COMPARISON / "dimensions.json"
STACKS = COMPARISON / "stacks.json"
OBSERVATIONS = COMPARISON / "observations"
CAPABILITIES = COMPARISON / "capabilities"
CAPABILITY_EXPECTED = CAPABILITIES / "expected"
EXTERNAL_STORIES = CAPABILITIES / "external"
REPORT = ROOT / "docs" / "comparison.md"

# Live sources for the generated tier. Each is read by its own rule below.
REGISTRY = ROOT / "docs" / "rfc" / "registry.toml"
MANIFEST = ROOT / "Cargo.toml"
TRANSPORT_KINDS = ROOT / "crates" / "sipx-transport" / "src" / "target.rs"
GATE = ROOT / "scripts" / "gate.py"
AUDIO_CLAIMS = ROOT / "scripts" / "check-audio-claims.py"

#: How long an observation may stand before `--check` refuses to publish it. Half a year is two
#: refreshes a year, and shorter than the release cadence of every subject the first dataset was
#: derived from — so a stack shipping a major version between refreshes is the case this catches.
#: Raising it is a decision, and it should look like one in a diff (`X-72`).
MAX_OBSERVATION_AGE_DAYS = 180

#: How long before the limit a run starts saying so. A wall with no notice is the failure people
#: learn to silence: `--check` was green until the day it was not, and the first dataset was derived
#: in one sitting, so every observation in it expires on the same day. Thirty days is roughly the
#: notice somebody needs to schedule a refresh deliberately rather than meet one mid-release, and it
#: is what makes a larger subject set survivable — subjects refreshed one at a time drift apart, and
#: the drift is the point (`X-77`).
STALE_WARNING_DAYS = 30

#: Named in the staleness failure, because a red gate that arrives on a date with no code change
#: behind it must be actionable or it becomes the thing people learn to silence.
REFRESH_COMMAND = "re-derive it with the compare-stacks skill, then ./scripts/comparison-report.py"

CONFIDENCE_TIERS = ("generated", "measured", "documented", "assessed")
CONFIDENCE_MEANING = {
    "generated": "Computed from this repository at render time",
    "measured": "A `reproduce` command re-derives it from the subject at the version named",
    "documented": "The subject's own documentation, release notes or advisories state it",
    "assessed": "Reviewer judgment from indirect evidence",
}
CONFIDENCE_HOLDER = {
    "generated": "this repository only",
    "measured": "any subject whose source can be read",
    "documented": "any subject",
    "assessed": "any subject, and kept in the minority",
}

# The key sets are closed. A checker that reads only the keys it knows walks past the rest, so a
# claim can sit in the source, never reach the generated page, and tell nobody — the failure this
# whole file exists to prevent, in miniature.
DIMENSION_KEYS = ({"id", "title", "question", "why"}, set())
STACK_KEYS = (
    {"id", "name", "language", "repository", "license"},
    {"is_self", "capability_inventory"},
)
FINDING_KEYS = (
    {"stack", "dimension", "confidence", "summary", "evidence", "version_evaluated",
     "evaluated_at"},
    {"reproduce", "rationale", "generated_from"},
)
# A marker says nobody looked, and nothing else. Letting it also carry a summary would give a row
# two states at once, which is the ambiguity the marker exists to remove.
MARKER_KEYS = ({"stack", "dimension", "not_evaluated"}, set())
EVIDENCE_KEYS = ({"note"}, {"url", "path"})
CAPABILITY_LEDGER_KEYS = (
    {
        "subject",
        "version_evaluated",
        "evaluated_at",
        "source_revision",
        "expected_capabilities",
        "capabilities",
    },
    set(),
)
CAPABILITY_KEYS = (
    {"id", "category", "title", "confidence", "ownership", "status", "evidence"},
    {"story", "rationale", "implementation"},
)

CAPABILITY_OWNERS = ("sipx", "sipx-clstr", "not-shipped", "not-applicable")
CAPABILITY_STATUS = {
    "sipx": {"implemented", "open"},
    "sipx-clstr": {"tracked"},
    "not-shipped": {"absent"},
    "not-applicable": {"excluded"},
}
CAPABILITY_CONFIDENCE = {"measured", "documented", "assessed"}
REQUIRED_CAPABILITY_CATEGORIES = {
    "authentication",
    "core",
    "dialogs",
    "endpoint",
    "examples",
    "lifecycle",
    "media",
    "methods",
    "operations",
    "transactions",
    "transports",
}

KEY_SETS = {
    "dimension": DIMENSION_KEYS,
    "stack": STACK_KEYS,
    "finding": FINDING_KEYS,
    "marker": MARKER_KEYS,
    "evidence": EVIDENCE_KEYS,
}

#: Placeholders a generated cell interpolates. `{rfc-count}` rather than a number, so the value
#: cannot be typed at all.
PLACEHOLDER = re.compile(r"\{([a-z0-9]+(?:-[a-z0-9]+)*)\}")

GENERATED_RULES = ("rfc-count", "gate-steps", "transports", "codecs", "unsafe-policy")

#: A fixed set for the checker's own tests, so they neither shell out nor depend on today's tree.
GENERATED_VALUES_FOR_TESTS = {rule: f"<{rule}>" for rule in GENERATED_RULES}


def _summary_line(script: pathlib.Path, pattern: re.Pattern, what: str) -> str:
    """Run a sibling checker and read one fact out of its success line.

    Raising rather than rendering is the `claimed_codecs` rule in `sync-website.py`: a red check
    means nothing is currently asserting the number, and publishing it anyway would make this page
    a second opinion about a fact that already has an owner.
    """
    done = subprocess.run(
        [sys.executable, str(script), "--check"],
        capture_output=True,
        text=True,
        cwd=ROOT,
        check=False,
    )
    if done.returncode != 0:
        raise ValueError(
            f"{script.name} is red; refusing to render {what} from it. Fix that check first"
        )
    found = pattern.search(done.stdout)
    if found is None:
        raise ValueError(f"{script.name} printed no {what} to read")
    return found.group(1)


def rule_rfc_count() -> str:
    """How many RFCs the conformance registry tracks."""
    return str(len(tomllib.loads(REGISTRY.read_text(encoding="utf-8"))["rfc"]))


def rule_gate_steps() -> str:
    """How many steps must pass before a change lands."""
    return _summary_line(GATE, re.compile(r"gate: (\d+) steps"), "a gate step count")


def rule_codecs() -> str:
    """Which codecs are claimed and backed by an encoder and a decoder."""
    return _summary_line(
        AUDIO_CLAIMS, re.compile(r"\d+ codecs claimed \(([^)]*)\)"), "a codec list"
    )


def rule_transports() -> str:
    """The transports the stack can carry signalling over, read from their wire spellings."""
    source = TRANSPORT_KINDS.read_text(encoding="utf-8")
    kinds = re.findall(r'Self::\w+ => "([A-Z0-9]+)"', source)
    if not kinds:
        raise ValueError(f"{TRANSPORT_KINDS.name} names no transports; the enum has moved")
    return ", ".join(kinds)


def rule_unsafe_policy() -> str:
    """What the workspace lint table says about `unsafe`."""
    found = re.search(r'unsafe_code\s*=\s*"(\w+)"', MANIFEST.read_text(encoding="utf-8"))
    if found is None:
        raise ValueError("the workspace manifest states no unsafe_code policy")
    return found.group(1)


RULES = {
    "rfc-count": rule_rfc_count,
    "gate-steps": rule_gate_steps,
    "transports": rule_transports,
    "codecs": rule_codecs,
    "unsafe-policy": rule_unsafe_policy,
}


def generated_values() -> dict[str, str]:
    """Recompute every generated value from its live source."""
    return {rule: RULES[rule]() for rule in GENERATED_RULES}


def dimensions() -> list[dict]:
    return json.loads(DIMENSIONS.read_text(encoding="utf-8")).get("dimensions", [])


def stacks() -> list[dict]:
    return json.loads(STACKS.read_text(encoding="utf-8")).get("stacks", [])


def observations() -> list[dict]:
    """Every observation, flattened, each carrying the stack it was filed under.

    The file's own `stack` key wins over its basename only in the sense that both are checked
    against each other — a file whose contents disagree with its name is a fault, not a choice.
    """
    found = []
    for path in sorted(OBSERVATIONS.glob("*.json")):
        loaded = json.loads(path.read_text(encoding="utf-8"))
        stack = loaded.get("stack", path.stem)
        for observation in loaded.get("observations", []):
            entry = dict(observation)
            entry.setdefault("stack", stack)
            entry["_file"] = path.name
            found.append(entry)
    return found


def dataset() -> tuple[list[dict], list[dict], list[dict]]:
    return dimensions(), stacks(), observations()


def capability_ledgers() -> list[dict]:
    """Leaf-level public capability inventories, one immutable subject per file."""
    found = []
    for path in sorted(CAPABILITIES.glob("*.json")):
        loaded = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(loaded, dict):
            loaded["_file"] = path.name
        found.append(loaded)
    return found


def external_story_urls(directory=None) -> set[str]:
    """Story URLs whose commit, path and Git blob identity are pinned in comparison data."""
    found = set()
    directory = pathlib.Path(directory) if directory is not None else EXTERNAL_STORIES
    for path in sorted(directory.glob("*.json")):
        loaded = json.loads(path.read_text(encoding="utf-8"))
        repository = loaded.get("repository")
        revision = loaded.get("source_revision")
        stories = loaded.get("stories", [])
        if not isinstance(repository, str) or not isinstance(revision, str):
            continue
        for story in stories if isinstance(stories, list) else []:
            if isinstance(story, dict) and isinstance(story.get("path"), str):
                found.add(f"{repository}/blob/{revision}/{story['path']}")
    return found


def remote_git_blob_identities(
    repository: str, revision: str, paths: list[str]
) -> dict[str, str]:
    """Resolve paths at one immutable commit without retaining the external checkout."""
    with tempfile.TemporaryDirectory(prefix="sipx-story-index-") as raw:
        git_dir = pathlib.Path(raw)
        commands = (
            ["git", "-C", str(git_dir), "init", "--bare", "--quiet"],
            [
                "git",
                "-C",
                str(git_dir),
                "fetch",
                "--quiet",
                "--depth=1",
                "--filter=blob:none",
                repository,
                revision,
            ],
        )
        for command in commands:
            subprocess.run(command, check=True, capture_output=True, text=True)
        found = {}
        for path in paths:
            result = subprocess.run(
                ["git", "-C", str(git_dir), "rev-parse", f"FETCH_HEAD:{path}"],
                check=True,
                capture_output=True,
                text=True,
            )
            found[path] = result.stdout.strip()
        return found


def external_story_index_problems(directory=None, blob_resolver=None) -> list[str]:
    """The external-story evidence is closed and verified at its pinned Git commit."""
    problems = []
    directory = pathlib.Path(directory) if directory is not None else EXTERNAL_STORIES
    blob_resolver = blob_resolver or remote_git_blob_identities
    for path in sorted(directory.glob("*.json")):
        loaded = json.loads(path.read_text(encoding="utf-8"))
        where = f"external story index {path.name}"
        if not isinstance(loaded, dict):
            problems.append(f"{where} is not an object")
            continue
        keys = set(loaded)
        required = {"repository", "source_revision", "stories"}
        for key in sorted(required - keys):
            problems.append(f"{where} is missing the required key {key!r}")
        for key in sorted(keys - required):
            problems.append(f"{where} carries the unknown key {key!r}")
        repository = loaded.get("repository")
        repository_valid = False
        if not isinstance(repository, str):
            problems.append(f"{where} has no repository URL")
        else:
            parsed = urlparse(repository)
            if parsed.scheme != "https" or not parsed.netloc:
                problems.append(f"{where} has an invalid repository URL")
            else:
                repository_valid = True
        revision = loaded.get("source_revision")
        revision_valid = (
            isinstance(revision, str)
            and re.fullmatch(r"[0-9a-f]{40}", revision) is not None
        )
        if not revision_valid:
            problems.append(f"{where} has no full immutable source revision")
        stories = loaded.get("stories")
        if not isinstance(stories, list) or not stories:
            problems.append(f"{where} has no story paths")
        else:
            seen = Counter()
            declared_blobs = {}
            for story in stories:
                if not isinstance(story, dict):
                    problems.append(f"{where} has a story entry that is not an object")
                    continue
                story_keys = set(story)
                if story_keys != {"path", "blob_sha"}:
                    problems.append(
                        f"{where} story entries require exactly 'path' and 'blob_sha'"
                    )
                story_path = story.get("path")
                if (
                    not isinstance(story_path, str)
                    or re.fullmatch(r"docs/stories/[A-Za-z0-9-]+\.md", story_path)
                    is None
                ):
                    problems.append(f"{where} has invalid story path {story_path!r}")
                else:
                    seen[story_path] += 1
                blob = story.get("blob_sha")
                if not isinstance(blob, str) or re.fullmatch(r"[0-9a-f]{40}", blob) is None:
                    problems.append(f"{where} has no Git blob identity for {story_path!r}")
                elif isinstance(story_path, str):
                    declared_blobs[story_path] = blob
            for story_path, count in seen.items():
                if count > 1:
                    problems.append(f"{where} repeats story path {story_path!r}")
            can_resolve = repository_valid and revision_valid and bool(declared_blobs)
            if can_resolve:
                try:
                    resolved = blob_resolver(repository, revision, sorted(declared_blobs))
                except (OSError, subprocess.SubprocessError, RuntimeError) as error:
                    problems.append(f"{where} could not verify pinned story blobs: {error}")
                else:
                    for story_path, declared in declared_blobs.items():
                        actual = resolved.get(story_path)
                        if actual != declared:
                            problems.append(
                                f"{where} declares blob {declared!r} for {story_path!r},"
                                f" but the pinned commit carries {actual!r}"
                            )
    return problems


def capability_expectations(directory=None) -> tuple[dict[str, tuple[str, set[str]]], list[str]]:
    """Load the separately reviewed exact-ID inventory for each pinned subject revision."""
    directory = pathlib.Path(directory) if directory is not None else CAPABILITY_EXPECTED
    expectations = {}
    problems = []
    for path in sorted(directory.glob("*.json")):
        loaded = json.loads(path.read_text(encoding="utf-8"))
        where = f"capability expectation {path.name}"
        if not isinstance(loaded, dict):
            problems.append(f"{where} is not an object")
            continue
        if set(loaded) != {"subject", "source_revision", "expected_ids"}:
            problems.append(
                f"{where} requires exactly 'subject', 'source_revision' and 'expected_ids'"
            )
        subject = loaded.get("subject")
        revision = loaded.get("source_revision")
        expected_ids = loaded.get("expected_ids")
        if not isinstance(subject, str) or path.name != f"{subject}.json":
            problems.append(f"{where} has an invalid subject or filename")
        if not isinstance(revision, str) or re.fullmatch(r"[0-9a-f]{40}", revision) is None:
            problems.append(f"{where} has no full immutable source revision")
        if not isinstance(expected_ids, list) or not expected_ids:
            problems.append(f"{where} has no expected capability IDs")
            continue
        invalid = [
            cap_id
            for cap_id in expected_ids
            if not isinstance(cap_id, str)
            or re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", cap_id) is None
        ]
        if invalid:
            problems.append(f"{where} has invalid capability IDs: {invalid!r}")
            continue
        expected_id_set = set(expected_ids)
        if len(expected_ids) != len(expected_id_set):
            problems.append(f"{where} repeats a capability ID")
        if (
            isinstance(subject, str)
            and isinstance(revision, str)
            and re.fullmatch(r"[0-9a-f]{40}", revision) is not None
        ):
            if subject in expectations:
                problems.append(f"{where} repeats subject {subject!r}")
            expectations[subject] = (revision, expected_id_set)
    return expectations, problems


def capability_where(ledger, capability=None) -> str:
    subject = ledger.get("subject", "?")
    if capability is None:
        return f"capability ledger {subject}"
    return f"{subject}/capability/{capability.get('id', '?')}"


def canonical_workspace_rust_path(source) -> bool:
    """Whether a path stays lexically and physically inside a workspace crate."""
    if not isinstance(source, str):
        return False
    pure = pathlib.PurePosixPath(source)
    if pure.is_absolute() or ".." in pure.parts or len(pure.parts) < 3:
        return False
    if pure.parts[0] != "crates" or pure.suffix != ".rs":
        return False
    crate_root = (ROOT / "crates").resolve()
    return (ROOT / source).resolve().is_relative_to(crate_root)


def capability_schema_problems(ledger) -> list[str]:
    """Closed key sets and the scalar constraints declared by the JSON schema."""
    if not isinstance(ledger, dict):
        return ["capability ledger is not an object"]
    problems = []
    required, optional = CAPABILITY_LEDGER_KEYS
    keys = {key for key in ledger if not key.startswith("_")}
    where = capability_where(ledger)
    problems.extend(
        f"{where} is missing the required key {key!r}" for key in sorted(required - keys)
    )
    problems.extend(
        f"{where} carries the unknown key {key!r}"
        for key in sorted(keys - required - optional)
    )
    subject = ledger.get("subject")
    if (
        not isinstance(subject, str)
        or re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", subject) is None
    ):
        problems.append(f"{where} has an invalid subject key")
    revision = ledger.get("source_revision")
    if not isinstance(revision, str) or re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        problems.append(f"{where} has an invalid source revision")
    version = ledger.get("version_evaluated")
    if not isinstance(version, str) or not version.strip():
        problems.append(f"{where} has an empty evaluated version")
    evaluated_at = ledger.get("evaluated_at")
    if not isinstance(evaluated_at, str) or re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", evaluated_at) is None:
        problems.append(f"{where} has an invalid evaluation date")
    expected = ledger.get("expected_capabilities")
    if not isinstance(expected, int) or isinstance(expected, bool) or expected < 1:
        problems.append(f"{where} has an invalid expected capability count")
    capabilities = ledger.get("capabilities", [])
    if not isinstance(capabilities, list) or not capabilities:
        return problems + [f"{where} has 'capabilities', which must be a list"]
    for capability in capabilities:
        if not isinstance(capability, dict):
            problems.append(f"{where} contains a capability that is not an object")
            continue
        required, optional = CAPABILITY_KEYS
        keys = set(capability)
        leaf = capability_where(ledger, capability)
        problems.extend(
            f"{leaf} is missing the required key {key!r}" for key in sorted(required - keys)
        )
        problems.extend(
            f"{leaf} carries the unknown key {key!r}"
            for key in sorted(keys - required - optional)
        )
        cap_id = capability.get("id")
        if (
            not isinstance(cap_id, str)
            or re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", cap_id) is None
        ):
            problems.append(f"{leaf} has an invalid stable capability key")
        for field in ("category", "title"):
            value = capability.get(field)
            if not isinstance(value, str) or not value.strip():
                problems.append(f"{leaf} has an empty {field}")
        for field in ("confidence", "ownership", "status"):
            value = capability.get(field)
            if not isinstance(value, str) or not value.strip():
                problems.append(f"{leaf} has an invalid {field}")
        for field in ("story", "rationale"):
            if field in capability:
                value = capability.get(field)
                if not isinstance(value, str) or not value.strip():
                    problems.append(f"{leaf} has an invalid {field}")
        if "implementation" in capability:
            implementation = capability.get("implementation")
            if not isinstance(implementation, list) or not implementation:
                problems.append(f"{leaf} has an invalid implementation list")
            else:
                for source in implementation:
                    if not canonical_workspace_rust_path(source):
                        problems.append(f"{leaf} has invalid implementation path {source!r}")
        entries = capability.get("evidence")
        if not isinstance(entries, list) or not entries:
            problems.append(f"{leaf} has an invalid evidence list")
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                problems.append(f"{leaf} has evidence that is not an object")
                continue
            entry_keys = set(entry)
            if "note" not in entry_keys or entry_keys - {"note", "url", "path"}:
                problems.append(f"{leaf} has evidence with invalid keys")
            note = entry.get("note")
            if not isinstance(note, str) or not note.strip():
                problems.append(f"{leaf} has evidence with an empty note")
            has_url = "url" in entry
            has_path = "path" in entry
            if has_url == has_path:
                problems.append(f"{leaf} evidence requires exactly one url or path")
            if has_url and (not isinstance(entry.get("url"), str) or not entry.get("url")):
                problems.append(f"{leaf} has an invalid evidence URL value")
            if has_path and (
                not isinstance(entry.get("path"), str) or not entry.get("path")
            ):
                problems.append(f"{leaf} has an invalid evidence path value")
    return problems


def capability_staleness_problems(ledger, today: datetime.date) -> list[str]:
    """A leaf inventory ages as one pinned reading, not as unrelated rows."""
    synthetic = {
        "stack": ledger.get("subject", "?"),
        "dimension": "capability-ledger",
        "evaluated_at": ledger.get("evaluated_at"),
    }
    return staleness_problems(synthetic, today)


def capability_evidence_problems(ledger, capability) -> list[str]:
    """Every leaf points at subject evidence that can stop being true."""
    where = capability_where(ledger, capability)
    entries = capability.get("evidence", [])
    if not isinstance(entries, list) or not entries:
        return [f"{where} cites no evidence"]
    problems = []
    for entry in entries:
        if not isinstance(entry, dict):
            problems.append(f"{where} has an evidence entry that is not a citation")
            continue
        problems.extend(f"{where}: {problem}" for problem in schema_problems("evidence", entry))
        pointers = {key for key in ("url", "path") if entry.get(key)}
        if len(pointers) != 1:
            problems.append(f"{where} must give exactly one evidence url or path")
        path = entry.get("path")
        if path is not None:
            if not isinstance(path, str) or not path:
                problems.append(f"{where} cites an invalid evidence path")
            elif not (ROOT / path).exists():
                problems.append(f"{where} cites {path}, which does not exist")
        note = entry.get("note")
        if not isinstance(note, str) or not note.strip():
            problems.append(f"{where} has evidence with an empty note")
        url = entry.get("url")
        if url is not None and not isinstance(url, str):
            problems.append(f"{where} cites a non-string evidence URL")
        elif isinstance(url, str):
            parsed = urlparse(url)
            if parsed.scheme not in {"http", "https"} or not parsed.netloc:
                problems.append(f"{where} cites an invalid evidence URL")
            revision = ledger.get("source_revision")
            path_parts = pathlib.PurePosixPath(parsed.path).parts
            pins_revision = any(
                marker in {"blob", "tree"} and pinned == revision
                for marker, pinned in zip(path_parts, path_parts[1:])
            )
            if capability.get("confidence") == "measured" and not pins_revision:
                problems.append(
                    f"{where} claims measured confidence without pinning its source revision"
                )
        if capability.get("confidence") == "measured" and not isinstance(url, str):
            problems.append(f"{where} claims measured confidence without immutable source evidence")
    return problems


def capability_problems(
    ledger_list,
    stack_list,
    today: datetime.date,
    external_stories=None,
    expectations=None,
) -> list[str]:
    """Ownership, disposition and discovery closure for leaf-level inventories."""
    known_stacks = {stack.get("id") for stack in stack_list}
    required_subjects = {
        stack.get("id")
        for stack in stack_list
        if stack.get("capability_inventory") is True
    }
    external_stories = set(external_stories or ())
    expectations = expectations or {}
    problems = []
    subjects = Counter()
    for ledger in ledger_list:
        if not isinstance(ledger, dict):
            problems.extend(capability_schema_problems(ledger))
            continue
        subject = ledger.get("subject")
        where = capability_where(ledger)
        subjects[subject] += 1
        if subject not in known_stacks:
            problems.append(f"{where} names a subject stacks.json does not declare")
        if ledger.get("_file") != f"{subject}.json":
            problems.append(f"{where} is filed as {ledger.get('_file')!r}; filename must match")
        if not ledger.get("version_evaluated") or not ledger.get("source_revision"):
            problems.append(f"{where} has no immutable version and source revision")
        problems.extend(capability_staleness_problems(ledger, today))
        expected = ledger.get("expected_capabilities")
        capabilities = ledger.get("capabilities", [])
        if (
            isinstance(expected, int)
            and isinstance(capabilities, list)
            and len(capabilities) != expected
        ):
            problems.append(
                f"{where} declares {expected} expected capabilities but carries {len(capabilities)}"
            )
        expectation = expectations.get(subject)
        if expectation is None:
            problems.append(f"{where} has no separately reviewed exact-ID inventory")
        elif isinstance(capabilities, list):
            expected_revision, expected_ids = expectation
            actual_ids = {
                capability.get("id")
                for capability in capabilities
                if isinstance(capability, dict)
                and isinstance(capability.get("id"), str)
            }
            if ledger.get("source_revision") != expected_revision:
                problems.append(f"{where} and its exact-ID inventory pin different revisions")
            missing_ids = expected_ids - actual_ids
            extra_ids = actual_ids - expected_ids
            if missing_ids:
                problems.append(
                    f"{where} omits expected capability IDs: {', '.join(sorted(missing_ids))}"
                )
            if extra_ids:
                problems.append(
                    f"{where} carries unreviewed capability IDs: {', '.join(sorted(extra_ids))}"
                )
            if isinstance(expected, int) and expected != len(expected_ids):
                problems.append(
                    f"{where} count ratchet disagrees with its exact-ID inventory"
                )

        seen = Counter()
        categories = set()
        for capability in ledger.get("capabilities", []):
            if not isinstance(capability, dict):
                continue
            leaf = capability_where(ledger, capability)
            cap_id = capability.get("id")
            if isinstance(cap_id, str):
                seen[cap_id] += 1
            category = capability.get("category")
            if isinstance(category, str):
                categories.add(category)
            owner = capability.get("ownership")
            status = capability.get("status")
            confidence = capability.get("confidence")
            if not isinstance(confidence, str) or confidence not in CAPABILITY_CONFIDENCE:
                problems.append(
                    f"{leaf} has unknown confidence {confidence!r}; choose one of"
                    f" {', '.join(sorted(CAPABILITY_CONFIDENCE))}"
                )
            elif confidence == "assessed" and not capability.get("rationale"):
                problems.append(f"{leaf} is assessed without a rationale")
            if not isinstance(owner, str) or owner not in CAPABILITY_OWNERS:
                problems.append(
                    f"{leaf} has unknown ownership {owner!r}; choose one of"
                    f" {', '.join(CAPABILITY_OWNERS)}"
                )
            elif not isinstance(status, str) or status not in CAPABILITY_STATUS[owner]:
                problems.append(
                    f"{leaf} has status {status!r}, which is not valid for ownership {owner!r}"
                )
            problems.extend(capability_evidence_problems(ledger, capability))

            story = capability.get("story")
            if owner == "sipx" and status == "open":
                if not isinstance(story, str) or not story:
                    problems.append(f"{leaf} is an open sipx row with no story")
                elif not (ROOT / story).is_file():
                    problems.append(f"{leaf} cites missing story {story}")
                else:
                    story_text = (ROOT / story).read_text(encoding="utf-8")
                    story_status = re.search(r"^status:\s*(\S+)", story_text, re.MULTILINE)
                    if story_status is None:
                        problems.append(f"{leaf} cites {story}, which has no status")
                    elif story_status.group(1) == "done":
                        problems.append(
                            f"{leaf} is still open but {story} is done; update the disposition"
                        )
            implementation = capability.get("implementation")
            if owner == "sipx" and status == "implemented":
                if not isinstance(implementation, list) or not implementation:
                    problems.append(f"{leaf} claims implementation with no Rust source evidence")
                else:
                    for source in implementation:
                        path = (ROOT / source).resolve() if isinstance(source, str) else ROOT
                        if (
                            not canonical_workspace_rust_path(source)
                            or not path.is_file()
                        ):
                            problems.append(
                                f"{leaf} cites {source!r} as implementation; it must be existing"
                                " Rust source in a workspace crate"
                            )
            elif implementation:
                problems.append(
                    f"{leaf} carries implementation evidence without implemented sipx ownership"
                )
            if owner == "sipx-clstr" and story not in external_stories:
                problems.append(
                    f"{leaf} is cluster-owned and has no story in the pinned external index"
                )
            if owner == "not-applicable" and not capability.get("rationale"):
                problems.append(f"{leaf} is excluded without a rationale")

        for cap_id, count in sorted(seen.items(), key=lambda item: str(item[0])):
            if count > 1:
                problems.append(f"{where} declares capability {cap_id!r} {count} times")
        missing_categories = REQUIRED_CAPABILITY_CATEGORIES - categories
        if missing_categories:
            problems.append(
                f"{where} omits required categories: {', '.join(sorted(missing_categories))}"
            )
    for subject, count in subjects.items():
        if count > 1:
            problems.append(f"capability subject {subject!r} has {count} ledgers")
    if not required_subjects:
        problems.append("no comparison stack requires a capability inventory")
    for subject in sorted(required_subjects - set(subjects)):
        problems.append(f"stack {subject!r} requires a capability ledger, but none exists")
    for subject in sorted(required_subjects - set(expectations)):
        problems.append(f"stack {subject!r} requires an exact-ID inventory, but none exists")
    for subject in sorted(set(subjects) - required_subjects, key=str):
        problems.append(f"capability ledger {subject!r} is not anchored by its comparison stack")
    for subject in sorted(set(expectations) - set(subjects)):
        problems.append(f"capability expectation {subject!r} has no corresponding ledger")
    return problems


def _comparative_load_contract():
    """Import the neutral load contract, whose hyphenated name keeps it off the import path."""
    spec = importlib.util.spec_from_file_location(
        "comparative_load", ROOT / "scripts" / "comparative-load.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


load_contract = _comparative_load_contract()

LOAD = COMPARISON / "load"
LOAD_DATASET_FILE = LOAD / "dataset.json"

#: Named in the load staleness failure for the same reason `REFRESH_COMMAND` is: a red gate on a
#: date with no code change behind it must say what to do about it.
LOAD_REFRESH_COMMAND = (
    "re-run it with scripts/comparative-load-run.py, then ./scripts/comparison-report.py"
)

LOAD_DATASET_SCHEMA = "sipx.comparative-load.dataset.v1"
LOAD_ENVIRONMENT_SCHEMA = "sipx.comparative-load.environment.v1"
LOAD_PREFLIGHT_SCHEMA = "sipx.comparative-load.preflight.v1"
LOAD_HEADROOM_SCHEMA = "sipx.comparative-load.headroom.v1"
LOAD_OMISSIONS_SCHEMA = "sipx.comparative-load.omissions.v1"
LOAD_OMISSION_REASON = "two_consecutive_failed_rates"

# The load evidence key sets are closed for the reason every other key set here is: a record
# that can quietly gain a field can quietly gain a claim nobody checks.
LOAD_DATASET_KEYS = {"schema", "evaluated_at", "driver", "endpoints", "scope"}
LOAD_DRIVER_KEYS = {"id", "revision", "artifact_sha256"}
LOAD_ENDPOINT_KEYS = {"id", "as_responder", "as_driver", "internal_state"}
LOAD_INTERNAL_KEYS = {"visibility", "note"}
LOAD_SCOPE_KEYS = {"workload", "not_inferred"}
LOAD_ENVIRONMENT_KEYS = {
    "schema",
    "captured_utc",
    "host",
    "socket_limits",
    "toolchains",
    "builds",
    "commands",
    "seed",
    "contract_sha256",
}
LOAD_HOST_KEYS = {
    "os",
    "kernel",
    "architecture",
    "logical_cpus",
    "memory_bytes",
    "cpu_governor",
    "clock",
}
LOAD_SOCKET_KEYS = {
    "rlimit_nofile_soft",
    "rlimit_nofile_hard",
    "rmem_max",
    "wmem_max",
    "rmem_default",
    "wmem_default",
}
LOAD_ENV_BUILD_KEYS = {
    "endpoint_id",
    "role",
    "revision",
    "artifact",
    "artifact_sha256",
    "build_command",
    "toolchain",
    "features",
    "dependencies",
}
LOAD_PREFLIGHT_KEYS = {
    "schema",
    "phase",
    "rate_per_second",
    "dialogs",
    "offered",
    "completed",
    "five_steps_observed",
    "post_drain_zero",
    "passed",
    "started_utc",
    "elapsed_ms",
}
LOAD_HEADROOM_KEYS = {
    "schema",
    "fixture",
    "rate_per_second",
    "offered",
    "completed",
    "completion_ratio",
    "setup_p99_ms",
    "driver_cpu_fraction",
    "passed",
    "started_utc",
    "elapsed_ms",
}
LOAD_OMISSIONS_KEYS = {"schema", "omitted"}
LOAD_OMITTED_KEYS = {"rate_index", "rate_per_second", "reason"}

#: The correctness qualification X-99 requires before any capacity work: one hundred low-rate
#: dialogs per measured direction, on top of the contract's own twenty-dialog preflight.
LOAD_QUALIFICATION_DIALOGS = 100
LOAD_PREFLIGHT_DIALOGS = 20
LOAD_LOW_RATE = 1
LOAD_HEADROOM_CPU_LIMIT = 0.8

LOAD_RUN_PARTS = (
    "manifest",
    "environment",
    "preflight",
    "qualification",
    "headroom",
    "omissions",
)


def load_dataset():
    """The published comparative-load dataset, or None when the evidence does not exist."""
    if not LOAD_DATASET_FILE.exists():
        return None
    return json.loads(LOAD_DATASET_FILE.read_text(encoding="utf-8"))


def _measured_run_keys(dataset):
    keys = []
    for endpoint in (dataset or {}).get("endpoints", []):
        if not isinstance(endpoint, dict):
            continue
        for role in ("as_responder", "as_driver"):
            entry = endpoint.get(role)
            if isinstance(entry, dict) and entry.get("status") == "measured":
                run = entry.get("run")
                if isinstance(run, str) and run:
                    keys.append(run)
    return keys


def load_runs(dataset, base=None):
    """Every measured run directory the dataset references, loaded the way it is checked."""
    base = pathlib.Path(base) if base is not None else LOAD
    runs = {}
    for key in _measured_run_keys(dataset):
        directory = base / key
        run = {}
        for part in LOAD_RUN_PARTS:
            path = directory / f"{part}.json"
            if path.is_file():
                run[part] = json.loads(path.read_text(encoding="utf-8"))
        results = {}
        for path in sorted((directory / "results").glob("rate*-rep*.json")):
            found = re.fullmatch(r"rate(\d+)-rep(\d+)\.json", path.name)
            if found is None:
                continue
            results[(int(found.group(1)), int(found.group(2)))] = json.loads(
                path.read_text(encoding="utf-8")
            )
        run["results"] = results
        runs[key] = run
    return runs


def _load_keys(where, record, required) -> list[str]:
    if not isinstance(record, dict):
        return [f"{where} is not an object"]
    keys = set(record)
    problems = [f"{where} is missing the required key {key!r}" for key in sorted(required - keys)]
    problems.extend(
        f"{where} carries the unknown key {key!r}" for key in sorted(keys - required)
    )
    return problems


def _load_staleness(dataset, today) -> list[str]:
    stamp = dataset.get("evaluated_at")
    if not isinstance(stamp, str) or not stamp:
        return ["comparative load dataset has no 'evaluated_at'"]
    try:
        taken = datetime.date.fromisoformat(stamp)
    except ValueError:
        return [f"comparative load dataset has evaluated_at {stamp!r}, not a YYYY-MM-DD date"]
    age = (today - taken).days
    if age > MAX_OBSERVATION_AGE_DAYS:
        return [
            f"comparative load dataset is stale: evaluated {age} days ago, and the limit is"
            f" {MAX_OBSERVATION_AGE_DAYS}. {LOAD_REFRESH_COMMAND}"
        ]
    return []


def _load_role_problems(where, entry, runs) -> list[str]:
    if not isinstance(entry, dict):
        return [f"{where} is not an object"]
    status = entry.get("status")
    if status == "measured":
        problems = _load_keys(where, entry, {"status", "run"})
        run = entry.get("run")
        if not isinstance(run, str) or not run:
            problems.append(f"{where} is measured and names no run directory")
        elif run not in runs:
            problems.append(f"{where} names the run {run!r}, which was not loaded")
        return problems
    if status == "not_measured":
        problems = _load_keys(where, entry, {"status", "reason"})
        if "run" in entry:
            problems.append(
                f"{where} is not_measured and still names a run; a disclosed omission can"
                " never carry a performance number"
            )
        reason = entry.get("reason")
        if not isinstance(reason, str) or not reason.strip():
            problems.append(f"{where} discloses no reason; an unexplained omission reads as a finding")
        return problems
    return [f"{where} has status {status!r}; it must be measured or not_measured"]


def _load_preflight_problems(where, record, phase, dialogs) -> list[str]:
    problems = _load_keys(where, record, LOAD_PREFLIGHT_KEYS)
    if problems:
        return problems
    if record.get("schema") != LOAD_PREFLIGHT_SCHEMA:
        problems.append(f"{where} must carry schema {LOAD_PREFLIGHT_SCHEMA!r}")
    if record.get("phase") != phase:
        problems.append(f"{where} must record the {phase!r} phase")
    if record.get("rate_per_second") != LOAD_LOW_RATE:
        problems.append(f"{where} must offer dialogs at the low rate of {LOAD_LOW_RATE}/s")
    if record.get("dialogs") != dialogs:
        problems.append(
            f"{where} configured {record.get('dialogs')!r} dialogs; the {phase} phase requires"
            f" exactly {dialogs} (one hundred low-rate dialogs qualify a direction)"
            if phase == "qualification"
            else f"{where} configured {record.get('dialogs')!r} dialogs; the {phase} phase"
            f" requires exactly {dialogs}"
        )
    complete = (
        record.get("passed") is True
        and record.get("completed") == record.get("offered") == record.get("dialogs")
        and record.get("five_steps_observed") is True
        and record.get("post_drain_zero") is True
    )
    if not complete:
        problems.append(
            f"{where} is recorded under a measured direction, but its correctness"
            " prerequisite failed; record the direction as not measured: correctness"
            " prerequisite failed, never as a performance number"
        )
    return problems


def _load_headroom_problems(where, record, ceiling) -> list[str]:
    problems = _load_keys(where, record, LOAD_HEADROOM_KEYS)
    if problems:
        return problems
    if record.get("schema") != LOAD_HEADROOM_SCHEMA:
        problems.append(f"{where} must carry schema {LOAD_HEADROOM_SCHEMA!r}")
    if record.get("rate_per_second") != 2 * ceiling:
        problems.append(
            f"{where} ran at {record.get('rate_per_second')!r}/s; the driver must prove twice"
            f" the tested ceiling ({2 * ceiling}/s) before any endpoint is measured"
        )
    fraction = record.get("driver_cpu_fraction")
    if not isinstance(fraction, (int, float)) or isinstance(fraction, bool) or not (
        0 <= fraction < LOAD_HEADROOM_CPU_LIMIT
    ):
        problems.append(
            f"{where} records driver CPU fraction {fraction!r}, which is not under the"
            f" {LOAD_HEADROOM_CPU_LIMIT} headroom threshold; the driver may be the bottleneck"
            " and the execution is invalid"
        )
    offered = record.get("offered")
    completed = record.get("completed")
    ratio_ok = (
        isinstance(offered, int)
        and isinstance(completed, int)
        and offered >= 1_000
        and completed * 1_000 >= offered * 999
    )
    if record.get("passed") is not True or not ratio_ok:
        problems.append(f"{where} did not meet the capacity predicate at twice the ceiling")
    p99 = record.get("setup_p99_ms")
    if not isinstance(p99, int) or isinstance(p99, bool) or p99 > 250:
        problems.append(f"{where} setup p99 {p99!r} does not meet the 250 ms loopback bound")
    return problems


def _load_environment_problems(where, environment, manifest, dataset) -> list[str]:
    problems = _load_keys(where, environment, LOAD_ENVIRONMENT_KEYS)
    if problems:
        return problems
    if environment.get("schema") != LOAD_ENVIRONMENT_SCHEMA:
        problems.append(f"{where} must carry schema {LOAD_ENVIRONMENT_SCHEMA!r}")
    problems.extend(_load_keys(f"{where}.host", environment.get("host"), LOAD_HOST_KEYS))
    problems.extend(
        _load_keys(f"{where}.socket_limits", environment.get("socket_limits"), LOAD_SOCKET_KEYS)
    )
    toolchains = environment.get("toolchains")
    if not isinstance(toolchains, list) or not toolchains:
        problems.append(f"{where} records no toolchains")
    commands = environment.get("commands")
    if not isinstance(commands, list) or not commands:
        problems.append(f"{where} records no commands")
    if environment.get("contract_sha256") != load_contract.contract_hash():
        problems.append(
            f"{where} contract hash does not match docs/specs/comparative-load.md; the recorded"
            f" run predates the current contract. {LOAD_REFRESH_COMMAND}"
        )
    if manifest is not None and environment.get("seed") != manifest.get("seed"):
        problems.append(f"{where} seed disagrees with the manifest")
    builds = environment.get("builds")
    if not isinstance(builds, list) or len(builds) != 2:
        problems.append(f"{where} must record exactly the driver and responder builds")
        return problems
    by_role = {}
    for index, build in enumerate(builds):
        build_where = f"{where}.builds[{index}]"
        problems.extend(_load_keys(build_where, build, LOAD_ENV_BUILD_KEYS))
        if isinstance(build, dict):
            by_role[build.get("role")] = build
    if manifest is not None:
        for manifest_build in manifest.get("builds", []):
            role = manifest_build.get("role")
            build = by_role.get(role)
            if build is None:
                problems.append(f"{where} records no {role} build")
                continue
            for field in ("endpoint_id", "revision", "artifact_sha256"):
                if build.get(field) != manifest_build.get(field):
                    problems.append(
                        f"{where} {role} build {field} disagrees with the manifest artifact"
                        " hash and pin; the evidence does not describe the build that ran"
                    )
    driver = (dataset or {}).get("driver", {})
    driver_build = by_role.get("driver")
    if isinstance(driver, dict) and isinstance(driver_build, dict):
        if driver_build.get("artifact_sha256") != driver.get("artifact_sha256"):
            problems.append(
                f"{where} driver artifact hash disagrees with the dataset's pinned driver;"
                " a comparison across runs requires the same measured instrument"
            )
    return problems


def _load_ladder_problems(where, run) -> list[str]:
    manifest = run.get("manifest")
    if not isinstance(manifest, dict):
        return []
    try:
        rates = load_contract.ladder_rates(int(manifest["ceiling"]))
    except (KeyError, TypeError, ValueError, load_contract.ContractError):
        return []
    problems = []
    results = run.get("results", {})
    by_rate = {}
    for (rate_index, repetition), record in sorted(results.items()):
        result_where = f"{where}/results/rate{rate_index}-rep{repetition}.json"
        try:
            load_contract.validate_result(record, manifest)
        except load_contract.ContractError as error:
            problems.append(f"{result_where}: {error}")
            continue
        run_part = record.get("run", {})
        if run_part.get("rate_index") != rate_index or run_part.get("repetition") != repetition:
            problems.append(f"{result_where} is filed under the wrong rate or repetition")
            continue
        by_rate.setdefault(rate_index, {})[repetition] = record
    attempted = sorted(by_rate)
    if attempted != list(range(len(attempted))):
        problems.append(f"{where} attempted rates must be the ladder prefix, without gaps")
        return problems
    rate_alive = []
    for rate_index in attempted:
        repetitions = by_rate[rate_index]
        if sorted(repetitions) != list(range(load_contract.REPETITIONS)):
            problems.append(
                f"{where} rate {rates[rate_index]}/s must carry exactly"
                f" {load_contract.REPETITIONS} repetitions"
            )
        rate_alive.append(
            any(record.get("status") == "passed" for record in repetitions.values())
        )
    expected_omitted = set()
    for index in range(1, len(rate_alive)):
        if not rate_alive[index] and not rate_alive[index - 1]:
            expected_omitted = set(range(index + 1, len(rates)))
            break
    if set(attempted) & expected_omitted:
        problems.append(f"{where} measured a rate after two consecutive rates had failed")
    missing = set(range(len(rates))) - set(attempted) - expected_omitted
    if missing:
        problems.append(
            f"{where} omitted rate indices {sorted(missing)} without two consecutive failed"
            " rates to justify the omission"
        )
    omissions = run.get("omissions")
    listed = set()
    if isinstance(omissions, dict):
        problems.extend(_load_keys(f"{where}/omissions.json", omissions, LOAD_OMISSIONS_KEYS))
        if omissions.get("schema") not in (None, LOAD_OMISSIONS_SCHEMA):
            problems.append(f"{where}/omissions.json must carry schema {LOAD_OMISSIONS_SCHEMA!r}")
        for entry in omissions.get("omitted", []) if isinstance(omissions.get("omitted"), list) else []:
            entry_where = f"{where}/omissions.json"
            problems.extend(_load_keys(entry_where, entry, LOAD_OMITTED_KEYS))
            if not isinstance(entry, dict):
                continue
            rate_index = entry.get("rate_index")
            if isinstance(rate_index, int) and not isinstance(rate_index, bool):
                listed.add(rate_index)
                if 0 <= rate_index < len(rates) and entry.get("rate_per_second") != rates[rate_index]:
                    problems.append(
                        f"{entry_where} omitted rate {rate_index} does not carry its ladder rate"
                    )
            if entry.get("reason") != LOAD_OMISSION_REASON:
                problems.append(
                    f"{entry_where} omission reason must be {LOAD_OMISSION_REASON!r}; an"
                    " unattempted rate is an omission fact, never a zero measurement"
                )
    if listed != expected_omitted:
        problems.append(
            f"{where} omission record lists rates {sorted(listed)} but the results justify"
            f" {sorted(expected_omitted)}; omitted and measured rates must agree"
        )
    return problems


def _load_run_problems(where, run, dataset, endpoint_id, role) -> list[str]:
    problems = []
    for part in LOAD_RUN_PARTS:
        if part not in run:
            problems.append(f"{where} is missing {part}.json")
    manifest = run.get("manifest")
    if manifest is not None:
        try:
            load_contract.validate_manifest(manifest)
        except load_contract.ContractError as error:
            problems.append(f"{where}/manifest.json: {error}")
            manifest = None
    if manifest is not None:
        direction = manifest.get("direction", {})
        if direction.get(role) != endpoint_id:
            problems.append(
                f"{where}/manifest.json direction does not put {endpoint_id!r} in the"
                f" {role} role this dataset claims"
            )
        driver = (dataset or {}).get("driver", {})
        if isinstance(driver, dict) and direction.get("driver" if role == "responder" else "responder") != driver.get("id"):
            problems.append(
                f"{where}/manifest.json does not pair {endpoint_id!r} with the dataset's"
                " pinned measuring driver"
            )
    if "environment" in run:
        problems.extend(
            _load_environment_problems(f"{where}/environment.json", run["environment"], manifest, dataset)
        )
    if "preflight" in run:
        problems.extend(
            _load_preflight_problems(
                f"{where}/preflight.json", run["preflight"], "preflight", LOAD_PREFLIGHT_DIALOGS
            )
        )
    if "qualification" in run:
        problems.extend(
            _load_preflight_problems(
                f"{where}/qualification.json",
                run["qualification"],
                "qualification",
                LOAD_QUALIFICATION_DIALOGS,
            )
        )
    if "headroom" in run and manifest is not None:
        problems.extend(
            _load_headroom_problems(f"{where}/headroom.json", run["headroom"], int(manifest["ceiling"]))
        )
    if manifest is not None and not run.get("results"):
        problems.append(f"{where} carries no raw per-repetition results")
    problems.extend(_load_ladder_problems(where, run))
    return problems


def load_problems(dataset, runs, stack_list, today) -> list[str]:
    """Every way the published load result could outrun its evidence."""
    if dataset is None:
        return [
            "docs/comparison/load/dataset.json is missing; the comparative load result is"
            " X-99 evidence and the page cannot publish measurements it does not hold"
        ]
    problems = _load_keys("comparative load dataset", dataset, LOAD_DATASET_KEYS)
    if problems:
        return problems
    if dataset.get("schema") != LOAD_DATASET_SCHEMA:
        problems.append(f"comparative load dataset must carry schema {LOAD_DATASET_SCHEMA!r}")
    problems.extend(_load_staleness(dataset, today))
    driver = dataset.get("driver")
    problems.extend(_load_keys("comparative load driver", driver, LOAD_DRIVER_KEYS))
    if isinstance(driver, dict):
        artifact = driver.get("artifact_sha256")
        if not isinstance(artifact, str) or len(artifact) != 64:
            problems.append("comparative load driver has no full artifact hash")
    known_stacks = {stack.get("id") for stack in stack_list}
    endpoints = dataset.get("endpoints")
    if not isinstance(endpoints, list) or not endpoints:
        problems.append("comparative load dataset names no endpoints")
        return problems
    scope = dataset.get("scope")
    problems.extend(_load_keys("comparative load scope", scope, LOAD_SCOPE_KEYS))
    if isinstance(scope, dict):
        workload = scope.get("workload")
        if not isinstance(workload, str) or not workload.strip():
            problems.append("comparative load scope states no workload")
        not_inferred = scope.get("not_inferred")
        if not isinstance(not_inferred, list) or not not_inferred:
            problems.append(
                "comparative load scope must name what the first result does not cover;"
                " an unstated limitation reads as coverage"
            )
    for endpoint in endpoints:
        where = f"comparative load endpoint {endpoint.get('id', '?') if isinstance(endpoint, dict) else '?'}"
        problems.extend(_load_keys(where, endpoint, LOAD_ENDPOINT_KEYS))
        if not isinstance(endpoint, dict):
            continue
        endpoint_id = endpoint.get("id")
        if endpoint_id not in known_stacks:
            problems.append(f"{where} names a subject stacks.json does not declare")
        if isinstance(driver, dict) and endpoint_id == driver.get("id"):
            problems.append(f"{where} is also the measuring driver; the instrument is not a subject")
        internal = endpoint.get("internal_state")
        problems.extend(_load_keys(f"{where}.internal_state", internal, LOAD_INTERNAL_KEYS))
        if isinstance(internal, dict):
            if internal.get("visibility") not in ("endpoint-reported", "harness-observed"):
                problems.append(
                    f"{where}.internal_state visibility must be endpoint-reported or"
                    " harness-observed; unobserved internal state is disclosed, not inferred"
                )
            note = internal.get("note")
            if not isinstance(note, str) or not note.strip():
                problems.append(f"{where}.internal_state carries no disclosure note")
        for role_key, role in (("as_responder", "responder"), ("as_driver", "driver")):
            entry = endpoint.get(role_key)
            role_where = f"{where}.{role_key}"
            problems.extend(_load_role_problems(role_where, entry, runs))
            if isinstance(entry, dict) and entry.get("status") == "measured":
                run = runs.get(entry.get("run"))
                if run is not None:
                    problems.extend(
                        _load_run_problems(entry.get("run"), run, dataset, endpoint_id, role)
                    )
    return problems


def _achieved(record) -> float:
    return record.get("counts", {}).get("completed", 0) / (
        load_contract.MEASUREMENT_MS / 1_000
    )


def _rate_rows(run):
    """Per-rate outcome rows: (rate, outcome, median achieved, spread, p99) or omissions."""
    manifest = run.get("manifest", {})
    rates = load_contract.ladder_rates(int(manifest.get("ceiling", 1)))
    by_rate = {}
    for (rate_index, repetition), record in sorted(run.get("results", {}).items()):
        by_rate.setdefault(rate_index, []).append(record)
    omitted = {
        entry.get("rate_index")
        for entry in run.get("omissions", {}).get("omitted", [])
        if isinstance(entry, dict)
    }
    rows = []
    for rate_index, rate in enumerate(rates):
        if rate_index in omitted or rate_index not in by_rate:
            rows.append((rate, None))
            continue
        records = by_rate[rate_index]
        achieved = sorted(_achieved(record) for record in records)
        passed = sum(1 for record in records if record.get("status") == "passed")
        p99 = statistics.median(
            record.get("latency_ms", {}).get("setup", {}).get("p99", 0) for record in records
        )
        rows.append((rate, (passed, len(records), achieved, p99)))
    return rows


def _capacity(run):
    """The §7 capacity point: highest fully supported rate below the first failed pair."""
    rows = _rate_rows(run)
    supported = []
    consecutive_failures = 0
    for rate, outcome in rows:
        if outcome is None:
            break
        passed, total, achieved, _ = outcome
        if passed == total:
            supported.append((rate, achieved))
            consecutive_failures = 0
        else:
            consecutive_failures += 1
            if consecutive_failures >= 2:
                break
    if not supported:
        return None
    return supported[-1]


def render_load_section(dataset, runs, stack_list) -> list[str]:
    """The generated summary: medians and spread, disclosed omissions, and no ranking."""
    if dataset is None:
        return []
    names = {stack.get("id"): stack.get("name", stack.get("id", "?")) for stack in stack_list}
    scope = dataset.get("scope", {})
    not_inferred = ", ".join(scope.get("not_inferred", []))
    out = [
        "## Comparative signalling load",
        "",
        f"**One neutral workload — {cell(scope.get('workload', ''))} — offered by the same"
        " pinned driver to each endpoint acting as responder.**",
        "",
        "The driver proves at least twice the tested ceiling against a packaged minimal fixture"
        " before any endpoint is measured, one hundred low-rate dialogs qualify protocol"
        " correctness before any capacity work, and the fixed six-rate ladder runs five"
        " repetitions per rate at open-loop offered load — the driver never raises or lowers"
        " what it offers as a target slows. Raw per-repetition records, environment inventory"
        " and hashes live under `docs/comparison/load/` and regenerate this section.",
        "",
        f"The following are **not inferred** from this result: {cell(not_inferred)}.",
        "",
    ]
    capacities = []
    for endpoint in dataset.get("endpoints", []):
        endpoint_id = endpoint.get("id")
        name = cell(str(names.get(endpoint_id, endpoint_id)))
        out.append(f"### Responder capacity: {name}")
        out.append("")
        responder = endpoint.get("as_responder", {})
        if responder.get("status") != "measured":
            out.append(f"_Not measured: {cell(responder.get('reason', ''))}_")
            out.append("")
            continue
        run = runs.get(responder.get("run"), {})
        out += [
            "| Rate (calls/s) | Outcome | Median achieved (dialogs/s) | Spread [min, max] | Setup p99 (ms, median) |",
            "|---|---|---|---|---|",
        ]
        for rate, outcome in _rate_rows(run):
            if outcome is None:
                out.append(
                    f"| {rate} | _not run: two consecutive rates failed_ | — | — | — |"
                )
                continue
            passed, total, achieved, p99 = outcome
            verdict = (
                f"supported ({passed}/{total})"
                if passed == total
                else f"failed ({passed}/{total} repetitions passed)"
            )
            median = statistics.median(achieved)
            out.append(
                f"| {rate} | {verdict} | {median:.1f} | [{achieved[0]:.1f}, {achieved[-1]:.1f}]"
                f" | {p99:.0f} |"
            )
        out.append("")
        capacity = _capacity(run)
        if capacity is None:
            out.append("No rate was supported by all five repetitions.")
        else:
            rate, achieved = capacity
            out.append(
                f"Capacity point: **{rate} calls/s**, achieved interval"
                f" [{achieved[0]:.1f}, {achieved[-1]:.1f}] dialogs/s over five repetitions."
            )
            capacities.append((name, achieved[0], achieved[-1]))
        out.append("")
        driver_entry = endpoint.get("as_driver", {})
        if driver_entry.get("status") != "measured":
            out.append(
                f"- Caller (UAC) direction: not measured — {cell(driver_entry.get('reason', ''))}"
            )
        internal = endpoint.get("internal_state", {})
        out.append(
            f"- Internal state visibility: `{internal.get('visibility', '?')}` —"
            f" {cell(internal.get('note', ''))}"
        )
        out.append("")
    if len(capacities) >= 2:
        (name_a, low_a, high_a), (name_b, low_b, high_b) = capacities[0], capacities[1]
        if low_a <= high_b and low_b <= high_a:
            out.append(
                "The achieved-throughput intervals overlap, so this comparison is"
                " **inconclusive** on this machine and profile."
            )
        else:
            higher = name_a if low_a > high_b else name_b
            lower = name_b if higher == name_a else name_a
            out.append(
                f"The measured interval for {higher} is higher on this machine and profile"
                f" than the interval for {lower}. That statement holds for these exact builds,"
                " this machine and this profile only — it is not a general ranking."
            )
        out.append("")
    return out


def is_marker(observation) -> bool:
    return "not_evaluated" in observation


def kind_of(observation) -> str:
    return "marker" if is_marker(observation) else "finding"


def where_of(observation) -> str:
    """The row's identity, which every message about it opens with."""
    return f"{observation.get('stack', '?')}/{observation.get('dimension', '?')}"


def schema_problems(kind: str, record) -> list[str]:
    """Ways a record departs from its closed key set.

    Asks whether the record is shaped like a claim at all, which is a different question from
    whether the claim is true — and a record that is not cannot be checked, only ignored.
    """
    required, optional = KEY_SETS[kind]
    if kind == "dimension":
        where = f"dimension {record.get('id', '?')!r}"
    elif kind == "stack":
        where = f"stack {record.get('id', '?')!r}"
    elif kind == "evidence":
        where = "an evidence entry"
    else:
        where = where_of(record)

    keys = {k for k in record if not k.startswith("_")}
    problems = [f"{where} is missing the required key {k!r}" for k in sorted(required - keys)]

    for key in sorted(keys - required - optional):
        hint = ""
        if key == "score":
            # Named explicitly, because this is the one somebody adds on purpose.
            hint = (
                " — a weighted total is refused because it hides the confidence tier behind a"
                " number, and a number nobody can falsify is the property this page must not have"
            )
        elif kind == "marker" and key in FINDING_KEYS[0] | FINDING_KEYS[1]:
            # The other one somebody adds on purpose: a row that hedges by saying both "nobody
            # looked" and what they would have found.
            hint = (
                " — a 'not_evaluated' marker says nobody looked and makes no other claim. Drop the"
                " marker and file a finding, or drop the finding"
            )
        problems.append(f"{where} carries the unknown key {key!r}{hint}")

    if kind == "stack" and "capability_inventory" in record:
        if not isinstance(record.get("capability_inventory"), bool):
            problems.append(f"{where} has a non-boolean capability_inventory marker")

    return problems


def evidence_problems(observation) -> list[str]:
    """Ways a citation fails to point at something that can stop being true."""
    where = where_of(observation)
    entries = observation.get("evidence", [])
    if not isinstance(entries, list):
        return [f"{where} has 'evidence', which must be a list of citations"]
    if not entries:
        return [
            f"{where} cites no evidence; give it a url or a repository path, or drop the row —"
            " prose alone is not evidence here"
        ]

    problems = []
    for entry in entries:
        if not isinstance(entry, dict):
            problems.append(f"{where} has an evidence entry that is not a citation")
            continue
        problems.extend(f"{where}: {p}" for p in schema_problems("evidence", entry))
        pointers = {k for k in ("url", "path") if entry.get(k)}
        if len(pointers) != 1:
            problems.append(
                f"{where} has an evidence entry naming {sorted(pointers) or 'neither'}; give it"
                " exactly one of 'url' or 'path' so a reader knows where to look"
            )
        path = entry.get("path")
        if isinstance(path, str) and path and not (ROOT / path).exists():
            problems.append(f"{where} cites {path}, which does not exist")
    return problems


def confidence_problems(observation, selves: set[str]) -> list[str]:
    """The obligation each tier carries, and who may hold it."""
    where = where_of(observation)
    tier = observation.get("confidence")
    problems = []

    if tier not in CONFIDENCE_TIERS:
        return [
            f"{where} claims the confidence tier {tier!r}, which is not one of"
            f" {', '.join(CONFIDENCE_TIERS)}"
        ]

    if tier == "generated" and observation.get("stack") not in selves:
        problems.append(
            f"{where} claims the generated tier, which only the stack marked is_self may hold —"
            " a value this repository computes says nothing about anyone else's code. Demote it to"
            " measured with a reproduce command"
        )
    if tier == "measured" and not observation.get("reproduce"):
        problems.append(
            f"{where} claims the measured tier with no 'reproduce' command; give the command that"
            " re-derives it at the version named, or demote it to documented"
        )
    if tier == "assessed" and not observation.get("rationale"):
        problems.append(
            f"{where} claims the assessed tier with no 'rationale'; say what the judgment rests on"
            " so a reader can disagree with it, or demote it to documented"
        )
    if tier != "generated" and observation.get("generated_from"):
        problems.append(
            f"{where} names 'generated_from' at the {tier} tier; only a generated cell"
            " interpolates a computed value"
        )
    return problems


def generated_problems(observation, values: dict[str, str]) -> list[str]:
    """A generated cell states its numbers as placeholders, so they cannot be typed."""
    where = where_of(observation)
    rules = observation.get("generated_from") or []
    text = observation.get("summary", "") or ""
    used = set(PLACEHOLDER.findall(text))
    problems = []

    if observation.get("confidence") != "generated":
        for rule in sorted(used & set(values)):
            problems.append(
                f"{where} interpolates {{{rule}}} without the generated tier; a value computed"
                " from this repository is not a finding about another stack"
            )
        return problems

    if not rules:
        problems.append(f"{where} claims the generated tier and names no 'generated_from' rule")
    for rule in rules:
        if rule not in values:
            problems.append(
                f"{where} names the generation rule {rule!r}, which does not exist; the rules are"
                f" {', '.join(sorted(values))}"
            )
        elif rule not in used:
            problems.append(
                f"{where} declares the rule {rule!r} and has no {{{rule}}} placeholder to put it"
                " in — a generated value that is typed instead of substituted can drift"
            )
    for rule in sorted(used - set(rules)):
        if rule in values:
            problems.append(
                f"{where} interpolates {{{rule}}} without declaring it in 'generated_from'"
            )
        else:
            problems.append(f"{where} interpolates {{{rule}}}, which is not a generation rule")
    return problems


def workspace_version() -> str:
    """The version this repository currently is, read from the workspace manifest."""
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    return manifest["workspace"]["package"]["version"]


def self_version_problems(observation, selves: set[str], version: str) -> list[str]:
    """A generated cell must say it was taken at the version it is actually computed from.

    Generated values are recomputed at render time from the current tree, so the moment the
    workspace version moves, a `version_evaluated` left behind claims those numbers were measured
    at a release they were not. Nothing else catches it: the value is a plain string, the public
    fact guard does not read this cell, and the numbers themselves stay correct — only the version
    attached to them goes quietly wrong. It went wrong exactly once, during the rebase that put
    this work on top of a release (`X-77`).
    """
    if observation.get("confidence") != "generated":
        return []
    if observation.get("stack") not in selves:
        return []
    stated = observation.get("version_evaluated")
    if stated == version:
        return []
    return [
        f"{where_of(observation)} is generated from the current tree but says it was evaluated at"
        f" {stated!r}, and the workspace is {version!r}. Set version_evaluated to the workspace"
        " version and regenerate"
    ]


def staleness_problems(observation, today: datetime.date) -> list[str]:
    """A comparison ages the moment it ships; refusing to report is the honest answer."""
    where = where_of(observation)
    stamp = observation.get("evaluated_at")
    if not isinstance(stamp, str) or not stamp:
        return [f"{where} has no 'evaluated_at'; an undated observation cannot be called stale"]
    try:
        taken = datetime.date.fromisoformat(stamp)
    except ValueError:
        return [f"{where} has evaluated_at {stamp!r}, which is not a YYYY-MM-DD date"]

    age = (today - taken).days
    if age > MAX_OBSERVATION_AGE_DAYS:
        return [
            f"{where} is stale: evaluated {age} days ago, and the limit is"
            f" {MAX_OBSERVATION_AGE_DAYS}. {REFRESH_COMMAND}"
        ]
    return []


def _expiry_days(observation, today: datetime.date):
    """Days left before this observation passes the limit, or `None` if it has no usable date.

    A marker has nothing to go stale — nobody looked, and that does not age — and a malformed or
    missing date is `staleness_problems`' business, not this one's.
    """
    if is_marker(observation):
        return None
    stamp = observation.get("evaluated_at")
    if not isinstance(stamp, str) or not stamp:
        return None
    try:
        taken = datetime.date.fromisoformat(stamp)
    except ValueError:
        return None
    return MAX_OBSERVATION_AGE_DAYS - (today - taken).days


def expiring_soon(observation_list, today: datetime.date) -> list[str]:
    """Observations close enough to the limit to be worth acting on.

    Deliberately **not** called from `check`. That function returns failures, and folding a warning
    into it would either fail the build thirty days early or teach a reader that some of what it
    returns is advisory — and a checker whose result needs interpreting is the thing this file
    exists to avoid being.
    """
    warnings = []
    for observation in observation_list:
        left = _expiry_days(observation, today)
        if left is not None and 0 <= left <= STALE_WARNING_DAYS:
            warnings.append(
                f"{where_of(observation)} expires in {plural(left, 'day')}. {REFRESH_COMMAND}"
            )
    return warnings


def days_until_expiry(observation_list, today: datetime.date):
    """How long the soonest observation has left, or `None` if none of them can say."""
    left = [
        days
        for days in (_expiry_days(o, today) for o in observation_list)
        if days is not None
    ]
    return min(left) if left else None


def coverage_problems(dimension_list, stack_list, observation_list) -> list[str]:
    """Every stack answers every question, or says in the data that nobody looked."""
    known_dimensions = {d.get("id") for d in dimension_list}
    known_stacks = {s.get("id") for s in stack_list}
    problems = []

    seen = Counter()
    for observation in observation_list:
        stack, dimension = observation.get("stack"), observation.get("dimension")
        where = where_of(observation)
        if stack not in known_stacks:
            problems.append(
                f"{where} is filed against the stack {stack!r}, which stacks.json does not list"
            )
        if dimension not in known_dimensions:
            problems.append(
                f"{where} answers the dimension {dimension!r}, which dimensions.json does not ask"
            )
        seen[(stack, dimension)] += 1

    for (stack, dimension), count in sorted(seen.items(), key=lambda kv: str(kv[0])):
        if count > 1:
            problems.append(
                f"{stack}/{dimension} is answered {count} times; one stack gives one answer per"
                " dimension"
            )

    for stack in stack_list:
        for dimension in dimension_list:
            pair = (stack.get("id"), dimension.get("id"))
            if pair not in seen:
                problems.append(
                    f"{pair[0]}/{pair[1]} has neither an observation nor a 'not_evaluated' marker;"
                    " an empty cell must say whether nobody looked or nothing was found"
                )
    return problems


def marker_problems(observation) -> list[str]:
    """A marker says nobody looked, with a reason, and makes no other claim."""
    where = where_of(observation)
    reason = observation.get("not_evaluated")
    if not isinstance(reason, str) or not reason.strip():
        return [
            f"{where} carries an empty 'not_evaluated'; say why nobody looked, because an"
            " unexplained omission reads as a finding"
        ]
    return []


def check(dimension_list, stack_list, observation_list, values, today) -> list[str]:
    """Every claim the evidence does not back up."""
    selves = {s.get("id") for s in stack_list if s.get("is_self")}
    version = workspace_version()
    problems = []

    for dimension in dimension_list:
        problems.extend(schema_problems("dimension", dimension))
    for stack in stack_list:
        problems.extend(schema_problems("stack", stack))

    if len(selves) != 1:
        problems.append(
            f"stacks.json marks {len(selves)} stacks is_self; exactly one is this repository, and"
            " only that one may hold generated cells"
        )

    for observation in observation_list:
        # `.get` throughout: an observation can be malformed, and a checker that crashes on one
        # reports nothing about the rest of the page.
        problems.extend(schema_problems(kind_of(observation), observation))
        if is_marker(observation):
            problems.extend(marker_problems(observation))
            continue
        if not observation.get("version_evaluated"):
            problems.append(
                f"{where_of(observation)} has no 'version_evaluated'; a finding with no version"
                " has no subject"
            )
        problems.extend(evidence_problems(observation))
        problems.extend(confidence_problems(observation, selves))
        problems.extend(generated_problems(observation, values))
        problems.extend(self_version_problems(observation, selves, version))
        problems.extend(staleness_problems(observation, today))

    problems.extend(coverage_problems(dimension_list, stack_list, observation_list))
    return problems


def plural(count: int, noun: str) -> str:
    """A success line that says `1 stacks` reads as a script nobody finished."""
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def substitute(text: str, values: dict[str, str]) -> str:
    """Put the recomputed values into a generated cell's placeholders."""
    return PLACEHOLDER.sub(lambda m: values.get(m.group(1), m.group(0)), text)


def cell(text: str) -> str:
    """Markdown table cells cannot carry a bar, and a finding often wants one."""
    return text.replace("|", "\\|")


def evidence_cell(observation) -> str:
    links = []
    for entry in observation.get("evidence", []):
        note = cell(entry.get("note", "evidence"))
        if entry.get("url"):
            links.append(f"[{note}]({entry['url']})")
        elif entry.get("path"):
            href = os.path.relpath(ROOT / entry["path"], REPORT.parent)
            links.append(f"[{note}]({href.replace(os.path.sep, '/')})")
    return " · ".join(links) or "—"


def render_capability_ledgers(ledger_list, stack_list) -> list[str]:
    """Render the finite parity target without collapsing leaves into a score."""
    names = {stack.get("id"): stack.get("name", stack.get("id", "?")) for stack in stack_list}
    out = []
    for ledger in ledger_list:
        subject = ledger.get("subject", "?")
        out += [
            f"## Endpoint capability ledger: {cell(str(names.get(subject, subject)))}",
            "",
            "This is the leaf-level ownership profile for one immutable subject release. It is a",
            "finite discovery gate, not a score: an open sipx row names the story that closes it,",
            "and a platform row is excluded or assigned to the cluster repository explicitly.",
            "The immutable source revision is retained in the checked data and each row states the",
            "subject version it was evaluated against.",
            "",
            "| Category | Capability | Subject version | Confidence | Ownership | Status | Evidence | Story or rationale |",
            "|---|---|---|---|---|---|---|---|",
        ]
        for capability in ledger.get("capabilities", []):
            story = capability.get("story")
            if isinstance(story, str) and story.startswith("http"):
                disposition = f"[tracking story]({story})"
            elif isinstance(story, str) and story:
                href = os.path.relpath(ROOT / story, REPORT.parent).replace(os.path.sep, "/")
                disposition = f"[tracking story]({href})"
            else:
                disposition = cell(capability.get("rationale", "—"))
            evidence = evidence_cell({"evidence": capability.get("evidence", [])})
            out.append(
                f"| {cell(str(capability.get('category', '—')))} |"
                f" {cell(str(capability.get('title', capability.get('id', '—'))))} |"
                f" `{cell(str(ledger.get('version_evaluated', '—')))}` |"
                f" `{cell(str(capability.get('confidence', '—')))}` |"
                f" `{cell(str(capability.get('ownership', '—')))}` |"
                f" `{cell(str(capability.get('status', '—')))}` | {evidence} | {disposition} |"
            )
        out.append("")
    return out


def render(dimension_list, stack_list, observation_list, values, ledger_list=None, load=None) -> str:
    answers = {
        (o.get("stack"), o.get("dimension")): o for o in observation_list
    }

    out = [
        "# Stack comparison",
        "",
        "<!-- Generated by scripts/comparison-report.py from docs/comparison/. Do not edit. -->",
        "",
        "What choosing sipx wins and what it costs, against the stacks a reader is actually",
        "weighing it against. Every cell carries the tier of confidence behind it, because this",
        "comparison is asymmetric: sipx's own column is computed from this repository, and every",
        "other column is somebody reading someone else's code.",
        "",
        "`scripts/comparison-report.py --check` runs in CI. It fails if a claim cites no evidence,",
        "if a measurement carries no command to re-run it, if a judgment carries no reasoning, or",
        "if any observation has aged past its limit. There is no list of claims it agrees to skip:",
        "a row that cannot be evidenced is demoted to a lower tier or removed, and both change what",
        "this page says.",
        "",
        "## How to read a confidence tier",
        "",
        "| Tier | Means | Who may hold it |",
        "|---|---|---|",
    ]
    for tier in CONFIDENCE_TIERS:
        out.append(f"| `{tier}` | {CONFIDENCE_MEANING[tier]} | {CONFIDENCE_HOLDER[tier]} |")

    out += [
        "",
        f"An observation older than {MAX_OBSERVATION_AGE_DAYS} days fails the check rather than",
        f"being published with a note, and from {STALE_WARNING_DAYS} days out every run says so —",
        "the deadline is meant to be met deliberately rather than discovered. A stack that was not",
        "evaluated on a question says so in its own row, so an empty cell never has to be",
        "interpreted.",
        "",
        "## The stacks",
        "",
        "| Stack | Language | Licence | Source |",
        "|---|---|---|---|",
    ]
    for stack in stack_list:
        name = cell(stack.get("name", stack.get("id", "?")))
        out.append(
            f"| {name} | {cell(stack.get('language', '—'))} |"
            f" {cell(stack.get('license', '—'))} |"
            f" [repository]({stack.get('repository', '')}) |"
        )
    out.append("")

    for dimension in dimension_list:
        out += [
            f"## {dimension.get('title', dimension.get('id', '?'))}",
            "",
            f"**{cell(dimension.get('question', ''))}**",
            "",
            dimension.get("why", ""),
            "",
            "| Stack | Finding | Confidence | Evidence | Reproduce |",
            "|---|---|---|---|---|",
        ]
        for stack in stack_list:
            name = cell(stack.get("name", stack.get("id", "?")))
            observation = answers.get((stack.get("id"), dimension.get("id")))
            if observation is None:
                # `coverage_problems` has already failed the run; render something rather than
                # crash, so the message a reader gets is the checker's and not a traceback.
                out.append(f"| {name} | — | — | — | — |")
                continue
            if is_marker(observation):
                reason = cell(observation.get("not_evaluated", ""))
                out.append(f"| {name} | _Not evaluated: {reason}_ | — | — | — |")
                continue
            tier = observation.get("confidence", "—")
            version = cell(str(observation.get("version_evaluated", "—")))
            summary = cell(substitute(observation.get("summary", ""), values))
            if observation.get("rationale"):
                summary += f" _Rationale: {cell(observation['rationale'])}_"
            reproduce = observation.get("reproduce")
            command = f"`{cell(reproduce)}`" if reproduce else "—"
            out.append(
                f"| {name} | {summary} | `{tier}` · at {version} |"
                f" {evidence_cell(observation)} | {command} |"
            )
        out.append("")

    out.extend(render_capability_ledgers(ledger_list or [], stack_list))

    if load is not None:
        load_data, load_run_map = load
        out.extend(render_load_section(load_data, load_run_map, stack_list))

    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="verify claims and that the report is current"
    )
    args = parser.parse_args()

    dimension_list, stack_list, observation_list = dataset()
    ledger_list = capability_ledgers()
    expectations, expectation_problems = capability_expectations()

    # Shape before substance. `render` reads records directly, so a malformed one would crash it —
    # and a traceback in place of "sipx/media carries the unknown key 'score'" tells whoever added
    # the row nothing about what to do next.
    malformed = [p for d in dimension_list for p in schema_problems("dimension", d)]
    malformed += [p for s in stack_list for p in schema_problems("stack", s)]
    malformed += [p for o in observation_list for p in schema_problems(kind_of(o), o)]
    malformed += [p for ledger in ledger_list for p in capability_schema_problems(ledger)]
    malformed += external_story_index_problems()
    malformed += expectation_problems
    if malformed:
        print("The comparison registry does not match its schema:", file=sys.stderr)
        for problem in malformed:
            print(f"  {problem}", file=sys.stderr)
        return 1

    values = generated_values()
    today = datetime.date.today()
    problems = check(dimension_list, stack_list, observation_list, values, today)
    problems.extend(
        capability_problems(
            ledger_list,
            stack_list,
            today,
            external_story_urls(),
            expectations,
        )
    )
    load_data = load_dataset()
    load_run_map = load_runs(load_data)
    problems.extend(load_problems(load_data, load_run_map, stack_list, today))
    rendered = render(
        dimension_list,
        stack_list,
        observation_list,
        values,
        ledger_list,
        load=(load_data, load_run_map),
    )

    # Notice, not a result. Printed in both modes and before the verdict, so it is visible on the
    # run that still passes — which is the only run on which it is any use.
    for notice in expiring_soon(observation_list, today):
        print(f"notice: {notice}", file=sys.stderr)

    if args.check:
        if REPORT.exists() and REPORT.read_text(encoding="utf-8") != rendered:
            problems.append(
                f"{REPORT.relative_to(ROOT)} is out of date; run scripts/comparison-report.py"
            )
        if problems:
            print("Comparison claims the evidence does not back up:", file=sys.stderr)
            for problem in problems:
                print(f"  {problem}", file=sys.stderr)
            return 1
        # The countdown rides on the success line rather than waiting for the warning band, so
        # "when does this need refreshing" is answerable from any green run.
        left = days_until_expiry(observation_list, today)
        countdown = "" if left is None else f" (next expires in {plural(left, 'day')})"
        print(
            f"comparison: {plural(len(stack_list), 'stack')} over"
            f" {plural(len(dimension_list), 'dimension')}, every claim evidenced,"
            f" {plural(sum(len(ledger.get('capabilities', [])) for ledger in ledger_list), 'capability row')}"
            f" owned, none stale{countdown}"
        )
        return 0

    if problems:
        for problem in problems:
            print(f"warning: {problem}", file=sys.stderr)
    REPORT.write_text(rendered, encoding="utf-8")
    print(
        f"wrote {REPORT.relative_to(ROOT)}: {plural(len(stack_list), 'stack')} over"
        f" {plural(len(dimension_list), 'dimension')}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
