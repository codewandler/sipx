#!/usr/bin/env python3
"""Tests for `check-cfg-callers.py`, on trees whose right answer is written down here.

That checker reads Rust with a text scanner in order to answer a question no local build can:
which `cfg`-gated item is dead on a platform this host cannot compile. Two things follow, and both
shape this suite.

**Its silence is indistinguishable from a clean tree.** A reader that stops recognising a shape
reports nothing, and nothing is what a green run looks like. So every rule has a fixture that must
be *reported*, and the ones that must stay quiet are built as its siblings — same tree, one
predicate changed — rather than as separate happy paths. `TheThingsThatAreNotItems` is that
principle applied to the reader's own regressions: each of those three fixtures made the checker
silent about the real defect while it was being written, and each is a shape this workspace
actually contains.

**A false report is worse than a missed one here**, because it asks an author to write a `cfg` that
is wrong. So the quiet half is asserted at least as hard as the loud half, and the two directions
of the platform rule — an item broader than its callers, an item narrower than its callers — are
tested as a pair.

Everything runs the real script over a fabricated `crates/` tree. The one exception is
`TheRepositoryItself`, which runs it over this repository: a checker that only ever meets its own
fixtures is the same mistake one level up.
"""

import importlib.util
import pathlib
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-cfg-callers.py"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

#: The command the story `X-125` measured as reachable from a Linux gate host. Written once, and
#: asserted to be what both the gate step and `ci.yml` run — a target that drifted between the two
#: would be a cross check of something CI never checks.
CROSS_TARGET = "x86_64-pc-windows-gnu"

_gate = None


