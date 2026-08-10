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
import re
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

        `M-83` retired the rollout boundary, so this now runs over every published crate. It
        failed against the widened tree naming a hundred enums, which is the population `M-74`
        held five crates' worth of.
        """
        self.assertEqual([], guard.enum_problems(guard.guarded(self.published)))

    def test_the_guard_reads_enums_out_of_every_crate_it_covers(self):
        """A blinded reader passes every rule that runs over it, and says so at exit 0.

        `X-116` put Rust's test-module attribute in a crate-root comment, and `code` truncates
        each file at the first one — that single comment made this reader see thirteen empty
        crates. A count is the cheapest assertion that cannot hold while the reader is blind.

        A binary crate is skipped and not excused: `reachable` returns nothing for one because it
        has no public API to name, so zero there is the right answer rather than a blind reader.
        Asserting over it would have made this fail the moment `M-83` widened the rule to
        `sipx-cli`, which is the opposite of what the assertion is for.
        """
        for name in guard.guarded(self.published):
            if guard.entry_point(name).name != "lib.rs":
                continue
            with self.subTest(crate=name):
                self.assertGreater(len(guard.reachable_enums(name)), 0)

    def test_the_enum_rule_holds_every_published_crate_but_the_one_with_a_reason(self):
        """`M-83`: the rollout boundary is gone, so the only crate outside is a stated decision.

        Before `M-83` this would have named the six crates `GUARDED_SURFACE` left out for the
        rollout's convenience. Afterwards there is exactly one, and it is out under `A-9` rather
        than because nobody had got to it yet.
        """
        self.assertEqual(
            sorted(set(self.published) - {guard.CLOSED_VOCABULARY}),
            sorted(guard.guarded(self.published)),
        )
        self.assertEqual([guard.CLOSED_VOCABULARY], guard.outside_the_rule(self.published))
        self.assertIn(guard.CLOSED_VOCABULARY, self.published)

    def test_the_excused_crates_enums_are_counted_rather_than_suppressed(self):
        """What `A-9` holds out of the rule is a number on every run, not a silence.

        The same assertion the struct debt gets below, for the same reason: an exclusion that
        reported zero would mean either that `sipx-app-protocol` had come into line — in which
        case `A-9` is now free to be retired — or that the reader had gone blind. Neither is
        something a run should be able to pass over without printing it.
        """
        self.assertGreater(len(guard.enum_problems(guard.outside_the_rule(self.published))), 0)

    def test_every_breakable_public_struct_is_non_exhaustive_or_argued_at_the_type(self):
        """`M-80`'s failing-first API assertion: additive fields must stay additive.

        Before `M-80` this reported six types, `Encoded` and `Packet` among them — the two whose
        field additions in `M-75` and `M-79` broke in-tree literals and are the reason the rule
        exists at all.

        `M-92` removed the type boundary that kept the other twenty-nine out of the population, so
        this failed against the tree it was widened over and names all of them. It now asserts both
        halves of the rule: marked-or-argued, and — for a marked type — buildable-or-argued.
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

    def test_every_relay_path_byte_buffer_is_redacted_or_argued_at_the_type(self):
        """`M-110`'s failing-first assertion, over the workspace rather than over a fixture.

        Before `M-110` this named six types: `Packet`, `Rtcp` and `SdesItem`, whose derived `Debug`
        rendered a call's payload and an RFC 3550 §6.5 identity, and the ICE and STUN carriers that
        now carry the argument `M-107` had recorded only in a commit message.
        """
        self.assertEqual(
            [], guard.byte_buffer_problems(guard.on_the_relay_path(self.published))
        )

    def test_the_guard_reads_byte_buffers_out_of_the_crates_it_covers(self):
        """The blinding assertion the enum and struct rules get, for the byte rule's selector."""
        self.assertEqual(
            [],
            guard.unread_relay_path(
                guard.byte_buffer_carriers(guard.on_the_relay_path(self.published))
            ),
        )

    def test_the_relay_path_scope_names_crates_that_exist(self):
        for name in guard.RELAY_PATH:
            with self.subTest(crate=name):
                self.assertIn(name, self.published)

    def test_every_at_rest_byte_buffer_is_redacted_or_argued_at_the_type(self):
        """`M-126`: the application host is a resting place, not an unreviewed remainder.

        Before the scope decision this reports `WssMessage`: its text variant can carry the
        realtime contract's base64 call audio, and its binary variant carries arbitrary peer
        bytes, while the derived `Debug` rendered either one whole.
        """
        self.assertIn("sipx-app", guard.AUDIO_AT_REST)
        self.assertEqual(
            [], guard.byte_buffer_problems(guard.where_the_call_rests(self.published))
        )

    def test_the_at_rest_scope_names_crates_that_exist(self):
        for name in guard.AUDIO_AT_REST:
            with self.subTest(crate=name):
                self.assertIn(name, self.published)

    def test_what_the_relay_path_scope_holds_out_is_counted(self):
        """A stated scope reporting zero would mean the workspace had one relay path and no SIP
        surface, or that the reader had gone blind. It has neither."""
        self.assertGreater(len(guard.outstanding_byte_buffers(self.published)), 0)

    def test_public_pages_name_every_checked_diagnostics_scope(self):
        """`X-145`: adding a protected crate cannot silently leave the public claim behind."""
        self.assertEqual([], guard.diagnostic_documentation_problems())

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
                # `M-65`'s nine built-in processors, grouped by what they do to a frame rather than
                # by which one they are: level (gain, polarity), shape (hard and soft clipping),
                # glitch (bit crushing, the stutter line), filter (the one-pole pair and the
                # peaking band), plus the arithmetic and frame plumbing they share. Deterministic
                # sample transforms — they load no model and claim no codec, which is why they
                # carry no entry in the codec table this file is otherwise about.
                "dsp::effects",
                "dsp::effects::arithmetic",
                "dsp::effects::filter",
                "dsp::effects::frame",
                "dsp::effects::glitch",
                "dsp::effects::level",
                "dsp::effects::shape",
                # `M-66`'s noise reduction: `dsp::noise` is the interface — a declaration naming
                # warm-up, activity input and host requirement, plus the trait a reducer
                # implements — and `dsp::noise::subband` is the one implementation this workspace
                # ships behind it, a three-band statistical suppressor built from `M-65`'s one-pole
                # section and integer arithmetic. Both belong here for the same reason the effects
                # do and one more: this is the module a reader would most expect to load a model,
                # and it loads none. It cannot — the processor contract gives it no socket, no file
                # and no device — so it carries no codec claim, no model claim and no accelerator
                # dependency, which is exactly what `HostRequirement::PortableInteger` declares.
                "dsp::noise",
                # `M-114`'s activity hint: the policy that turns drained analysis observations into
                # the `voice_active` parameter `dsp::noise`'s own declaration names. It decides
                # *when* to tell a reducer that speech is present and never decides whether it is —
                # every judgement is a comparison between caller-supplied positions and declared
                # counts, with no clock, no allocation after construction and no detector of its
                # own. So it loads no model and makes no claim about recognising anything.
                "dsp::noise::hint",
                "dsp::noise::subband",
                # `X-109`'s magnitude sweep, and the reason it is measurement rather than DSP: it
                # runs a processor against a shipped quarter-wave probe table and reports a ratio.
                # Every step is integer arithmetic, so the figure is identical on every machine and
                # under any load — which is what makes it evidence. It transforms no call audio,
                # loads no model and names no codec.
                "dsp::response",
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

    def test_a_struct_with_no_constructor_is_reported_too(self):
        """`M-92` removed the type boundary that used to make this shape quiet.

        A struct with public fields breaks a downstream literal whether or not the crate also
        publishes a `new`, so the rule that dropped this one out of scope excused twenty-nine
        types on the media surface. What the boundary was protecting is now
        `TheConstructorObligation` below.
        """
        problems = self.problems_for(self.BREAKABLE.partition("impl Encoded")[0])
        self.assertEqual(1, len(problems))
        self.assertIn("Encoded", problems[0])

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


