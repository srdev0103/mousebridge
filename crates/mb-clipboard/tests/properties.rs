//! Property tests for clipboard synchronisation.
//!
//! The example tests exercise the sequences we thought of. These run arbitrary
//! interleavings of copies on both machines and assert the one thing that must
//! never fail: **the exchange always settles**.
//!
//! A clipboard loop is not a subtle bug. It saturates the link, spins both
//! machines, and does not stop until something is restarted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_clipboard::{ClipboardContent, ClipboardSync, SyncDecision};
use proptest::prelude::*;

/// Which machine performs an action.
#[derive(Debug, Clone, Copy)]
enum Actor {
    A,
    B,
}

fn arb_actor() -> impl Strategy<Value = Actor> {
    prop_oneof![Just(Actor::A), Just(Actor::B)]
}

/// A small pool of values, so the same content genuinely recurs — which is where
/// the interesting cases are.
fn arb_content() -> impl Strategy<Value = ClipboardContent> {
    prop_oneof![
        Just("alpha"),
        Just("beta"),
        Just("gamma"),
        Just("a longer piece of text that a person might plausibly copy"),
    ]
    .prop_map(|text| ClipboardContent::text(text).expect("valid"))
}

/// Runs one copy to completion, returning how many messages crossed the wire.
///
/// Panics if the exchange does not settle, which is the failure being guarded
/// against.
fn settle(
    a: &mut ClipboardSync,
    b: &mut ClipboardSync,
    actor: Actor,
    content: &ClipboardContent,
) -> usize {
    let mut messages = 0;
    let mut pending = Some((actor, content.clone()));

    for _ in 0..64 {
        let Some((from, item)) = pending.take() else {
            return messages;
        };

        let (sender, receiver) = match from {
            Actor::A => (&mut *a, &mut *b),
            Actor::B => (&mut *b, &mut *a),
        };

        if sender.on_local_change(&item) != SyncDecision::Send {
            return messages;
        }
        messages += 1;

        // The receiver applies it, which changes its own clipboard, which its
        // own watcher reports as a local change.
        if receiver.on_remote_update(&item) == SyncDecision::Apply {
            pending = Some((
                match from {
                    Actor::A => Actor::B,
                    Actor::B => Actor::A,
                },
                item,
            ));
        }
    }
    panic!("clipboard synchronisation did not settle after {messages} messages");
}

proptest! {
    /// Any interleaving of copies on either machine settles.
    #[test]
    fn arbitrary_copying_always_settles(
        actions in prop::collection::vec((arb_actor(), arb_content()), 0..60)
    ) {
        let mut a = ClipboardSync::new();
        let mut b = ClipboardSync::new();

        for (actor, content) in &actions {
            let messages = settle(&mut a, &mut b, *actor, content);
            prop_assert!(
                messages <= 1,
                "a single copy crossed the wire {} times",
                messages
            );
        }
    }

    /// Every applied update is accounted for by a suppressed echo.
    ///
    /// If applications outran suppressions the loop would be growing, just
    /// slowly enough not to trip the bound above.
    #[test]
    fn applications_and_suppressions_stay_balanced(
        actions in prop::collection::vec((arb_actor(), arb_content()), 1..60)
    ) {
        let mut a = ClipboardSync::new();
        let mut b = ClipboardSync::new();

        for (actor, content) in &actions {
            settle(&mut a, &mut b, *actor, content);
        }

        let applied = a.stats().applied + b.stats().applied;
        let suppressed = a.stats().echoes_suppressed + b.stats().echoes_suppressed;
        prop_assert!(
            suppressed >= applied,
            "{} updates applied but only {} echoes suppressed",
            applied,
            suppressed
        );
    }

    /// Resetting never causes a loop.
    ///
    /// Reset clears the history a disconnect made meaningless. Clearing it at the
    /// wrong moment could plausibly re-open the loop.
    #[test]
    fn resetting_mid_exchange_still_settles(
        actions in prop::collection::vec((arb_actor(), arb_content()), 0..40),
        reset_at in 0usize..40,
    ) {
        let mut a = ClipboardSync::new();
        let mut b = ClipboardSync::new();

        for (index, (actor, content)) in actions.iter().enumerate() {
            if index == reset_at {
                a.reset();
                b.reset();
            }
            let messages = settle(&mut a, &mut b, *actor, content);
            prop_assert!(messages <= 1, "crossed {} times after a reset", messages);
        }
    }

    /// Content identity is stable and collision-free across the pool.
    #[test]
    fn distinct_content_never_shares_a_hash(a in arb_content(), b in arb_content()) {
        if a == b {
            prop_assert_eq!(a.hash(), b.hash());
        } else {
            prop_assert_ne!(a.hash(), b.hash());
        }
    }
}
