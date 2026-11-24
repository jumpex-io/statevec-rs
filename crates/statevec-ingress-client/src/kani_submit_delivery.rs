//! Production-bound proofs of `SubmitDelivery`, over full-width identities.
//! The transition table and history relation are specified in
//! `docs/cluster-next/plans_v1/formal-verification-feasibility.md`.
//!
//! Induction over the history of one submitted intent:
//! - Initialization: `admission_selects_the_initial_state` establishes the
//!   exposure relation for both states `for_admission` selects.
//! - Preservation: `exposure_relation_is_preserved_by_every_method` starts from
//!   any state and exposure satisfying the relation, then applies one arbitrary
//!   method to arbitrary identities. Its only `kani::assume` is that hypothesis;
//!   `NonZeroU64` excludes a zero request ID by representation, not by assumption.
//! - So the relation holds after every finite sequence of these methods. No retry
//!   count or history length is bounded, and these methods have no loops to unwind.
//!
//! Consequences: `Unsent` means nothing was exposed, and `Sole(h)` means at most
//! `h` was, which is what `first_reply` relies on. The table harnesses fix every
//! transition, including that `Uncertain` is never left.
//!
//! Trusted here, not proved:
//! - The owner calls `dispatch` for every possibly exposed submission (a reconcile
//!   write carries none), calls `unwritten` only for a zero-write observation from
//!   the physical port, writes `delivery` only through `for_admission` and these
//!   methods, and allocates IDs. Terminal consumption ends a history. Consumer tests
//!   check examples of these, not all of them.
//! - The physical port's zero-write observations are truthful.
//! - The application uses `try_submit` only for an intent never exposed before; an
//!   uncertain original goes through `try_import`, as the client user guide requires.
//!   `Unsent` means "nothing exposed" only under that contract.
//! - These methods depend only on `self` and their arguments. A harness applies one
//!   step, so state hidden elsewhere would not reach the induction.
//! - The proofs compile on Kani's pinned nightly, while shipping builds use `stable`.

use super::*;

type Pair = (u64, NonZeroU64);

// A read-only view for before/after comparison. The production type gains no
// `Clone`, `Copy`, `Eq` or `Hash` for verification.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Observed {
    Unsent,
    Pending(Pair),
    Sole(Pair),
    Uncertain,
}

fn observe(state: &SubmitDelivery) -> Observed {
    match state {
        SubmitDelivery::Unsent => Observed::Unsent,
        SubmitDelivery::Pending { operation_id, request_id } => Observed::Pending((*operation_id, *request_id)),
        SubmitDelivery::Sole { operation_id, request_id } => Observed::Sole((*operation_id, *request_id)),
        SubmitDelivery::Uncertain => Observed::Uncertain,
    }
}

fn any_pair() -> Pair {
    (kani::any(), kani::any())
}

fn any_state() -> SubmitDelivery {
    match kani::any::<u8>() {
        0 => SubmitDelivery::Unsent,
        1 => SubmitDelivery::Pending { operation_id: kani::any(), request_id: kani::any() },
        2 => SubmitDelivery::Sole { operation_id: kani::any(), request_id: kani::any() },
        _ => SubmitDelivery::Uncertain,
    }
}

/// What this intent's history may have exposed. Harness-local, never production
/// state: it defines the claim and is not a model of the transitions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Exposure {
    None,
    One(Pair),
    /// Several submissions, or an imported history of unknown length.
    Many,
}

impl Exposure {
    fn any() -> Self {
        match kani::any::<u8>() {
            0 => Self::None,
            1 => Self::One(any_pair()),
            _ => Self::Many,
        }
    }

    fn after_dispatch(self, input: Pair) -> Self {
        match self {
            Self::None => Self::One(input),
            Self::One(_) | Self::Many => Self::Many,
        }
    }

    /// A truthful zero-write observation retracts only that exact submission.
    fn after_zero_write(self, input: Pair) -> Self {
        if self == Self::One(input) { Self::None } else { self }
    }
}

fn exposure_relation_holds(state: &SubmitDelivery, exposure: Exposure) -> bool {
    match observe(state) {
        Observed::Unsent => exposure == Exposure::None,
        Observed::Pending(held) | Observed::Sole(held) => exposure == Exposure::None || exposure == Exposure::One(held),
        Observed::Uncertain => true,
    }
}

#[kani::proof]
fn admission_selects_the_initial_state() {
    let transmissions: u8 = kani::any();
    let state = SubmitDelivery::for_admission(transmissions);
    let (expected, exposure) =
        if transmissions == 0 { (Observed::Unsent, Exposure::None) } else { (Observed::Uncertain, Exposure::Many) };
    assert!(observe(&state) == expected, "only a fresh intent starts Unsent");
    assert!(exposure_relation_holds(&state, exposure), "the relation holds at admission");
    kani::cover!(transmissions == 0, "fresh");
    kani::cover!(transmissions == 2, "import");
}