class TheConstructorObligation(unittest.TestCase):
    """`M-92`: `#[non_exhaustive]` has a cost, and the rule charges it to the type that took it.

    `M-80` narrowed the struct rule to types that already published a `new`, because marking one
    that publishes nothing leaves a caller no way to build it at all. That hazard is real; using it
    to drop the type out of the rule was not, so it is asserted here as the second half of the
    rule. A marked struct must publish a way to build it or say why nothing outside builds one.
    """

    #: Marked, with public fields, and nothing to build it with. The shape the obligation is for.
    STRANDED = (
        "/// A payload.\n"
        "#[non_exhaustive]\n"
        "pub struct Encoded {\n"
        "    /// What it is encoded in.\n"
        "    pub payload_type: u8,\n"
        "}\n"
    )

    def test_a_marked_struct_with_no_way_to_build_it_is_reported(self):
        problems = self.problems_for(self.STRANDED)
        self.assertEqual(1, len(problems))
        self.assertIn("Encoded", problems[0])
        self.assertIn("publishes no constructor", problems[0])

    def test_a_new_satisfies_it(self):
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED + "\nimpl Encoded {\n    /// One.\n"
                "    pub fn new(payload_type: u8) -> Self {\n"
                "        Self { payload_type }\n    }\n}\n"
            ),
        )

    def test_a_constructor_under_another_name_satisfies_it(self):
        """`Sdes::cname` and `RemoteCandidate::signalled` are constructors; only `new` was not."""
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED + "\nimpl Encoded {\n    /// A silent one.\n"
                "    pub fn silence() -> Self {\n"
                "        Self { payload_type: 0 }\n    }\n}\n"
            ),
        )

    def test_a_fallible_constructor_satisfies_it(self):
        """Which of `Self`, `Option<Self>` and `Result<Self, E>` a type returns is its business."""
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED + "\nimpl Encoded {\n    /// Perhaps one.\n"
                "    pub fn parse(bytes: &[u8]) -> Option<Self> {\n"
                "        None\n    }\n}\n"
            ),
        )

    def test_a_parameter_type_with_parentheses_does_not_hide_the_return(self):
        """`RtcpQualityHook::new` takes an `impl Fn(..)`, which a first-`)` reader stops inside."""
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED + "\nimpl Encoded {\n    /// From a source.\n"
                "    pub fn from_source(source: impl Fn(u8) -> u8, seed: u8) -> Self {\n"
                "        Self { payload_type: source(seed) }\n    }\n}\n"
            ),
        )

    def test_a_method_is_not_a_constructor(self):
        """It needs a value of the type before it can be called, so it builds nobody one."""
        problems = self.problems_for(
            self.STRANDED + "\nimpl Encoded {\n    /// A copy.\n"
            "    pub fn clone_of(&self) -> Self {\n"
            "        Self { payload_type: self.payload_type }\n    }\n}\n"
        )
        self.assertEqual(1, len(problems))

    def test_a_derived_default_satisfies_it(self):
        """`T::default()` plus an assignment reaches every value the literal did.

        This is what keeps `MediaDiscardCounts` from needing a twenty-argument `new`.
        """
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED.replace(
                    "#[non_exhaustive]\n", "#[derive(Debug, Default)]\n#[non_exhaustive]\n"
                )
            ),
        )

    def test_a_hand_written_default_satisfies_it(self):
        """`ice::Timers`' default is the RFCs' recommended durations and cannot be derived."""
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED + "\nimpl Default for Encoded {\n"
                "    fn default() -> Self {\n        Self { payload_type: 8 }\n    }\n}\n"
            ),
        )

    def test_a_stated_reason_satisfies_it(self):
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED.replace(
                    "/// A payload.\n",
                    "/// A payload.\n///\n"
                    "/// Built by this crate only: a worker hands these out.\n",
                )
            ),
        )

    def test_the_complete_rationale_does_not_stand_in_for_it(self):
        """They are opposite claims: one argues for the attribute this type already carries."""
        problems = self.problems_for(
            self.STRANDED.replace(
                "/// A payload.\n",
                "/// A payload.\n///\n/// Complete by design: the wire carries these and no more.\n",
            )
        )
        self.assertEqual(1, len(problems))

    def test_an_unmarked_struct_is_not_asked_for_a_constructor(self):
        """The obligation follows the attribute. A type that argued its way out owes nothing."""
        self.assertEqual(
            [],
            self.problems_for(
                self.STRANDED.replace("#[non_exhaustive]\n", "").replace(
                    "/// A payload.\n",
                    "/// A payload.\n///\n/// Complete by design: the wire carries these and no more.\n",
                )
            ),
        )

    def test_a_marked_struct_with_no_public_field_owes_nothing(self):
        """The rule's population is unchanged: no literal, no breakage, no obligation."""
        self.assertEqual(
            [], self.problems_for(self.STRANDED.replace("    pub payload_type", "    payload_type"))
        )

    def problems_for(self, source: str) -> list[str]:
        return demo_crate({"lib.rs": source}, guard.struct_problems)


