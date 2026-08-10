#!/usr/bin/env python3
"""Adversarial fixtures for the release-workflow structural guard."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
CHECKER = ROOT / "scripts" / "check-release-workflow.py"
SPEC = importlib.util.spec_from_file_location("release_workflow_check", CHECKER)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)
WORKFLOW = (ROOT / ".github" / "workflows" / "crates-io.yml").read_text(encoding="utf-8")
RESUME_WORKFLOW = (ROOT / ".github" / "workflows" / "crates-io-resume.yml").read_text(
    encoding="utf-8"
)
SPEC_TEXT = (ROOT / "docs" / "specs" / "release-workflow.md").read_text(encoding="utf-8")


class CurrentWorkflow(unittest.TestCase):
    def test_current_workflow_satisfies_the_contract(self) -> None:
        self.assertEqual([], checker.workflow_problems(WORKFLOW))
        self.assertEqual([], checker.resume_workflow_problems(RESUME_WORKFLOW))
        self.assertEqual([], checker.specification_problems(SPEC_TEXT))
        self.assertEqual([], checker.check())


class ExpressionContextMutations(unittest.TestCase):
    def test_runner_context_cannot_be_read_before_the_release_job_has_a_runner(self) -> None:
        mutated = WORKFLOW.replace(
            "      RELEASE_TAG: ${{ github.ref_name }}\n",
            "      INVALID_PATH: ${{ runner.temp }}/before-a-runner-exists\n"
            "      RELEASE_TAG: ${{ github.ref_name }}\n",
            1,
        )
        self.assertIn(
            "release job environment reads runner context before a runner exists",
            checker.workflow_problems(mutated),
        )


class AuthorityMutations(unittest.TestCase):
    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertIn(old, WORKFLOW, f"fixture no longer contains {old!r}")
        problems = checker.workflow_problems(WORKFLOW.replace(old, new, 1))
        self.assertIn(expected, problems)

    def test_tag_push_and_manual_resume_are_both_required(self) -> None:
        self.assert_mutation('      - "v*"', '      - "release-disabled"', "no version-tag push entry")
        self.assert_mutation(
            "  workflow_dispatch:\n",
            "  disabled_dispatch:\n",
            "no manual resume entry",
        )

    def test_manual_resume_must_use_the_selected_tag_ref(self) -> None:
        self.assert_mutation(
            "RELEASE_TAG: ${{ github.ref_name }}",
            "RELEASE_TAG: ${{ inputs.tag }}",
            "release tag is not derived from the selected ref",
        )
        self.assert_mutation(
            '"$REQUESTED_RELEASE_TAG" != "$RELEASE_TAG"',
            '"$REQUESTED_RELEASE_TAG" == "$RELEASE_TAG"',
            "manual confirmation need not equal the selected tag",
        )
        self.assert_mutation(
            '"$GITHUB_REF_TYPE" != tag',
            '"$GITHUB_REF_TYPE" != branch',
            "workflow does not require the selected ref to be the release tag",
        )

    def test_environment_timeout_and_non_cancelling_serialization_are_required(self) -> None:
        self.assert_mutation("      name: release\n", "      name: staging\n", "release runs outside the approved environment")
        self.assert_mutation("    timeout-minutes: 180\n", "", "release job has no finite timeout")
        self.assert_mutation(
            "  cancel-in-progress: false\n",
            "  cancel-in-progress: true\n",
            "release concurrency can cancel a publication",
        )

    def test_repository_authority_stays_read_only(self) -> None:
        self.assert_mutation(
            "  contents: read\n",
            "  contents: write\n",
            "workflow permissions are not read-only",
        )
        problems = checker.workflow_problems(
            WORKFLOW.replace(
                "    env:\n      RELEASE_TAG:",
                "    permissions:\n      contents: write\n    env:\n      RELEASE_TAG:",
                1,
            )
        )
        self.assertIn("publication job can write repository contents", problems)
        problems = checker.workflow_problems(
            WORKFLOW.replace("          persist-credentials: false\n", "", 1)
        )
        self.assertIn("release checkout persists a credential", problems)

    def test_cargo_secret_name_and_empty_refusal_are_required(self) -> None:
        problems = checker.workflow_problems(
            WORKFLOW.replace("secrets.CARGO_REGISTRY_TOKEN", "secrets.RELEASE_TOKEN")
        )
        self.assertIn("Cargo secret does not use the repository convention", problems)
        self.assert_mutation(
            '[[ -z "$CARGO_REGISTRY_TOKEN" ]]',
            '[[ "x" == "y" ]]',
            "empty Cargo secret is not refused",
        )

    def test_complete_gate_receives_the_provenance_denylist_secret(self) -> None:
        self.assert_mutation(
            "SIPX_DENYLIST: ${{ secrets.SIPX_DENYLIST }}",
            "SIPX_DENYLIST: unavailable",
            "complete gate does not receive the provenance denylist secret",
        )
        duplicated = WORKFLOW.replace(
            "    env:\n      RELEASE_TAG:",
            "    env:\n      SIPX_DENYLIST: ${{ secrets.SIPX_DENYLIST }}\n      RELEASE_TAG:",
            1,
        )
        self.assertIn(
            "provenance denylist secret is not confined to the gate step",
            checker.workflow_problems(duplicated),
        )
        self.assert_mutation(
            '[[ -z "$SIPX_DENYLIST" ]]',
            '[[ "configured" == "configured" ]]',
            "empty provenance denylist is not refused before the gate",
        )

    def test_annotated_clean_main_tag_is_required(self) -> None:
        self.assert_mutation("git cat-file -t", "git cat-file -e", "lightweight tags are not refused")
        self.assert_mutation(
            "git status --porcelain=v1 --untracked-files=all",
            "git status --porcelain=v1 --untracked-files=no",
            "dirty or untracked release files are not refused",
        )
        self.assert_mutation(
            'git merge-base --is-ancestor "$release_sha" origin/main',
            'git show "$release_sha"',
            "release commit is not required on main",
        )

    def test_event_and_workflow_source_are_bound_to_the_tag_commit(self) -> None:
        self.assert_mutation(
            '"$GITHUB_WORKFLOW_SHA" != "$release_sha"',
            '"$GITHUB_WORKFLOW_SHA" != "$GITHUB_SHA"',
            "event and workflow SHAs are not bound to the release commit",
        )
        self.assert_mutation(
            'expected_workflow_ref="$GITHUB_REPOSITORY/.github/workflows/crates-io.yml@refs/tags/$RELEASE_TAG"',
            'expected_workflow_ref="$GITHUB_REPOSITORY/.github/workflows/crates-io.yml@refs/heads/main"',
            "workflow source is not required from the release tag",
        )


class DistributionMutations(unittest.TestCase):
    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertIn(old, WORKFLOW, f"fixture no longer contains {old!r}")
        self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_exact_confirmation_and_consumer_proof_are_required(self) -> None:
        self.assert_mutation(
            '--confirm-publish "$RELEASE_TAG"',
            "--confirm-publish wrong-tag",
            "publication bypasses exact tag confirmation",
        )
        self.assert_mutation(
            '--authorize-ci-publish "$RELEASE_TAG@$RELEASE_SHA"',
            '--authorize-ci-publish "$RELEASE_TAG@HEAD"',
            "publication bypasses exact CI tag and commit authorization",
        )
        self.assert_mutation(
            "--verify-consumer",
            "--inspect-dirty-contents",
            "exact registry consumer proof is absent",
        )

    def test_frontier_loop_must_be_finite_and_observe_all_visible(self) -> None:
        self.assert_mutation(
            "max_invocations=$((public_count + 1))",
            "max_invocations=999999",
            "frontier loop is not bounded by public package count",
        )
        self.assert_mutation(
            "all public packages are already registry-visible",
            "publication probably finished",
            "frontier loop does not require the all-visible observation",
        )

    def test_the_rate_limit_budget_is_named_and_spans_the_frontier_loop(self) -> None:
        # X-127. Left at the helper's default the bound is invisible in the file that runs the
        # release, and without a ledger shared by the loop each invocation restarts it — leaving
        # `timeout-minutes` as the only bound on a whole publication.
        self.assert_mutation(
            "--registry-retry-budget-seconds 4800",
            "--registry-wait-seconds 300",
            "publication does not name a finite rate-limit budget",
        )
        self.assert_mutation(
            '--registry-retry-ledger "$pacing_ledger"',
            "--registry-wait-seconds 300",
            "frontier loop does not carry one rate-limit budget across its invocations",
        )
        # The defect the ledger exists to prevent: one per invocation is one budget per
        # invocation, which is the state this story found.
        per_invocation = WORKFLOW.replace(
            '          pacing_ledger="$RUNNER_TEMP/sipx-release-pacing.json"\n', "", 1
        ).replace(
            '            echo "release frontier invocation $invocation of $max_invocations"\n',
            '            echo "release frontier invocation $invocation of $max_invocations"\n'
            '            pacing_ledger="$RUNNER_TEMP/sipx-release-pacing-$invocation.json"\n',
            1,
        )
        self.assertIn(
            "frontier loop does not carry one rate-limit budget across its invocations",
            checker.workflow_problems(per_invocation),
        )

    def test_locked_rehearsal_must_precede_publication(self) -> None:
        rehearsal = (
            "      - name: Rehearse the locked registry packages\n"
            "        run: ./scripts/release.py --dry-run\n\n"
        )
        self.assertIn(rehearsal, WORKFLOW)
        mutated = WORKFLOW.replace(rehearsal, "", 1) + "\n" + rehearsal
        self.assertIn(
            "locked rehearsal, publication, consumer, Pages and GitHub release steps are out of order",
            checker.workflow_problems(mutated),
        )

    def test_direct_cargo_publication_is_refused(self) -> None:
        problems = checker.workflow_problems(WORKFLOW + "\n      cargo publish --workspace\n")
        self.assertIn(
            "workflow calls cargo publish directly instead of the release helper", problems
        )


class EvidenceMutations(unittest.TestCase):
    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertIn(old, WORKFLOW, f"fixture no longer contains {old!r}")
        self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_pages_must_be_bound_to_sha_and_both_surfaces(self) -> None:
        self.assert_mutation(
            "head_sha=$RELEASE_SHA",
            "head_sha=main",
            "Pages run is not selected by release head SHA",
        )
        self.assert_mutation(
            'deploy docs site" and .conclusion == "success',
            'build docs site" and .conclusion == "success',
            "Pages evidence does not require the deployment job",
        )
        self.assert_mutation(
            "https://codewandler.github.io/sipx/api/sipx_call/index.html",
            "https://codewandler.github.io/sipx/",
            "public API is not probed",
        )

    def test_release_credentials_are_not_job_scoped(self) -> None:
        problems = checker.workflow_problems(
            WORKFLOW.replace(
                "    env:\n      RELEASE_TAG:",
                "    env:\n      GH_TOKEN: ${{ github.token }}\n      RELEASE_TAG:",
                1,
            )
        )
        self.assertIn("a release credential is exposed at job scope", problems)

    def test_pages_token_is_step_scoped(self) -> None:
        self.assert_mutation(
            "      - name: Verify Pages deployment from the release commit\n        env:\n          GH_TOKEN: ${{ github.token }}",
            "      - name: Verify Pages deployment from the release commit\n        env:\n          UNUSED_TOKEN: ${{ github.token }}",
            "GitHub read token is not scoped to the Pages step",
        )


class ArtifactMutations(unittest.TestCase):
    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertIn(old, WORKFLOW, f"fixture no longer contains {old!r}")
        self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_every_native_target_and_runner_is_fixed(self) -> None:
        self.assert_mutation(
            "runner: ubuntu-24.04-arm",
            "runner: ubuntu-24.04",
            "artifact matrix does not contain exactly one aarch64-unknown-linux-musl on ubuntu-24.04-arm",
        )
        self.assert_mutation(
            "target: x86_64-pc-windows-msvc",
            "target: i686-pc-windows-msvc",
            "artifact matrix does not contain exactly one x86_64-pc-windows-msvc on windows-2025",
        )

    def test_artifact_build_and_aggregation_are_exact(self) -> None:
        self.assert_mutation(
            'cargo build --locked --release -p sipx-cli --target "$TARGET" --no-default-features',
            'cargo build --release -p sipx-cli --target "$TARGET"',
            "artifact build does not use the locked no-feature target command",
        )
        self.assert_mutation(
            "needs: [release, cli_artifacts]",
            "needs: release",
            "artifact aggregation does not require every matrix job",
        )
        self.assert_mutation(
            "python3 scripts/release-artifacts.py aggregate",
            "python3 scripts/release-artifacts.py summarize",
            "artifact aggregation does not invoke the exact-set validator",
        )

    def test_stable_kind_and_asset_bytes_are_held(self) -> None:
        self.assert_mutation(
            "release_flag=--latest",
            "release_flag=--prerelease",
            "stable GitHub Release is not marked latest",
        )
        self.assert_mutation(
            "--expected \"$expected\" --actual \"$actual\" --allow-missing",
            "--expected \"$expected\" --actual \"$actual\"",
            "existing release assets are not byte-checked before upload",
        )
        problems = checker.workflow_problems(WORKFLOW + "\n          gh release upload --clobber\n")
        self.assertIn("release assets may be overwritten", problems)


class PublicityBoundaryMutations(unittest.TestCase):
    def test_an_announcement_job_is_refused(self) -> None:
        mutated = WORKFLOW + "\n  announce:\n    runs-on: ubuntu-latest\n"
        self.assertIn(
            "workflow contains an announcement job", checker.workflow_problems(mutated)
        )

    def test_posting_commands_beyond_the_github_release_are_refused(self) -> None:
        commands = (
            "gh issue create --title released",
            "gh api --method POST repos/example/example/dispatches",
            "curl -X POST https://example.invalid/hook",
        )
        for command in commands:
            with self.subTest(command=command):
                problems = checker.workflow_problems(WORKFLOW + f"\n      - run: {command}\n")
                self.assertIn(
                    "workflow contains an external announcement or posting side effect",
                    problems,
                )

    def test_the_github_release_is_exact_and_resumable(self) -> None:
        mutations = (
            ("--verify-tag", "--generate-notes", "GitHub release does not verify the existing tag"),
            ("--prerelease", "--draft", "stable and prerelease kinds are not selected from the version"),
            (
                '--notes-file "$RELEASE_NOTES"',
                '--notes "looks ready"',
                "GitHub release does not consume reviewed notes",
            ),
        )
        for old, new, expected in mutations:
            with self.subTest(old=old):
                self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_the_specification_keeps_publicity_separate(self) -> None:
        mutated = SPEC_TEXT.replace(
            "MUST NOT post broader publicity",
            "may post broader publicity",
            1,
        )
        self.assertIn(
            "specification does not separate the GitHub prerelease from broader publicity",
            checker.specification_problems(mutated),
        )

    def test_the_specification_confines_the_provenance_denylist(self) -> None:
        mutated = SPEC_TEXT.replace(
            "MUST be exposed only to the complete-gate step",
            "may be exposed to the job",
            1,
        )
        self.assertIn(
            "specification does not confine the provenance denylist to the gate step",
            checker.specification_problems(mutated),
        )

    def test_the_specification_holds_release_kind_and_asset_bytes(self) -> None:
        mutated = SPEC_TEXT.replace(
            "a\nstable version creates a non-prerelease release",
            "every version creates a prerelease",
            1,
        )
        self.assertIn(
            "specification does not distinguish stable and prerelease records",
            checker.specification_problems(mutated),
        )
        mutated = SPEC_TEXT.replace(
            "no existing\nasset may be overwritten or deleted",
            "existing assets may be overwritten",
            1,
        )
        self.assertIn(
            "specification permits existing release assets to be replaced",
            checker.specification_problems(mutated),
        )

    def test_the_specification_binds_and_separates_recovery(self) -> None:
        mutated = SPEC_TEXT.replace(
            "recovery workflow MUST verify the named failed run through the Actions API before exposing the\nCargo credential",
            "recovery may trust a supplied run ID after exposing the credential",
            1,
        )
        self.assertIn(
            "specification does not bind recovery to failed-run evidence before credentials",
            checker.specification_problems(mutated),
        )
        mutated = SPEC_TEXT.replace(
            "Recovery tooling and\nrelease bytes live in separate checkouts",
            "Recovery tooling may modify the release checkout",
            1,
        )
        self.assertIn(
            "specification does not separate recovery tooling from release bytes",
            checker.specification_problems(mutated),
        )


class RecoveryMutations(unittest.TestCase):
    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertIn(old, RESUME_WORKFLOW, f"recovery fixture no longer contains {old!r}")
        problems = checker.resume_workflow_problems(RESUME_WORKFLOW.replace(old, new, 1))
        self.assertIn(expected, problems)

    def test_recovery_is_manual_protected_read_only_and_serialized(self) -> None:
        self.assert_mutation(
            "  workflow_dispatch:\n",
            "  push:\n",
            "recovery has an automatic entry",
        )
        self.assert_mutation(
            "      failed_run_id:\n",
            "      ignored_run_id:\n",
            "recovery has no required failed run input",
        )
        self.assert_mutation(
            "      name: release\n",
            "      name: staging\n",
            "recovery runs outside the protected release environment",
        )
        self.assert_mutation(
            "  cancel-in-progress: false\n",
            "  cancel-in-progress: true\n",
            "recovery concurrency can cancel a publication",
        )
        self.assert_mutation(
            "  contents: read\n",
            "  contents: write\n",
            "recovery permissions are not read-only",
        )

    def test_controller_and_release_checkouts_stay_separate_and_immutable(self) -> None:
        self.assert_mutation(
            "          ref: ${{ github.sha }}\n          path: controller\n",
            "          ref: main\n          path: controller\n",
            "fixed controller checkout is absent or mutable",
        )
        self.assert_mutation(
            "          ref: refs/tags/${{ inputs.tag }}\n          path: release\n",
            "          ref: ${{ github.sha }}\n          path: release\n",
            "immutable release checkout is absent or not separate",
        )
        self.assert_mutation(
            ".github/workflows/crates-io-resume.yml@refs/heads/main",
            ".github/workflows/crates-io-resume.yml@refs/heads/recovery",
            "recovery workflow source is not required from exact main",
        )
        self.assert_mutation(
            'git -C "$SIPX_RELEASE_ROOT" status --porcelain=v1 --untracked-files=all',
            'git -C "$SIPX_RELEASE_ROOT" status --porcelain=v1 --untracked-files=no',
            "release checkout cleanliness is not required",
        )

    def test_failed_run_is_bound_to_original_workflow_tag_and_step_results(self) -> None:
        mutations = (
            (
                'run.get("path") != ".github/workflows/crates-io.yml"',
                'run.get("path") != ".github/workflows/anything.yml"',
                "failed run is not bound to the ordinary workflow",
            ),
            (
                'run.get("head_sha") != sha or run.get("head_branch") != tag',
                'run.get("head_sha") != run.get("head_sha")',
                "failed run is not bound to tag and release SHA",
            ),
            (
                '"Run the complete release gate": "success"',
                '"Run a partial gate": "success"',
                "recovery does not require the complete gate to have succeeded",
            ),
            (
                '"Rehearse the locked registry packages": "success"',
                '"Skip package rehearsal": "success"',
                "recovery does not require rehearsal to have succeeded",
            ),
            (
                '"Publish dependency-ready frontiers under a finite bound": "failure"',
                '"Publish dependency-ready frontiers under a finite bound": "success"',
                "recovery does not require publication to have failed",
            ),
        )
        for old, new, expected in mutations:
            with self.subTest(expected=expected):
                self.assert_mutation(old, new, expected)

    def test_recovery_uses_fixed_controller_interface_and_exact_authority(self) -> None:
        self.assert_mutation(
            '--release-root "$SIPX_RELEASE_ROOT"',
            '--release-root "$CONTROLLER_ROOT"',
            "recovery publication does not name the immutable release root",
        )
        self.assert_mutation(
            '--authorize-ci-recovery "$RELEASE_TAG@$RELEASE_SHA@$SIPX_FAILED_RELEASE_RUN_ID"',
            '--authorize-ci-recovery "$RELEASE_TAG@$RELEASE_SHA@1"',
            "recovery authorization is not bound to tag, release SHA and failed run",
        )
        # Mutate the second root argument: consumer verification must independently name it.
        first = RESUME_WORKFLOW.index('--release-root "$SIPX_RELEASE_ROOT"')
        second = RESUME_WORKFLOW.index('--release-root "$SIPX_RELEASE_ROOT"', first + 1)
        mutated = (
            RESUME_WORKFLOW[:second]
            + '--release-root "$CONTROLLER_ROOT"'
            + RESUME_WORKFLOW[second + len('--release-root "$SIPX_RELEASE_ROOT"') :]
        )
        self.assertIn(
            "recovery exact consumer proof is absent",
            checker.resume_workflow_problems(mutated),
        )

    def test_recovery_pins_tag_object_and_original_packager_toolchain(self) -> None:
        self.assert_mutation(
            "EXPECTED_RELEASE_TAG_OBJECT: 04a19dff6a7d7b6c072c98d18ad4b42407955d4b",
            "EXPECTED_RELEASE_TAG_OBJECT: movable",
            "recovery does not pin the beta tag object",
        )
        self.assert_mutation(
            "RUSTUP_TOOLCHAIN: 1.97.1",
            "RUSTUP_TOOLCHAIN: stable",
            "recovery does not pin the original packager toolchain",
        )
        self.assert_mutation(
            'rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal',
            "rustup toolchain install stable --profile minimal",
            "recovery does not install the pinned packager toolchain",
        )

        query = 'git -C "$SIPX_RELEASE_ROOT" ls-remote --refs --tags origin "refs/tags/$RELEASE_TAG"'
        occurrences = []
        start = 0
        while True:
            found = RESUME_WORKFLOW.find(query, start)
            if found < 0:
                break
            occurrences.append(found)
            start = found + len(query)
        self.assertEqual(2, len(occurrences), "release-root remote-tag query count changed")
        second = occurrences[1]
        mutated = RESUME_WORKFLOW[:second] + "printf stale" + RESUME_WORKFLOW[second + len(query) :]
        self.assertIn(
            "recovery does not recheck the remote tag object before every helper write",
            checker.resume_workflow_problems(mutated),
        )

        github_query = 'git ls-remote --refs --tags origin "refs/tags/$RELEASE_TAG"'
        self.assertIn(github_query, RESUME_WORKFLOW)
        mutated = RESUME_WORKFLOW.replace(github_query, "printf stale", 1)
        self.assertIn(
            "recovery does not recheck the tag object before GitHub prerelease handling",
            checker.resume_workflow_problems(mutated),
        )

    def test_recovery_frontier_and_downstream_evidence_remain_bounded_and_exact(self) -> None:
        self.assert_mutation(
            "max_invocations=$((public_count + 1))",
            "max_invocations=999999",
            "recovery frontier loop is not bounded by public package count",
        )
        self.assert_mutation(
            "all public packages are already registry-visible",
            "publication probably finished",
            "recovery frontier does not require the all-visible observation",
        )
        self.assert_mutation(
            "head_sha=$RELEASE_SHA",
            "head_sha=main",
            "recovery Pages run is not selected by release SHA",
        )
        self.assert_mutation(
            "--consumer-timeout-seconds 900",
            "--consumer-timeout-seconds 0",
            "recovery consumer command has no finite bound",
        )

    def test_the_recovery_rate_limit_budget_is_named_and_spans_its_frontier_loop(self) -> None:
        self.assert_mutation(
            "--registry-retry-budget-seconds 7200",
            "--registry-wait-seconds 300",
            "recovery publication does not name a finite rate-limit budget",
        )
        self.assert_mutation(
            '--registry-retry-ledger "$pacing_ledger"',
            "--registry-wait-seconds 300",
            "recovery frontier loop does not carry one rate-limit budget across its invocations",
        )
        per_invocation = RESUME_WORKFLOW.replace(
            '          pacing_ledger="$RUNNER_TEMP/sipx-recovery-pacing.json"\n', "", 1
        ).replace(
            '            echo "recovery frontier invocation $invocation of $max_invocations"\n',
            '            echo "recovery frontier invocation $invocation of $max_invocations"\n'
            '            pacing_ledger="$RUNNER_TEMP/sipx-recovery-pacing-$invocation.json"\n',
            1,
        )
        self.assertIn(
            "recovery frontier loop does not carry one rate-limit budget across its invocations",
            checker.resume_workflow_problems(per_invocation),
        )

    def test_recovery_write_authority_is_dependent_and_posting_is_refused(self) -> None:
        self.assert_mutation(
            "    needs: recover\n",
            "    needs: []\n",
            "recovery GitHub prerelease is not dependent",
        )
        self.assert_mutation(
            "          persist-credentials: false\n",
            "          persist-credentials: true\n",
            "fixed controller checkout is absent or mutable",
        )
        mutated = RESUME_WORKFLOW + "\n      - run: gh issue create --title released\n"
        self.assertIn(
            "recovery contains an external announcement or posting side effect",
            checker.resume_workflow_problems(mutated),
        )


def _block(text: str, first_step: str, next_step: str) -> str:
    """One step's source, from its name line up to the name line of the step after it."""

    start = text.index(f"      - name: {first_step}\n")
    end = text.index(f"      - name: {next_step}\n")
    assert start < end, f"{first_step!r} no longer precedes {next_step!r}"
    return text[start:end]


