#!/usr/bin/env python3
"""Tests for check-audio-claims.py, the guard that holds every published crate to what it implements.

The guard replaces a sentence that was wrong for the life of the project, so what is worth
testing is that it would have caught it: a codec named in the blurb and implemented nowhere, and
the same claim restated in the other places `X-25` found it.

It has now been wrong twice, and the second time is the more instructive. `X-26` removed the
RFC 4733 DTMF claim from `sipx-audio` and it survived in `README.md`'s crate table, because the
first version of this guard read three strings and the README was not one of them — the check
passed at exit 0 with the untruth on the front page. `X-35` generalised the guard from *codecs in
one crate* to *front doors of every published crate*, so the tests below are organised by the
three rules that generalisation introduced: membership, restatement and backing.

The false-positive direction matters as much. The claim vocabulary reads English prose, and a
guard that fired on the crate documentation *disclaiming* G.722 would make it impossible to write
the decision down — which is the other half of what `X-26` had to deliver. That the summary stops
at the first blank comment line is therefore a tested property, not an implementation detail. So
is the newer half of the same argument: `sipx-call` provides RFC 4733 DTMF through `send_digits`,
and a backing rule that only accepted the word `dtmf` would have called a true claim an
over-claim and been switched off by whoever hit it second.
"""

import importlib.util
import pathlib
import subprocess
import tempfile
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Rust's test-module attribute, assembled rather than written out.
#:
#: `TheModuleReader` reads *this file* as if it were Rust, and `guard.code` truncates a file at the
#: first literal occurrence of this attribute. Spelling it out above the fixtures that need it
#: would cut this file short of the declarations that test reads — which is exactly `X-116`'s
#: defect, reproduced inside the file that exists to catch it.
TEST_ATTRIBUTE = "#[cfg" + "(test)]"


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


def door(text, where="a description", crate="sipx-audio"):
    return guard.FrontDoor(crate=crate, where=where, text=text)


def crate(name="sipx-audio", doors=(), modules=(), vocabulary=()):
    return guard.Crate(
        name=name,
        doors=tuple(doors),
        modules=tuple(modules),
        vocabulary=frozenset(vocabulary),
    )


def five_doors(description="", summary="", package="", readme="", website=""):
    """A crate's five front doors, labelled the way the guard labels them."""
    return (
        door(description, where="crates/x/Cargo.toml description"),
        door(summary, where="crates/x/src/lib.rs summary"),
        door(package, where="crates/x/README.md summary"),
        door(readme, where="README.md crate table"),
        door(website, where="website/docs/guides/as-a-library.md crate table"),
    )


