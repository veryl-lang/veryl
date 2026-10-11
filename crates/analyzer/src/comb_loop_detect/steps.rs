//! Analysis steps of one module.
//!
//! Deciding whether a guarded walk closes is NP-hard, and summaries of
//! instances and calls can grow exponentially with nesting. Every stage of a
//! module's analysis therefore takes its work from one counter: building the
//! procedures, importing summaries, closing loop recurrences, deciding cycles
//! and dividing them for reports. A stage that cannot take the steps it needs
//! stops as incomplete; it never widens a dependency to save work, so running
//! out can miss a loop but never reports one that does not exist.
//!
//! Procedures and cycle decisions are independent of their peers, so they
//! share the steps by [`Steps::share`]: one that runs out cannot starve the
//! others, and which loops are found does not depend on declaration order.
//!
//! Steps count work, not time, so a result does not depend on the machine.

#[cfg(test)]
use std::cell::Cell;

/// Steps one module may take.
pub(super) const STEP_LIMIT: usize = 1 << 23;

/// Steps each shared stage may take in its first round.
pub(crate) const FIRST_ALLOWANCE: usize = 1 << 10;

#[cfg(test)]
thread_local! {
    static LIMIT: Cell<usize> = const { Cell::new(STEP_LIMIT) };
    static TAKEN: Cell<usize> = const { Cell::new(0) };
}

/// Run `f` with modules limited to `limit` steps.
#[cfg(test)]
pub(crate) fn with_step_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(usize);
    impl Drop for Reset {
        fn drop(&mut self) {
            LIMIT.set(self.0);
        }
    }
    let _reset = Reset(LIMIT.replace(limit));
    f()
}

#[cfg(test)]
pub(crate) fn reset_steps_taken() {
    TAKEN.set(0);
}

/// Steps taken by every module since the last reset.
#[cfg(test)]
pub(crate) fn steps_taken() -> usize {
    TAKEN.get()
}

/// The steps that remain for one module. A stage takes them by `&mut`, so
/// nested work cannot spend the same steps twice.
pub(super) struct Steps {
    remaining: usize,
}

impl Steps {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        let limit = LIMIT.get();
        #[cfg(not(test))]
        let limit = STEP_LIMIT;
        Self { remaining: limit }
    }

    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }

    /// Take `count` steps, or none when fewer remain.
    pub(super) fn take(&mut self, count: usize) -> bool {
        let Some(remaining) = self.remaining.checked_sub(count) else {
            return false;
        };
        record(count);
        self.remaining = remaining;
        true
    }

    /// Run work that counts its steps down in a plain counter, starting from
    /// every remaining step, and keep what it leaves.
    pub(super) fn lend<T>(&mut self, f: impl FnOnce(&mut usize) -> T) -> T {
        let lent = self.remaining;
        let result = f(&mut self.remaining);
        self.remaining = self.remaining.min(lent);
        record(lent - self.remaining);
        result
    }

    /// Run `f` on at most `share` of the remaining steps. The others stay
    /// set aside for later stages, whatever `f` takes.
    pub(super) fn confine<T>(&mut self, share: usize, f: impl FnOnce(&mut Self) -> T) -> T {
        let mut confined = Self {
            remaining: share.min(self.remaining),
        };
        self.remaining -= confined.remaining;
        let result = f(&mut confined);
        self.remaining += confined.remaining;
        result
    }

    /// Run independent stages so that which of them complete does not
    /// depend on their order. Rounds run every pending stage on the same
    /// allowance, doubling it each round; a stage whose result says it ran
    /// out runs again in the next round. A stage that needs `n` steps wastes
    /// fewer than `n` before its allowance reaches `n`, so every stage
    /// completes, whatever the order, when the stages together need at most
    /// half of the steps they may take. They may take every remaining step,
    /// or half of them when `reserve` keeps the rest for later stages. A
    /// stage pending alone takes all of those at once, and stages still out
    /// of steps then keep their last result.
    pub(super) fn share<T>(
        &mut self,
        stages: usize,
        reserve: bool,
        mut run: impl FnMut(&mut Self, usize) -> T,
        ran_out: impl Fn(&T) -> bool,
    ) -> Vec<T> {
        // Set the reserve aside once, so neither the stages nor their
        // retries can reach it.
        let available = if reserve {
            self.remaining - self.remaining / 2
        } else {
            self.remaining
        };
        self.confine(available, |steps| {
            steps.share_all(stages, &mut run, ran_out)
        })
    }

    fn share_all<T>(
        &mut self,
        stages: usize,
        run: &mut impl FnMut(&mut Self, usize) -> T,
        ran_out: impl Fn(&T) -> bool,
    ) -> Vec<T> {
        let mut results = (0..stages).map(|_| None).collect::<Vec<Option<T>>>();
        let mut pending = (0..stages).collect::<Vec<_>>();
        let mut allowance = FIRST_ALLOWANCE;
        while !pending.is_empty() {
            let alone = pending.len() == 1;
            let mut next = Vec::new();
            let mut limited = alone;
            for &stage in &pending {
                let available = self.remaining;
                limited |= allowance >= available;
                let share = if alone {
                    available
                } else {
                    allowance.min(available)
                };
                let result = self.confine(share, |steps| run(steps, stage));
                if ran_out(&result) {
                    next.push(stage);
                }
                results[stage] = Some(result);
            }
            if limited && next.len() == pending.len() {
                break;
            }
            pending = next;
            allowance = allowance.saturating_mul(2);
        }
        results
            .into_iter()
            .map(|result| result.expect("every stage runs"))
            .collect()
    }
}

fn record(_count: usize) {
    #[cfg(test)]
    TAKEN.set(TAKEN.get().saturating_add(_count));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_stages_leave_the_reserve_untouched() {
        let mut steps = Steps { remaining: 6500 };
        // Each stage needs 1,500 steps, taken one at a time.
        let results = steps.share(
            2,
            true,
            |steps, _| (0..1500).all(|_| steps.take(1)),
            |done| !done,
        );
        assert_eq!(results.len(), 2);
        assert!(
            steps.remaining() >= 3250,
            "{} steps left",
            steps.remaining()
        );
    }
}
