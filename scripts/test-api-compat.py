#!/usr/bin/env python3
"""Adversarial vectors for the Supported Rust API compatibility boundary."""

import copy
import hashlib
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import api_compat  # noqa: E402

PACKAGED_SPEC = importlib.util.spec_from_file_location(
    "packaged_endpoint_consumer", ROOT / "scripts" / "check-packaged-endpoint-consumer.py"
)
assert PACKAGED_SPEC is not None and PACKAGED_SPEC.loader is not None
packaged_endpoint_consumer = importlib.util.module_from_spec(PACKAGED_SPEC)
PACKAGED_SPEC.loader.exec_module(packaged_endpoint_consumer)


class CompatibilityVectors(unittest.TestCase):
    def setUp(self) -> None:
        self.baseline = {
            "items": {
                "sipx_fixture::kept": {
                    "kind": "function",
                    "function": {
                        "inputs": [{"primitive": "u32"}],
                        "output": None,
                    },
                },
                "sipx_fixture::Choice": {
                    "kind": "enum",
                    "attrs": ["non_exhaustive"],
                    "variants": {"First": {"kind": "plain"}},
                },
                "sipx_fixture::Handler": {
                    "kind": "trait",
                    "attrs": [],
                    "items": {
                        "handle": {"kind": "function", "has_default": False}
                    },
                },
                "sipx_fixture::Alias": {
                    "kind": "function",
                    "definition": "sipx_fixture::inner::kept",
                    "function": {
                        "inputs": [{"primitive": "u32"}],
                        "output": None,
                    },
                },
            },
            "experimental_roots": [],
        }

    def candidate(self) -> dict:
        return copy.deepcopy(self.baseline)

    def assert_breaks(self, candidate: dict, fragment: str) -> None:
        problems = api_compat.compatibility_problems(
            "sipx-fixture", self.baseline, candidate
        )
        self.assertTrue(problems)
        self.assertIn(fragment, "\n".join(problems))

    def test_api_1_removed_supported_function_breaks(self) -> None:
        candidate = self.candidate()
        del candidate["items"]["sipx_fixture::kept"]
        self.assert_breaks(candidate, "sipx_fixture::kept")

    def test_api_2_changed_parameter_breaks(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::kept"]["function"]["inputs"][0] = {
            "primitive": "u64"
        }
        self.assert_breaks(candidate, "inputs")

    def test_api_3_new_required_trait_method_breaks(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["items"]["required"] = {
            "kind": "function",
            "has_default": False,
        }
        self.assert_breaks(candidate, "required trait item")

    def test_api_4_loss_of_non_exhaustive_breaks(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Choice"]["attrs"] = []
        self.assert_breaks(candidate, "non_exhaustive")

    def test_api_5_reserved_additions_are_compatible(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::added"] = {"kind": "function"}
        candidate["items"]["sipx_fixture::Choice"]["variants"]["Second"] = {
            "kind": "plain"
        }
        candidate["items"]["sipx_fixture::Handler"]["items"]["helper"] = {
            "kind": "function",
            "has_default": True,
        }
        self.assertEqual(
            api_compat.compatibility_problems(
                "sipx-fixture", self.baseline, candidate
            ),
            [],
        )

    def test_exhaustive_enum_addition_breaks(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Choice"]["attrs"] = []
        self.baseline["items"]["sipx_fixture::Choice"]["attrs"] = []
        candidate["items"]["sipx_fixture::Choice"]["variants"]["Second"] = {
            "kind": "plain"
        }
        self.assert_breaks(candidate, "without a baseline non_exhaustive reservation")

    def test_public_field_addition_without_reservation_breaks(self) -> None:
        self.baseline["items"]["sipx_fixture::Options"] = {
            "kind": "struct",
            "attrs": [],
            "fields": {"first": {"primitive": "u32"}},
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Options"]["fields"]["second"] = {
            "primitive": "u32"
        }
        self.assert_breaks(candidate, "added public field")

    def test_enum_payload_type_change_breaks(self) -> None:
        payload = self.baseline["items"]["sipx_fixture::Choice"]["variants"]["First"]
        payload["kind"] = {"tuple": [{"primitive": "u32"}]}
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Choice"]["variants"]["First"]["kind"] = {
            "tuple": [{"primitive": "u64"}]
        }
        self.assert_breaks(candidate, "variant.kind.tuple[0].primitive")

    def test_new_private_field_breaks_construction(self) -> None:
        self.baseline["items"]["sipx_fixture::Options"] = {
            "kind": "struct",
            "attrs": [],
            "fields": {"first": {"primitive": "u32"}},
            "has_stripped_fields": False,
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Options"]["has_stripped_fields"] = True
        self.assert_breaks(candidate, "added a private or stripped field")

    def test_private_field_direction_is_additive_when_already_reserved(self) -> None:
        for attrs, old_stripped, new_stripped in (
            ([], True, False),
            (["non_exhaustive"], False, True),
        ):
            with self.subTest(attrs=attrs, old=old_stripped, new=new_stripped):
                self.baseline["items"]["sipx_fixture::Options"] = {
                    "kind": "struct",
                    "attrs": attrs,
                    "fields": {},
                    "has_stripped_fields": old_stripped,
                }
                candidate = self.candidate()
                candidate["items"]["sipx_fixture::Options"][
                    "has_stripped_fields"
                ] = new_stripped
                self.assertEqual(
                    api_compat.compatibility_problems(
                        "sipx-fixture", self.baseline, candidate
                    ),
                    [],
                )

    def test_public_field_addition_is_compatible_when_struct_was_already_private(self) -> None:
        self.baseline["items"]["sipx_fixture::Options"] = {
            "kind": "struct",
            "attrs": [],
            "fields": {"first": {"primitive": "u32"}},
            "has_stripped_fields": True,
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Options"]["fields"]["second"] = {
            "primitive": "u64"
        }
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_union_field_addition_is_source_compatible(self) -> None:
        self.baseline["items"]["sipx_fixture::Storage"] = {
            "kind": "union",
            "attrs": [],
            "fields": {"first": {"primitive": "u32"}},
            "has_stripped_fields": False,
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Storage"]["fields"]["second"] = {
            "primitive": "u64"
        }
        candidate["items"]["sipx_fixture::Storage"]["has_stripped_fields"] = True
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_variant_level_non_exhaustive_reserves_additional_fields(self) -> None:
        variant = self.baseline["items"]["sipx_fixture::Choice"]["variants"]["First"]
        variant.update(
            {
                "attrs": ["non_exhaustive"],
                "kind": {
                    "struct": {
                        "fields": {"first": {"primitive": "u32"}},
                        "has_stripped_fields": False,
                    }
                },
                "discriminant": None,
            }
        )
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Choice"]["variants"]["First"]["kind"][
            "struct"
        ]["fields"]["second"] = {"primitive": "u64"}
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_parent_non_exhaustive_does_not_reserve_variant_fields(self) -> None:
        variant = self.baseline["items"]["sipx_fixture::Choice"]["variants"]["First"]
        variant.update(
            {
                "attrs": [],
                "kind": {"tuple": [{"primitive": "u32"}]},
                "discriminant": None,
            }
        )
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Choice"]["variants"]["First"]["kind"][
            "tuple"
        ].append({"primitive": "u64"})
        self.assert_breaks(candidate, "variant.kind.tuple")

    def test_trait_object_safety_loss_breaks_even_with_a_defaulted_addition(self) -> None:
        self.baseline["items"]["sipx_fixture::Handler"]["is_dyn_compatible"] = True
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["is_dyn_compatible"] = False
        candidate["items"]["sipx_fixture::Handler"]["items"]["helper"] = {
            "kind": "function",
            "has_default": True,
        }
        self.assert_breaks(candidate, "is_dyn_compatible")

    def test_gaining_trait_object_safety_is_additive(self) -> None:
        self.baseline["items"]["sipx_fixture::Handler"]["is_dyn_compatible"] = False
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["is_dyn_compatible"] = True
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_defaulted_trait_method_with_existing_impl_is_compatible(self) -> None:
        self.baseline["items"]["sipx_fixture::Handler"]["implementations"] = {
            "fixed-digest": {
                "trait": {"resolved": "sipx_fixture::Handler"},
                "for": {"resolved": "sipx_fixture::Target"},
                "items": {},
            }
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["items"]["helper"] = {
            "kind": "function",
            "has_default": True,
        }
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_adding_a_default_to_existing_trait_item_is_compatible(self) -> None:
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["items"]["handle"][
            "has_default"
        ] = True
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )
        self.baseline = candidate
        removal = self.candidate()
        removal["items"]["sipx_fixture::Handler"]["items"]["handle"][
            "has_default"
        ] = False
        self.assert_breaks(removal, "has_default")

    def test_lost_synthetic_auto_trait_impl_breaks(self) -> None:
        self.baseline["items"]["sipx_fixture::Opaque"] = {
            "kind": "struct",
            "attrs": [],
            "fields": {},
            "implementations": {
                "Send": {"trait": "core::marker::Send", "is_synthetic": True}
            },
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Opaque"]["implementations"] = {}
        self.assert_breaks(candidate, "removed implementation")

    def test_gained_positive_auto_trait_impl_is_additive(self) -> None:
        self.baseline["items"]["sipx_fixture::Opaque"] = {
            "kind": "struct",
            "attrs": [],
            "fields": {},
            "implementations": {},
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Opaque"]["implementations"] = {
            "send": {
                "trait": {"resolved": "core::marker::Send"},
                "is_synthetic": True,
            }
        }
        self.assertEqual(
            api_compat.compatibility_problems("sipx-fixture", self.baseline, candidate),
            [],
        )

    def test_lost_public_blanket_trait_impl_breaks(self) -> None:
        self.baseline["items"]["sipx_fixture::Handler"]["implementations"] = {
            "blanket": {
                "trait": "sipx_fixture::Handler",
                "blanket_impl": {"generic": "T"},
            }
        }
        candidate = self.candidate()
        candidate["items"]["sipx_fixture::Handler"]["implementations"] = {}
        self.assert_breaks(candidate, "removed implementation")

    def test_api_6_removed_reexport_breaks(self) -> None:
        candidate = self.candidate()
        del candidate["items"]["sipx_fixture::Alias"]
        self.assert_breaks(candidate, "sipx_fixture::Alias")

    def test_api_7_supported_item_cannot_be_hidden_as_experimental(self) -> None:
        candidate = self.candidate()
        candidate["all_items"] = copy.deepcopy(candidate["items"])
        candidate["experimental_roots"] = ["sipx_fixture::kept"]
        del candidate["items"]["sipx_fixture::kept"]
        self.assert_breaks(candidate, "newly classified Experimental")


class PackageBoundaryVectors(unittest.TestCase):
    def test_api_8_live_workspace_patch_is_refused(self) -> None:
        archive_root = ROOT / "target" / "api-compat" / "archives"
        live = ROOT / "crates" / "sipx-call"
        problem = api_compat.archive_patch_problem(ROOT, archive_root, live)
        self.assertIsNotNone(problem)
        self.assertIn("packaged archive", problem or "")

    def test_api_8_extracted_archive_is_accepted(self) -> None:
        archive_root = ROOT / "target" / "api-compat" / "archives"
        extracted = archive_root / "sipx-call-1.0.0"
        self.assertIsNone(api_compat.archive_patch_problem(ROOT, archive_root, extracted))

    def test_api_8_live_mutation_cannot_change_extracted_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = pathlib.Path(directory) / "workspace"
            live = workspace / "crates" / "sipx-call" / "src" / "lib.rs"
            extracted = workspace / "target" / "archives" / "sipx-call-1.0.0"
            archived = extracted / "src" / "lib.rs"
            live.parent.mkdir(parents=True)
            archived.parent.mkdir(parents=True)
            live.write_text("pub fn frozen() {}\n")
            archived.write_text(live.read_text())
            before = hashlib.sha256(archived.read_bytes()).hexdigest()
            live.write_text("pub fn changed_after_packaging() {}\n")
            self.assertEqual(hashlib.sha256(archived.read_bytes()).hexdigest(), before)
            self.assertIsNotNone(
                api_compat.archive_patch_problem(workspace, extracted.parent, live.parent.parent)
            )
            self.assertIsNone(
                api_compat.archive_patch_problem(workspace, extracted.parent, extracted)
            )

    def test_consumer_metadata_must_resolve_every_node_to_its_archive(self) -> None:
        archive = ROOT / "target" / "api-compat" / "archives" / "sipx-call-1.0.0"
        metadata = {
            "resolve": {"root": "fixture 0.1.0"},
            "packages": [
                {
                    "id": "fixture 0.1.0",
                    "name": "sipx-v1-endpoint-consumer",
                    "version": "0.1.0",
                    "source": None,
                    "manifest_path": str(ROOT / "fixture" / "Cargo.toml"),
                },
                {
                    "id": "sipx-call 1.0.0",
                    "name": "sipx-call",
                    "version": "1.0.0",
                    "source": None,
                    "manifest_path": str(archive / "Cargo.toml"),
                }
            ]
        }
        self.assertEqual(
            api_compat.archive_resolution_problems(
                metadata, {"sipx-call": archive}, "1.0.0"
            ),
            [],
        )
        metadata["packages"][1]["source"] = "registry+https://example.invalid/index"
        self.assertIn(
            "not an archive path",
            "\n".join(
                api_compat.archive_resolution_problems(
                    metadata, {"sipx-call": archive}, "1.0.0"
                )
            ),
        )

    def test_consumer_metadata_rejects_wrong_version_and_extra_sipx_node(self) -> None:
        archive = ROOT / "target" / "api-compat" / "archives" / "sipx-call-1.0.0"
        metadata = {
            "resolve": {"root": "fixture 0.1.0"},
            "packages": [
                {
                    "id": "fixture 0.1.0",
                    "name": "sipx-v1-endpoint-consumer",
                    "version": "0.1.0",
                },
                {
                    "id": "sipx-call 0.9.0",
                    "name": "sipx-call",
                    "version": "0.9.0",
                    "source": None,
                    "manifest_path": str(archive / "Cargo.toml"),
                },
                {
                    "id": "sipx-unexpected 1.0.0",
                    "name": "sipx-unexpected",
                    "version": "1.0.0",
                    "source": None,
                    "manifest_path": str(archive / "Cargo.toml"),
                },
            ],
        }
        problems = api_compat.archive_resolution_problems(
            metadata, {"sipx-call": archive}, "1.0.0"
        )
        joined = "\n".join(problems)
        self.assertIn("unexpected sipx node `sipx-call` 0.9.0", joined)
        self.assertIn("unexpected sipx node `sipx-unexpected` 1.0.0", joined)
        self.assertIn("contains 0 `sipx-call` 1.0.0 nodes", joined)

    def test_tree_digest_changes_when_extracted_source_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / "src" / "lib.rs"
            source.parent.mkdir()
            source.write_text("pub fn frozen() {}\n")
            before = api_compat.tree_digest(root)
            source.write_text("pub fn changed() {}\n")
            self.assertNotEqual(api_compat.tree_digest(root), before)


class RustdocCanonicalVectors(unittest.TestCase):
    @staticmethod
    def item(item_id: int, name: str | None, visibility: str, inner: dict) -> dict:
        return {
            "id": item_id,
            "crate_id": 0,
            "name": name,
            "visibility": visibility,
            "attrs": [],
            "inner": inner,
        }

    @staticmethod
    def function() -> dict:
        return {
            "function": {
                "sig": {
                    "inputs": [["value", {"primitive": "u32"}]],
                    "output": None,
                    "is_c_variadic": False,
                },
                "generics": {"params": [], "where_predicates": []},
                "header": {
                    "is_const": False,
                    "is_unsafe": False,
                    "is_async": False,
                    "abi": "Rust",
                },
                "has_body": True,
            }
        }

    def test_reachable_paths_and_reexports_are_canonical(self) -> None:
        document = {
            "root": 1,
            "index": {
                "1": self.item(
                    1,
                    "sipx_fixture",
                    "public",
                    {"module": {"is_crate": True, "items": [2, 3, 6]}},
                ),
                "2": self.item(2, "visible", "public", self.function()),
                "3": self.item(
                    3,
                    "private",
                    "default",
                    {"module": {"is_crate": False, "items": [5]}},
                ),
                "5": self.item(5, "hidden", "public", self.function()),
                "6": self.item(
                    6,
                    None,
                    "public",
                    {
                        "use": {
                            "source": "private::hidden",
                            "name": "Alias",
                            "id": 5,
                            "is_glob": False,
                        }
                    },
                ),
            },
            "paths": {
                "1": {"crate_id": 0, "path": ["sipx_fixture"], "kind": "module"},
                "2": {
                    "crate_id": 0,
                    "path": ["sipx_fixture", "visible"],
                    "kind": "function",
                },
                "3": {
                    "crate_id": 0,
                    "path": ["sipx_fixture", "private"],
                    "kind": "module",
                },
                "5": {
                    "crate_id": 0,
                    "path": ["sipx_fixture", "private", "hidden"],
                    "kind": "function",
                },
            },
        }
        items = api_compat.RustdocApi(document).public_items()
        self.assertIn("sipx_fixture::visible", items)
        self.assertIn("sipx_fixture::Alias", items)
        self.assertNotIn("sipx_fixture::private::hidden", items)
        self.assertEqual(
            items["sipx_fixture::Alias"]["definition"],
            "sipx_fixture::private::hidden",
        )
        self.assertNotIn('"id"', json.dumps(items))

    def test_type_impls_drop_projections_but_trait_impls_retain_blankets(self) -> None:
        implementation = {
            "is_unsafe": False,
            "generics": {"params": [], "where_predicates": []},
            "provided_trait_methods": [],
            "trait": {"resolved_path": {"path": "Send", "id": 2, "args": None}},
            "for": {"resolved_path": {"path": "Opaque", "id": 3, "args": None}},
            "items": [],
            "is_negative": False,
            "is_synthetic": True,
            "blanket_impl": {"generic": "T"},
        }
        document = {
            "root": 3,
            "index": {
                "1": self.item(1, None, "default", {"impl": implementation}),
                "3": self.item(3, "Opaque", "public", {"struct": {"kind": "unit", "generics": {}, "impls": [1]}}),
            },
            "paths": {
                "2": {"crate_id": 1, "path": ["core", "marker", "Send"], "kind": "trait"},
                "3": {"crate_id": 0, "path": ["sipx_fixture", "Opaque"], "kind": "struct"},
            },
        }
        projected = copy.deepcopy(document["index"]["1"])
        projected["id"] = 4
        projected["inner"]["impl"]["is_synthetic"] = False
        document["index"]["4"] = projected
        direct = copy.deepcopy(document["index"]["1"])
        direct["id"] = 5
        direct["inner"]["impl"]["is_synthetic"] = False
        direct["inner"]["impl"]["blanket_impl"] = None
        document["index"]["5"] = direct
        negative = copy.deepcopy(document["index"]["1"])
        negative["id"] = 6
        negative["inner"]["impl"]["is_negative"] = True
        document["index"]["6"] = negative
        internal = copy.deepcopy(document["index"]["1"])
        internal["id"] = 7
        internal["inner"]["impl"]["trait"] = {
            "resolved_path": {"path": "Freeze", "id": 8, "args": None}
        }
        document["index"]["7"] = internal
        document["paths"]["8"] = {
            "crate_id": 1,
            "path": ["core", "marker", "Freeze"],
            "kind": "trait",
        }
        api = api_compat.RustdocApi(document)
        type_impls = api.implementations([1, 4, 5, 6, 7], owner="type")
        trait_impls = api.implementations([4], owner="trait")
        self.assertEqual(len(type_impls), 2)
        self.assertEqual(len(trait_impls), 1)
        self.assertTrue(any(record["is_synthetic"] for record in type_impls.values()))
        self.assertTrue(any(record["blanket_impl"] is None for record in type_impls.values()))
        self.assertEqual(next(iter(trait_impls.values()))["blanket_impl"], {"generic": "T"})
        self.assertTrue(all(len(key) == 64 and "{" not in key for key in type_impls))

    def test_defaulted_trait_method_does_not_change_existing_impl_record(self) -> None:
        implementation = {
            "is_unsafe": False,
            "generics": {"params": [], "where_predicates": []},
            "provided_trait_methods": [],
            "trait": {"resolved_path": {"path": "Handler", "id": 2, "args": None}},
            "for": {"resolved_path": {"path": "Target", "id": 3, "args": None}},
            "items": [6],
            "is_negative": False,
            "is_synthetic": False,
            "blanket_impl": None,
        }
        document = {
            "root": 3,
            "index": {
                "1": self.item(1, None, "default", {"impl": implementation}),
                "3": self.item(3, "Target", "public", {"struct": {"kind": "unit", "generics": {}, "impls": [1]}}),
                "6": self.item(6, "handle", "default", self.function()),
            },
            "paths": {
                "2": {"crate_id": 0, "path": ["sipx_fixture", "Handler"], "kind": "trait"},
                "3": {"crate_id": 0, "path": ["sipx_fixture", "Target"], "kind": "struct"},
            },
        }
        before = api_compat.RustdocApi(document).implementation(1)
        document["index"]["1"]["inner"]["impl"]["provided_trait_methods"] = ["helper"]
        document["index"]["7"] = self.item(7, "helper", "default", self.function())
        document["index"]["1"]["inner"]["impl"]["items"].append(7)
        after = api_compat.RustdocApi(document).implementation(1)
        self.assertEqual(before, after)

    def test_enum_payload_fields_are_kept_despite_default_visibility(self) -> None:
        document = {
            "root": 1,
            "index": {
                "1": self.item(1, "sipx_fixture", "public", {"module": {"is_crate": True, "items": []}}),
                "2": self.item(2, "Tuple", "public", {"variant": {"kind": {"tuple": [3]}, "discriminant": None}}),
                "3": self.item(3, None, "default", {"struct_field": {"primitive": "u32"}}),
                "4": self.item(4, "Named", "public", {"variant": {"kind": {"struct": {"fields": [5], "has_stripped_fields": False}}, "discriminant": None}}),
                "5": self.item(5, "value", "default", {"struct_field": {"primitive": "u64"}}),
            },
            "paths": {"1": {"crate_id": 0, "path": ["sipx_fixture"], "kind": "module"}},
        }
        api = api_compat.RustdocApi(document)
        self.assertEqual(api.variant(2)[1]["kind"], {"tuple": [{"primitive": "u32"}]})
        self.assertEqual(
            api.variant(4)[1]["kind"],
            {"struct": {"fields": {"value": {"primitive": "u64"}}, "has_stripped_fields": False}},
        )

    def test_plain_struct_records_stripped_private_fields(self) -> None:
        document = {
            "root": 1,
            "index": {
                "1": self.item(1, "Options", "public", {"struct": {"kind": {"plain": {"fields": [], "has_stripped_fields": True}}, "generics": {}, "impls": []}}),
            },
            "paths": {"1": {"crate_id": 0, "path": ["sipx_fixture", "Options"], "kind": "struct"}},
        }
        self.assertTrue(api_compat.RustdocApi(document).item(1)["has_stripped_fields"])


class ClassificationEdgeVectors(unittest.TestCase):
    def test_cross_package_reexport_inherits_experimental_classification(self) -> None:
        items = {
            "sipx_front::Alias": {
                "kind": "function",
                "definition": "sipx_backend::experimental::function",
            },
            "sipx_front::kept": {"kind": "function"},
        }
        supported = api_compat.filter_supported_items(
            items, {"sipx_backend": ["sipx_backend::experimental"]}
        )
        self.assertNotIn("sipx_front::Alias", supported)
        self.assertIn("sipx_front::kept", supported)

    def test_impl_referring_to_experimental_api_is_pruned(self) -> None:
        items = {
            "sipx_front::Kept": {
                "kind": "struct",
                "implementations": {
                    "drop": {
                        "trait": {"resolved": "sipx_backend::experimental::Trait"},
                        "for": {"resolved": "sipx_front::Kept"},
                        "generics": {},
                    },
                    "keep": {
                        "trait": {"resolved": "sipx_backend::Stable"},
                        "for": {"resolved": "sipx_front::Kept"},
                        "generics": {},
                    },
                },
            }
        }
        supported = api_compat.filter_supported_items(
            items, {"sipx_backend": ["sipx_backend::experimental"]}
        )
        self.assertEqual(set(supported["sipx_front::Kept"]["implementations"]), {"keep"})

    def test_experimental_enum_variant_is_pruned_through_a_reexport(self) -> None:
        items = {
            "sipx_front::Error": {
                "kind": "enum",
                "definition": "sipx_front::error::Error",
                "variants": {"Stable": {"kind": "plain"}, "Draft": {"kind": "plain"}},
            }
        }
        supported = api_compat.filter_supported_items(
            items, {"sipx_front": ["sipx_front::error::Error::Draft"]}
        )
        self.assertEqual(set(supported["sipx_front::Error"]["variants"]), {"Stable"})
        known = api_compat.known_classification_paths(items)
        self.assertIn("sipx_front::Error::Draft", known)
        self.assertIn("sipx_front::error::Error::Draft", known)

    def test_supported_signature_cannot_expose_experimental_api(self) -> None:
        items = {
            "sipx_front::bad": {
                "kind": "function",
                "function": {"output": {"resolved": "sipx_backend::experimental::Type"}},
            }
        }
        with self.assertRaisesRegex(api_compat.ApiCompatError, "exposes Experimental API"):
            api_compat.filter_supported_items(
                items, {"sipx_backend": ["sipx_backend::experimental"]}
            )

    def test_supported_record_cannot_freeze_an_unresolved_identity(self) -> None:
        items = {
            "sipx_front::bad": {
                "kind": "function",
                "function": {"output": {"resolved": "unresolved::struct::SameName"}},
            }
        }
        with self.assertRaisesRegex(api_compat.ApiCompatError, "unresolved rustdoc identity"):
            api_compat.filter_supported_items(items, {"sipx_front": []})


class BaselineVersionVectors(unittest.TestCase):
    def document(self, version: str) -> dict:
        return {
            "schema": api_compat.SCHEMA,
            "workspace_version": version,
            "rustdoc": {"toolchain": "fixed"},
            "packages": {},
        }

    def test_stable_v1_bump_keeps_the_release_candidate_baseline(self) -> None:
        self.assertEqual(
            api_compat.document_problems(
                self.document("1.0.0-rc.23"), self.document("1.0.0")
            ),
            [],
        )

    def test_next_major_requires_a_new_baseline(self) -> None:
        problems = api_compat.document_problems(
            self.document("1.0.0"), self.document("2.0.0-rc.1")
        )
        self.assertIn("next-major baseline", "\n".join(problems))

    def test_v1_baseline_writer_refuses_a_future_major(self) -> None:
        with self.assertRaisesRegex(api_compat.ApiCompatError, "future major"):
            api_compat.baseline_document(self.document("2.0.0-rc.1"))

    def test_existing_v1_baseline_cannot_be_overwritten_to_bless_a_removal(self) -> None:
        existing = self.document("1.0.0-rc.23")
        existing["packages"] = {
            "sipx-fixture": {
                "library": True,
                "items": {"sipx_fixture::kept": {"kind": "function"}},
            }
        }
        candidate = copy.deepcopy(existing)
        candidate["packages"]["sipx-fixture"]["items"] = {}
        problems = api_compat.baseline_update_problems(existing, candidate)
        self.assertIn("removed Supported path", "\n".join(problems))

    def test_additive_roll_forward_guards_the_addition_afterward(self) -> None:
        existing = self.document("1.0.0-rc.23")
        existing["packages"] = {
            "sipx-fixture": {"library": True, "items": {}}
        }
        additive = copy.deepcopy(existing)
        additive["packages"]["sipx-fixture"]["items"]["sipx_fixture::added"] = {
            "kind": "function"
        }
        self.assertEqual(api_compat.baseline_update_problems(existing, additive), [])
        later = copy.deepcopy(existing)
        self.assertIn(
            "removed Supported path",
            "\n".join(api_compat.document_problems(additive, later)),
        )

    def test_additive_package_can_roll_forward_and_is_then_guarded(self) -> None:
        existing = self.document("1.0.0-rc.23")
        additive = copy.deepcopy(existing)
        additive["packages"]["sipx-new"] = {"library": True, "items": {}}
        self.assertIn(
            "missing from the baseline",
            "\n".join(api_compat.document_problems(existing, additive)),
        )
        self.assertEqual(api_compat.baseline_update_problems(existing, additive), [])
        self.assertIn(
            "publishable package `sipx-new` is missing",
            "\n".join(api_compat.document_problems(additive, existing)),
        )

    def test_v2_documents_cannot_pass_the_v1_selector(self) -> None:
        problems = api_compat.document_problems(
            self.document("2.0.0-rc.1"), self.document("2.0.0")
        )
        self.assertIn("v1 baseline selector", "\n".join(problems))


class PackagedConsumerLockVectors(unittest.TestCase):
    def test_offline_rehearsal_uses_the_release_candidates_committed_lock(self) -> None:
        with tempfile.TemporaryDirectory(prefix="sipx-package-lock-") as temporary:
            root = pathlib.Path(temporary)
            consumer = root / "consumer"
            consumer.mkdir()
            workspace_lock = root / "Cargo.lock"
            workspace_lock.write_text("release candidate graph\n", encoding="utf-8")
            (consumer / "Cargo.lock").write_text("fresh resolver graph\n", encoding="utf-8")

            packaged_endpoint_consumer.seed_consumer_lockfile(consumer, workspace_lock)

            self.assertEqual(
                "release candidate graph\n",
                (consumer / "Cargo.lock").read_text(encoding="utf-8"),
            )

    def test_pruned_consumer_lock_cannot_introduce_a_new_registry_identity(self) -> None:
        with tempfile.TemporaryDirectory(prefix="sipx-package-lock-") as temporary:
            root = pathlib.Path(temporary)
            workspace_lock = root / "workspace.lock"
            consumer_lock = root / "consumer.lock"
            workspace_lock.write_text(
                'version = 4\n\n[[package]]\nname = "dependency"\nversion = "1.2.3"\n'
                'source = "registry+https://example.com/index"\nchecksum = "released"\n',
                encoding="utf-8",
            )
            consumer_lock.write_text(workspace_lock.read_text(encoding="utf-8"), encoding="utf-8")
            self.assertIsNone(
                packaged_endpoint_consumer.lockfile_resolution_problem(
                    workspace_lock, consumer_lock
                )
            )

            consumer_lock.write_text(
                workspace_lock.read_text(encoding="utf-8").replace("1.2.3", "1.2.4"),
                encoding="utf-8",
            )
            self.assertIn(
                "outside the workspace lock",
                packaged_endpoint_consumer.lockfile_resolution_problem(
                    workspace_lock, consumer_lock
                )
                or "",
            )


class CiToolchainVectors(unittest.TestCase):
    def workflow(
        self, installer: str, *, toolchain: str | None = None, separate: bool = False
    ) -> str:
        first = "" if separate else "\n      - run: ./scripts/check-packaged-endpoint-consumer.py --check"
        second = (
            "\n  package:\n    steps:\n      - run: ./scripts/check-packaged-endpoint-consumer.py --check"
            if separate
            else ""
        )
        selection = (
            f"        with:\n          toolchain: {toolchain}\n"
            if toolchain is not None
            else ""
        )
        return (
            "jobs:\n  test:\n    steps:\n"
            f"      - uses: dtolnay/rust-toolchain@{installer}\n"
            f"{selection}"
            "      - run: ./scripts/check-api-compat.py --check"
            f"{first}{second}\n"
        )

    def test_pin_must_be_in_the_job_running_both_checks(self) -> None:
        self.assertIsNone(
            api_compat.ci_toolchain_problem(
                self.workflow("nightly", toolchain="nightly-2026-07-28"),
                "nightly-2026-07-28",
            )
        )
        self.assertIn(
            "run together",
            api_compat.ci_toolchain_problem(
                self.workflow(
                    "nightly", toolchain="nightly-2026-07-28", separate=True
                ),
                "nightly-2026-07-28",
            )
            or "",
        )

    def test_rolling_nightly_cannot_substitute_for_exact_pin(self) -> None:
        self.assertIn(
            "does not install exact toolchain",
            api_compat.ci_toolchain_problem(
                self.workflow("nightly"), "nightly-2026-07-28"
            )
            or "",
        )

    def test_commented_exact_pin_cannot_mask_a_rolling_nightly(self) -> None:
        workflow = self.workflow("nightly").replace(
            "      - uses: dtolnay/rust-toolchain@nightly\n",
            "      - uses: dtolnay/rust-toolchain@nightly\n"
            "      # toolchain: nightly-2026-07-28\n",
        )
        self.assertIn(
            "does not install exact toolchain",
            api_compat.ci_toolchain_problem(workflow, "nightly-2026-07-28") or "",
        )


if __name__ == "__main__":
    unittest.main()
