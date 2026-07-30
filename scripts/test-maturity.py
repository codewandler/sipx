#!/usr/bin/env python3
"""Tests for `maturity.py`, against fixtures with known counts rather than the real sources.

The arithmetic is the whole product here, so it is asserted on data whose answers are written down in
the test. Running the generator against the real registry and eyeballing the table would prove only
that it produces a table.

The property that matters most is the one in `a_predicate_is_met_only_when_every_story_is_closed`: a
predicate's state must come from the board and nowhere else, because the alternative — a hand-kept
list of which predicates are met — is exactly the drift this generator exists to remove.
"""

import importlib.util
import pathlib
import sys
import unittest

sys.dont_write_bytecode = True

_SPEC = importlib.util.spec_from_file_location(
    "maturity", pathlib.Path(__file__).resolve().parent / "maturity.py"
)
maturity = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(maturity)


class TheLayerArithmetic(unittest.TestCase):
    """`partial` must never be folded into `implemented`, and the columns must add up."""

    ROWS = [
        {"layer": "media", "status": "implemented"},
        {"layer": "media", "status": "partial"},
        {"layer": "media", "status": "partial"},
        {"layer": "media", "status": "none"},
        {"layer": "media", "status": "syntax"},
        {"layer": "core", "status": "implemented"},
        {"layer": "core", "status": "n/a"},
    ]

    def test_each_status_is_counted_in_its_own_column(self):
        counts = maturity.layer_counts(self.ROWS)
        self.assertEqual(counts["media"]["implemented"], 1)
        self.assertEqual(counts["media"]["partial"], 2, "two partial rows are two, not one done")
        self.assertEqual(counts["media"]["none"], 1)

    def test_the_columns_sum_to_the_total(self):
        """A row that does not add up invites a reader to derive a percentage from what is shown."""
        for layer, counts in maturity.layer_counts(self.ROWS).items():
            named = (
                counts["implemented"] + counts["partial"] + counts["none"] + counts["other"]
            )
            self.assertEqual(
                named, counts["total"], f"{layer}'s columns do not account for every row"
            )

    def test_an_unknown_status_lands_in_other_rather_than_vanishing(self):
        counts = maturity.layer_counts([{"layer": "wire", "status": "something-new"}])
        self.assertEqual(counts["wire"]["other"], 1)
        self.assertEqual(counts["wire"]["total"], 1)

    def test_a_row_with_no_layer_is_still_counted(self):
        """Silently dropping a row would understate the distance, which is the wrong direction."""
        counts = maturity.layer_counts([{"status": "partial"}])
        self.assertEqual(counts["?"]["total"], 1)


class ThePillarArithmetic(unittest.TestCase):
    FOUND = {
        "A-1": {"status": "done", "pillar": "Build"},
        "A-2": {"status": "ready", "pillar": "Build"},
        "A-3": {"status": "backlog", "pillar": "Media"},
        "A-4": {"status": "blocked", "pillar": "Media"},
        "A-5": {"status": "in-progress", "pillar": "Media"},
        "A-6": {"status": "done", "pillar": "Media"},
    }

    def test_blocked_counts_as_open(self):
        """A story parked on a dependency is distance, not progress."""
        per_pillar, _ = maturity.pillar_counts(self.FOUND)
        self.assertEqual(per_pillar["Media"], 3, "backlog, blocked and in-progress are all open")

    def test_done_is_counted_separately_and_not_as_open(self):
        per_pillar, done = maturity.pillar_counts(self.FOUND)
        self.assertEqual(done, 2)
        self.assertEqual(per_pillar["Build"], 1)


