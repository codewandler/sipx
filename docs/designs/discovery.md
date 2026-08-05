# Design: Endpoint discovery

**Status:** outline · **Pillar:** Phone · **Epic:** `discovery` · **Stories:** `P-5`, `P-6`, `S-24`, `T-24`

## Why

sipx can call any endpoint you can already name, and cannot help you name one. `sipx dial` takes a
URI; `sipx register` binds you to a registrar. Neither answers the first question a person actually
has, which is **"who is there to call?"** — and the second, which is "let me call them without
retyping `sip:alice@192.0.2.17:5060`."

This is the front-door gap. Vision principle 6 says a feature that cannot be asserted on from a
script is not finished, and today a script that wants to place a call must be handed a URI from
somewhere outside sipx entirely. The epic closes that: one command that lists what can be called,
and a `dial` that accepts what the list prints.

**What this epic is not.** The vision's non-goals are explicit that sipx is "a library and a phone",
and that "routing engines and dial plans are things you build *with* it". Discovery stops at
*naming*: it tells you who exists and lets you call one of them. It never decides, on a call's
behalf, where that call should go — the moment a lookup is consulted while routing someone else's
INVITE, that is a dial plan and it belongs in an application, not here.

**RFC 3263 is not this.** `Locating SIP Servers` is implemented and does the last mile of a call you
have already decided to place: URI in, transport and host out. That is *resolution*. This epic is
*enumeration* — producing the set of URIs in the first place. They compose (a discovered peer is
dialled through 3263 like any other), and conflating them is the likeliest way to file a duplicate
story.

## Approach

Three sources of truth, each answering a genuinely different question, unified behind one command
and one name-resolution step. They are independent: any one is useful alone, and the epic is
sequenced so the cheapest lands first.

| Source | Answers | RFC | Today |
|---|---|---|---|
| **The peer book** | "What has this phone been told about?" | none | absent |
| **A registrar** | "Who is registered at this domain?" | 3680 | live UAC; automatic gap recovery remains |
| **The local link** | "Who is on this network right now?" | 6762 + 6763 | absent |

1. **`sipx peers` (`P-5`)** — one command, one merged list, machine-readable in the form `P-1`
   established, with each entry saying **which source it came from and how stale it is**. A peer
   learned from a multicast response thirty seconds ago and one typed into a config file are not
   the same kind of fact, and a list that flattens them invites a user to trust the wrong one.
2. **Dial a peer by name (`P-6`)** — `sipx dial alice` where `alice` came from that list. The
   resolution step is a *lookup then a normal dial*, not a new dial path, so everything `P-3`
   already proved about placing a call keeps holding.
3. **Learn who is registered (`S-24`)** — RFC 3680's `reg` event package, which sipx already parses
   at `partial`. This is the case with real infrastructure: subscribe at the registrar, receive the
   registered contacts, keep them current from the NOTIFYs.
4. **Discover on the local link (`T-24`)** — mDNS/DNS-SD `_sip._udp.local`. The no-infrastructure
   case, and the one story in the epic that needs a scope decision before it is `ready` (below).

The peer book comes first deliberately. It needs no protocol work, it is what makes `P-6`
expressible, and it is the fallback the other two degrade into when a registrar refuses a
subscription or a network blocks multicast.

## Alternatives considered

- **Presence as the listing mechanism (RFC 3856, `partial`; PIDF implemented).** Rejected as the
  *primary* source: presence answers "is Alice available", which presumes you already know Alice
  exists. It is a natural later enrichment of a list built some other way — a column, not the
  table — and folding it in now would make every entry cost a subscription.
- **A registrar of our own, so `sipx peers` lists its own bindings.** Rejected: sipx serving
  registrations for other endpoints is a PBX, which is an explicit non-goal. Subscribing to
  *someone else's* registrar is the same information without becoming the infrastructure.
- **Scanning a subnet for port 5060.** Rejected outright. It is indistinguishable from
  reconnaissance, it is wrong on any network with more than one broadcast domain, and mDNS is the
  answer the standards already give for the same question.
