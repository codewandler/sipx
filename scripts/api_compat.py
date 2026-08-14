"""Deterministic Supported Rust API extraction and compatibility rules.

The command-line entry point is ``check-api-compat.py``.  This module is kept importable so the
adversarial vectors exercise the same comparison and archive-boundary code as the release check.
Only the Python standard library is used; an ordinary run has no package-manager or network edge.
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import re
import subprocess
import tomllib
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parent.parent
TOOLCHAIN_FILE = ROOT / "docs" / "api" / "toolchain.toml"
BASELINE = ROOT / "docs" / "api" / "v1-supported.json"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
BEGIN = "<!-- BEGIN sipx-api-classification -->"
END = "<!-- END sipx-api-classification -->"
SCHEMA = 1
BASELINE_MAJOR = 1
SOURCE_AUTO_TRAITS = {
    "core::marker::Send",
    "core::marker::Sync",
    "core::marker::Unpin",
    "core::panic::unwind_safe::RefUnwindSafe",
    "core::panic::unwind_safe::UnwindSafe",
}


class ApiCompatError(RuntimeError):
    """A deterministic extraction prerequisite or classification contract failed."""


def _run(args: list[str], *, cwd: pathlib.Path = ROOT) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=os.environ.copy(),
    )


def load_toolchain() -> dict[str, Any]:
    try:
        value = tomllib.loads(TOOLCHAIN_FILE.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ApiCompatError(f"cannot read {TOOLCHAIN_FILE.relative_to(ROOT)}: {error}") from error
    required = ("channel", "commit", "target", "rustdoc_format")
    missing = [key for key in required if key not in value]
    if missing:
        raise ApiCompatError(
            f"{TOOLCHAIN_FILE.relative_to(ROOT)} lacks {', '.join(sorted(missing))}"
        )
    if not re.fullmatch(r"nightly-\d{4}-\d{2}-\d{2}", str(value["channel"])):
        raise ApiCompatError("the API rustdoc toolchain must be a date-pinned nightly")
    if not re.fullmatch(r"[0-9a-f]{40}", str(value["commit"])):
        raise ApiCompatError("the API rustdoc commit must be a full 40-digit hash")
    return value


def verify_toolchain(config: dict[str, Any]) -> None:
    channel = str(config["channel"])
    result = _run(["rustc", f"+{channel}", "-vV"])
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        suffix = f": {detail[-1]}" if detail else ""
        raise ApiCompatError(
            f"required API toolchain `{channel}` is not installed{suffix}; ordinary checks never "
            "install or update toolchains"
        )
    fields: dict[str, str] = {}
    for line in result.stdout.splitlines():
        key, separator, value = line.partition(":")
        if separator:
            fields[key.strip()] = value.strip()
    if fields.get("commit-hash") != config["commit"]:
        raise ApiCompatError(
            f"`{channel}` has commit {fields.get('commit-hash', 'unknown')}, expected "
            f"{config['commit']}"
        )
    if fields.get("host") != config["target"]:
        raise ApiCompatError(
            f"`{channel}` host is {fields.get('host', 'unknown')}, expected {config['target']}"
        )


def verify_ci_toolchain(config: dict[str, Any]) -> None:
    """Keep CI's installer on the same exact nightly the baseline records."""
    try:
        text = WORKFLOW.read_text(encoding="utf-8")
    except OSError as error:
        raise ApiCompatError(f"cannot read {WORKFLOW.relative_to(ROOT)}: {error}") from error
    problem = ci_toolchain_problem(text, str(config["channel"]))
    if problem is not None:
        raise ApiCompatError(f"{WORKFLOW.relative_to(ROOT)}: {problem}")


def ci_toolchain_problem(text: str, channel: str) -> str | None:
    """Require the exact nightly in the same CI job, before both API boundary checks."""
    jobs: dict[str, str] = {}
    starts = list(re.finditer(r"(?m)^  ([a-zA-Z0-9_-]+):\s*$", text))
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(text)
        jobs[match.group(1)] = text[match.start() : end]
    commands = (
        "./scripts/check-api-compat.py --check",
        "./scripts/check-packaged-endpoint-consumer.py --check",
    )
    def active_step(block: str, key: str, value: str) -> re.Match[str] | None:
        return re.search(
            rf"(?m)^\s*-\s+{key}:\s*{re.escape(value)}(?:\s*(?:#.*)?)?$",
            block,
        )

    owners = [
        name
        for name, block in jobs.items()
        if all(active_step(block, "run", command) is not None for command in commands)
    ]
    if len(owners) != 1:
        return "both API compatibility checks must run together in exactly one CI job"
    block = jobs[owners[0]]
    installer = "dtolnay/rust-toolchain@nightly"
    installer_match = active_step(block, "uses", installer)
    if installer_match is None:
        return f"CI job `{owners[0]}` does not use the nightly toolchain installer"
    next_step = re.search(r"(?m)^\s*-\s+", block[installer_match.end() :])
    installer_end = (
        installer_match.end() + next_step.start()
        if next_step is not None
        else len(block)
    )
    installer_step = block[installer_match.start() : installer_end]
    if re.search(
        rf"(?m)^\s+toolchain:\s*{re.escape(channel)}(?:\s*(?:#.*)?)?$",
        installer_step,
    ) is None:
        return f"CI job `{owners[0]}` does not install exact toolchain `{channel}`"
    command_positions = [active_step(block, "run", command) for command in commands]
    if any(
        installer_match.start() > match.start()
        for match in command_positions
        if match is not None
    ):
        return f"CI job `{owners[0]}` installs `{channel}` after an API compatibility check"
    return None