PREFLIGHT_BLOCK = _block(
    WORKFLOW,
    "Require exact-SHA main CI and Pages deployment evidence",
    "Install release build prerequisites",
)
RESTORE_BLOCK = _block(
    WORKFLOW,
    "Restore the Actions-managed Rust artifact cache",
    "Run the complete release gate",
)


class PreflightMutations(unittest.TestCase):
    """X-93: the cheap evidence read in front of the gate may not become the evidence."""

    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertTrue(old in WORKFLOW, f"fixture no longer contains {old!r}")
        self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_the_preflight_must_exist_and_stand_between_validation_and_the_gate(self) -> None:
        self.assertIn(
            "read-only exact-SHA CI and Pages preflight is absent",
            checker.workflow_problems(WORKFLOW.replace(PREFLIGHT_BLOCK, "", 1)),
        )
        moved_after_the_gate = WORKFLOW.replace(PREFLIGHT_BLOCK, "", 1) + PREFLIGHT_BLOCK
        self.assertIn(
            "preflight does not run before the expensive gate",
            checker.workflow_problems(moved_after_the_gate),
        )
        validation = "      - name: Validate the immutable annotated tag\n"
        moved_before_validation = WORKFLOW.replace(PREFLIGHT_BLOCK, "", 1).replace(
            validation, PREFLIGHT_BLOCK + validation, 1
        )
        self.assertIn(
            "preflight reads Actions evidence before the tag is validated",
            checker.workflow_problems(moved_before_validation),
        )

    def test_the_preflight_must_bind_its_own_evidence_to_the_release_commit(self) -> None:
        # Each mutation hits the preflight's copy, which is the first in the file. Both the
        # preflight-specific rule and the paired count fire: one copy of this evidence is exactly
        # the substitution the story refuses.
        for old, new, expected in (
            (
                "head_sha=$RELEASE_SHA",
                "head_sha=main",
                "preflight does not select the CI run by release head SHA",
            ),
            (
                ".head_sha == env.RELEASE_SHA",
                ".head_sha != null",
                "preflight does not recheck the returned run against the release head SHA",
            ),
            (
                'deploy docs site" and .conclusion == "success',
                'build docs site" and .conclusion == "success',
                "preflight does not require the successful Pages deployment job",
            ),
        ):
            with self.subTest(expected=expected):
                self.assert_mutation(old, new, expected)

    def test_the_preflight_must_refuse_missing_evidence_rather_than_report_it(self) -> None:
        refusals = PREFLIGHT_BLOCK.replace("exit 1", "exit 0")
        problems = checker.workflow_problems(WORKFLOW.replace(PREFLIGHT_BLOCK, refusals, 1))
        self.assertIn("preflight accepts a missing exact-SHA CI run", problems)
        self.assertIn("preflight accepts a missing Pages deployment job", problems)

    def test_the_preflight_read_token_is_scoped_to_it(self) -> None:
        unscoped = PREFLIGHT_BLOCK.replace(
            "        env:\n          GH_TOKEN: ${{ github.token }}\n", "", 1
        )
        self.assertIn(
            "preflight GitHub read token is not scoped to the preflight step",
            checker.workflow_problems(WORKFLOW.replace(PREFLIGHT_BLOCK, unscoped, 1)),
        )

    def test_the_preflight_may_not_take_over_the_public_http_proof(self) -> None:
        probing = PREFLIGHT_BLOCK.replace(
            '          echo "preflight: CI run $run_id job $deployment_job is bound to $RELEASE_SHA"\n',
            "          curl --fail --silent https://codewandler.github.io/sipx/docs/getting-started\n",
            1,
        )
        problems = checker.workflow_problems(WORKFLOW.replace(PREFLIGHT_BLOCK, probing, 1))
        self.assertIn(
            "preflight probes the public site instead of leaving that to the Pages proof", problems
        )
        self.assertIn("public guide probe is no longer the single post-consumer proof", problems)

    def test_the_post_consumer_pages_proof_still_states_its_own_evidence(self) -> None:
        # Mutate the *second* copy: the preflight's survives, so only a counted rule can see this.
        for marker, expected in (
            ("head_sha=$RELEASE_SHA", "Pages run is not selected by release head SHA"),
            (
                ".head_sha == env.RELEASE_SHA",
                "returned Pages run is not checked against release head SHA",
            ),
            (
                'deploy docs site" and .conclusion == "success',
                "Pages evidence does not require the deployment job",
            ),
        ):
            with self.subTest(expected=expected):
                second = WORKFLOW.index(marker, WORKFLOW.index(marker) + len(marker))
                mutated = WORKFLOW[:second] + "unbound" + WORKFLOW[second + len(marker) :]
                self.assertIn(expected, checker.workflow_problems(mutated))