class ThePreambleReader(unittest.TestCase):
    """`M-96`: what is written immediately above an item, including where the file begins.

    `preamble` bounds the text at the blank line above the item and used to read `rfind`'s `-1` —
    *no blank line at all* — as if it were a match, adding 2 to it and starting the slice at byte 1.
    Every item in a file's first paragraph therefore lost the file's first character, so an opening
    `/// Exhaustive by design:` arrived as `// Exhaustive by design:` and matched nothing, and an
    opening `#[non_exhaustive]` arrived as `[non_exhaustive]`. The reader reported both types as
    unguarded while their guard sat one byte out of view.

    It cannot fire on this workspace, where every file opens with a `//!` module comment and no
    guarded item sits in the first paragraph, which is why the shape is asserted here rather than
    left for the next reader to notice. These are the blindness assertions of the two rules above,
    narrowed to the one position the reader could not see.
    """

    #: The rationale as the file's opening line, which is the byte the reader dropped.
    OPENING_ENUM = "/// Exhaustive by design: a stream flows one way or the other.\npub enum Flow { In, Out }\n"

    #: The same shape for the struct rule, whose rationale is spelled with the other phrase.
    OPENING_STRUCT = (
        "/// Complete by design: the wire carries these and no more.\n"
        "pub struct Encoded {\n"
        "    /// What it is encoded in.\n"
        "    pub payload_type: u8,\n"
        "}\n"
    )

    def test_a_rationale_on_a_file_s_first_line_argues_an_enum_out(self):
        self.assertEqual([], demo_crate({"lib.rs": self.OPENING_ENUM}))

    def test_an_attribute_on_a_file_s_first_line_guards_an_enum(self):
        """The mirror of the row above: the guard is an attribute rather than a sentence."""
        self.assertEqual(
            [],
            demo_crate(
                {"lib.rs": "#[non_exhaustive]\npub enum Flow { In, Out }\n"},
            ),
        )

    def test_a_rationale_on_a_file_s_first_line_argues_a_struct_out(self):
        self.assertEqual(
            [], demo_crate({"lib.rs": self.OPENING_STRUCT}, guard.struct_problems)
        )

    def test_no_blank_line_above_keeps_the_whole_preamble(self):
        self.assertEqual(
            "/// Exhaustive by design: a stream flows one way or the other.\n",
            guard.preamble(self.OPENING_ENUM, self.OPENING_ENUM.index("pub enum")),
        )

    def test_a_blank_line_at_the_very_top_still_bounds_the_preamble(self):
        """The two facts the arithmetic has to tell apart: `rfind` at `-1` and `rfind` at `0`.

        Here the blank line is a real match at index 0, so it still bounds the preamble and the
        text above it stays out — a fix that merely clamped the start to zero would drag it in.
        """
        source = "\n\n/// A failure.\npub enum Flow { In, Out }\n"
        self.assertEqual("/// A failure.\n", guard.preamble(source, source.index("pub enum")))

    def test_a_distant_reason_still_does_not_classify_a_first_paragraph_item(self):
        """Widening the slice must not reach past the blank line into another item's argument."""
        problems = demo_crate(
            {
                "lib.rs": "/// Exhaustive by design: this explains another item.\n"
                "pub const EARLIER: u8 = 1;\n\n"
                "/// Which way a stream flows.\npub enum Flow { In, Out }\n"
            }
        )
        self.assertEqual(1, len(problems))
        self.assertIn("Flow", problems[0])