#[kani::proof]
fn dispatch_stores_the_pair_only_from_unsent() {
    let mut state = any_state();
    let before = observe(&state);
    let input = any_pair();
    state.dispatch(input.0, input.1);
    let after = observe(&state);
    match before {
        Observed::Unsent => assert!(after == Observed::Pending(input), "first dispatch retains the exact pair"),
        _ => assert!(after == Observed::Uncertain, "any other dispatch is uncertain"),
    }
    kani::cover!(matches!(before, Observed::Unsent), "unsent");
    kani::cover!(matches!(before, Observed::Pending(_)), "pending");
    kani::cover!(matches!(before, Observed::Sole(_)), "sole");
    kani::cover!(matches!(before, Observed::Uncertain), "uncertain");
}

#[kani::proof]
fn returned_moves_only_an_exact_pending_to_sole() {
    let mut state = any_state();
    let before = observe(&state);
    let input = any_pair();
    state.returned(input.0, input.1);
    let after = observe(&state);
    match before {
        Observed::Pending(held) if held == input => {
            assert!(after == Observed::Sole(held), "exact returned retains the pair in Sole");
        }
        _ => assert!(after == before, "any other returned preserves state and pair"),
    }
    kani::cover!(matches!(before, Observed::Unsent), "unsent");
    kani::cover!(matches!(before, Observed::Pending(held) if held == input), "exact pending");
    kani::cover!(
        matches!(before, Observed::Pending(held) if held.0 == input.0 && held.1 != input.1),
        "pending with another request"
    );
    kani::cover!(
        matches!(before, Observed::Pending(held) if held.0 != input.0 && held.1 == input.1),
        "pending with another operation"
    );
    kani::cover!(matches!(before, Observed::Sole(held) if held == input), "sole with its own pair");
    kani::cover!(matches!(before, Observed::Sole(held) if held != input), "sole with another pair");
    kani::cover!(matches!(before, Observed::Uncertain), "uncertain");
}

#[kani::proof]
fn unwritten_restores_only_an_exact_pending() {
    let mut state = any_state();
    let before = observe(&state);
    let input = any_pair();
    state.unwritten(input.0, input.1);
    let after = observe(&state);
    match before {
        Observed::Pending(held) if held == input => {
            assert!(after == Observed::Unsent, "exact unwritten restores Unsent");
        }
        _ => assert!(after == before, "any other unwritten preserves state and pair"),
    }
    kani::cover!(matches!(before, Observed::Unsent), "unsent");
    kani::cover!(matches!(before, Observed::Pending(held) if held == input), "exact pending");
    kani::cover!(
        matches!(before, Observed::Pending(held) if held.0 == input.0 && held.1 != input.1),
        "pending with another request"
    );
    kani::cover!(
        matches!(before, Observed::Pending(held) if held.0 != input.0 && held.1 == input.1),
        "pending with another operation"
    );
    kani::cover!(matches!(before, Observed::Sole(held) if held == input), "sole with its own pair");
    kani::cover!(matches!(before, Observed::Sole(held) if held != input), "sole with another pair");
    kani::cover!(matches!(before, Observed::Uncertain), "uncertain");
}

#[kani::proof]
fn exposure_relation_is_preserved_by_every_method() {
    let mut state = any_state();
    let before_exposure = Exposure::any();
    kani::assume(exposure_relation_holds(&state, before_exposure));
    let before = observe(&state);
    let input = any_pair();
    let exposure = match kani::any::<u8>() {
        0 => {
            state.dispatch(input.0, input.1);
            before_exposure.after_dispatch(input)
        }
        1 => {
            state.returned(input.0, input.1);
            before_exposure
        }
        _ => {
            state.unwritten(input.0, input.1);
            before_exposure.after_zero_write(input)
        }
    };
    assert!(exposure_relation_holds(&state, exposure), "every method preserves the relation");
    let after = observe(&state);
    kani::cover!(matches!(before, Observed::Unsent) && matches!(after, Observed::Pending(_)), "first dispatch");
    kani::cover!(matches!(before, Observed::Pending(_)) && matches!(after, Observed::Sole(_)), "exact return");
    kani::cover!(
        matches!(before, Observed::Pending(_))
            && matches!(after, Observed::Unsent)
            && before_exposure != Exposure::None,
        "zero write retracts the submission"
    );
    kani::cover!(
        matches!(after, Observed::Sole(_)) && before_exposure != Exposure::None && exposure == Exposure::None,
        "zero write after Sole clears exposure but keeps Sole"
    );
    kani::cover!(
        matches!(before_exposure, Exposure::One(_))
            && matches!(after, Observed::Uncertain)
            && exposure == Exposure::Many,
        "a second submission"
    );
}
