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

    def test_the_github_prerelease_is_exact_and_resumable(self) -> None:
        mutations = (
            ("--verify-tag", "--generate-notes", "GitHub prerelease does not verify the existing tag"),
            ("--prerelease", "--latest", "GitHub Release is not a prerelease"),
            (
                '--notes-file "$RELEASE_NOTES"',
                '--notes "looks ready"',
                "GitHub prerelease does not consume reviewed notes",
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


if __name__ == "__main__":
    unittest.main()