class TheAttributeAndProseAboutIt(unittest.TestCase):
    """`M-97`: an attribute is syntax, and a doc comment naming one is prose about syntax.

    `preamble` hands both rules one string holding a type's attributes *and* its doc comment, and
    the marked test used to ask whether that string contained `#[non_exhaustive]`. A type whose
    documentation explained a decision by naming the attribute therefore satisfied the rule without
    carrying it — the one way past a check whose whole design is that an exception is written where
    a reader can grep for it. `M-83` wrote three such sentences in one diff, and `sipx-media`'s
    `ProviderKind` had been passing on prose alone since `M-74`.

    The reason phrases are held the same way, for the narrower half of the same question. They are
    `///` phrases and so genuinely prose, but a doc comment that *quotes* one while arguing about a
    different type used to classify this one; requiring the phrase to open a line of the preamble
    closes the quoting case. What no rule can close is a phrase written at the start of a line about
    somebody else's type, and that stays a reader's job.
    """

    #: An enum documented by naming the attribute and not carrying it — which is `ProviderKind`'s
    #: shape, and a sentence this workspace has good reason to write.
    NAMED_NOT_MARKED = (
        "/// Which way a stream flows.\n"
        "///\n"
        "/// Closed by design, and deliberately not `#[non_exhaustive]`: a stream has two ends.\n"
        "pub enum Flow { In, Out }\n"
    )

    #: The same shape for the struct rule, with the constructor that keeps the answer about the
    #: attribute rather than about `M-92`'s second obligation.
    NAMED_NOT_MARKED_STRUCT = (
        "/// A payload.\n"
        "///\n"
        "/// Deliberately not `#[non_exhaustive]`: the wire carries these and no more.\n"
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

    def test_a_doc_comment_naming_the_attribute_does_not_guard_an_enum(self):
        problems = demo_crate({"lib.rs": self.NAMED_NOT_MARKED})
        self.assertEqual(1, len(problems))
        self.assertIn("`Flow`", problems[0])

    def test_a_doc_comment_naming_the_attribute_does_not_guard_a_struct(self):
        problems = demo_crate({"lib.rs": self.NAMED_NOT_MARKED_STRUCT}, guard.struct_problems)
        self.assertEqual(1, len(problems))
        self.assertIn("`Encoded`", problems[0])
        self.assertIn(guard.COMPLETE_REASON, problems[0])

    def test_the_attribute_itself_still_guards_both_rules(self):
        """The narrowing has to leave the thing it narrowed to."""
        self.assertEqual(
            [], demo_crate({"lib.rs": self.NAMED_NOT_MARKED.replace("pub enum", "#[non_exhaustive]\npub enum")})
        )
        self.assertEqual(
            [],
            demo_crate(
                {
                    "lib.rs": self.NAMED_NOT_MARKED_STRUCT.replace(
                        "pub struct", "#[non_exhaustive]\npub struct"
                    )
                },
                guard.struct_problems,
            ),
        )

    def test_an_indented_attribute_is_still_the_attribute(self):
        """An item written inside an inline `mod` carries its attribute at that indentation."""
        self.assertTrue(guard.marked("    /// Flow.\n    #[non_exhaustive]\n"))

    def test_the_attribute_named_inside_a_doc_line_is_not_the_attribute(self):
        self.assertFalse(guard.marked("/// Unlike [`Side`], which is `#[non_exhaustive]`.\n"))

    def test_a_quoted_reason_phrase_does_not_argue_a_type_out(self):
        """The reason half: a phrase quoted mid-sentence is about the sentence, not this type."""
        problems = demo_crate(
            {
                "lib.rs": "/// Which way a stream flows.\n"
                "///\n"
                "/// See [`Side`], whose `/// Exhaustive by design:` note explains the pattern.\n"
                "pub enum Flow { In, Out }\n"
            }
        )
        self.assertEqual(1, len(problems))
        self.assertIn("`Flow`", problems[0])

    def test_a_reader_blind_to_the_attribute_reports_every_marked_type(self):
        """Why narrowing is safe to attempt: over-narrowing here is loud rather than quiet.

        `X-131` and `X-132` pinned the same property on the CLI-reference readers. A marked test
        that stopped recognising the attribute does not excuse a type — it reports every marked
        type in the workspace as unguarded, which is a red gate and not a silent pass.
        """
        marked = guard.marked
        guard.marked = lambda above: False
        try:
            enums = demo_crate(
                {"lib.rs": "/// Flow.\n#[non_exhaustive]\npub enum Flow { In, Out }\n"}
            )
            structs = demo_crate(
                {
                    "lib.rs": self.NAMED_NOT_MARKED_STRUCT.replace(
                        "pub struct", "#[non_exhaustive]\npub struct"
                    )
                },
                guard.struct_problems,
            )
        finally:
            guard.marked = marked
        self.assertEqual(1, len(enums))
        self.assertIn("`Flow`", enums[0])
        self.assertEqual(1, len(structs))
        self.assertIn("`Encoded`", structs[0])


#: A type holding call audio with nothing but the derive over it, which is the thing `M-61` found
#: on `AnalysisFrame` and `M-107` on `PcmFrame`.
LEAKING = "/// One frame of call audio.\n#[derive(Debug)]\npub struct Frame { samples: Vec<i16> }\n"

#: The redaction that answers the rule, written the way both fixes write it.
REDACTION = (
    "impl std::fmt::Debug for Frame {\n"
    "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
    '        f.debug_struct("Frame").field("samples", &self.samples.len()).finish()\n'
    "    }\n"
    "}\n"
)


class TheSampleBufferRule(unittest.TestCase):
    """`M-107`: a public type holding call audio implements `Debug` or says why its buffer is not.

    Two defects in two crates, one shape. `sipx_audio::AnalysisFrame`'s derived `Debug` rendered
    every one of the up-to-65,536 samples it borrowed and was reachable from a refusal record
    `sipx-call` wrote (`M-61`); `sipx_media::PcmFrame` had the same derive over an owned buffer and
    was latent only because nothing logged it yet. What a review caught twice is what this asks a
    checker for.

    The tests below are in three groups, because three separate things have to hold: the rule
    selects the shape a buffer is written in, it does not select the byte buffers that are not
    audio, and it fails loudly in both directions its readers can narrow.
    """

    def problems(self, source):
        return demo_crate({"lib.rs": source}, guard.sample_buffer_problems)

    # -- what it selects ------------------------------------------------------------------

    def test_a_type_holding_samples_with_only_the_derive_is_reported(self):
        problems = self.problems(LEAKING)
        self.assertEqual(1, len(problems))
        self.assertIn("`Frame`", problems[0])
        self.assertIn("PCM samples", problems[0])

    def test_every_shape_a_sample_buffer_is_written_in_is_one(self):
        """Owned, borrowed, boxed and fixed. A rule that read only `Vec<i16>` would have held
        `PcmSamples` and missed `AnalysisFrame`, `DspFrame` and G.722's twenty-four-sample delay
        line — three of the four types the workspace actually leaked from."""
        for field in (
            "samples: Vec<i16>",
            "samples: &'a [i16]",
            "samples: Box<[i16]>",
            "qmf_delay: [i16; 24]",
            "samples: Vec<f32>",
        ):
            with self.subTest(field=field):
                problems = self.problems(
                    f"/// One frame.\n#[derive(Debug)]\npub struct Frame<'a> {{ {field} }}\n"
                )
                self.assertEqual(1, len(problems), field)

    def test_an_enum_holding_samples_is_selected_as_a_struct_is(self):
        """`PcmSamples` is the enum every owned buffer in the workspace is made of, so a rule that
        read only structs would have missed the type all the others delegate to."""
        problems = self.problems(
            "/// Owned samples.\n#[derive(Debug)]\npub enum Samples { Signed16(Vec<i16>) }\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("`Samples`", problems[0])

    # -- what answers it, and what it deliberately does not select ------------------------

    def test_a_hand_written_debug_answers_the_rule(self):
        self.assertEqual([], self.problems(LEAKING.replace("#[derive(Debug)]\n", "") + REDACTION))

    def test_the_implementation_is_recognised_in_every_spelling_of_the_path(self):
        for opening in (
            "impl std::fmt::Debug for Frame {",
            "impl core::fmt::Debug for Frame {",
            "impl fmt::Debug for Frame {",
            "impl<'a> fmt::Debug for Frame<'a> {",
        ):
            with self.subTest(opening=opening):
                source = LEAKING.replace("#[derive(Debug)]\n", "") + opening + "\n}\n"
                self.assertEqual([], self.problems(source))

    def test_a_rationale_beside_the_type_answers_it_too(self):
        """The escape a coefficient table needs. It opens a doc line, like every other rationale
        this file accepts, so a phrase quoted mid-sentence does not classify a type."""
        argued = LEAKING.replace(
            "/// One frame of call audio.",
            "/// A window.\n///\n/// Not call audio: these are the Hann coefficients.",
        )
        self.assertEqual([], self.problems(argued))
        quoted = LEAKING.replace(
            "/// One frame of call audio.",
            "/// One frame. See [`Window`], whose `/// Not call audio:` note explains the escape.",
        )
        self.assertEqual(1, len(self.problems(quoted)))

    def test_a_byte_buffer_is_not_a_sample_buffer(self):
        """The stated limit, and the reason the rule can run over every published crate at once.

        `Vec<u8>` and `Bytes` are a G.711 payload here and a `Call-ID`, a SIP body and a STUN
        attribute three crates over. Selecting them would report the whole SIP surface, where the
        bytes *are* what a protocol log is for — so this rule holds element types and `M-110`'s
        holds a scope. `TheByteBufferRule` is the other half; the two must not learn each other's
        selectors, which is what this asserts from this side.
        """
        for field in ("payload: Vec<u8>", "payload: Bytes", "channels: &'static [u8]"):
            with self.subTest(field=field):
                self.assertEqual(
                    [],
                    self.problems(
                        f"/// A packet.\n#[derive(Debug)]\npub struct Frame {{ {field} }}\n"
                    ),
                )

    def test_a_buffer_named_in_prose_is_not_a_buffer(self):
        """`type_body` drops comments and string literals: a doc line explaining that a field used
        to be a `Vec<i16>`, or an `#[error]` message quoting one, is prose about a buffer."""
        self.assertEqual(
            [],
            self.problems(
                "/// A count.\n"
                "#[derive(Debug)]\n"
                "pub struct Frame {\n"
                "    /// How many samples there were, replacing the `Vec<i16>` this used to hold.\n"
                "    samples: usize,\n"
                "}\n"
            ),
        )

    def test_an_unbalanced_brace_in_a_variant_doc_does_not_stop_the_reader(self):
        """`sipx-testkit`'s `Malformed` documents a variant as ``/// `not json{` ``, and the struct
        rule's own body reader — which counts brace depth over the raw text — never closes it. The
        reader here drops comments first, so the type after that one is still read."""
        source = (
            "/// Frames that are not frames.\n"
            "#[derive(Debug)]\n"
            "pub enum Malformed {\n"
            "    /// `not json{` — a text frame that is not JSON at all.\n"
            "    NotJson,\n"
            "}\n"
            "\n" + LEAKING
        )
        problems = self.problems(source)
        self.assertEqual(1, len(problems))
        self.assertIn("`Frame`", problems[0])

    # -- how each reader fails when it narrows --------------------------------------------

    def test_a_reader_blind_to_the_implementation_reports_every_carrier(self):
        """Why the rule asks for an implementation instead of forbidding the derive.

        `M-97` made this correction to `marked` and `X-131`/`X-132` to the CLI-reference readers:
        the side of a rule that is *narrowed* has to be the side whose failure reports types. A
        reader that stopped recognising a hand-written `Debug` reports every redacted type in the
        workspace, which is a red gate; one that stopped recognising `#[derive(Debug)]` would have
        excused every leaking one at exit 0.
        """
        pattern = guard._IMPLEMENTS_DEBUG
        guard._IMPLEMENTS_DEBUG = r"(?!)"
        try:
            problems = self.problems(LEAKING.replace("#[derive(Debug)]\n", "") + REDACTION)
        finally:
            guard._IMPLEMENTS_DEBUG = pattern
        self.assertEqual(1, len(problems))
        self.assertIn("`Frame`", problems[0])

    def test_a_reader_blind_to_a_sample_buffer_fails_the_run_rather_than_passing_it(self):
        """The other direction, which no report of types can cover.

        A selector fails by selecting nothing, so narrowing `_SAMPLE_BUFFER` is the one change in
        this file that would be quiet. The population is therefore held to a floor: a reader that
        recognises almost none of a workspace built out of `i16` buffers says so.
        """
        buffer = guard._SAMPLE_BUFFER
        guard._SAMPLE_BUFFER = re.compile(r"(?!)")
        try:
            carriers = demo_crate({"lib.rs": LEAKING}, guard.sample_buffer_carriers)
        finally:
            guard._SAMPLE_BUFFER = buffer
        self.assertEqual([], carriers)
        self.assertEqual(1, len(guard.unreadable_surface(carriers)))
        self.assertIn("has narrowed", guard.unreadable_surface(carriers)[0])

    def test_a_workspace_that_still_reads_is_not_reported(self):
        """The floor has to leave the thing it was set above."""
        self.assertEqual(
            [], guard.unreadable_surface([("path", "Frame", 0)] * guard._PLAUSIBLE_CARRIERS)
        )

    def test_the_byte_rules_phrase_does_not_answer_this_one(self):
        """The two escapes are not interchangeable, asserted from the sample rule's side.

        "Not the call" is a weaker claim than "not call audio" is here: a Hann window is not the
        call *and* not call audio, but a rule that accepted the first would let a type answer the
        samples question with a sentence about bytes. `TheByteBufferRule` asserts the other
        direction, which is the one that matters more.
        """
        argued = LEAKING.replace(
            "/// One frame of call audio.",
            "/// A window.\n///\n/// Not the call: these are coefficients.",
        )
        self.assertEqual(1, len(self.problems(argued)))


#: `M-68`'s finding in the smallest shape that holds it: a public type whose derived `Debug`
#: renders the call's audio through a private field, holding no buffer of its own. `M-107`'s reader
#: passed the tree that contained this, which is what `M-121` is.
INDIRECT = (
    "/// One call direction's live graph.\n"
    "#[derive(Debug)]\n"
    "pub struct Graph { slot: Slot }\n"
    "\n"
    "/// What the graph is carrying.\n"
    "#[derive(Debug)]\n"
    "struct Slot { front: Vec<i16> }\n"
)

#: The redaction that answers the rule from the private carrier, which is where `M-68` wrote it.
SLOT_REDACTION = (
    "impl std::fmt::Debug for Slot {\n"
    "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
    '        f.debug_struct("Slot").field("carrying", &self.front.len()).finish()\n'
    "    }\n"
    "}\n"
)

#: The chain `M-68` found by hand, hop for hop, at the visibilities it was written in. `DspGraph`
#: holds a `SlotRef`, which holds an `Arc<Mutex<Slot>>`, which holds the live generation, which
#: holds the three buffers — the frame in flight among them.
FOUR_HOPS = (
    "/// One call direction's live DSP graph.\n"
    "#[derive(Debug)]\n"
    "pub struct DspGraph { slot: SlotRef }\n"
    "\n"
    "#[derive(Debug)]\n"
    "pub(crate) struct SlotRef { slot: Arc<Mutex<Slot>>, direction: AudioDirection }\n"
    "\n"
    "#[derive(Debug)]\n"
    "struct Slot { live: Option<Live>, generation: u64 }\n"
    "\n"
    "#[derive(Debug)]\n"
    "struct Live { buffers: Buffers, position: u64 }\n"
    "\n"
    "#[derive(Debug)]\n"
    "struct Buffers { front: Vec<i16>, back: Vec<i16>, scratch: Vec<i16> }\n"
)


def hops(count: int) -> str:
    """A public type whose derived `Debug` reaches a buffer `count` private fields away."""
    source = "/// The head of a chain.\n#[derive(Debug)]\npub struct Head { next: Hop0 }\n"
    for index in range(count - 1):
        source += f"\n#[derive(Debug)]\nstruct Hop{index} {{ next: Hop{index + 1} }}\n"
    return source + f"\n#[derive(Debug)]\nstruct Hop{count - 1} {{ samples: Vec<i16> }}\n"


class TheCarrierChase(unittest.TestCase):
    """`M-121`: the raw-audio rule follows a public type's fields into the crate's private ones.

    `M-107`'s reader looked at a public type's *own* fields, which was the right first cut and one
    hop short of the defect. `M-68` then found `sipx_media::DspGraph` rendering every sample of the
    frame in flight through four private hops — `DspGraph` holds a `SlotRef`, which holds an
    `Arc<Mutex<Slot>>`, which holds the live generation, which holds the buffers — and the checker
    passed the tree that contained it, because `DspGraph` holds no buffer of its own.

    Four things have to hold and each is a group below: the chase reaches a buffer however many
    private hops away it is written, a redaction anywhere on the chain answers it, the chase does
    not follow what a `Debug` would not render, and it says where it stopped looking.
    """

    def problems(self, source):
        return demo_crate({"lib.rs": source}, guard.sample_buffer_problems)

    # -- what it now reaches --------------------------------------------------------------

    def test_a_public_debug_over_a_private_carrier_is_reported(self):
        """`M-68`'s finding, as the Acceptance states it: one hop, and nothing else."""
        problems = self.problems(INDIRECT)
        self.assertEqual(1, len(problems))
        self.assertIn("`Graph`", problems[0])
        self.assertIn("PCM samples", problems[0])

    def test_the_four_hop_chain_m68_found_by_hand_is_reported(self):
        problems = self.problems(FOUR_HOPS)
        self.assertEqual(1, len(problems))
        self.assertIn("`DspGraph`", problems[0])

    def test_the_report_names_the_chain_and_the_type_the_redaction_belongs_on(self):
        """A rule that reported only the public type would send a reviewer to the one file where
        there is nothing to fix: the buffer is on `Buffers`, four hops down."""
        problem = self.problems(FOUR_HOPS)[0]
        self.assertIn("DspGraph -> SlotRef -> Slot -> Live -> Buffers", problem)
        self.assertIn("`Buffers`", problem)

    def test_a_private_carrier_in_another_module_is_reached(self):
        """`SlotRef` lives in `dsp::graph` and `DspGraph` in `dsp`; a reader confined to one file
        would have missed the first hop of the chain this rule exists for."""
        problems = demo_crate(
            {
                "lib.rs": "mod graph;\n\n/// A handle.\n#[derive(Debug)]\n"
                "pub struct Graph { slot: graph::SlotRef }\n",
                "graph.rs": "#[derive(Debug)]\npub(crate) struct SlotRef { front: Vec<i16> }\n",
            },
            guard.sample_buffer_problems,
        )
        self.assertEqual(1, len(problems))
        self.assertIn("Graph -> SlotRef", problems[0])

    def test_an_indirect_carrier_joins_the_population_the_floor_is_read_from(self):
        """The printed count and `unreadable_surface` both read this, so a rule that reported a
        chain without counting it would report a carrier the run says it never found.

        `Slot` is not in it and holds the buffer: the population is *reachable public* types, and
        a private carrier is what the chase walks through rather than something a caller can name.
        """
        carriers = demo_crate({"lib.rs": INDIRECT}, guard.sample_buffer_carriers)
        self.assertEqual(["Graph"], sorted(name for _path, name, _offset in carriers))

    # -- what answers it ------------------------------------------------------------------

    def test_a_hand_written_debug_on_the_public_type_answers_the_rule(self):
        """The Acceptance's other half. A type that writes its own rendering decides what it
        prints, which is the whole of what this rule asks anybody for."""
        redaction = (
            "impl std::fmt::Debug for Graph {\n"
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
            '        f.debug_struct("Graph").finish()\n'
            "    }\n"
            "}\n"
        )
        self.assertEqual([], self.problems(INDIRECT.replace("#[derive(Debug)]\npub", "pub") + redaction))

    def test_a_hand_written_debug_on_the_private_carrier_cuts_the_chain(self):
        """Where `M-68` actually wrote its fixes, and the reason the chase stops rather than
        reports: a derived `Debug` above a redacted one renders the redaction."""
        source = INDIRECT.replace("/// What the graph is carrying.\n#[derive(Debug)]\n", "")
        self.assertEqual([], self.problems(source + SLOT_REDACTION))

    def test_a_redaction_halfway_down_cuts_everything_behind_it(self):
        """`Live` renders shape, so `Slot` and `SlotRef` above it cannot render samples however
        many buffers `Buffers` holds."""
        source = FOUR_HOPS.replace("#[derive(Debug)]\nstruct Live", "struct Live") + (
            "impl std::fmt::Debug for Live {\n"
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
            '        f.debug_struct("Live").finish()\n'
            "    }\n"
            "}\n"
        )
        self.assertEqual([], self.problems(source))

    def test_a_private_carrier_can_argue_its_buffer_is_not_call_audio(self):
        """The coefficient table's escape, reachable from the same phrase at the same anchor
        wherever on the chain it is written."""
        argued = INDIRECT.replace(
            "/// What the graph is carrying.",
            "/// A window.\n///\n/// Not call audio: these are the Hann coefficients.",
        )
        self.assertEqual([], self.problems(argued))

    # -- what it deliberately does not follow ----------------------------------------------

    def test_a_container_that_does_not_render_what_it_holds_is_not_followed(self):
        """`sipx_media::MediaSession` holds an `mpsc::Sender<Frame>`, and a channel's `Debug`
        prints the channel. Following it would report a leak that cannot happen, and a rule that
        reports what cannot happen is switched off by whoever hits it second."""
        source = (
            "/// A session.\n#[derive(Debug)]\npub struct Session { outgoing: mpsc::Sender<Frame> }\n"
            "\n#[derive(Debug)]\nstruct Frame { samples: Vec<i16> }\n"
        )
        self.assertEqual([], self.problems(source))

    def test_a_container_that_does_render_what_it_holds_is_followed(self):
        """The other side of the same decision, and the shape `M-68` was written in: every one of
        `Arc`, `Mutex`, `Option` and `VecDeque` renders what it holds."""
        source = (
            "/// A session.\n#[derive(Debug)]\n"
            "pub struct Session { incoming: Arc<Mutex<Option<VecDeque<Frame>>>> }\n"
            "\n#[derive(Debug)]\nstruct Frame { samples: Vec<i16> }\n"
        )
        self.assertEqual(1, len(self.problems(source)))

    def test_a_name_declared_twice_resolves_to_the_one_beside_it(self):
        """`sipx-audio` declares three private `Band`s, and only G.722's holds a delay line. A
        reader that resolved a name across the whole crate would report the peaking filter for a
        buffer in a codec it has never heard of."""
        problems = demo_crate(
            {
                "lib.rs": "pub mod filter;\npub mod g722;\n",
                "filter.rs": "/// A peaking band.\n#[derive(Debug)]\n"
                "pub struct Peaking { bands: [Band; 2] }\n"
                "\n#[derive(Debug)]\nstruct Band { upper: i32, lower: i32 }\n",
                "g722.rs": "#[derive(Debug)]\nstruct Band { qmf_delay: [i16; 24] }\n",
            },
            guard.sample_buffer_problems,
        )
        self.assertEqual([], problems)

    def test_a_cycle_in_the_type_graph_terminates(self):
        """A graph is walked and not a tree: `sipx-media`'s `MediaSession` holds a
        `Mutex<Vec<MediaSession>>` of retired generations."""
        source = (
            "/// A ring.\n#[derive(Debug)]\npub struct Head { next: Node }\n"
            "\n#[derive(Debug)]\nstruct Node { back: Head, count: usize }\n"
        )
        self.assertEqual([], self.problems(source))

    # -- where it stops looking, and how each reader fails when it narrows ------------------

    def test_the_chase_reaches_the_stated_depth(self):
        problems = self.problems(hops(guard._CARRIER_HOPS))
        self.assertEqual(1, len(problems))
        self.assertIn("`Head`", problems[0])

    def test_a_carrier_past_the_stated_depth_is_not_reported(self):
        """The stated limit, asserted rather than described. A chain one hop longer than the
        depth is what the docstring says stays a reviewer's question."""
        self.assertEqual([], self.problems(hops(guard._CARRIER_HOPS + 1)))

    def test_a_test_only_item_does_not_hide_the_rest_of_its_file(self):
        """The reader cuts a file at its *test module*, which is what its own docstring claims.
        Cutting at the first `cfg(test)` anywhere cut `sipx-media`'s DSP graph at a test-only
        constructor 1,300 lines above the buffers, so every private type on `M-68`'s chain was
        invisible to the reader widened to chase it."""
        source = (
            "impl Graph {\n"
            f"    {TEST_ATTRIBUTE}\n"
            "    fn probe(&self) -> usize { 0 }\n"
            "}\n"
            "\n" + INDIRECT
        )
        self.assertEqual(1, len(self.problems(source)))

    def test_the_test_module_is_still_cut_off(self):
        """The half that has to keep holding: a fixture inside a test module is not the crate's."""
        self.assertEqual([], self.problems(f"{TEST_ATTRIBUTE}\nmod tests {{\n{INDIRECT}}}\n"))

    def test_a_reader_blind_to_the_implementation_reports_every_chain(self):
        """`sample_buffer_problems`' argument, carried down the chain: the side that is narrowed
        has to be the side whose failure reports types. A reader that stopped recognising a
        hand-written `Debug` reports every redacted carrier the chase reaches."""
        pattern = guard._IMPLEMENTS_DEBUG
        guard._IMPLEMENTS_DEBUG = r"(?!)"
        try:
            source = INDIRECT.replace("/// What the graph is carrying.\n#[derive(Debug)]\n", "")
            problems = self.problems(source + SLOT_REDACTION)
        finally:
            guard._IMPLEMENTS_DEBUG = pattern
        self.assertEqual(1, len(problems))
        self.assertIn("`Graph`", problems[0])


#: A relay-path type holding octets with nothing but the derive: the shape `M-110` was filed for.
CARRYING = "/// One packet.\n#[derive(Debug)]\npub struct Packet { payload: Bytes }\n"

#: The redaction that answers the byte rule, written the way the fix writes it.
LENGTH_ONLY = (
    "impl std::fmt::Debug for Packet {\n"
    "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
    '        f.debug_struct("Packet").field("payload", &self.payload.len()).finish()\n'
    "    }\n"
    "}\n"
)


class TheByteBufferRule(unittest.TestCase):
    """`M-110`: on the relay path a public type holding octets redacts them or says why not.

    `M-107` stated that this could not be checked, and the statement was about `Bytes` in general
    and correct about `Bytes` in general: 58 reachable public types in this workspace hold a byte
    buffer, and for a `Call-ID`, a URI, a SIP body or a STUN attribute, rendering the octets is the
    entire purpose of the log. What that reasoning missed is that the 58 are spread across eleven
    crates. Narrow by **scope** rather than by element type and the same obligation becomes
    decidable: on the two crates the call's own bytes pass through as bytes there are eight
    carriers, and a person can decide all eight once and a checker can hold them afterwards.

    The tests are in four groups, because four separate things have to hold: the rule selects the
    shape a byte buffer is written in, the right sentence answers it and the wrong one does not,
    the scope is a scope rather than a suppression list, and every reader fails loudly when it
    narrows.
    """

    def problems(self, source):
        return demo_crate({"lib.rs": source}, guard.byte_buffer_problems)

    # -- what it selects ------------------------------------------------------------------

    def test_a_type_holding_octets_with_only_the_derive_is_reported(self):
        problems = self.problems(CARRYING)
        self.assertEqual(1, len(problems))
        self.assertIn("`Packet`", problems[0])
        # Both halves of the sentence, not the phrasing that happened to join them: the message was
        # reworded when the at-rest scope landed and this assertion went stale against wording
        # rather than against behaviour.
        self.assertIn("octets", problems[0])
        self.assertIn("buffer", problems[0])

    def test_every_shape_a_byte_buffer_is_written_in_is_one(self):
        """Owned, borrowed, boxed, fixed, and both `bytes` crate types.

        `Packet` and `Encoded` hold `Bytes`, the ICE agent holds `Vec<u8>`, and a rule that read
        only one of those would have held one end of the relay path and missed the other.
        """
        for field in (
            "payload: Bytes",
            "buffer: BytesMut",
            "payload: Vec<u8>",
            "payload: &'a [u8]",
            "payload: Box<[u8]>",
            "secret: [u8; 32]",
        ):
            with self.subTest(field=field):
                problems = self.problems(
                    f"/// One packet.\n#[derive(Debug)]\npub struct Packet<'a> {{ {field} }}\n"
                )
                self.assertEqual(1, len(problems), field)

    def test_an_enum_variant_holding_octets_is_selected_as_a_struct_is(self):
        """`Rtcp` keeps the body of a packet it does not model in a variant, so a rule that read
        only structs would have held both ends of the relay path and missed the middle."""
        problems = self.problems(
            "/// A control packet.\n"
            "#[derive(Debug)]\n"
            "pub enum Control { Other { payload: Bytes } }\n"
        )
        self.assertEqual(1, len(problems))
        self.assertIn("`Control`", problems[0])

    def test_a_sample_buffer_is_not_a_byte_buffer(self):
        """The two selectors stay apart. A `Vec<i16>` is the other rule's business and is held over
        every published crate; teaching this one to see it would report it twice and would make the
        scope look like the workspace."""
        self.assertEqual(
            [],
            self.problems(
                "/// One frame.\n#[derive(Debug)]\npub struct Frame { samples: Vec<i16> }\n"
            ),
        )

    # -- what answers it, and what deliberately does not ----------------------------------

    def test_a_hand_written_debug_answers_the_rule(self):
        self.assertEqual([], self.problems(CARRYING.replace("#[derive(Debug)]\n", "") + LENGTH_ONLY))

    def test_a_rationale_beside_the_type_answers_it_too(self):
        """The escape ICE and STUN need, and it opens a doc line like every other rationale here,
        so a phrase quoted mid-sentence does not classify a type."""
        argued = CARRYING.replace(
            "/// One packet.",
            "/// A check.\n///\n/// Not the call: these are STUN control bytes.",
        )
        self.assertEqual([], self.problems(argued))
        quoted = CARRYING.replace(
            "/// One packet.",
            "/// A packet. See [`Check`], whose `/// Not the call:` note explains the escape.",
        )
        self.assertEqual(1, len(self.problems(quoted)))

    def test_the_audio_phrase_does_not_answer_this_rule(self):
        """The `SdesItem` case, and the whole reason there is a fifth phrase (`M-110`).

        An RFC 3550 §6.5 item's bytes are a `CNAME`, a `NAME` or an `EMAIL` — a login name,
        somebody's real name, somebody's email address. "Not call audio" written over one is
        **true**, and a rule whose escape was that phrase would have been switched off truthfully
        by the one type on the relay path that carries personal data. A redaction check that a true
        sentence disables checks nothing, so the byte rule asks the question it actually has:
        neither the call nor a participant.
        """
        audio = CARRYING.replace(
            "/// One packet.",
            "/// An SDES item.\n///\n/// Not call audio: RFC 3550 §6.5 makes this a CNAME.",
        )
        problems = self.problems(audio)
        self.assertEqual(1, len(problems))
        self.assertIn("`Packet`", problems[0])

    # -- the scope, and what it holds out -------------------------------------------------

    def test_the_scope_is_the_relay_path_and_nothing_else(self):
        self.assertEqual(list(guard.RELAY_PATH), guard.on_the_relay_path(list(guard.RELAY_PATH)))
        self.assertEqual([], guard.on_the_relay_path(["sipx-sip", "sipx-call", "sipx-demo"]))

    def test_widening_the_struct_rollout_boundary_does_not_widen_this_scope(self):
        """`RELAY_PATH` and `MEDIA_SURFACE` name the same two crates and mean opposite things about
        their own future: one is a rollout boundary `M-83` expects to be paid down and widened, the
        other is a stated scope whose widening would report forty SIP headers. One constant used
        twice would have made the first change silently perform the second."""
        boundary = guard.MEDIA_SURFACE
        guard.MEDIA_SURFACE = (*boundary, "sipx-sip")
        try:
            self.assertNotIn("sipx-sip", guard.on_the_relay_path(["sipx-sip", *boundary]))
        finally:
            guard.MEDIA_SURFACE = boundary

    def test_a_carrier_outside_the_scope_is_counted_rather_than_reported(self):
        """A scope whose remainder nobody prints is a suppression list with a better name — the
        argument `outside_the_rule` and `outstanding_structs` each make. The count is also how the
        one real gap stays visible: `sipx_app_protocol::Source::Inline` documents itself as PCM and
        derives its `Debug`, and is `M-117`."""
        self.assertEqual(
            [],
            demo_crate(
                {"lib.rs": CARRYING},
                lambda crates: guard.byte_buffer_problems(guard.on_the_relay_path(crates)),
            ),
        )
        self.assertEqual(
            1, len(demo_crate({"lib.rs": CARRYING}, guard.outstanding_byte_buffers))
        )

    # -- how each reader fails when it narrows --------------------------------------------

    def test_a_reader_blind_to_the_implementation_reports_every_carrier(self):
        """`sample_buffer_problems`' argument, unchanged: the side of a rule that gets narrowed has
        to be the side whose failure reports types. Asking for the implementation means a reader
        that stops recognising one reports all eight carriers; forbidding the derive would have
        excused all eight at exit 0."""
        pattern = guard._IMPLEMENTS_DEBUG
        guard._IMPLEMENTS_DEBUG = r"(?!)"
        try:
            problems = self.problems(
                CARRYING.replace("#[derive(Debug)]\n", "") + LENGTH_ONLY
            )
        finally:
            guard._IMPLEMENTS_DEBUG = pattern
        self.assertEqual(1, len(problems))
        self.assertIn("`Packet`", problems[0])

    def test_a_reader_blind_to_a_byte_buffer_fails_the_run_rather_than_passing_it(self):
        """The selector is the one narrowing no report of types can cover, so the population it
        selected is held to a floor and a run that recognises almost none of the relay path says so
        instead of passing."""
        buffer = guard._BYTE_BUFFER
        guard._BYTE_BUFFER = re.compile(r"(?!)")
        try:
            carriers = demo_crate({"lib.rs": CARRYING}, guard.byte_buffer_carriers)
        finally:
            guard._BYTE_BUFFER = buffer
        self.assertEqual([], carriers)
        self.assertEqual(1, len(guard.unread_relay_path(carriers)))
        self.assertIn("has narrowed", guard.unread_relay_path(carriers)[0])

    def test_a_relay_path_that_still_reads_is_not_reported(self):
        """The floor has to leave the thing it was set above."""
        self.assertEqual(
            [],
            guard.unread_relay_path([("path", "Packet", 0)] * guard._PLAUSIBLE_RELAY_CARRIERS),
        )


class TheDiagnosticDocumentationRule(unittest.TestCase):
    """`X-145`: the public privacy claim follows the checker scopes that make it true."""

    PAGES = {
        pathlib.Path("privacy.md"): (
            "relay path `sipx-media` `sipx-rtp`; comes to rest "
            "`sipx-app-protocol` `sipx-app` `sipx-testkit`; scripts/check-audio-claims.py"
        ),
        pathlib.Path("logging.md"): "scripts/check-audio-claims.py",
    }

    def test_a_new_crate_in_either_scope_is_reported(self):
        for scope, relay, at_rest in (
            ("relay path", (*guard.RELAY_PATH, "sipx-new-relay"), guard.AUDIO_AT_REST),
            ("comes to rest", guard.RELAY_PATH, (*guard.AUDIO_AT_REST, "sipx-new-store")),
        ):
            with self.subTest(scope=scope):
                problems = guard.diagnostic_documentation_problems(relay, at_rest, self.PAGES)
                self.assertEqual(1, len(problems))
                self.assertIn(scope, problems[0])
                self.assertIn("appears on neither public page", problems[0])

    def test_a_reasoned_exemption_is_explicit_and_an_empty_one_is_not(self):
        relay = (*guard.RELAY_PATH, "sipx-internal")
        self.assertEqual(
            [],
            guard.diagnostic_documentation_problems(
                relay,
                guard.AUDIO_AT_REST,
                self.PAGES,
                (("relay path", "sipx-internal", "not part of the public API"),),
            ),
        )
        problems = guard.diagnostic_documentation_problems(
            relay,
            guard.AUDIO_AT_REST,
            self.PAGES,
            (("relay path", "sipx-internal", ""),),
        )
        self.assertEqual(1, len(problems))

    def test_both_pages_name_the_checker(self):
        pages = dict(self.PAGES)
        pages[pathlib.Path("logging.md")] = "no enforcement named here"
        problems = guard.diagnostic_documentation_problems(
            guard.RELAY_PATH, guard.AUDIO_AT_REST, pages
        )
        self.assertEqual(1, len(problems))
        self.assertIn("logging.md", problems[0])


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