class BuildCacheMutations(unittest.TestCase):
    """X-93: the artifact cache is an optimisation, and every rule here says so structurally."""

    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertTrue(old in WORKFLOW, f"fixture no longer contains {old!r}")
        self.assertIn(expected, checker.workflow_problems(WORKFLOW.replace(old, new, 1)))

    def test_the_cache_is_restored_only_after_the_tag_is_validated(self) -> None:
        self.assertIn(
            "Actions-managed Rust artifact cache is never restored",
            checker.workflow_problems(WORKFLOW.replace(RESTORE_BLOCK, "", 1)),
        )
        validation = "      - name: Validate the immutable annotated tag\n"
        hoisted = WORKFLOW.replace(RESTORE_BLOCK, "", 1).replace(
            validation, RESTORE_BLOCK + validation, 1
        )
        self.assertIn(
            "artifact cache is restored before the immutable tag is validated",
            checker.workflow_problems(hoisted),
        )
        after_the_gate = WORKFLOW.replace(RESTORE_BLOCK, "", 1).replace(
            "      - name: Rehearse the locked registry packages\n",
            RESTORE_BLOCK + "      - name: Rehearse the locked registry packages\n",
            1,
        )
        self.assertIn(
            "artifact cache is not restored before and saved after the complete gate",
            checker.workflow_problems(after_the_gate),
        )

    def test_the_cache_may_not_end_a_release_or_restore_inexactly(self) -> None:
        self.assert_mutation(
            "        continue-on-error: true\n",
            "",
            "a failed cache step can stop the release: 'Restore the Actions-managed Rust artifact cache'",
        )
        loosened = RESTORE_BLOCK.replace(
            "          path: |\n",
            "          restore-keys: |\n            sipx-release-gate-v1-\n          path: |\n",
            1,
        )
        self.assertIn(
            "artifact cache accepts an inexact restore key",
            checker.workflow_problems(WORKFLOW.replace(RESTORE_BLOCK, loosened, 1)),
        )
        self.assert_mutation(
            "          key: ${{ steps.build_cache_key.outputs.key }}\n",
            "          key: sipx-release-gate-v1\n",
            "artifact cache key is not the derived key",
        )
        self.assert_mutation(
            "        if: steps.build_cache.outputs.cache-hit != 'true'\n",
            "",
            "artifact cache is re-archived even when the exact entry was restored",
        )

    def test_every_named_key_input_is_load_bearing(self) -> None:
        for old, new, label in (
            ("$RUNNER_OS-$RUNNER_ARCH-${ImageOS:-unknown}", "$RUNNER_OS", "runner image"),
            ("sha256sum Cargo.lock", "sha256sum Cargo.toml", "the workspace lockfile"),
            (
                ".github/workflows/ci.yml | sha256sum",
                "/dev/null | sha256sum",
                "the CI flags the gate runs under",
            ),
            ("rustc +stable -vV", "rustc -vV", "the stable toolchain"),
            ('rustc +"$msrv" -vV', "true", "the MSRV toolchain"),
            (
                "pkg-config --modversion opus openssl alsa",
                "echo unknown",
                "the native feature libraries",
            ),
        ):
            with self.subTest(label=label):
                self.assert_mutation(
                    old, new, f"release build cache key does not cover {label}"
                )

    def test_the_cache_may_not_reach_the_isolated_helper_roots(self) -> None:
        self.assert_mutation(
            "            target\n",
            "            target\n            ${{ runner.temp }}/sipx-registry-consumer\n",
            "'Restore the Actions-managed Rust artifact cache' does not cache exactly the "
            "workspace build and registry downloads",
        )
        self.assertIn(
            "release workflow sets a shared CARGO_TARGET_DIR",
            checker.workflow_problems(
                WORKFLOW.replace(
                    "      RELEASE_TAG:", "      CARGO_TARGET_DIR: /shared\n      RELEASE_TAG:", 1
                )
            ),
        )

    def test_a_cache_result_may_not_decide_whether_a_proof_runs(self) -> None:
        conditioned = WORKFLOW.replace(
            "      - name: Run the complete release gate\n",
            "      - name: Run the complete release gate\n"
            "        if: steps.build_cache.outputs.cache-hit != 'true'\n",
            1,
        )
        problems = checker.workflow_problems(conditioned)
        self.assertIn("a release step is conditioned on a cache hit", problems)
        self.assertIn(
            "normative release step 'Run the complete release gate' is conditional", problems
        )
        skipped = WORKFLOW.replace(
            "      - name: Verify the exact registry consumer and installed CLI\n",
            "      - name: Verify the exact registry consumer and installed CLI\n"
            "        if: github.event_name == 'push'\n",
            1,
        )
        self.assertIn(
            "normative release step 'Verify the exact registry consumer and installed CLI' "
            "is conditional",
            checker.workflow_problems(skipped),
        )

    def test_the_cold_and_warm_figures_are_recorded_and_kept(self) -> None:
        self.assert_mutation(
            './scripts/gate.py --timings "$GATE_TIMINGS"',
            "./scripts/gate.py",
            "the gate run records no timings",
        )
        timings = _block(
            WORKFLOW, "Record the gate's cold or warm timings", "Preserve the gate timings record"
        )
        self.assertIn(
            "the gate's cold or warm timings are never recorded",
            checker.workflow_problems(WORKFLOW.replace(timings, "", 1)),
        )
        self.assertIn(
            "recorded timings are skipped when the gate fails",
            checker.workflow_problems(
                WORKFLOW.replace(timings, timings.replace("        if: always()\n", "", 1), 1)
            ),
        )
        self.assertIn(
            "gate, summary and artifact do not share one runner-temporary timings path",
            checker.workflow_problems(
                WORKFLOW.replace(
                    "${{ runner.temp }}/sipx-gate-timings.json",
                    "${{ runner.temp }}/a-different-gate-timings.json",
                    1,
                )
            ),
        )

    def test_a_node_cache_may_skip_installation_and_nothing_further(self) -> None:
        self.assert_mutation(
            "          cache-dependency-path: website/package-lock.json",
            "          cache-dependency-path: website/package.json",
            "the Node dependency cache is not keyed on the exact website lockfile",
        )
        self.assert_mutation(
            "            target\n",
            "            target\n            website/build\n",
            "a cache holds website/build, which would skip a build rather than an install",
        )