def workspace_metadata() -> dict[str, Any]:
    result = _run(
        [
            "cargo",
            "metadata",
            "--format-version=1",
            "--no-deps",
            "--locked",
            "--offline",
        ]
    )
    if result.returncode != 0:
        raise ApiCompatError(f"cargo metadata failed:\n{result.stderr.strip()}")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ApiCompatError(f"cargo metadata returned invalid JSON: {error}") from error


def publishable_packages(metadata: dict[str, Any]) -> list[dict[str, Any]]:
    members = set(metadata["workspace_members"])
    packages = [
        package
        for package in metadata["packages"]
        if package["id"] in members and package.get("publish") != []
    ]
    return sorted(packages, key=lambda package: package["name"])


def library_target(package: dict[str, Any]) -> dict[str, Any] | None:
    libraries = [
        target
        for target in package["targets"]
        if any(kind in ("lib", "rlib", "proc-macro") for kind in target["kind"])
    ]
    if len(libraries) > 1:
        raise ApiCompatError(f"{package['name']} has more than one library target")
    return libraries[0] if libraries else None


def parse_classification(source: pathlib.Path, crate_name: str) -> list[str]:
    text = source.read_text(encoding="utf-8")
    if text.count(BEGIN) != 1 or text.count(END) != 1:
        raise ApiCompatError(
            f"{source.relative_to(ROOT)} must contain exactly one {BEGIN!r}/{END!r} pair"
        )
    begin = text.index(BEGIN)
    end = text.index(END)
    stability = text.find("//! # Stability")
    if stability < 0 or not stability < begin < end:
        raise ApiCompatError(
            f"{source.relative_to(ROOT)} classification must be inside its crate-level # Stability rustdoc"
        )
    next_heading = text.find("//! # ", stability + len("//! # Stability"))
    if next_heading >= 0 and end > next_heading:
        raise ApiCompatError(
            f"{source.relative_to(ROOT)} classification lies outside the # Stability section"
        )
    body = text[begin + len(BEGIN) : end]
    entries: list[str] = []
    saw_none = False
    for raw_line in body.splitlines():
        line = raw_line.strip()
        if line.startswith("//!"):
            line = line[3:].strip()
        if not line or line == "**Experimental Rust API roots:**":
            continue
        if line == "- None.":
            saw_none = True
            continue
        match = re.fullmatch(r"- \[`([^`]+)`\](?:\((crate(?:::[^)]+)+)\))?", line)
        if match is None:
            raise ApiCompatError(
                f"{source.relative_to(ROOT)} has invalid classification line {line!r}"
            )
        entry = match.group(1)
        target = match.group(2)
        expected_target = "crate" + entry[entry.find("::") :]
        if target is not None and target != expected_target:
            raise ApiCompatError(
                f"{source.relative_to(ROOT)} root `{entry}` links to `{target}`, expected "
                f"`{expected_target}`"
            )
        entries.append(entry)
    if saw_none and entries:
        raise ApiCompatError(f"{source.relative_to(ROOT)} mixes `None` with Experimental roots")
    if not saw_none and not entries:
        raise ApiCompatError(f"{source.relative_to(ROOT)} classification is empty")
    expected = crate_name.replace("-", "_")
    for entry in entries:
        if not entry.startswith(expected + "::"):
            raise ApiCompatError(
                f"{source.relative_to(ROOT)} root `{entry}` is not inside `{expected}`"
            )
        if entry.endswith("::") or "*" in entry:
            raise ApiCompatError(
                f"{source.relative_to(ROOT)} root `{entry}` must be one exact Rust path"
            )
    if entries != sorted(set(entries)):
        raise ApiCompatError(
            f"{source.relative_to(ROOT)} Experimental roots must be unique and sorted"
        )
    for index, root in enumerate(entries):
        for other in entries[index + 1 :]:
            if other.startswith(root + "::"):
                raise ApiCompatError(
                    f"{source.relative_to(ROOT)} roots `{root}` and `{other}` overlap"
                )
    return entries


