---
id: X-131
title: Help prose cannot name another command's flag
pillar: Quality
status: backlog
priority: 5
design:
epic:
areas: [sipx-cli, docs]
predicate:
announcement:
note: check-cli-reference reads --flag tokens out of a command's whole help text, so a cross-reference in prose reads as an undocumented option
---

# Help prose cannot name another command's flag

## Goal

Let one command's help text refer to another command's flag by name, without
`check-cli-reference.py` reporting that flag as an undocumented option of the command whose help
merely mentioned it.

## Acceptance

- [ ] `check-cli-reference.py` derives a command's option set from the option entries of its help,
      not from every `--token` appearing anywhere in the text.
- [ ] A test or fixture proves the narrowed reader still catches the case the wide one exists for:
      a real option present in the executable help and absent from the public page.
- [ ] `sipx load-responder`'s `--max-active` guidance names the generator's flag as `--concurrency`
      again, since that is the spelling an operator types.
- [ ] `./scripts/gate.py` green.

## Notes

Found while writing `X-129`, which needed the responder's ceiling guidance to point at the
generator's concurrency flag. The first draft said "give this ceiling headroom over the generator's
`--concurrency`" in the doc comment on `LoadResponderOptions::max_active`, and the checker reported:

```
cli reference: load-responder: executable option `--concurrency` is not documented
```

`help_options` in `scripts/check-cli-reference.py` matches `--[a-z][a-z0-9-]*` over the entire
`--help` output, so a flag named in a description becomes a flag of that command. `X-129` worked
around it by writing "the generator's concurrency" in prose and spelling the flag only on the
public page, which is a smaller thing to say than the sentence it wanted.

Not urgent, and not a defect in what the checker is *for*: it exists so an option cannot ship
undocumented, and it does that. Narrowing the reader has to keep that property, which is why the
second row above is on this story and not optional — a parse that reads only option entries is
easy to write and easy to make too permissive.
