#!/usr/bin/env python3
"""Independent semantic mutations of each consumed Rust contract surface."""

import importlib.util
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "generator", ROOT / "scripts/generate-browser.py"
)
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)


class Generation(unittest.TestCase):
    def mutation(self, old, new, section):
        with tempfile.TemporaryDirectory(prefix="sipx-generation-") as directory:
            tree = pathlib.Path(directory)
            for relative in [
                "crates/sipx-wasm/src/contract.rs",
                "crates/sipx-wasm/src/abi.rs",
                *generator.generate(ROOT),
            ]:
                target = tree / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, target)
            source = tree / "crates/sipx-wasm/src/contract.rs"
            text = source.read_text()
            self.assertIn(old, text)
            source.write_text(text.replace(old, new, 1))
            before = json.loads(
                generator.generate(ROOT)["browser/generated/contract.json"]
            )
            after = json.loads(
                generator.generate(tree)["browser/generated/contract.json"]
            )
            self.assertNotEqual(
                before[section],
                after[section],
                section
                + " mutation must affect generated semantics, not only the source hash",
            )
            result = subprocess.run(
                [
                    "python3",
                    str(ROOT / "scripts/generate-browser.py"),
                    "--root",
                    str(tree),
                    "--check",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("browser generation drift:", result.stderr)

    def test_abi_signature(self):
        self.mutation(
            "fn sipx_teardown_timer_id(index: u32)",
            "fn sipx_teardown_timer_id(index: u64)",
            "abi",
        )

    def test_command_field(self):
        self.mutation(
            "expires: u32 => expires_value", "expires: u64 => expires_value", "commands"
        )

    def test_event_field(self):
        self.mutation("min: u64 = *min", "min: u32 = *min", "events")

    def test_error_discriminant(self):
        self.mutation("State = -6", "State = -60", "errors")


if __name__ == "__main__":
    unittest.main(verbosity=2)