class RustdocApi:
    """Canonicalize one rustdoc JSON crate without preserving numeric item identifiers."""

    def __init__(self, document: dict[str, Any]):
        self.document = document
        self.index: dict[str, dict[str, Any]] = document["index"]
        self.paths: dict[str, dict[str, Any]] = document["paths"]

    def stable_id(self, item_id: int | str | None) -> str | None:
        if item_id is None:
            return None
        key = str(item_id)
        summary = self.paths.get(key)
        if summary is not None:
            return "::".join(summary["path"])
        item = self.index.get(key)
        if item is None:
            return None
        inner = item.get("inner", {})
        kind = next(iter(inner), "item")
        name = item.get("name") or "anonymous"
        return f"unresolved::{kind}::{name}"

    def value(self, value: Any) -> Any:
        if isinstance(value, list):
            return [self.value(element) for element in value]
        if not isinstance(value, dict):
            return value
        result: dict[str, Any] = {}
        resolved = self.stable_id(value.get("id")) if "id" in value else None
        for key, element in sorted(value.items()):
            if key == "id":
                continue
            if key == "path" and resolved is not None:
                continue
            result[key] = self.value(element)
        if resolved is not None:
            result["resolved"] = resolved
        return result

    @staticmethod
    def attributes(item: dict[str, Any]) -> list[Any]:
        kept: list[Any] = []
        for attribute in item.get("attrs", []):
            if attribute == "non_exhaustive":
                kept.append("non_exhaustive")
            elif isinstance(attribute, dict) and "repr" in attribute:
                kept.append({"repr": attribute["repr"]})
            elif isinstance(attribute, str) and "repr(" in attribute:
                kept.append(attribute)
        return sorted(kept, key=lambda value: json.dumps(value, sort_keys=True))

    def field(self, item_id: int | str | None) -> dict[str, Any] | None:
        if item_id is None:
            return None
        item = self.index.get(str(item_id))
        if item is None or item.get("visibility") != "public":
            return None
        return {
            "name": item.get("name"),
            "type": self.value(item["inner"]["struct_field"]),
        }

    def variant_field(self, item_id: int | str) -> dict[str, Any]:
        """Retain enum payload fields, whose rustdoc visibility is normally `default`."""
        item = self.index[str(item_id)]
        return {
            "name": item.get("name"),
            "type": self.value(item["inner"]["struct_field"]),
        }

    def variant(self, item_id: int | str) -> tuple[str, dict[str, Any]]:
        item = self.index[str(item_id)]
        variant = item["inner"]["variant"]
        kind = variant["kind"]
        if isinstance(kind, dict) and "tuple" in kind:
            shape: Any = {
                "tuple": [self.variant_field(field)["type"] for field in kind["tuple"]]
            }
        elif isinstance(kind, dict) and "struct" in kind:
            detail = kind["struct"]
            fields = [self.variant_field(field) for field in detail["fields"]]
            shape = {
                "struct": {
                    "fields": {field["name"]: field["type"] for field in fields},
                    "has_stripped_fields": detail.get("has_stripped_fields", False),
                }
            }
        else:
            shape = kind
        return item["name"], {
            "attrs": self.attributes(item),
            "kind": shape,
            "discriminant": self.value(variant.get("discriminant")),
        }

    def trait_member(self, item_id: int | str) -> tuple[str, dict[str, Any]]:
        item = self.index[str(item_id)]
        kind = next(iter(item["inner"]))
        inner = item["inner"][kind]
        record: dict[str, Any] = {"kind": kind, "attrs": self.attributes(item)}
        if kind == "function":
            record.update(self.function(inner))
            record["has_default"] = bool(inner.get("has_body"))
        elif kind == "assoc_const":
            record["type"] = self.value(inner.get("type"))
            record["has_default"] = inner.get("value") is not None
        elif kind == "assoc_type":
            record.update(self.value(inner))
            record["has_default"] = inner.get("type") is not None
        else:
            record["value"] = self.value(inner)
        return item.get("name") or "_", record

    def implementation(self, item_id: int | str) -> tuple[str, dict[str, Any]] | None:
        item = self.index.get(str(item_id))
        if item is None or "impl" not in item.get("inner", {}):
            return None
        inner = item["inner"]["impl"]
        if inner.get("trait") is None:
            return None
        trait = self.value(inner["trait"])
        for_type = self.value(inner.get("for"))
        generics = self.value(inner.get("generics"))
        blanket = self.value(inner.get("blanket_impl"))
        identity = {
            "trait": trait,
            "for": for_type,
            "generics": generics,
            "blanket_impl": blanket,
            "is_negative": inner.get("is_negative", False),
        }
        encoded_identity = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        key = hashlib.sha256(encoded_identity).hexdigest()
        members: dict[str, Any] = {}
        for member_id in inner.get("items", []):
            member = self.index.get(str(member_id))
            if member is None or member.get("name") is None:
                continue
            member_kind = next(iter(member.get("inner", {})), "item")
            if member_kind == "function":
                continue
            members[member["name"]] = {
                "kind": member_kind,
                "value": self.value(member["inner"].get(member_kind)),
            }
        return key, {
            "trait": trait,
            "for": for_type,
            "generics": generics,
            "blanket_impl": blanket,
            "is_unsafe": inner.get("is_unsafe", False),
            "is_negative": inner.get("is_negative", False),
            "is_synthetic": inner.get("is_synthetic", False),
            "items": dict(sorted(members.items())),
        }

    def implementations(self, ids: list[int], *, owner: str) -> dict[str, Any]:
        """Keep promised impls without freezing rustdoc's external blanket projections.

        Type pages synthesize external blanket projections for every matching type. Those are
        derivative compiler output, not impls this crate can preserve. Direct impls and synthetic
        auto traits remain type-side commitments. Trait pages retain their declared implementation
        set, including blanket impls and impls for foreign types.
        """
        result: dict[str, Any] = {}
        for item_id in ids:
            item = self.index.get(str(item_id), {})
            inner = item.get("inner", {}).get("impl", {})
            if inner.get("is_negative", False):
                continue
            if owner == "type":
                if inner.get("is_synthetic", False):
                    trait_paths = resolved_paths(self.value(inner.get("trait")))
                    keep = bool(trait_paths & SOURCE_AUTO_TRAITS)
                else:
                    keep = inner.get("blanket_impl") is None
            elif owner == "trait":
                keep = True
            else:
                raise ApiCompatError(f"unknown implementation owner {owner!r}")
            if not keep:
                continue
            implementation = self.implementation(item_id)
            if implementation is not None:
                key, record = implementation
                result[key] = record
        return dict(sorted(result.items()))

    def function(self, inner: dict[str, Any]) -> dict[str, Any]:
        signature = inner["sig"]
        return {
            "inputs": [self.value(type_) for _, type_ in signature["inputs"]],
            "output": self.value(signature.get("output")),
            "is_c_variadic": signature.get("is_c_variadic", False),
            "generics": self.value(inner.get("generics", {})),
            "header": self.value(inner.get("header", {})),
        }

    def item(self, item_id: int | str, *, definition: str | None = None) -> dict[str, Any]:
        item = self.index[str(item_id)]
        kind = next(iter(item["inner"]))
        inner = item["inner"][kind]
        record: dict[str, Any] = {"kind": kind, "attrs": self.attributes(item)}
        if definition is not None:
            record["definition"] = definition
        if kind == "function":
            record["function"] = self.function(inner)
        elif kind == "struct":
            struct_kind = inner["kind"]
            fields: dict[str, Any] = {}
            shape = "unit"
            has_stripped_fields = False
            if isinstance(struct_kind, dict) and "plain" in struct_kind:
                shape = "plain"
                detail = struct_kind["plain"]
                has_stripped_fields = detail.get("has_stripped_fields", False)
                for field_id in detail["fields"]:
                    field = self.field(field_id)
                    if field is not None:
                        fields[str(field["name"])] = field["type"]
            elif isinstance(struct_kind, dict) and "tuple" in struct_kind:
                shape = "tuple"
                for position, field_id in enumerate(struct_kind["tuple"]):
                    field = self.field(field_id)
                    if field is not None:
                        fields[str(position)] = field["type"]
                    else:
                        has_stripped_fields = True
            record.update(
                {
                    "shape": shape,
                    "fields": dict(sorted(fields.items())),
                    "has_stripped_fields": has_stripped_fields,
                    "generics": self.value(inner.get("generics", {})),
                    "implementations": self.implementations(
                        inner.get("impls", []), owner="type"
                    ),
                }
            )
        elif kind == "union":
            fields = [self.field(field_id) for field_id in inner.get("fields", [])]
            record.update(
                {
                    "fields": {
                        field["name"]: field["type"] for field in fields if field is not None
                    },
                    "has_stripped_fields": inner.get("has_stripped_fields", False),
                    "generics": self.value(inner.get("generics", {})),
                    "implementations": self.implementations(
                        inner.get("impls", []), owner="type"
                    ),
                }
            )
        elif kind == "enum":
            variants = dict(self.variant(variant_id) for variant_id in inner.get("variants", []))
            record.update(
                {
                    "variants": dict(sorted(variants.items())),
                    "generics": self.value(inner.get("generics", {})),
                    "implementations": self.implementations(
                        inner.get("impls", []), owner="type"
                    ),
                }
            )
        elif kind == "trait":
            members = dict(self.trait_member(member_id) for member_id in inner.get("items", []))
            record.update(
                {
                    "items": dict(sorted(members.items())),
                    "generics": self.value(inner.get("generics", {})),
                    "bounds": self.value(inner.get("bounds", [])),
                    "is_unsafe": inner.get("is_unsafe", False),
                    "is_auto": inner.get("is_auto", False),
                    "is_dyn_compatible": inner.get("is_dyn_compatible", False),
                    "implementations": self.implementations(
                        inner.get("implementations", []), owner="trait"
                    ),
                }
            )
        elif kind == "module":
            record["is_crate"] = inner.get("is_crate", False)
        elif kind in ("assoc_const", "assoc_type"):
            _, member = self.trait_member(item_id)
            record.update(member)
        else:
            record[kind] = self.value(inner)
        return record

    def public_items(self) -> dict[str, dict[str, Any]]:
        items: dict[str, dict[str, Any]] = {}
        root_id = str(self.document["root"])

        def add_item(item_id: int | str, path: str, definition: str | None = None) -> None:
            key = str(item_id)
            raw = self.index.get(key)
            if raw is None:
                summary = self.paths.get(key)
                if summary is None:
                    raise ApiCompatError(f"public path {path} has no rustdoc item or summary")
                items[path] = {
                    "kind": summary["kind"],
                    "attrs": [],
                    **({"definition": definition} if definition is not None else {}),
                }
                return
            items[path] = self.item(key, definition=definition)
            inner_kind = next(iter(raw["inner"]))
            inner = raw["inner"][inner_kind]
            if inner_kind not in ("struct", "union", "enum"):
                return
            owner_definition = definition or self.stable_id(item_id) or path
            for impl_id in inner.get("impls", []):
                implementation = self.index.get(str(impl_id), {}).get("inner", {}).get("impl")
                if implementation is None or implementation.get("trait") is not None:
                    continue
                for member_id in implementation.get("items", []):
                    member = self.index.get(str(member_id))
                    if member is None or member.get("visibility") != "public" or not member.get("name"):
                        continue
                    member_path = f"{path}::{member['name']}"
                    member_definition = self.stable_id(member_id)
                    if member_definition is None or member_definition.startswith("unresolved::"):
                        member_definition = f"{owner_definition}::{member['name']}"
                    add_item(member_id, member_path, member_definition if member_definition != member_path else None)

        visited_modules: set[tuple[str, str]] = set()

        def walk_module(module_id: str, module_path: str) -> None:
            visit = (module_id, module_path)
            if visit in visited_modules:
                return
            visited_modules.add(visit)
            module = self.index[module_id]["inner"]["module"]
            for child_id in module.get("items", []):
                child = self.index.get(str(child_id))
                if child is None or child.get("visibility") != "public":
                    continue
                if "use" in child["inner"]:
                    use = child["inner"]["use"]
                    target_id = use.get("id")
                    definition = self.stable_id(target_id)
                    if target_id is None or definition is None:
                        raise ApiCompatError(
                            f"public re-export {module_path}::{use.get('name')} has no rustdoc target"
                        )
                    target = self.index.get(str(target_id))
                    if use.get("is_glob"):
                        if target is None or "module" not in target.get("inner", {}):
                            raise ApiCompatError(
                                f"public glob re-export in {module_path} has no local module target"
                            )
                        walk_module(str(target_id), module_path)
                        continue
                    alias = f"{module_path}::{use['name']}"
                    add_item(target_id, alias, definition if definition != alias else None)
                    if target is not None and "module" in target.get("inner", {}):
                        walk_module(str(target_id), alias)
                    continue
                name = child.get("name")
                if not name:
                    continue
                child_path = f"{module_path}::{name}"
                definition = self.stable_id(child_id)
                add_item(child_id, child_path, definition if definition != child_path else None)
                if "module" in child["inner"]:
                    walk_module(str(child_id), child_path)

        root_path = self.stable_id(root_id) or "crate"
        add_item(root_id, root_path)
        walk_module(root_id, root_path)
        return dict(sorted(items.items()))