def gate():
    """Import `gate.py` lazily, so a fixture test still reports its own failure without it."""
    global _gate
    if _gate is None:
        spec = importlib.util.spec_from_file_location("gate", ROOT / "scripts" / "gate.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _gate = module
    return _gate


class Fixture(unittest.TestCase):
    """A throwaway `crates/` tree, and the real checker run over it."""

    def setUp(self):
        self.root = pathlib.Path(tempfile.mkdtemp(prefix="sipx-cfg-callers-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)

    def write(self, where: str, body: str) -> pathlib.Path:
        path = self.root / where
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(body).lstrip(), encoding="utf-8")
        return path

    def run_check(self, *arguments: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--check", *arguments],
            capture_output=True,
            text=True,
            cwd=self.root,
        )

    def report(self) -> subprocess.CompletedProcess:
        return self.run_check("--root", str(self.root))

    def assertReported(self, name: str) -> str:
        """The tree must be refused, and the refusal must name the item."""
        done = self.report()
        output = done.stdout + done.stderr
        self.assertEqual(1, done.returncode, f"`{name}` was accepted:\n{output}")
        self.assertIn(name, output, f"the report does not name `{name}`:\n{output}")
        return output

    def assertQuiet(self) -> str:
        done = self.report()
        output = done.stdout + done.stderr
        self.assertEqual(0, done.returncode, f"a correct tree was refused:\n{output}")
        return output


class TheShapeThatWentRed(Fixture):
    """`X-125`'s instance, at its own paths, with its own `cfg`s.

    `versioned_bytes` was gated on the feature; its only caller on the feature *and*
    `target_os = "linux"`. Every push for a day left `device audio compiles (macos-15)` and
    `(windows-2025)` red while the local gate was green, because on Linux the caller compiles and
    the helper is used. The two fixtures below are that file before and after the one-line fix.
    """

    #: The caller, at the path and under the `cfg` it really carries. The helper's gate is the only
    #: thing the two fixtures differ in.
    CALLER = """
        mod support;

        #[cfg(all(feature = "device-audio", target_os = "linux"))]
        #[tokio::test]
        async fn dph_12_wav_and_virtual_device_carry_the_same_clip() {
            let listing = support::strict_json::versioned_bytes("device", &out.stdout);
            assert_eq!(listing["devices"][0]["input"], true);
        }
        """

    def helper(self, gate_line: str) -> None:
        self.write("crates/sipx-cli/tests/cli.rs", self.CALLER)
        self.write("crates/sipx-cli/tests/support/mod.rs", "pub(crate) mod strict_json;\n")
        self.write(
            "crates/sipx-cli/tests/support/strict_json.rs",
            f"""
            pub(crate) fn versioned(producer: &str, text: &str) -> String {{
                format!("{{producer}}:{{text}}")
            }}

            {gate_line}
            pub(crate) fn versioned_bytes(producer: &str, bytes: &[u8]) -> String {{
                versioned(producer, std::str::from_utf8(bytes).expect("JSON result is UTF-8"))
            }}
            """,
        )

    def test_the_helper_that_lost_its_callers_platform_is_reported(self):
        self.helper('#[cfg(feature = "device-audio")]')
        output = self.assertReported("versioned_bytes")
        self.assertIn(
            'target_os = "linux"',
            output,
            "the report does not name the configuration the item outlives its caller in, which is "
            "the only part of it an author can act on",
        )
        self.assertIn("crates/sipx-cli/tests/cli.rs", output, "the caller is not pointed at")

    def test_the_one_line_fix_makes_it_quiet(self):
        """The remedy has to cost exactly what the real fix cost: the caller's own `cfg`."""
        self.helper('#[cfg(all(feature = "device-audio", target_os = "linux"))]')
        self.assertQuiet()


class TheRule(Fixture):
    """What "broader than its callers" means, in both directions and through `any` and `not`."""

    def unit(self, item_gate: str, caller_gate: str, *, extra: str = "") -> None:
        self.write(
            "crates/sipx-media/src/lib.rs",
            f"""
            {item_gate}
            fn helper() -> u8 {{
                7
            }}

            {caller_gate}
            pub fn caller() -> u8 {{
                helper()
            }}
            {extra}
            """,
        )

    def test_an_item_broader_than_its_only_caller_is_reported(self):
        self.unit('#[cfg(unix)]', '#[cfg(all(unix, feature = "opus"))]')
        self.assertReported("helper")

    def test_an_item_narrower_than_its_callers_is_not(self):
        """The mirror image, and the one a careless rule would report.

        `target_os = "linux"` on the item and `unix` on the caller is an item that exists in
        *fewer* configurations than its callers, which is not dead code anywhere.
        """
        self.unit('#[cfg(target_os = "linux")]', "#[cfg(unix)]")
        self.assertQuiet()

    def test_an_item_on_a_whole_family_used_only_on_one_of_its_platforms_is_reported(self):
        """The other direction of the same implication, and it is a finding rather than a silence.

        `unix` on the item and `target_os = "linux"` on the caller is *broader*: the item compiles
        on macOS and the caller does not. `target_os = "linux"` implies `unix`, so the comparison
        has to run that way round and only that way round.
        """
        self.unit("#[cfg(unix)]", '#[cfg(target_os = "linux")]')
        self.assertReported("helper")

    def test_a_feature_a_caller_requires_and_the_item_does_not_is_reported(self):
        self.unit('#[cfg(feature = "opus")]', '#[cfg(all(feature = "opus", feature = "dtls"))]')
        output = self.assertReported("helper")
        self.assertIn('feature = "dtls"', output)

    def test_any_contributes_only_what_all_its_branches_agree_on(self):
        """`any(all(unix, f), all(windows, f))` requires `f`, so an item gated `f` matches it."""
        self.unit(
            '#[cfg(feature = "opus")]',
            '#[cfg(any(all(unix, feature = "opus"), all(windows, feature = "opus")))]',
        )
        self.assertQuiet()

    def test_an_item_gated_any_of_two_platforms_used_on_one_is_reported(self):
        self.unit(
            '#[cfg(any(target_os = "linux", target_os = "macos"))]',
            '#[cfg(target_os = "linux")]',
        )
        self.assertReported("helper")

    def test_a_negated_atom_is_an_atom(self):
        self.unit("#[cfg(unix)]", '#[cfg(all(unix, not(feature = "opus")))]')
        output = self.assertReported("helper")
        self.assertIn('not(feature = "opus")', output)

    def test_two_platform_variants_of_one_name_are_not_reported(self):
        """The ordinary idiom: one `helper` per platform, each used by the same caller.

        Both declarations share a name, so the callers of one are the callers of the other, and
        neither is broader than that set. A rule that reported this would fire on nearly every
        portable module in the workspace.
        """
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(unix)]
            fn helper() -> u8 {
                7
            }

            #[cfg(windows)]
            fn helper() -> u8 {
                8
            }

            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        self.assertQuiet()


class TheThingsThatAreNotItems(Fixture):
    """Three shapes that made this checker silent while it was being written.

    Every one of them is in the real workspace, and every one of them turned the reader's idea of
    "which lines this `cfg` covers" into something else. They are here because a scanner's
    regressions are invisible: it keeps exiting 0.
    """

    def test_a_cfg_on_a_function_parameter_does_not_gate_the_body(self):
        """`sipx-transport`'s `dial_ws` — its `wss` parameter, read as an item, gated the body.

        Every call inside then looked like it required `wss`, so `dial_ws` was reported as
        outliving a caller that requires exactly what it does. A false report, on correct code.
        """
        self.write(
            "crates/sipx-transport/src/tcp.rs",
            """
            #[cfg(feature = "ws")]
            fn dial_ws(
                key: u8,
                #[cfg(feature = "wss")] client: Option<u8>,
            ) -> u8 {
                key
            }

            #[cfg(feature = "ws")]
            pub fn send_ws(
                key: u8,
                #[cfg(feature = "wss")] client: Option<u8>,
            ) -> u8 {
                dial_ws(
                    key,
                    #[cfg(feature = "wss")]
                    client,
                )
            }
            """,
        )
        self.assertQuiet()

    def test_a_multi_line_attribute_between_the_cfg_and_the_item_is_stepped_over(self):
        """`cli.rs` writes `#[allow(\\n …,\\n reason = "…"\\n)]` between the `cfg` and its test.

        A reader that skipped only the attribute's first line took its second for the subject, gave
        the `cfg` a four-line extent, and left the whole test body — including the call this
        checker exists to see — outside every gate around it.
        """
        self.write(
            "crates/sipx-cli/tests/cli.rs",
            """
            mod support;

            #[cfg(all(feature = "device-audio", target_os = "linux"))]
            #[tokio::test]
            #[allow(
                clippy::too_many_lines,
                reason = "the two complete process calls stay together"
            )]
            async fn dph_12_wav_and_virtual_device_carry_the_same_clip() {
                support::helper();
            }
            """,
        )
        self.write(
            "crates/sipx-cli/tests/support/mod.rs",
            """
            #[cfg(feature = "device-audio")]
            pub(crate) fn helper() {}
            """,
        )
        self.assertReported("helper")

    def test_a_brace_inside_a_string_literal_does_not_close_the_block(self):
        """`cli.rs` builds an ALSA configuration with `format!`, and that string contains `}}`.

        Counted as structure, those braces closed the enclosing test hundreds of lines early: the
        `cfg` around it stopped applying, and its call to the helper read as requiring nothing.
        """
        self.write(
            "crates/sipx-cli/tests/cli.rs",
            """
            mod support;

            #[cfg(all(feature = "device-audio", target_os = "linux"))]
            #[tokio::test]
            async fn dph_12_wav_and_virtual_device_carry_the_same_clip() {
                let alsa = format!(
                    "pcm.sipx_dph12 {{\\n  type file\\n  slave.pcm \\"null\\"\\n  file \\"{}\\"\\n}}\\n",
                    sink.display(),
                );
                support::helper(&alsa);
            }
            """,
        )
        self.write(
            "crates/sipx-cli/tests/support/mod.rs",
            """
            #[cfg(feature = "device-audio")]
            pub(crate) fn helper(_alsa: &str) {}
            """,
        )
        self.assertReported("helper")

    def test_a_cfg_on_a_struct_field_is_not_a_cfg_on_the_next_item(self):
        self.write(
            "crates/sipx-transport/src/pool.rs",
            """
            struct Reply {
                key: u8,
                #[cfg(feature = "quic")]
                quic_reply: Option<u8>,
            }

            #[cfg(feature = "ws")]
            fn helper() -> u8 {
                0
            }

            #[cfg(feature = "ws")]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        self.assertQuiet()


class TheBoundaries(Fixture):
    """Where the rule deliberately stops, asserted so a later widening is a decision.

    Each of these is a *silence*, and a silence nobody wrote down is what this repository has
    shipped three times. The checker states them in its own output; these tests hold it to them.
    """

    def test_a_pub_item_in_a_library_src_is_left_alone(self):
        """It is reachable from outside the crate, so it is not dead code in any configuration."""
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(feature = "opus")]
            pub fn helper() -> u8 {
                7
            }

            #[cfg(all(feature = "opus", feature = "dtls"))]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        self.assertQuiet()

    def test_a_pub_item_under_a_test_root_is_examined_like_any_other(self):
        """A test binary exports to nobody, so `pub` buys an item nothing there. `X-125`'s
        instance lived in exactly such a module."""
        self.write(
            "crates/sipx-media/tests/support/mod.rs",
            """
            #[cfg(feature = "opus")]
            pub fn helper() -> u8 {
                7
            }
            """,
        )
        self.write(
            "crates/sipx-media/tests/codec.rs",
            """
            mod support;

            #[cfg(all(feature = "opus", target_os = "linux"))]
            #[test]
            fn uses_it() {
                assert_eq!(support::helper(), 7);
            }
            """,
        )
        self.assertReported("helper")

    def test_an_item_nothing_uses_is_left_to_clippy(self):
        """Plain dead code on this host too. `-D warnings` refuses it before the gate gets here."""
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(feature = "opus")]
            fn helper() -> u8 {
                7
            }
            """,
        )
        self.assertQuiet()

    def test_a_use_in_another_compilation_unit_does_not_count_as_a_use(self):
        """A `tests/` binary cannot see a private item in `src/`, so it cannot keep it alive.

        The item therefore has no use in its own unit and falls to the rule above rather than being
        reported. This asserts the quiet, not a finding: it is where the unit boundary is.
        """
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(feature = "opus")]
            fn helper() -> u8 {
                7
            }
            """,
        )
        self.write(
            "crates/sipx-media/tests/codec.rs",
            """
            #[cfg(all(feature = "opus", target_os = "linux"))]
            #[test]
            fn uses_it() {
                assert_eq!(helper(), 7);
            }
            """,
        )
        self.assertQuiet()

    def test_a_gate_on_the_mod_declaration_reaches_the_file_it_names(self):
        """`sipx-transport` gates `pub mod quic;` on the feature; `quic.rs` says so nowhere.

        Without following that, every item in the file looks broader than its callers by exactly
        the feature the module is behind — which would report most of the optional transports.
        """
        self.write(
            "crates/sipx-transport/src/lib.rs",
            """
            #[cfg(feature = "quic")]
            pub mod quic;
            """,
        )
        self.write(
            "crates/sipx-transport/src/quic.rs",
            """
            #[cfg(unix)]
            fn helper() -> u8 {
                7
            }

            #[cfg(all(unix, feature = "quic"))]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        self.assertQuiet()

    def test_the_same_tree_without_the_mod_gate_is_reported(self):
        """The sibling that proves the test above is testing the propagation and not the tree."""
        self.write("crates/sipx-transport/src/lib.rs", "pub mod quic;\n")
        self.write(
            "crates/sipx-transport/src/quic.rs",
            """
            #[cfg(unix)]
            fn helper() -> u8 {
                7
            }

            #[cfg(all(unix, feature = "quic"))]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        self.assertReported("helper")


class TheOutputSaysWhatItCovers(Fixture):
    """A green line that does not say what it looked at is the failure mode, not the remedy."""

    def clean_tree(self) -> None:
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(feature = "opus")]
            fn helper() -> u8 {
                7
            }

            #[cfg(feature = "opus")]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )

    def test_a_pass_states_its_scope_and_what_it_counted(self):
        self.clean_tree()
        output = self.assertQuiet()
        self.assertIn("scope:", output, "a green run does not say what it covered")
        self.assertIn("crates/", output)
        self.assertIn("Rust files", output, "a green run does not say how much it read")
        self.assertIn("cfg attributes", output)
        self.assertIn("gated items with a use", output)

    def test_a_pass_states_what_it_cannot_see(self):
        self.clean_tree()
        output = self.assertQuiet()
        self.assertIn("not covered", output, "the blind spots are implied by a green line")
        self.assertIn("check-features.sh", output)

    def test_a_failure_states_its_scope_too(self):
        self.write(
            "crates/sipx-media/src/lib.rs",
            """
            #[cfg(unix)]
            fn helper() -> u8 {
                7
            }

            #[cfg(all(unix, feature = "opus"))]
            pub fn caller() -> u8 {
                helper()
            }
            """,
        )
        output = self.assertReported("helper")
        self.assertIn("scope:", output)
        self.assertIn("not covered", output)

    def test_a_tree_it_read_almost_nothing_of_is_a_failure_and_not_a_pass(self):
        """The assertion that it scanned something, exercised the only way it can be.

        The floor applies to a run over this repository — the one where a silent reader would be
        mistaken for a clean workspace. So the script is copied into a nearly empty tree and run
        the way the gate runs it, with no `--root` to excuse the emptiness.
        """
        self.write("crates/sipx-media/src/lib.rs", "pub fn caller() -> u8 {\n    7\n}\n")
        (self.root / "scripts").mkdir(parents=True, exist_ok=True)
        shutil.copy2(SCRIPT, self.root / "scripts" / SCRIPT.name)
        done = subprocess.run(
            [sys.executable, str(self.root / "scripts" / SCRIPT.name), "--check"],
            capture_output=True,
            text=True,
            cwd=self.root,
        )
        output = done.stdout + done.stderr
        self.assertEqual(
            1,
            done.returncode,
            f"a workspace it read almost nothing of was reported clean:\n{output}",
        )
        self.assertIn("too little", output)
        self.assertIn("scope:", output)


class TheRepositoryItself(unittest.TestCase):
    """The half no fixture can assert: that this runs, on this tree, from the gate and from CI."""

    def test_this_workspace_has_no_item_outliving_its_callers(self):
        done = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"], capture_output=True, text=True
        )
        self.assertEqual(
            0,
            done.returncode,
            f"the workspace has a cfg-gated item dead on a platform CI builds:\n"
            f"{done.stdout}{done.stderr}",
        )

    def test_the_gate_runs_it_and_ci_does_too(self):
        """A check nothing runs is a file. `X-22`'s property, for this step."""
        steps = gate().gate_steps("1.0.0")
        mine = [step for step in steps if SCRIPT.name in " ".join(step.command)]
        self.assertEqual(
            1, len(mine), "the cfg-caller guard is not a gate step, so nothing runs it locally"
        )
        jobs = gate().parse_workflow(WORKFLOW.read_text())
        self.assertIn(
            mine[0].ci_job,
            jobs,
            f"gate step `{mine[0].name}` names CI job `{mine[0].ci_job}`, which ci.yml does not "
            f"define",
        )

    def test_the_gate_runs_this_suite_too(self):
        steps = gate().gate_steps("1.0.0")
        mine = [
            step
            for step in steps
            if pathlib.Path(__file__).name in " ".join(step.command)
        ]
        self.assertEqual(
            1,
            len(mine),
            "the suite that proves the guard still recognises its own shapes is not a gate step",
        )

    def test_the_reachable_cross_target_is_a_gate_step(self):
        """`X-125` measured `x86_64-pc-windows-gnu` as buildable here; a measurement nobody runs
        decays into a claim. `x86_64-apple-darwin` is not, which is why this is one target and not
        a matrix — see `NOT_RUN_LOCALLY['device-portable']` for that half."""
        steps = gate().gate_steps("1.0.0")
        mine = [step for step in steps if CROSS_TARGET in " ".join(step.command)]
        self.assertEqual(
            1,
            len(mine),
            f"nothing in the gate cross-checks {CROSS_TARGET}, so the one non-Linux "
            f"configuration this host can build is checked by CI alone",
        )
        jobs = gate().parse_workflow(WORKFLOW.read_text())
        self.assertIn(mine[0].ci_job, jobs)
        self.assertTrue(
            any(CROSS_TARGET in run for run in jobs[mine[0].ci_job].runs),
            f"CI job `{mine[0].ci_job}` does not name {CROSS_TARGET}, so the gate would be "
            f"checking a target CI never checks",
        )

    def test_the_unreachable_half_keeps_its_reason_beside_the_exclusion(self):
        """`device-portable` stays CI-only, and its reason has to carry the measurement.

        The story's whole complaint about the old entry was that "requires the platform audio SDKs"
        is true of linking and false of the `cfg` disagreement that actually went red. The next
        person must not have to re-derive which half is which.
        """
        excluded = gate().NOT_RUN_LOCALLY
        self.assertIn("device-portable", excluded)
        why = excluded["device-portable"]
        self.assertIn("apple-darwin", why, "the reason does not say which target cannot be built")
        self.assertIn(
            CROSS_TARGET,
            why,
            "the reason does not say which half of the job the gate now covers, so a reader still "
            "has to re-derive it",
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