class ThePredicateRule(unittest.TestCase):
    """The rule that keeps this report from becoming a second, drifting source of truth."""

    def test_a_predicate_is_met_only_when_every_story_is_closed(self):
        predicate = maturity.Predicate(1, "example", "computed", ["X-1", "X-2"])
        found = {"X-1": {"status": "done"}, "X-2": {"status": "ready"}}
        open_blockers, missing = maturity.predicate_state(predicate, found)
        self.assertEqual(open_blockers, ["X-2"])
        self.assertEqual(missing, [])

        found["X-2"]["status"] = "done"
        open_blockers, missing = maturity.predicate_state(predicate, found)
        self.assertEqual(open_blockers, [], "every story closed means the predicate is met")

    def test_a_story_that_does_not_exist_is_unknown_not_met(self):
        """Renaming or deleting a story must not silently satisfy a predicate.

        This is the failure mode that would make the whole report worthless: a predicate whose
        blocker list points at nothing would read as *met*, so deleting a story would look like
        finishing it.
        """
        predicate = maturity.Predicate(1, "example", "computed", ["X-404"])
        _, missing = maturity.predicate_state(predicate, {})
        self.assertEqual(missing, ["X-404"])

    def test_every_declared_predicate_names_a_story_that_exists(self):
        """The real board, not a fixture: a typo in `ALPHA` would otherwise report as progress."""
        found = maturity.stories()
        for predicate in maturity.ALPHA:
            for blocker in predicate.blockers:
                self.assertIn(
                    blocker,
                    found,
                    f"predicate {predicate.number} names {blocker}, which is not on the board",
                )

    def test_an_attested_predicate_says_why_it_is_not_computed(self):
        """An attestation reported as a measurement is the one thing this file must not do."""
        for predicate in maturity.ALPHA:
            if predicate.kind == "attested":
                self.assertTrue(
                    predicate.detail,
                    f"predicate {predicate.number} is attested and must say why",
                )


class TheReport(unittest.TestCase):
    def test_the_committed_report_is_what_the_sources_say(self):
        """`--check`'s subject, asserted here too so a stale report fails the suite as well."""
        self.assertEqual(
            maturity.existing().strip(),
            maturity.render().strip(),
            "docs/maturity.md has drifted; run ./scripts/maturity.py",
        )

    def test_the_report_states_its_blind_spot(self):
        """The report must name the limit of its own reachability claim.

        This used to assert the string `unverified against callers`, which was the caveat `X-30`
        through `X-37` left standing. `X-38` resolved it by definition — the surface *is* what the
        shipped application uses — and the replacement bullet quotes the old phrase while saying it is
        gone, so the original assertion went on passing against text that no longer made the claim.
        A test that cannot fail for the reason it was written is the defect `X-36` was filed over, so
        it is pinned to the new limit instead: one application, entered per crate.
        """
        text = maturity.render()
        self.assertIn("What this cannot see", text)
        self.assertIn(
            "one application's opinion",
            text,
            "the report must say that its reachable surface is one application's, not everyone's",
        )
        self.assertIn(
            "per crate",
            text,
            "the surface is entered per crate, so a supported module nothing names is not caught",
        )

    def test_predicate_one_reports_the_application_as_its_basis(self):
        """`X-38`: predicate 1 stopped being an attestation and says what it is computed from.

        Both halves matter. If it went back to `attested` the report would be claiming less than the
        gate now enforces; if it claimed to be computed without naming the application and the checker,
        a reader could not tell what the computation was over — and the definition *is* the result here.
        """
        predicate = next(item for item in maturity.ALPHA if item.number == 1)
        self.assertEqual(predicate.kind, "computed")
        self.assertIn(maturity.SURFACE_APPLICATION, predicate.detail)
        self.assertIn(maturity.SURFACE_CHECKER, predicate.detail)

    def test_a_computed_predicate_is_not_labelled_an_attestation(self):
        """The note over predicate 1 must not call its mechanical check an attestation."""
        text = maturity.render()
        self.assertNotIn("Predicate 1 is attested", text)
        self.assertIn("Predicate 1 is computed", text)

    def test_the_resolved_caveat_is_gone_from_the_layer_table(self):
        """The four layers that carried `**no**` must no longer say a caller has not been found."""
        text = maturity.render()
        self.assertNotIn("| **no** |", text)
        self.assertIn("Reachability basis", text)

    def test_no_aggregate_percentage_is_reported(self):
        """One number over unlike layers is the metric this story exists to refuse."""
        self.assertNotIn("%", maturity.render())

    def test_the_reachability_layers_match_the_checker(self):
        """If `rfc-report.py` widens its scope, the caveat here is wrong until it follows."""
        checker = (
            pathlib.Path(__file__).resolve().parent / "rfc-report.py"
        ).read_text()
        for layer in maturity.REACHABILITY_CHECKED:
            self.assertIn(
                f'"{layer}"',
                checker,
                f"maturity.py claims {layer} is reachability-checked and rfc-report.py does not",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