def _below(path: str, root: str) -> bool:
    return path == root or path.startswith(root + "::")


def resolved_paths(value: Any) -> set[str]:
    """Collect canonical rustdoc references from a canonical item fragment."""
    if isinstance(value, list):
        return set().union(*(resolved_paths(element) for element in value))
    if not isinstance(value, dict):
        return set()
    found = {str(value["resolved"])} if "resolved" in value else set()
    for element in value.values():
        found.update(resolved_paths(element))
    return found


def unresolved_markers(value: Any) -> set[str]:
    """Find fallback identifiers, which are not safe canonical API identities."""
    if isinstance(value, str):
        return {value} if value.startswith("unresolved::") else set()
    if isinstance(value, list):
        return set().union(*(unresolved_markers(element) for element in value))
    if isinstance(value, dict):
        return set().union(*(unresolved_markers(element) for element in value.values()))
    return set()


def filter_supported_items(
    all_items: dict[str, dict[str, Any]], classifications_by_crate: dict[str, list[str]]
) -> dict[str, dict[str, Any]]:
    """Apply each publishable crate's visible classification to definitions and aliases."""
    roots = [root for values in classifications_by_crate.values() for root in values]
    supported: dict[str, dict[str, Any]] = {}
    for path, record in all_items.items():
        definition = record.get("definition", path)
        if any(_below(path, root) or _below(definition, root) for root in roots):
            continue
        filtered = dict(record)
        for child_kind in ("variants", "items"):
            children = record.get(child_kind)
            if children is not None:
                filtered[child_kind] = {
                    name: child
                    for name, child in children.items()
                    if not any(
                        _below(f"{path}::{name}", root)
                        or _below(f"{definition}::{name}", root)
                        for root in roots
                    )
                }
        implementations = record.get("implementations")
        if implementations is not None:
            filtered["implementations"] = {
                key: implementation
                for key, implementation in implementations.items()
                if not any(
                    _below(reference, root)
                    for reference in resolved_paths(
                        {
                            "trait": implementation.get("trait"),
                            "for": implementation.get("for"),
                            "generics": implementation.get("generics"),
                        }
                    )
                    for root in roots
                )
            }
        exposed_record = {key: value for key, value in filtered.items() if key != "definition"}
        unresolved = sorted(unresolved_markers(filtered))
        if unresolved:
            raise ApiCompatError(
                f"Supported path `{path}` contains unresolved rustdoc identity `{unresolved[0]}`"
            )
        exposed = sorted(
            reference
            for reference in resolved_paths(exposed_record)
            if any(_below(reference, root) for root in roots)
        )
        if exposed:
            raise ApiCompatError(
                f"Supported path `{path}` exposes Experimental API `{exposed[0]}` in its "
                "signature; classify the exposing path Experimental or remove that edge"
            )
        supported[path] = filtered
    return supported