- **A `sipx contacts` file format of our own.** Deferred inside `P-5`, and settled there — see
  [Decided: the peer book](#decided-the-peer-book-p-5). The concern was that inventing a format
  before the other two sources exist risks a schema that fits only the case that needed no
  protocol; the answer was to make the file hold the two fields a peer cannot do without and put
  everything a source might add — `source` today, freshness later — in the *output* rather than in
  the file.

## Decided: the peer book (`P-5`)

The design left the book's location and format open on purpose; `P-5` closed it, and this is the
record.

**Format — one peer per line, `name` whitespace `uri`.** `#` starts a comment line, blank lines are
ignored, and a line is exactly two whitespace-separated fields:

```text
# who this phone knows about
alice   sip:alice@192.0.2.17:5060
bob     sips:bob@example.com
```

Chosen because principle 6 requires it be usable from a shell, and this is the only shape a shell
already has verbs for: `echo "alice sip:alice@host" >> "$book"` writes it and `while read -r name
uri` reads it. TOML, JSON and INI were all rejected on the same ground — none is appendable or
readable from a script without either a parser or a `sed` invocation that corrupts the file the
second time it runs, and none is worth a third-party dependency for two fields. sipx has written
its own codec rather than take a dependency before (`sipx-app-protocol`), and two fields is a much
smaller thing to write than that was.

A third field is refused rather than folded into the URI. A SIP URI contains no whitespace, so a
third field is a trailing comment or a typo, and taking the rest of the line would produce an entry
that is undiallable and says so nowhere until someone tries to call it. For the same reason a
malformed line fails the whole listing, naming the line: skipping it prints a list that is short by
one and never mentions it, which is the "partial list presented as complete" this design forbids.

**Location — `--book <FILE>`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
`$HOME/.config/sipx/peers`.** The flag/environment/default order is the one `sipx register` already
uses for a password: the flag is the convenience, the environment is what a script sets once, and
the XDG path is where a person's own book lives. With none of them available the command exits
`usage` and says which two knobs would fix it, rather than inventing a path.

**Output — one `Report` per peer, which is `P-1`'s convention applied to a list.** `--json` prints
one object per line, extending `P-1`'s "one line per result" the way `sipx answer` already prints
two lines for one call; the human form prints the same fields as aligned blocks separated by a
blank line. Using the same `Report` for both is what keeps "the two carry the same facts" true by
construction rather than by discipline — a bespoke table for the human form would be prettier and
would be the second output convention this repo does not want.

Every entry carries `status`, `name`, `uri` and `source`. `source` is `book` and is the extension
point: `S-24` adds `registrar` and `T-24` adds `local-link` to the same stream, and `status`
discriminates a peer line from the refusal line a registrar's "no" will need — a script selects
entries with `select(.status == "peer")` and is not broken by either.

**Not decided here: staleness.** A book entry has no age — it is as true as the last time someone
typed it — and inventing an `age` field for it would report a number that means nothing. The
freshness field belongs to the sources that have one, and a consumer must key it off `source`.

**A book that cannot be read is a non-zero typed error, never an empty list.** A missing file, an
unreadable one and a malformed line are all failures; only a book that exists, parses, and holds
no peers is an empty list with exit zero. An empty list on a fresh machine reads as "there is
nobody to call" when the truth is "you have not been told about anyone", and a script cannot tell
those apart after the fact.

## Risks & open questions

- **Does mDNS belong in sipx at all?** This is the epic's one real scope decision and `T-24` is
  blocked on it. It is a second protocol with its own parser eating unauthenticated multicast
  input, and it likely means a new dependency — against a vision that prizes "a smaller stack whose
  every path is tested". The case for it: it is the only source that works with no infrastructure,
  and it is what makes `sipx peers` interesting on a laptop. The case against: it is a lot of new
  attack surface for a convenience, and non-negotiable "no panics on network input" now applies to
  a whole new parser. **Decide before cutting the story loose**, and record the decision here the
  way `X-26` recorded G.722's.
- **Enumeration is a capability, not a courtesy.** Listing who is registered at a domain is
  something a registrar may reasonably refuse, and RFC 3680 subscriptions are authorized for that
  reason. sipx must surface a refusal as a refusal — never present a partial list as complete, and
  never imply that discovery routes around authorization.
- **Staleness is the failure mode users will actually hit.** A discovered peer that has since gone
  away produces a call that fails at INVITE time rather than at list time. Every entry carrying its
  source and age is the mitigation; a TTL that silently expires entries mid-script is not.
- ~~**Where does the peer book live?**~~ Settled by `P-5` — see [Decided: the peer book](#decided-the-peer-book-p-5)
  above. `--book`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
  `$HOME/.config/sipx/peers`, holding a line per peer.

## Acceptance / done

A shell script can run `sipx peers`, take a name out of the machine-readable output, pass it to
`sipx dial`, and place a call — with no URI written anywhere in the script. That is the epic's
end-to-end proof, and it is the same shape as the one `P-1` set for the CLI as a whole.
