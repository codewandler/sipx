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
import json
import os.path
import pathlib
import re
import subprocess
import sys
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
STACK_KEYS = ({"id", "name", "language", "repository", "license"}, {"is_self"})
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


def external_story_index_problems(directory=None) -> list[str]:
    """The offline external-story evidence is closed and pins each Git blob identity."""
    problems = []
    directory = pathlib.Path(directory) if directory is not None else EXTERNAL_STORIES
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
        if not isinstance(repository, str):
            problems.append(f"{where} has no repository URL")
        else:
            parsed = urlparse(repository)
            if parsed.scheme != "https" or not parsed.netloc:
                problems.append(f"{where} has an invalid repository URL")
        revision = loaded.get("source_revision")
        if not isinstance(revision, str) or re.fullmatch(r"[0-9a-f]{40}", revision) is None:
            problems.append(f"{where} has no full immutable source revision")
        stories = loaded.get("stories")
        if not isinstance(stories, list) or not stories:
            problems.append(f"{where} has no story paths")
        else:
            seen = Counter()
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
            for story_path, count in seen.items():
                if count > 1:
                    problems.append(f"{where} repeats story path {story_path!r}")
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
        if len(expected_ids) != len(set(expected_ids)):
            problems.append(f"{where} repeats a capability ID")
        if isinstance(subject, str) and isinstance(revision, str):
            if subject in expectations:
                problems.append(f"{where} repeats subject {subject!r}")
            expectations[subject] = (revision, set(expected_ids))
    return expectations, problems


def capability_where(ledger, capability=None) -> str:
    subject = ledger.get("subject", "?")
    if capability is None:
        return f"capability ledger {subject}"
    return f"{subject}/capability/{capability.get('id', '?')}"


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
                    if (
                        not isinstance(source, str)
                        or re.fullmatch(r"crates/.+\.rs", source) is None
                    ):
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
            if (
                capability.get("confidence") == "measured"
                and ledger.get("source_revision") not in url
            ):
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
    external_stories = set(external_stories or ())
    expectations = expectations or {}
    problems = []
    subjects = Counter()
    for ledger in ledger_list:
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
            seen[cap_id] += 1
            categories.add(capability.get("category"))
            owner = capability.get("ownership")
            status = capability.get("status")
            confidence = capability.get("confidence")
            if confidence not in CAPABILITY_CONFIDENCE:
                problems.append(
                    f"{leaf} has unknown confidence {confidence!r}; choose one of"
                    f" {', '.join(sorted(CAPABILITY_CONFIDENCE))}"
                )
            elif confidence == "assessed" and not capability.get("rationale"):
                problems.append(f"{leaf} is assessed without a rationale")
            if owner not in CAPABILITY_OWNERS:
                problems.append(
                    f"{leaf} has unknown ownership {owner!r}; choose one of"
                    f" {', '.join(CAPABILITY_OWNERS)}"
                )
            elif status not in CAPABILITY_STATUS[owner]:
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
                        path = ROOT / source if isinstance(source, str) else ROOT
                        if (
                            not isinstance(source, str)
                            or not source.startswith("crates/")
                            or path.suffix != ".rs"
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
    return problems


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


def render(dimension_list, stack_list, observation_list, values, ledger_list=None) -> str:
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
    rendered = render(dimension_list, stack_list, observation_list, values, ledger_list)

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