class SpeedSpecificationMutations(unittest.TestCase):
    """The normative half of X-93: prose the checker greps, so each clause is mutated here."""

    def assert_mutation(self, old: str, new: str, expected: str) -> None:
        self.assertTrue(old in SPEC_TEXT, f"specification no longer contains {old!r}")
        self.assertIn(expected, checker.specification_problems(SPEC_TEXT.replace(old, new, 1)))

    def test_the_preflight_clauses_are_normative(self) -> None:
        self.assert_mutation(
            "Missing or wrong-SHA evidence MUST stop the release before the gate.",
            "Missing evidence is reported and the release continues.",
            "specification does not put a read-only preflight in front of the gate",
        )
        self.assert_mutation(
            "It MUST NOT probe the public site",
            "It may probe the public site",
            "specification lets the preflight replace the post-consumer Pages proof",
        )
        self.assert_mutation(
            "MUST NOT be substituted for the complete gate or for any other normative proof",
            "may stand in for the complete gate",
            "specification lets CI success substitute for a normative proof",
        )

    def test_the_cache_clauses_are_normative(self) -> None:
        self.assert_mutation(
            "MAY be restored, and only after the immutable-tag facts of §3 are established",
            "MAY be restored at any point in the job",
            "specification does not place the artifact cache after immutable-tag validation",
        )
        self.assert_mutation(
            "no cache may hold the isolated `CARGO_HOME` or target directories",
            "a cache may hold every build directory",
            "specification does not keep the isolated helper roots out of the cache",
        )
        self.assert_mutation(
            "MUST still run every one of the gate's steps",
            "MAY skip the steps whose artifacts were restored",
            "specification does not require every gate step to run on a cache miss",
        )

    def test_the_retention_rules_are_normative(self) -> None:
        self.assert_mutation(
            "retained only if the recorded cold and warm figures differ by at least 60 seconds",
            "retained because it is obviously faster",
            "specification states no retention rule for the artifact cache",
        )
        self.assert_mutation(
            "MAY skip installation and MUST NOT skip the site, anchor or rustdoc builds",
            "MAY skip the site build",
            "specification lets a Node dependency cache skip more than installation",
        )

    def test_the_timings_context_rules_are_normative(self) -> None:
        self.assert_mutation(
            "job-level environment expressions cannot read runner context",
            "job-level environment expressions may read runner context",
            "specification lets job-level environment expressions read runner context",
        )
        self.assert_mutation(
            "The gate, summary and preserved artifact MUST name the same temporary file",
            "The gate, summary and preserved artifact may name different temporary files",
            "specification lets timing evidence name different temporary files",
        )


if __name__ == "__main__":
    unittest.main()