class TheRepositoryItself(unittest.TestCase):
    """The state the gate demands, asserted here so a failure names which half broke."""

    def setUp(self):
        self.published = guard.published()
        self.tables = {
            path: guard.table(path, heading)
            for path, heading in (guard.README_TABLE, guard.GUIDE_TABLE)
        }
        self.crates = [guard.read(name, self.tables) for name in self.published]

    def test_both_crate_tables_name_exactly_the_crates_that_publish(self):
        self.assertEqual([], guard.membership_problems(self.tables, self.published))

    def test_every_published_crate_sets_and_ships_a_readme(self):
        """A-9's failing-first package assertion: ten of eleven had no landing page."""
        self.assertEqual([], guard.readme_problems(self.published))
        for name in self.published:
            with self.subTest(crate=name):
                packaged = subprocess.run(
                    ["cargo", "package", "-p", name, "--list", "--allow-dirty"],
                    cwd=ROOT,
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout.splitlines()
                relative = guard.package_readme(name).relative_to(guard.CRATES / name)
                self.assertIn(str(relative), packaged)

    def test_every_reachable_public_enum_is_non_exhaustive_or_argued_at_the_type(self):
        """A-9's failing-first API assertion: additive variants must stay additive.

        `M-74`'s row, once `M-78` corrected which enums the rule selects. Before that it read a
        name suffix, so `MediaProfile`, `IcePolicy`, `Keying`, `RtcpMode` and `Codec` — every data
        enum on the media path — were outside a contract that covered an internal `ParseError`.
        """
        self.assertEqual([], guard.enum_problems(guard.guarded(self.published)))

    def test_the_guard_reads_enums_out_of_every_crate_it_covers(self):
        """A blinded reader passes every rule that runs over it, and says so at exit 0.

        `X-116` put Rust's test-module attribute in a crate-root comment, and `code` truncates
        each file at the first one — that single comment made this reader see thirteen empty
        crates. A count is the cheapest assertion that cannot hold while the reader is blind.
        """
        for name in guard.guarded(self.published):
            with self.subTest(crate=name):
                self.assertGreater(len(guard.reachable_enums(name)), 0)

    def test_the_rollout_boundary_names_crates_that_exist(self):
        """A boundary able to name one enum would be a suppression list with a better name."""
        for name in guard.GUARDED_SURFACE:
            with self.subTest(crate=name):
                self.assertIn(name, self.published)

    def test_every_breakable_public_struct_is_non_exhaustive_or_argued_at_the_type(self):
        """`M-80`'s failing-first API assertion: additive fields must stay additive.

        Before `M-80` this reported six types, `Encoded` and `Packet` among them — the two whose
        field additions in `M-75` and `M-79` broke in-tree literals and are the reason the rule
        exists at all.
        """
        self.assertEqual([], guard.struct_problems(guard.guarded_structs(self.published)))

    def test_the_guard_reads_structs_out_of_every_crate_it_covers(self):
        """The blinding assertion of the enum rule above, for the struct rule.

        A rule that selects nothing passes at exit 0, and `X-116` is the proof that happens by
        accident rather than by design.
        """
        for name in guard.guarded_structs(self.published):
            with self.subTest(crate=name):
                self.assertGreater(len(guard.breakable_structs(name)), 0)

    def test_the_struct_boundary_names_crates_that_exist(self):
        for name in guard.MEDIA_SURFACE:
            with self.subTest(crate=name):
                self.assertIn(name, self.published)

    def test_the_struct_debt_is_reported_rather_than_suppressed(self):
        """The remainder is a number on every run, not a list nobody reads.

        A boundary that reported zero outstanding structs would mean the rule had either finished
        or gone blind, and on this workspace it has done neither.
        """
        self.assertGreater(len(guard.outstanding_structs(self.published)), 0)

    def test_every_claim_every_crate_makes_is_backed(self):
        """`X-35`'s failing-first assertion.

        Before `X-35` this failed on `README.md`'s crate row for `sipx-audio`, which claimed
        "RFC 4733 DTMF" — removed from the crate by `X-26` and left standing here — and on the
        same row naming Opus without saying it is behind a feature. The guard could not have seen
        either, because `README.md` was not one of the strings it read.
        """
        self.assertEqual([], [problem for c in self.crates for problem in guard.claim_problems(c)])

    def test_no_front_door_out_promises_the_crates_own_listing(self):
        self.assertEqual(
            [], [problem for c in self.crates for problem in guard.agreement_problems(c)]
        )

    def test_every_crate_has_five_front_doors_and_none_of_them_is_empty(self):
        """A door the reader cannot find is a door that can promise anything."""
        for c in self.crates:
            with self.subTest(crate=c.name):
                self.assertEqual(5, len(c.doors))
                for found in c.doors:
                    with self.subTest(where=found.where):
                        self.assertTrue(found.text.strip())

    def test_the_crate_this_check_started_from_is_still_published(self):
        """The codec rule is scoped to one crate by name; a rename would silence it."""
        self.assertIn(guard.CODEC_CRATE, self.published)


class TheMembershipRule(unittest.TestCase):
    """A published crate no table describes has no front door to be wrong in."""

    def test_a_published_crate_missing_from_a_table_is_reported(self):
        problems = guard.membership_problems(
            {ROOT / "README.md": {"sipx-sip": "core"}}, ["sipx-sip", "sipx-app"]
        )
        self.assertEqual(1, len(problems))
        self.assertIn("sipx-app", problems[0])

    def test_a_table_row_for_a_crate_that_does_not_publish_is_reported(self):
        """A table cannot advertise a crate excluded by package metadata."""
        problems = guard.membership_problems(
            {ROOT / "README.md": {"sipx-sip": "core", "sipx-testkit": "harnesses"}},
            ["sipx-sip"],
        )
        self.assertEqual(1, len(problems))
        self.assertIn("sipx-testkit", problems[0])
        self.assertIn("do not publish", problems[0])

    def test_a_table_that_names_exactly_the_published_crates_is_not(self):
        self.assertEqual(
            [],
            guard.membership_problems({ROOT / "README.md": {"sipx-sip": "core"}}, ["sipx-sip"]),
        )

    def test_the_reader_keys_a_row_on_the_crate_cell_whichever_column_it_is_in(self):
        """`README.md` names the crate first and the guide names it second."""
        readme, guide = guard.README_TABLE[0], guard.GUIDE_TABLE[0]
        self.assertIn("Sans-IO", guard.table(readme, guard.README_TABLE[1])["sipx-sip"])
        self.assertIn("offer/answer", guard.table(guide, guard.GUIDE_TABLE[1])["sipx-sdp"])

    def test_a_table_the_reader_cannot_find_is_an_error(self):
        with self.assertRaises(ValueError):
            guard.table(guard.README_TABLE[0], "## Crates We Renamed This Heading")

    def test_the_second_table_in_a_file_is_not_swept_up(self):
        """`as-a-library.md` carries a second crate table, of rustdoc links."""
        rows = guard.table(guard.GUIDE_TABLE[0], guard.GUIDE_TABLE[1])
        self.assertNotIn("codewandler.github.io", rows["sipx-sip"])


class TheRestatementRule(unittest.TestCase):
    """A restatement may say less than the crate's own listing, and not more."""

    def test_a_table_row_claiming_what_the_description_does_not_is_reported(self):
        """The shape of `X-35`: `README.md` named DTMF and the manifest did not.

        Both tables carry the claim here, so the table-versus-table rule stays quiet and this
        asserts the restatement rule alone.
        """
        problems = guard.agreement_problems(
            crate(
                doors=five_doors(
                    description="G.711",
                    readme="G.711, RFC 4733 DTMF",
                    website="G.711, RFC 4733 DTMF",
                )
            )
        )
        self.assertEqual(2, len(problems))
        for problem in problems:
            with self.subTest(problem=problem):
                self.assertIn("RFC 4733 DTMF", problem)
                self.assertIn("and not more", problem)

    def test_a_terser_restatement_is_not(self):
        """`Sans-IO SIP core.` is a good first line and a bad capability list."""
        self.assertEqual(
            [],
            guard.agreement_problems(
                crate(doors=five_doors(description="G.711, WAV, mixing", summary="Audio.", readme="G.711", website="G.711"))
            ),
        )

    def test_two_tables_that_promise_different_things_are_reported(self):
        problems = guard.agreement_problems(
            crate(doors=five_doors(description="G.711, WAV", readme="G.711, WAV", website="G.711"))
        )
        self.assertEqual(1, len(problems))
        self.assertIn("one crate, one answer", problems[0])

    def test_a_crate_with_one_table_door_is_an_error(self):
        """Without both tables, one can promise what the other denies and nothing compares them."""
        with self.assertRaises(ValueError):
            guard.agreement_problems(crate(doors=five_doors()[:4]))


class TheModuleReader(unittest.TestCase):
    """Everything a claim is checked against is derived from this."""

    def setUp(self):
        entry = guard.entry_point(guard.CODEC_CRATE)
        self.modules = guard.modules(guard.CODEC_CRATE, entry)

    def test_it_reads_the_modules_the_crate_declares(self):
        self.assertEqual(
            # `analysis` (voice activity) and `signal` (level, clipping, silence) are
            # deterministic signal analysis, not codecs — they carry no audio claim and
            # load no model. `dsp` and its two children are `M-63`'s processor contract and
            # conformance harness: a sans-I/O frame interface with no algorithm behind it, so
            # likewise no audio claim. All are listed because this asserts what the crate
            # *declares*, and a module the reader cannot see is a claim nothing checks — which is
            # why this is a literal list rather than a rule that would grow to fit whatever it
            # found. Adding a module to `sipx-audio` is meant to fail here until someone says
            # what it is.
            [
                "analysis",
                "dsp",
                "dsp::conformance",
                "dsp::contract",
                "g711",
                "g722",
                "l16",
                "mix",
                "opus",
                "pcm",
                "signal",
                "wav",
            ],
            sorted(m.name for m in self.modules),
        )

    def test_an_optional_codec_carries_the_feature_that_gates_it(self):
        by_name = {m.name: m for m in self.modules}
        self.assertEqual("opus", by_name["opus"].feature)
        self.assertEqual("", by_name["g711"].feature)
        self.assertEqual("", by_name["g722"].feature)
        self.assertEqual("", by_name["l16"].feature)

    def test_the_codec_modules_go_both_ways(self):
        by_name = {m.name: m for m in self.modules}
        for name in ("g711", "g722", "l16", "opus"):
            with self.subTest(module=name):
                self.assertTrue(by_name[name].provides("encode"))
                self.assertTrue(by_name[name].provides("decode"))

    def test_a_module_declared_and_missing_is_an_error(self):
        with self.assertRaises(ValueError):
            guard.modules("sipx-audio", ROOT / "scripts" / "test-audio-claims.py")

    def test_it_walks_into_a_nested_module_directory(self):
        """`sipx-media` keeps `dtls` and `ice` in directories with a `mod.rs`."""
        names = {m.name for m in guard.modules("sipx-media", guard.entry_point("sipx-media"))}
        self.assertIn("dtls", names)
        self.assertIn("dtls::openssl", names)

    def test_a_binary_crate_is_read_without_requiring_public_items(self):
        """`sipx-cli` exposes nothing `pub`; reading only public items would back no claim."""
        entry = guard.entry_point("sipx-cli")
        self.assertEqual("main.rs", entry.name)
        vocabulary = guard.crate_vocabulary(entry, guard.modules("sipx-cli", entry))
        self.assertIn("dial", vocabulary)
        self.assertIn("answer", vocabulary)

    def test_the_vocabulary_is_items_and_not_the_prose_around_them(self):
        """The item patterns are anchored to a line start, and the test module is cut off.

        Unanchored, "the same type name" in a comment is an item called `name`. And this project
        names its tests as whole sentences, so counting them would put most of English behind
        every crate — `an_unmeasurable_round_trip_is_absent_rather_than_zero` is a test in
        `sipx-cli`, and the words below come from that module and nowhere else.
        """
        entry = guard.entry_point("sipx-cli")
        vocabulary = guard.crate_vocabulary(entry, guard.modules("sipx-cli", entry))
        for word in ("unmeasurable", "mistaken", "everything", "rather"):
            with self.subTest(word=word):
                self.assertNotIn(word, vocabulary)

    def test_an_implausibly_small_crate_is_an_error(self):
        """A reader that has drifted finds nothing, backs nothing, and passes everything."""
        with self.assertRaises(ValueError):
            guard.crate_vocabulary(ROOT / "docs" / "vision.md", [])


class TheItemReader(unittest.TestCase):
    """What counts as an item that can back a claim."""

    def test_an_identifier_is_split_into_words(self):
        self.assertEqual({"ice", "agent"}, guard.words("IceAgent"))
        self.assertEqual({"send", "digits"}, guard.words("send_digits"))

    def test_a_word_that_merely_contains_the_capability_does_not_back_it(self):
        """A substring test would let `Service` back ICE and `choice` back it twice."""
        self.assertNotIn("ice", guard.words("Service"))
        self.assertNotIn("ice", guard.words("choice"))

    def test_an_async_public_function_is_an_item(self):
        """`Call::play` is `pub async fn`; a pattern without `async` backed no playback claim."""
        found = [m.group("name") for m in guard._PUBLIC_ITEM.finditer("    pub async fn play(&self)")]
        self.assertEqual(["play"], found)

    def test_a_public_const_function_and_a_public_const_are_both_items(self):
        source = "pub const fn width() -> u8 { 8 }\npub const LIMIT: u8 = 9;\n"
        found = [m.group("name") for m in guard._PUBLIC_ITEM.finditer(source)]
        self.assertEqual(["width", "LIMIT"], found)

    def test_a_crate_private_item_is_not_a_public_item(self):
        self.assertEqual([], list(guard._PUBLIC_ITEM.finditer("pub(crate) fn dial() {}")))

    def test_a_crate_private_item_is_an_item_of_a_binary(self):
        found = [m.group("name") for m in guard._ANY_ITEM.finditer("pub(crate) async fn dial() {}")]
        self.assertEqual(["dial"], found)


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

    def test_a_package_readme_stops_after_its_lead_paragraph(self):
        readme = (
            "# sipx-audio\n\n"
            "Telephony audio: G.711.\n\n"
            "## Deliberately absent\n\n"
            "G.722 is not implemented.\n"
        )
        self.assertEqual("Telephony audio: G.711.", guard.markdown_summary(readme))
        found = guard.claimed(door(guard.markdown_summary(readme)), guard.CODECS)
        self.assertEqual(["G.711"], [claim.name for claim in found])

    def test_a_package_readme_without_an_h1_has_no_summary(self):
        self.assertEqual("", guard.markdown_summary("Telephony audio: G.711.\n"))


class TheExtensibilityRule(unittest.TestCase):
    """A-9: an exception is argued at the type, never hidden in a suppression list."""

    def test_an_exhaustive_enum_is_reported(self):
        problems = self.problems_for(
            "/// A failure.\n#[derive(Debug)]\npub enum DemoError { Failed }\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("DemoError", problems[0])

    def test_a_non_exhaustive_enum_is_not_reported(self):
        self.assertEqual(
            [],
            self.problems_for(
                "/// A failure.\n#[derive(Debug)]\n#[non_exhaustive]\n"
                "pub enum DemoError { Failed }\n"
            ),
        )

    def test_an_exhaustive_enum_with_an_adjacent_reason_is_not_reported(self):
        self.assertEqual(
            [],
            self.problems_for(
                "/// A failure.\n///\n/// Exhaustive by design: these are the complete states.\n"
                "#[derive(Debug)]\npub enum DemoError { Failed }\n"
            ),
        )

    def test_a_distant_reason_does_not_classify_the_type(self):
        problems = self.problems_for(
            "/// Exhaustive by design: this explains another item.\n"
            "pub const EARLIER: u8 = 1;\n\n"
            "/// A failure.\npub enum DemoError { Failed }\n"
        )
        self.assertEqual(1, len(problems))

    def problems_for(self, source: str) -> list[str]:
        return demo_crate({"lib.rs": source})


def demo_crate(files: dict[str, str], rule=None) -> list[str]:
    """Run an extensibility rule over a crate laid out from `path -> source` under `src/`.

    `rule` selects which of the two runs — they share the reachability walk, so both are exercised
    against the same fixtures rather than against two copies of one.
    """
    with tempfile.TemporaryDirectory() as directory:
        crates = pathlib.Path(directory) / "crates"
        src = crates / "sipx-demo" / "src"
        src.mkdir(parents=True)
        for name, source in files.items():
            written = src / name
            written.parent.mkdir(parents=True, exist_ok=True)
            written.write_text(source)
        original = guard.CRATES
        guard.CRATES = crates
        try:
            return (rule or guard.enum_problems)(["sipx-demo"])
        finally:
            guard.CRATES = original


#: An enum with no guard and no rationale, which is the thing the rule is looking for. Named
#: without an `Error` suffix on purpose: the rule this replaced keyed on that spelling, so a
#: fixture called `DemoError` would pass under both rules and prove nothing about either.
UNGUARDED = "/// Which way a stream flows.\n#[derive(Debug)]\npub enum Flow { In, Out }\n"


class TheReachabilityRule(unittest.TestCase):
    """`M-78`: which enums the rule selects, which is a visibility question and not a spelling one.

    The rule this replaced keyed on a name ending in `Error`. That is a convention standing in for
    "downstream code can `match` on this", and the two came apart in both directions: `MediaProfile`,
    `IcePolicy` and `Keying` are on `sipx-call`'s crate root and went unguarded for the life of the
    project, while a `ParseError` in a private module nothing re-exports was held to a contract no
    caller can depend on.

    Three shapes, and every one of them has to be asserted, because getting one right by making
    another wrong is the whole failure mode here.
    """

    def test_a_reachable_unguarded_enum_is_reported(self):
        """Shape one, and the assertion that the rule is not keyed on a name."""
        problems = demo_crate({"lib.rs": "pub mod stream;\n", "stream.rs": UNGUARDED})
        self.assertEqual(1, len(problems))
        self.assertIn("`Flow`", problems[0])
        self.assertIn("reachable from the crate root", problems[0])

    def test_a_reachable_enum_that_is_guarded_or_reasoned_is_quiet(self):
        """Shape two, both ways of satisfying it."""
        self.assertEqual(
            [],
            demo_crate(
                {
                    "lib.rs": "pub mod guarded;\npub mod reasoned;\n",
                    "guarded.rs": "/// Flow.\n#[non_exhaustive]\npub enum Flow { In, Out }\n",
                    "reasoned.rs": "/// Flow.\n///\n/// Exhaustive by design: a stream has two "
                    "ends.\npub enum Side { Near, Far }\n",
                }
            ),
        )

    def test_a_public_enum_inside_a_private_module_is_quiet(self):
        """Shape three: `pub` is not public when nothing outside the crate can write the path."""
        self.assertEqual([], demo_crate({"lib.rs": "mod stream;\n", "stream.rs": UNGUARDED}))

    def test_a_public_enum_inside_a_crate_visible_module_is_quiet(self):
        """`pub(crate) mod` and a bare `mod` are the same thing to a downstream `match`."""
        self.assertEqual(
            [], demo_crate({"lib.rs": "pub(crate) mod stream;\n", "stream.rs": UNGUARDED})
        )

    def test_a_private_module_the_crate_re_exports_is_reported(self):
        """`sipx-call` keeps `MediaProfile` in a private `media_policy` and publishes it anyway.

        This is the half a module-visibility-only reader gets wrong, and it is not a corner: it is
        how every crate in this workspace presents a curated root.
        """
        problems = demo_crate(
            {"lib.rs": "mod stream;\npub use stream::Flow;\n", "stream.rs": UNGUARDED}
        )
        self.assertEqual(1, len(problems))
        self.assertIn("`Flow`", problems[0])

    def test_a_re_export_publishes_only_what_it_names(self):
        """The sibling type in the same private module stays private, and stays quiet."""
        problems = demo_crate(
            {
                "lib.rs": "mod stream;\npub use stream::Flow;\n",
                "stream.rs": UNGUARDED + "\n/// Unpublished.\npub enum Hidden { One }\n",
            }
        )
        self.assertEqual(1, len(problems))
        self.assertIn("`Flow`", problems[0])

    def test_a_glob_re_export_publishes_the_whole_module(self):
        self.assertEqual(
            2,
            len(
                demo_crate(
                    {
                        "lib.rs": "mod stream;\npub use stream::*;\n",
                        "stream.rs": UNGUARDED + "\n/// Also out.\npub enum Other { One }\n",
                    }
                )
            ),
        )

    def test_a_name_forwarded_through_a_private_module_is_followed_to_its_declaration(self):
        """`sipx-sip` publishes transaction types out of a `mod.rs` that itself re-exports them.

        Stopping at the first hop would leave the declaration unguarded while the name is on the
        crate's surface, which is the state the guard exists to make impossible.
        """
        problems = demo_crate(
            {
                "lib.rs": "mod outer;\npub use outer::Flow;\n",
                "outer/mod.rs": "mod inner;\npub use inner::Flow;\n",
                "outer/inner.rs": UNGUARDED,
            }
        )
        self.assertEqual(1, len(problems))
        self.assertIn("inner.rs", problems[0])

    def test_a_test_module_is_not_public_api(self):
        """`code` cuts every file at the test-module attribute, and a fixture enum is not a promise.

        Load-bearing rather than incidental: `X-116` put that attribute in a crate-root comment
        and blinded the reader across all thirteen crates at once.
        """
        self.assertEqual(
            [],
            demo_crate(
                {
                    "lib.rs": "pub mod stream;\n",
                    "stream.rs": f"{TEST_ATTRIBUTE}\nmod tests {{\n{UNGUARDED}}}\n",
                }
            ),
        )

    def test_a_binary_crate_has_no_public_api_to_guard(self):
        self.assertEqual([], demo_crate({"main.rs": UNGUARDED}))

    def test_the_module_graph_records_what_each_declaration_exports(self):
        entry = guard.entry_point("sipx-call")
        graph = guard.module_graph("sipx-call", entry)
        self.assertTrue(graph["call"].exported)
        # `media_policy` is private and its types reach the root through `pub use`.
        self.assertFalse(graph["media_policy"].exported)

    def test_a_use_prefix_resolves_from_where_it_is_written(self):
        self.assertEqual(["ice"], guard.use_targets("session", "crate::ice"))
        self.assertEqual(["session::ice"], guard.use_targets("session", "self::ice"))
        self.assertEqual(["session::ice"], guard.use_targets("session::inner", "super::ice"))
        self.assertEqual(["ice"], guard.use_targets("session", "super::ice"))
        # A bare prefix is a sibling, an item of the root, or another crate; the last matches no
        # module of this crate and falls away on its own.
        self.assertEqual(["session::ice", "ice"], guard.use_targets("session", "ice"))


class TheClaimVocabulary(unittest.TestCase):
    """What counts as promising a capability, kept honest in both directions."""

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

    def test_dtls_srtp_is_not_read_as_a_plain_tls_claim(self):
        """A crate that keys media by DTLS has not thereby claimed a SIP transport."""
        named = [claim.name for claim in guard.claimed(door("DTLS-SRTP keying"), guard.CAPABILITIES)]
        self.assertIn("DTLS-SRTP", named)
        self.assertNotIn("TLS", named)

    def test_the_architecture_words_are_not_capability_claims(self):
        """`dialogs` and `transactions` are how sipx is built, not a capability a reader shops for.

        A vocabulary that included them would compare architecture between doors written at
        different altitudes, and report a one-line crate summary for saying less.
        """
        promised = door("Sans-IO SIP core: messages, parser, transactions and dialog state")
        self.assertEqual([], guard.claimed(promised, guard.VOCABULARY))


class TheBackingRule(unittest.TestCase):
    """A capability word with no item behind it is the defect, in every crate."""

    def test_an_unimplemented_codec_is_reported(self):
        problems = guard.claim_problems(
            crate(
                doors=five_doors(description="G.722 and G.711"),
                modules=[module(name="g711", header="G.711.")],
            )
        )
        self.assertEqual(1, len(problems))
        self.assertIn("G.722", problems[0])

    def test_a_codec_it_can_decode_but_not_encode_does_not_back_the_claim(self):
        """A codec sipx can decode and not encode cannot be offered, so it is not "supported"."""
        half = module(header="Opus (RFC 6716).", items=("decode", "Decoder"))
        self.assertIsNone(guard.implements(guard.Claim("Opus", r"\bOpus\b"), (half,)))

    def test_an_ungated_module_backs_a_codec_that_a_gated_one_also_names(self):
        """`opus.rs` names G.711 while explaining what Opus is for; G.711 is not optional."""
        gated = module(name="opus", feature="opus", header="Opus, unlike G.711, is wideband.")
        plain = module(name="g711", header="G.711 (ITU-T G.711).")
        backing = guard.implements(guard.Claim("G.711", r"G\.?711"), (gated, plain))
        self.assertEqual("g711", backing.name)

    def test_a_capability_with_nothing_behind_it_is_reported(self):
        """`sipx-call`'s description claimed bridging and the crate has no `Bridge`."""
        problems = guard.claim_problems(
            crate(
                name="sipx-call",
                doors=five_doors(description="Calls with bridging"),
                vocabulary={"call", "dial", "answer", "media", "hang"},
            )
        )
        self.assertEqual(1, len(problems))
        self.assertIn("bridging", problems[0])
        self.assertIn("`bridge`", problems[0])

    def test_a_capability_named_after_what_it_does_is_backed(self):
        """RFC 4733 DTMF, provided as `send_digits`. The synonym is the capability's other name."""
        self.assertEqual(
            [],
            guard.claim_problems(
                crate(
                    name="sipx-call",
                    doors=five_doors(description="Calls with DTMF"),
                    vocabulary={"send", "digits", "recv", "digit"},
                )
            ),
        )

    def test_the_codec_rule_does_not_run_outside_the_codec_crate(self):
        """Elsewhere a codec name describes a payload type carried, not an implementation."""
        self.assertEqual(
            [],
            guard.claim_problems(
                crate(
                    name="sipx-media",
                    doors=five_doors(description="Media sessions carrying G.711 and Opus"),
                    vocabulary={"media", "session", "bridge"},
                )
            ),
        )


class TheFeatureRule(unittest.TestCase):
    """An optional codec is off by default, and a blurb that omits that oversells the crate."""

    def test_naming_the_codec_alone_does_not_say_it_is_optional(self):
        self.assertFalse(guard.names_the_feature("Telephony audio: G.711 and Opus", "opus"))

    def test_naming_the_feature_does(self):
        self.assertTrue(guard.names_the_feature("Opus behind the `opus` feature", "opus"))

    def test_an_optional_codec_advertised_as_unconditional_is_reported(self):
        """`README.md`'s crate row said bare "Opus" — `X-35`'s fourth front door."""
        gated = module(name="opus", feature="opus", header="Opus (RFC 6716).")
        problems = guard.claim_problems(
            crate(doors=five_doors(description="Telephony audio: Opus"), modules=[gated])
        )
        self.assertEqual(1, len(problems))
        self.assertIn("off by default", problems[0])



class TheStabilityRule(unittest.TestCase):
    """`A-8`, alpha predicate 5. Only presence is checkable; honesty is not, and the rule says so."""

    def test_every_published_crate_says_what_it_guarantees(self):
        for name in guard.published():
            crate = guard.read(name, self.tables())
            self.assertEqual(
                guard.stability_problems(crate),
                [],
                f"{name} does not declare its stability",
            )

    def test_a_crate_with_no_stability_section_is_reported(self):
        """The state ten of eleven crates were in: fully documented, saying nothing about support."""
        problems = self.problems_for("//! A crate.\n//!\n//! It does things.\n")
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("no `# Stability` section", problems[0])

    def test_a_stability_section_that_classifies_nothing_is_reported(self):
        """A heading is not a declaration; the words are what a reader acts on."""
        problems = self.problems_for(
            "//! A crate.\n//!\n//! # Stability\n//!\n//! We take this seriously.\n"
        )
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("classifies nothing", problems[0])

    def test_a_declared_crate_passes(self):
        self.assertEqual(
            self.problems_for(
                "//! A crate.\n//!\n//! # Stability\n//!\n//! **Supported.** Depend on it.\n"
            ),
            [],
        )

    def test_the_declaration_must_be_in_the_crate_doc_and_not_in_code(self):
        """A `# Stability` heading inside a regular comment or a string is not documentation."""
        problems = self.problems_for(
            '//! A crate.\n\n// # Stability\nconst X: &str = "Supported";\n'
        )
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("no `# Stability` section", problems[0])

    # -- helpers

    def tables(self):
        return {path: guard.table(path, heading) for path, heading in (guard.README_TABLE, guard.GUIDE_TABLE)}

    def problems_for(self, doc: str) -> list[str]:
        """Run the rule against a fabricated entry point, borrowing a real crate's other fields."""
        with tempfile.TemporaryDirectory() as directory:
            entry = pathlib.Path(directory) / "lib.rs"
            entry.write_text(doc)
            crate = guard.read("sipx-sip", self.tables())
            original = guard.entry_point
            guard.entry_point = lambda _name: entry
            try:
                return guard.stability_problems(crate)
            finally:
                guard.entry_point = original


class TheStructExtensibilityRule(unittest.TestCase):
    """`M-80`: a public field is as much of a contract as a variant, and breaks the same way.

    The rule this adds had a comment arguing against itself in the file for the life of the
    project — "a struct can add a private field without breaking a caller" — which is true of the
    struct it describes and false of every struct that has `pub` fields. `M-75` and `M-79` are the
    two field additions that proved it, so what is asserted below is both directions: the shape
    that broke is reported, and the shapes that cannot break are not.
    """

    #: A struct with public fields and a published constructor, which is the shape `Encoded` and
    #: `Packet` both have and the shape the rule is looking for.
    BREAKABLE = (
        "/// A payload.\n"
        "pub struct Encoded {\n"
        "    /// What it is encoded in.\n"
        "    pub payload_type: u8,\n"
        "}\n\n"
        "impl Encoded {\n"
        "    /// One of these.\n"
        "    pub fn new(payload_type: u8) -> Self {\n"
        "        Self { payload_type }\n"
        "    }\n"
        "}\n"
    )

    def test_a_breakable_struct_is_reported(self):
        problems = self.problems_for(self.BREAKABLE)
        self.assertEqual(1, len(problems))
        self.assertIn("Encoded", problems[0])

    def test_a_non_exhaustive_struct_is_not_reported(self):
        self.assertEqual(
            [], self.problems_for(self.BREAKABLE.replace("pub struct", "#[non_exhaustive]\npub struct"))
        )

    def test_a_struct_with_an_adjacent_reason_is_not_reported(self):
        self.assertEqual(
            [],
            self.problems_for(
                self.BREAKABLE.replace(
                    "/// A payload.\n",
                    "/// A payload.\n///\n/// Complete by design: the wire carries these and no more.\n",
                )
            ),
        )

    def test_a_distant_reason_does_not_classify_the_type(self):
        problems = self.problems_for(
            "/// Complete by design: this explains another item.\n"
            "pub const EARLIER: u8 = 1;\n\n" + self.BREAKABLE
        )
        self.assertEqual(1, len(problems))

    def test_an_enum_rationale_does_not_satisfy_the_struct_rule(self):
        """The two phrases answer different questions, so one may not stand in for the other."""
        problems = self.problems_for(
            self.BREAKABLE.replace(
                "/// A payload.\n",
                "/// A payload.\n///\n/// Exhaustive by design: these are the complete states.\n",
            )
        )
        self.assertEqual(1, len(problems))

    def test_a_struct_with_no_public_field_is_not_reported(self):
        """The sentence the old comment made, which is true of exactly this shape."""
        self.assertEqual(
            [], self.problems_for(self.BREAKABLE.replace("    pub payload_type", "    payload_type"))
        )

    def test_a_crate_visible_field_is_not_a_public_field(self):
        """`pub(crate)` cannot be named from outside, so no downstream literal can name it."""
        self.assertEqual(
            [],
            self.problems_for(self.BREAKABLE.replace("    pub payload_type", "    pub(crate) payload_type")),
        )

    def test_a_struct_with_no_constructor_is_not_reported(self):
        """The type boundary: marking one of these leaves a caller no way to build it at all."""
        self.assertEqual([], self.problems_for(self.BREAKABLE.partition("impl Encoded")[0]))

    def test_another_types_constructor_does_not_hold_this_one(self):
        """`impl EncodedBuilder` is not `impl Encoded`, however the prefix reads."""
        problems = self.problems_for(
            self.BREAKABLE.partition("impl Encoded")[0]
            + "/// A builder.\npub struct EncodedBuilder {\n    /// Bits.\n    pub bits: u8,\n}\n\n"
            "impl EncodedBuilder {\n    /// One.\n    pub fn new() -> Self {\n"
            "        Self { bits: 0 }\n    }\n}\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("EncodedBuilder", problems[0])

    def test_a_unit_struct_is_not_reported(self):
        self.assertEqual(
            [],
            self.problems_for(
                "/// A marker.\npub struct Marker;\n\n"
                "impl Marker {\n    /// One.\n    pub fn new() -> Self {\n        Self\n    }\n}\n"
            ),
        )

    def test_a_tuple_struct_with_a_public_field_is_reported(self):
        problems = self.problems_for(
            "/// A sequence number.\npub struct RSeq(pub u32);\n\n"
            "impl RSeq {\n    /// One.\n    pub fn new(value: u32) -> Self {\n"
            "        Self(value)\n    }\n}\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("RSeq", problems[0])

    def test_a_tuple_struct_with_a_private_field_is_not_reported(self):
        self.assertEqual(
            [],
            self.problems_for(
                "/// A sequence number.\npub struct RSeq(u32);\n\n"
                "impl RSeq {\n    /// One.\n    pub fn new(value: u32) -> Self {\n"
                "        Self(value)\n    }\n}\n"
            ),
        )

    def test_generics_and_a_where_clause_do_not_hide_the_body(self):
        """`declaration` has to skip a parameter list to find the brace that opens the fields."""
        problems = self.problems_for(
            "/// A holder.\npub struct Holder<T>\nwhere\n    T: Clone,\n{\n"
            "    /// The value.\n    pub value: T,\n}\n\n"
            "impl<T: Clone> Holder<T> {\n    /// One.\n    pub fn new(value: T) -> Self {\n"
            "        Self { value }\n    }\n}\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("Holder", problems[0])

    def test_a_struct_in_an_unreachable_module_is_not_reported(self):
        """The reachability walk is shared with the enum rule, so it holds here too."""
        self.assertEqual(
            [],
            demo_crate(
                {"lib.rs": "//! Demo.\nmod hidden;\n", "hidden.rs": self.BREAKABLE},
                guard.struct_problems,
            ),
        )

    def test_a_struct_re_exported_out_of_a_private_module_is_reported(self):
        problems = demo_crate(
            {"lib.rs": "//! Demo.\nmod hidden;\npub use hidden::Encoded;\n", "hidden.rs": self.BREAKABLE},
            guard.struct_problems,
        )
        self.assertEqual(1, len(problems))
        self.assertIn("Encoded", problems[0])

    def problems_for(self, source: str) -> list[str]:
        return demo_crate({"lib.rs": source}, guard.struct_problems)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
