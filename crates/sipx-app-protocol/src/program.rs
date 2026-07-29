//! §6.3's pending program, as a type that cannot be appended to.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §6.3: a document
//! accepted in response to an event **replaces** the pending program rather than adding to it.
//! The interpreter could satisfy that by only ever assigning the queue wholesale — and it did,
//! for a while — but that is an invariant a reader has to verify by grepping for `push`, and one
//! a later edit can break silently.
//!
//! This module makes it a property of the type instead. [`Program`]'s queue is private **to this
//! file**, so no amount of editing `interpreter.rs` can add an instruction to a running program:
//! the only ways in are [`Program::replace`], which takes the whole thing, and
//! [`Program::abandon`], which empties it. There is deliberately no `push`, no `extend`, no
//! `append`, and no accessor handing out `&mut VecDeque`. Adding one is the only way to break
//! §6.3, and it has to happen here, in a file whose entire purpose is that rule.

use std::collections::VecDeque;

use crate::document::Instruction;

/// The instructions an app has queued for one call, and nothing else.
///
/// See the module header for why this is a newtype and not a `VecDeque` field.
#[derive(Debug, Default)]
pub(crate) struct Program {
    /// Private to this module. That privacy *is* §6.3's replacement rule.
    queue: VecDeque<Instruction>,
}

impl Program {
    /// §6.3's replacement: this document is now the whole program, and whatever was queued is
    /// gone. The only way instructions enter a `Program`.
    pub(crate) fn replace(&mut self, instructions: Vec<Instruction>) {
        self.queue = instructions.into();
    }

    /// Discard the queue without putting anything in its place — the call is being torn down on
    /// the host's own initiative and there is nothing left to run.
    pub(crate) fn abandon(&mut self) {
        self.queue.clear();
    }

    /// Take the next instruction to run (§6.1: strictly in order).
    pub(crate) fn take_next(&mut self) -> Option<Instruction> {
        self.queue.pop_front()
    }

    /// How many instructions are still queued.
    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Program;
    use crate::document::{Instruction, Verb};

    fn program(ids: &[&str]) -> Vec<Instruction> {
        ids.iter()
            .map(|id| Instruction::new(*id, Verb::Answer))
            .collect()
    }

    /// The rule §6.3 states, at the level of the type that holds the queue.
    #[test]
    fn replacing_discards_whatever_was_queued() {
        let mut queue = Program::default();
        queue.replace(program(&["a", "b", "c"]));
        assert_eq!(queue.len(), 3);

        queue.replace(program(&["z"]));
        assert_eq!(queue.len(), 1, "a replacement is not an append");
        assert_eq!(queue.take_next().map(|i| i.id), Some("z".to_owned()));
        assert_eq!(
            queue.take_next(),
            None,
            "nothing of the first program is left"
        );
    }

    #[test]
    fn instructions_come_back_in_the_order_they_went_in() {
        let mut queue = Program::default();
        queue.replace(program(&["one", "two"]));
        assert_eq!(queue.take_next().map(|i| i.id), Some("one".to_owned()));
        assert_eq!(queue.take_next().map(|i| i.id), Some("two".to_owned()));
        assert_eq!(queue.take_next(), None);
    }

    #[test]
    fn abandoning_leaves_nothing_to_run() {
        let mut queue = Program::default();
        queue.replace(program(&["a", "b"]));
        queue.abandon();
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.take_next(), None);
    }
}