def known_classification_paths(all_items: dict[str, dict[str, Any]]) -> set[str]:
    """Include nested public enum variants and trait items accepted as exact roots."""
    known = set(all_items)
    for path, record in all_items.items():
        definition = record.get("definition", path)
        for child_kind in ("variants", "items"):
            for name in record.get(child_kind, {}):
                known.add(f"{path}::{name}")
                known.add(f"{definition}::{name}")
    return known


def candidate_package(
    package: dict[str, Any], config: dict[str, Any], target_dir: pathlib.Path
) -> dict[str, Any]:
    library = library_target(package)
    if library is None:
        return {
            "library": False,
            "experimental_roots": [],
            "classification_sha256": hashlib.sha256(b"").hexdigest(),
            "items": {},
            "all_items": {},
        }
    source = pathlib.Path(library["src_path"])
    roots = parse_classification(source, package["name"])
    channel = str(config["channel"])
    target = str(config["target"])
    result = _run(
        [
            "cargo",
            f"+{channel}",
            "rustdoc",
            "-p",
            package["name"],
            "--lib",
            "--all-features",
            "--locked",
            "--offline",
            "--target",
            target,
            "--target-dir",
            str(target_dir),
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
        ]
    )
    if result.returncode != 0:
        raise ApiCompatError(
            f"rustdoc extraction failed for {package['name']}:\n{result.stderr.strip()}"
        )
    crate_name = library["name"].replace("-", "_")
    matches = list(target_dir.rglob(f"{crate_name}.json"))
    if len(matches) != 1:
        raise ApiCompatError(
            f"rustdoc extraction for {package['name']} produced {len(matches)} JSON files"
        )
    document = json.loads(matches[0].read_text(encoding="utf-8"))
    if document.get("format_version") != config["rustdoc_format"]:
        raise ApiCompatError(
            f"rustdoc JSON format is {document.get('format_version')}, expected "
            f"{config['rustdoc_format']}"
        )
    api = RustdocApi(document)
    all_items = api.public_items()
    known_paths = known_classification_paths(all_items)
    for root in roots:
        if root not in known_paths:
            raise ApiCompatError(
                f"{source.relative_to(ROOT)} Experimental root `{root}` is not public under all features"
            )
    supported = filter_supported_items(
        all_items, {package["name"].replace("-", "_"): roots}
    )
    classification_bytes = json.dumps(roots, separators=(",", ":")).encode()
    return {
        "library": True,
        "experimental_roots": roots,
        "classification_sha256": hashlib.sha256(classification_bytes).hexdigest(),
        "items": supported,
        "all_items": all_items,
    }


