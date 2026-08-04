# Design: supported test surfaces

**Status:** accepted · **Pillar:** Build · **Epic:** `test-surfaces` · **Stories:** X-75

## Why

The workspace has seeded links, virtual time and call fixtures, but downstream applications have no
supported package and guide for using them. A test facility available only to this repository is not
part of the library's usable surface, and it cannot be the foundation for later compatibility work.

## Approach

Publish one deliberately small in-process call harness whose time, bytes and loss are inputs. Keep it
silent unless the host installs tracing, compile its runnable example in CI, and inline that example
into the public guide. Decide the package boundary explicitly instead of exposing internal helpers by
accident. Cross-process benchmarking is a separate epic: it may consume this harness but must not
turn a deterministic library test into a wall-clock load generator.

## Exit

A downstream package can place and answer a socket-free call under deterministic time, inspect the
result, and follow a public runnable example; no library crate installs output globally; and the gate
compiles the exact example the guide presents.
