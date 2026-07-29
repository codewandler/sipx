#!/usr/bin/env python3
"""Tests for check-audio-claims.py, the guard that holds `sipx-audio` to what it implements.

The guard replaces a sentence that was wrong for the life of the project, so what is worth
testing is that it would have caught it: a codec named in the blurb and implemented nowhere, and
the same claim restated in the two other places `X-25` found it.

The false-positive direction matters as much. The claim vocabulary reads English prose, and a
guard that fired on the crate documentation *disclaiming* G.722 would make it impossible to write
the decision down — which is the other half of what `X-26` had to deliver. That the summary stops
at the first blank comment line is therefore a tested property, not an implementation detail.
"""

import importlib.util
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_module():
    """Import check-audio-claims.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "check_audio_claims", ROOT / "scripts" / "check-audio-claims.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


guard = load_module()


def module(name="g722", feature="", header="G.722 (ITU-T G.722).", items=("encode", "decode")):
    return guard.Module(name=name, feature=feature, header=header, items=tuple(items))


def door(text, where="a description"):
    return guard.FrontDoor(where=where, text=text)


class TheRepositoryItself(unittest.TestCase):
    """The state the gate demands, asserted here so a failure names which half broke."""

    def setUp(self):
        self.modules = guard.modules(guard.LIB.read_text(encoding="utf-8"))
        self.doors = guard.front_doors()

    def test_every_claim_the_crate_makes_is_implemented(self):
        self.assertEqual([], guard.claim_problems(self.doors, self.modules))

    def test_the_three_front_doors_claim_the_same_codecs(self):
        self.assertEqual([], guard.agreement_problems(self.doors))

    def test_all_three_front_doors_are_found(self):
        """A door the reader cannot find is a door that can promise anything."""
        self.assertEqual(3, len(self.doors))
        for found in self.doors:
            with self.subTest(where=found.where):
                self.assertTrue(found.text.strip())


class TheModuleReader(unittest.TestCase):
    """Everything a claim is checked against is derived from this."""

    def setUp(self):
        self.modules = guard.modules(guard.LIB.read_text(encoding="utf-8"))

    def test_it_reads_the_modules_the_crate_declares(self):
        self.assertEqual(["g711", "mix", "opus", "wav"], sorted(m.name for m in self.modules))

    def test_an_optional_codec_carries_the_feature_that_gates_it(self):
        by_name = {m.name: m for m in self.modules}
        self.assertEqual("opus", by_name["opus"].feature)
        self.assertEqual("", by_name["g711"].feature)

    def test_the_codec_modules_go_both_ways(self):
        by_name = {m.name: m for m in self.modules}
        for name in ("g711", "opus"):
            with self.subTest(module=name):
                self.assertTrue(by_name[name].provides("encode"))
                self.assertTrue(by_name[name].provides("decode"))

    def test_a_module_declared_and_missing_is_an_error(self):
        with self.assertRaises(ValueError):
            guard.modules("pub mod g711;\npub mod mix;\npub mod wav;\npub mod g722;\n")

    def test_an_implausibly_small_crate_is_an_error(self):
        """A reader that has drifted finds nothing, backs nothing, and passes everything."""
        with self.assertRaises(ValueError):
            guard.modules("pub mod g711;\n")


class TheSummary(unittest.TestCase):
    """Where the front page stops and the crate's own argument begins."""

    HEADER = (
        "//! Telephony audio: G.711.\n"
        "//!\n"
        "//! **G.722 is not implemented and is not planned.** X-26 removed the claim.\n"
        "\n"
        "pub mod g711;\n"
    )

    def test_it_stops_at_the_first_blank_comment_line(self):
        self.assertEqual("Telephony audio: G.711.", guard.summary(self.HEADER))

    def test_a_disclaimer_below_the_summary_is_not_a_claim(self):
        """The record of why a codec is absent must be writable in the file that lacks it."""
        found = guard.claimed(door(guard.summary(self.HEADER)), guard.CODECS)
        self.assertEqual(["G.711"], [claim.name for claim in found])

    def test_the_whole_header_would_have_read_it_as_a_claim(self):
        """Why the summary and not the header — the distinction is load-bearing."""
        found = guard.claimed(door(guard.header(self.HEADER)), guard.CODECS)
        self.assertIn("G.722", [claim.name for claim in found])


class TheClaimVocabulary(unittest.TestCase):
    """What counts as promising a codec, kept honest in both directions."""

    def test_the_defect_this_replaces(self):
        """The description as it stood from the scaffolding commit until X-26."""
        promised = door(
            "Telephony audio: G.711, G.722, PCM mixing and resampling, WAV I/O, RFC 4733 DTMF"
        )
        codecs = [claim.name for claim in guard.claimed(promised, guard.CODECS)]
        others = [claim.name for claim in guard.claimed(promised, guard.CAPABILITIES)]
        self.assertEqual(["G.711", "G.722"], codecs)
        self.assertEqual(["resampling", "RFC 4733 DTMF", "WAV", "mixing"], others)

    def test_the_two_g_dot_seven_codecs_are_not_each_other(self):
        """One digit apart, and one of them was real."""
        self.assertEqual(
            ["G.711"], [claim.name for claim in guard.claimed(door("G.711 only"), guard.CODECS)]
        )

    def test_a_codec_nobody_named_is_not_claimed(self):
        self.assertEqual([], guard.claimed(door("WAV I/O and PCM mixing"), guard.CODECS))

    def test_an_unimplemented_codec_is_reported(self):
        problems = guard.claim_problems([door("G.722 and G.711")], [module(name="g711", header="G.711.")])
        self.assertEqual(1, len(problems))
        self.assertIn("G.722", problems[0])

    def test_a_codec_it_can_decode_but_not_encode_does_not_back_the_claim(self):
        """A codec sipx can decode and not encode cannot be offered, so it is not "supported"."""
        half = module(header="Opus (RFC 6716).", items=("decode", "Decoder"))
        self.assertIsNone(guard.implements(guard.Claim("Opus", r"\bOpus\b"), [half]))

    def test_a_capability_with_no_symbol_behind_it_is_reported(self):
        problems = guard.claim_problems([door("resampling")], [module(items=("mix_into",))])
        self.assertEqual(1, len(problems))
        self.assertIn("resample", problems[0])


class TheFeatureRule(unittest.TestCase):
    """An optional codec is off by default, and a blurb that omits that oversells the crate."""

    def test_naming_the_codec_alone_does_not_say_it_is_optional(self):
        self.assertFalse(guard.names_the_feature("Telephony audio: G.711 and Opus", "opus"))

    def test_naming_the_feature_does(self):
        self.assertTrue(guard.names_the_feature("Opus behind the `opus` feature", "opus"))

    def test_an_optional_codec_advertised_as_unconditional_is_reported(self):
        gated = module(name="opus", feature="opus", header="Opus (RFC 6716).")
        problems = guard.claim_problems([door("Telephony audio: Opus")], [gated])
        self.assertEqual(1, len(problems))
        self.assertIn("off by default", problems[0])


class TheAgreementRule(unittest.TestCase):
    """Three strings describing one crate. X-25 found them disagreeing with the code together."""

    def test_front_doors_that_promise_different_codecs_are_reported(self):
        problems = guard.agreement_problems(
            [door("G.711", where="one"), door("G.711 and G.722", where="two")]
        )
        self.assertEqual(1, len(problems))
        self.assertIn("one crate, one answer", problems[0])

    def test_front_doors_that_agree_are_not(self):
        self.assertEqual(
            [],
            guard.agreement_problems(
                [door("G.711 and Opus", where="one"), door("Opus, G.711", where="two")]
            ),
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