def extract_candidate() -> dict[str, Any]:
    config = load_toolchain()
    verify_ci_toolchain(config)
    verify_toolchain(config)
    metadata = workspace_metadata()
    packages = publishable_packages(metadata)
    versions = sorted({package["version"] for package in packages})
    if len(versions) != 1:
        raise ApiCompatError(f"publishable workspace package versions disagree: {versions}")
    target_dir = ROOT / "target" / "api-compat" / "rustdoc"
    package_records: dict[str, Any] = {}
    for package in packages:
        package_records[package["name"]] = candidate_package(package, config, target_dir)
    classifications_by_crate = {
        name.replace("-", "_"): record["experimental_roots"]
        for name, record in package_records.items()
        if record["library"]
    }
    for record in package_records.values():
        record["items"] = filter_supported_items(
            record["all_items"], classifications_by_crate
        )
    classification = {
        name: record["experimental_roots"] for name, record in package_records.items()
    }
    return {
        "schema": SCHEMA,
        "workspace_version": versions[0],
        "rustdoc": {
            "toolchain": config["channel"],
            "commit": config["commit"],
            "target": config["target"],
            "format_version": config["rustdoc_format"],
            "feature_mode": "all-features",
        },
        "classification_sha256": hashlib.sha256(
            json.dumps(classification, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "packages": package_records,
    }


def baseline_document(candidate: dict[str, Any]) -> dict[str, Any]:
    version = str(candidate.get("workspace_version", ""))
    match = re.match(r"^(0|[1-9][0-9]*)\.", version)
    if match is None or int(match.group(1)) != BASELINE_MAJOR:
        raise ApiCompatError(
            f"cannot write {BASELINE.name} from workspace version {version!r}; the v1 baseline is "
            "immutable across a future major, which needs a new major-bound filename and review"
        )
    result = json.loads(json.dumps(candidate))
    for package in result["packages"].values():
        package.pop("all_items", None)
    return result


def _first_difference(baseline: Any, candidate: Any, prefix: str = "") -> str | None:
    if type(baseline) is not type(candidate):
        return prefix or "record"
    if isinstance(baseline, dict):
        for key in sorted(set(baseline) | set(candidate)):
            field = f"{prefix}.{key}" if prefix else key
            if key not in baseline or key not in candidate:
                return field
            difference = _first_difference(baseline[key], candidate[key], field)
            if difference is not None:
                return difference
        return None
    if isinstance(baseline, list):
        if len(baseline) != len(candidate):
            return prefix or "list"
        for index, (old, new) in enumerate(zip(baseline, candidate, strict=True)):
            difference = _first_difference(old, new, f"{prefix}[{index}]")
            if difference is not None:
                return difference
        return None
    return None if baseline == candidate else prefix or "value"


def _variant_difference(baseline: dict[str, Any], candidate: dict[str, Any]) -> str | None:
    """Compare one enum variant, honoring only its own non-exhaustive field reservation."""
    if "non_exhaustive" not in baseline.get("attrs", []):
        return _first_difference(baseline, candidate, "variant")
    attrs = _first_difference(
        baseline.get("attrs", []), candidate.get("attrs", []), "variant.attrs"
    )
    if attrs is not None:
        return attrs
    discriminant = _first_difference(
        baseline.get("discriminant"),
        candidate.get("discriminant"),
        "variant.discriminant",
    )
    if discriminant is not None:
        return discriminant
    old_kind = baseline.get("kind")
    new_kind = candidate.get("kind")
    if isinstance(old_kind, dict) and isinstance(new_kind, dict):
        if "tuple" in old_kind and "tuple" in new_kind:
            old_fields = old_kind["tuple"]
            new_fields = new_kind["tuple"]
            if len(new_fields) < len(old_fields):
                return "variant.kind.tuple"
            return _first_difference(
                old_fields, new_fields[: len(old_fields)], "variant.kind.tuple"
            )
        if "struct" in old_kind and "struct" in new_kind:
            old_detail = old_kind["struct"]
            new_detail = new_kind["struct"]
            stripped = _first_difference(
                old_detail.get("has_stripped_fields", False),
                new_detail.get("has_stripped_fields", False),
                "variant.kind.struct.has_stripped_fields",
            )
            if stripped is not None:
                return stripped
            old_fields = old_detail.get("fields", {})
            new_fields = new_detail.get("fields", {})
            for name, old_field in sorted(old_fields.items()):
                if name not in new_fields:
                    return f"variant.kind.struct.fields.{name}"
                changed = _first_difference(
                    old_field, new_fields[name], f"variant.kind.struct.fields.{name}"
                )
                if changed is not None:
                    return changed
            return None
    return _first_difference(old_kind, new_kind, "variant.kind")


def _trait_member_difference(baseline: dict[str, Any], candidate: dict[str, Any]) -> str | None:
    """A new default relieves implementors; removal of an existing default is breaking."""
    if baseline.get("has_default", False) and not candidate.get("has_default", False):
        return "trait item.has_default"
    old_common = dict(baseline)
    new_common = dict(candidate)
    old_common.pop("has_default", None)
    new_common.pop("has_default", None)
    return _first_difference(old_common, new_common, "trait item")


def compatibility_problems(
    package_name: str, baseline: dict[str, Any], candidate: dict[str, Any]
) -> list[str]:
    problems: list[str] = []
    baseline_items = baseline.get("items", {})
    candidate_items = candidate.get("items", {})
    all_candidate = candidate.get("all_items", candidate_items)
    experimental = candidate.get("experimental_roots", [])
    for path, old in sorted(baseline_items.items()):
        new = candidate_items.get(path)
        if new is None:
            all_record = all_candidate.get(path, {})
            definition = all_record.get("definition", path)
            if path in all_candidate and any(
                _below(path, root) or _below(definition, root) for root in experimental
            ):
                problems.append(
                    f"{package_name}: `{path}` was Supported and is newly classified Experimental"
                )
            else:
                problems.append(f"{package_name}: removed Supported path `{path}`")
            continue
        if old.get("kind") != new.get("kind"):
            problems.append(
                f"{package_name}: `{path}` changed kind from {old.get('kind')} to {new.get('kind')}"
            )
            continue

        if "non_exhaustive" in old.get("attrs", []) and "non_exhaustive" not in new.get(
            "attrs", []
        ):
            problems.append(f"{package_name}: `{path}` lost `non_exhaustive`")
            continue
        if "non_exhaustive" not in old.get("attrs", []) and "non_exhaustive" in new.get(
            "attrs", []
        ):
            problems.append(
                f"{package_name}: `{path}` added `non_exhaustive` after callers could rely on exhaustiveness"
            )
            continue
        if old.get("kind") == "trait" and old.get(
            "is_dyn_compatible", False
        ) and not new.get("is_dyn_compatible", False):
            problems.append(f"{package_name}: `{path}` lost `is_dyn_compatible`")
            continue

        old_common = dict(old)
        new_common = dict(new)
        for child in (
            "variants",
            "fields",
            "items",
            "implementations",
            "has_stripped_fields",
            "is_dyn_compatible",
        ):
            old_common.pop(child, None)
            new_common.pop(child, None)
        difference = _first_difference(old_common, new_common)
        if difference is not None:
            problems.append(f"{package_name}: `{path}` changed `{difference}`")
            continue

        kind = old.get("kind")
        if kind == "struct":
            if (
                not old.get("has_stripped_fields", False)
                and new.get("has_stripped_fields", False)
                and "non_exhaustive" not in old.get("attrs", [])
            ):
                problems.append(
                    f"{package_name}: `{path}` added a private or stripped field after callers "
                    "could construct it"
                )
            old_fields = old.get("fields", {})
            new_fields = new.get("fields", {})
            for field, old_field in sorted(old_fields.items()):
                if field not in new_fields:
                    problems.append(f"{package_name}: `{path}` removed public field `{field}`")
                else:
                    changed = _first_difference(old_field, new_fields[field], "type")
                    if changed is not None:
                        problems.append(
                            f"{package_name}: `{path}` changed public field `{field}` `{changed}`"
                        )
            additions = sorted(set(new_fields) - set(old_fields))
            if (
                additions
                and "non_exhaustive" not in old.get("attrs", [])
                and not old.get("has_stripped_fields", False)
            ):
                problems.append(
                    f"{package_name}: `{path}` added public field(s) {', '.join(additions)} "
                    "without a baseline non_exhaustive reservation"
                )
        elif kind == "union":
            old_fields = old.get("fields", {})
            new_fields = new.get("fields", {})
            for field, old_field in sorted(old_fields.items()):
                if field not in new_fields:
                    problems.append(f"{package_name}: `{path}` removed public field `{field}`")
                else:
                    changed = _first_difference(old_field, new_fields[field], "type")
                    if changed is not None:
                        problems.append(
                            f"{package_name}: `{path}` changed public field `{field}` `{changed}`"
                        )
        elif kind == "enum":
            old_variants = old.get("variants", {})
            new_variants = new.get("variants", {})
            for variant, old_variant in sorted(old_variants.items()):
                if variant not in new_variants:
                    problems.append(f"{package_name}: `{path}` removed variant `{variant}`")
                else:
                    changed = _variant_difference(old_variant, new_variants[variant])
                    if changed is not None:
                        problems.append(
                            f"{package_name}: `{path}::{variant}` changed `{changed}`"
                        )
            additions = sorted(set(new_variants) - set(old_variants))
            if additions and "non_exhaustive" not in old.get("attrs", []):
                problems.append(
                    f"{package_name}: `{path}` added variant(s) {', '.join(additions)} without a "
                    "baseline non_exhaustive reservation"
                )
        elif kind == "trait":
            old_members = old.get("items", {})
            new_members = new.get("items", {})
            for member, old_member in sorted(old_members.items()):
                if member not in new_members:
                    problems.append(f"{package_name}: `{path}` removed trait item `{member}`")
                else:
                    changed = _trait_member_difference(old_member, new_members[member])
                    if changed is not None:
                        problems.append(
                            f"{package_name}: `{path}::{member}` changed `{changed}`"
                        )
            for member in sorted(set(new_members) - set(old_members)):
                added = new_members[member]
                if added.get("kind") not in ("function", "assoc_const") or not added.get(
                    "has_default", False
                ):
                    problems.append(
                        f"{package_name}: `{path}` added required trait item `{member}`"
                    )

        old_impls = old.get("implementations", {})
        new_impls = new.get("implementations", {})
        for implementation, old_record in sorted(old_impls.items()):
            if implementation not in new_impls:
                problems.append(
                    f"{package_name}: `{path}` removed implementation `{implementation}`"
                )
            else:
                changed = _first_difference(old_record, new_impls[implementation], "implementation")
                if changed is not None:
                    problems.append(
                        f"{package_name}: `{path}` changed implementation `{implementation}` at `{changed}`"
                    )
    return problems


def document_problems(
    baseline: dict[str, Any],
    candidate: dict[str, Any],
    *,
    allow_additive_packages: bool = False,
) -> list[str]:
    problems: list[str] = []
    if baseline.get("schema") != SCHEMA:
        problems.append(f"baseline schema is {baseline.get('schema')}, expected {SCHEMA}")
    baseline_version = str(baseline.get("workspace_version", ""))
    candidate_version = str(candidate.get("workspace_version", ""))
    version_pattern = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
    baseline_match = version_pattern.match(baseline_version)
    candidate_match = version_pattern.match(candidate_version)
    if baseline_match is None or candidate_match is None:
        problems.append("baseline or candidate has an invalid workspace version")
    elif (
        int(baseline_match.group(1)) != BASELINE_MAJOR
        or int(candidate_match.group(1)) != BASELINE_MAJOR
    ):
        problems.append(
            f"the v{BASELINE_MAJOR} baseline selector cannot compare baseline "
            f"v{baseline_match.group(1)} with candidate v{candidate_match.group(1)}; cut the "
            "deliberately reviewed next-major baseline and selector"
        )
    if baseline.get("rustdoc") != candidate.get("rustdoc"):
        difference = _first_difference(baseline.get("rustdoc"), candidate.get("rustdoc"), "rustdoc")
        problems.append(f"baseline and candidate differ at `{difference}`")
    baseline_packages = baseline.get("packages", {})
    candidate_packages = candidate.get("packages", {})
    for name in sorted(set(baseline_packages) - set(candidate_packages)):
        problems.append(f"publishable package `{name}` is missing from the candidate")
    if not allow_additive_packages:
        for name in sorted(set(candidate_packages) - set(baseline_packages)):
            problems.append(
                f"publishable package `{name}` is missing from the baseline; review it with "
                "the compatible-only baseline updater"
            )
    for name in sorted(set(baseline_packages) & set(candidate_packages)):
        if baseline_packages[name].get("library") != candidate_packages[name].get("library"):
            problems.append(
                f"publishable package `{name}` changed whether it exposes a library target"
            )
            continue
        problems.extend(
            compatibility_problems(name, baseline_packages[name], candidate_packages[name])
        )
    return problems


def baseline_update_problems(
    existing: dict[str, Any], candidate: dict[str, Any]
) -> list[str]:
    """Roll additions forward, but never erase or change an existing v1 commitment."""
    return document_problems(existing, candidate, allow_additive_packages=True)


def archive_patch_problem(
    workspace_root: pathlib.Path, archive_root: pathlib.Path, patch_path: pathlib.Path
) -> str | None:
    """Refuse a consumer rehearsal that could observe the live source tree."""
    workspace = workspace_root.resolve()
    archive = archive_root.resolve()
    path = patch_path.resolve()
    try:
        path.relative_to(archive)
    except ValueError:
        try:
            path.relative_to(workspace)
            location = "live workspace source"
        except ValueError:
            location = "a path outside the owned archive extraction root"
        return f"consumer patch points at {location}, not a packaged archive: {path}"
    return None


def archive_resolution_problems(
    metadata: dict[str, Any], archive_paths: dict[str, pathlib.Path], version: str
) -> list[str]:
    """Prove Cargo resolved every sipx consumer node to the extracted archive tree."""
    problems: list[str] = []
    packages = metadata.get("packages", [])
    root_id = metadata.get("resolve", {}).get("root")
    sipx_nodes = [
        package
        for package in packages
        if package.get("name", "").startswith("sipx-")
        and (root_id is None or package.get("id") != root_id)
    ]
    expected_nodes = {(name, version) for name in archive_paths}
    actual_nodes = {(str(package.get("name")), str(package.get("version"))) for package in sipx_nodes}
    for name, actual_version in sorted(actual_nodes - expected_nodes):
        problems.append(
            f"consumer metadata contains unexpected sipx node `{name}` {actual_version}"
        )
    for name, expected in sorted(archive_paths.items()):
        matching = [
            package
            for package in sipx_nodes
            if package.get("name") == name and package.get("version") == version
        ]
        if len(matching) != 1:
            problems.append(
                f"consumer metadata contains {len(matching)} `{name}` {version} nodes, expected one"
            )
            continue
        package = matching[0]
        if package.get("source") is not None:
            problems.append(f"consumer resolved `{name}` from {package.get('source')}, not an archive path")
        actual = pathlib.Path(str(package.get("manifest_path", ""))).resolve().parent
        if actual != expected.resolve():
            problems.append(
                f"consumer resolved `{name}` at {actual}, expected extracted archive {expected.resolve()}"
            )
    return problems


def tree_digest(root: pathlib.Path) -> str:
    """Hash an extracted tree's paths, file modes and bytes, independent of mtimes."""
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*"), key=lambda value: value.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix().encode()
        if path.is_symlink():
            raise ApiCompatError(f"archive extraction tree contains a symbolic link: {path}")
        if path.is_dir():
            digest.update(b"D\0" + relative + b"\0")
        elif path.is_file():
            mode = path.stat().st_mode & 0o777
            digest.update(b"F\0" + relative + b"\0" + f"{mode:o}".encode() + b"\0")
            digest.update(path.read_bytes())
            digest.update(b"\0")
        else:
            raise ApiCompatError(f"archive extraction tree contains an unsupported node: {path}")
    return digest.hexdigest()
