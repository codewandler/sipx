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
SPEC_TEXT = (ROOT / "docs" / "specs" / "release-workflow.md").read_text(encoding="utf-8")


class CurrentWorkflow(unittest.TestCase):
    def test_current_workflow_satisfies_the_contract(self) -> None:
        self.assertEqual([], checker.workflow_problems(WORKFLOW))
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


if __name__ == "__main__":
    unittest.main()
