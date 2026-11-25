//! What a run raised that ends it or fails it: `$assert` /
//! `$assert_continue` failures, `$finish` in the design, and the `finish` /
//! `fail` requests of components.
//!
//! The simulator owns one [`SimStatus`] and hands it to the statements it
//! evaluates, so every source lands in the same place.  A testbench learns
//! that the run has to stop from what was latched when the last edge had been
//! evaluated: a request from a comb settled later, by a waveform dump or a
//! testbench read, stops the run at the next edge, as it does when the comb is
//! settled only by that edge.  The failures recorded are all reported, the
//! comb being settled once more when the run ends.

use crate::output_buffer;

#[derive(Default)]
pub struct SimStatus {
    /// Set by every request that ends the run: a fatal assertion, `$finish`
    /// in the design, and a component's `finish` or `fail`.
    stop: bool,
    fatal: Option<String>,
    continues: Vec<String>,
    /// `stop` as of the last [`SimStatus::latch`].
    latched_stop: bool,
}

impl SimStatus {
    pub fn reset(&mut self) {
        *self = SimStatus::default();
    }

    /// A fatal assertion failure; the first one is kept.
    pub fn record_fatal(&mut self, msg: String) {
        if output_buffer::muted() {
            return;
        }
        if self.fatal.is_none() {
            self.fatal = Some(msg);
        }
        self.stop = true;
    }

    /// An `$assert_continue` failure: reported, but the run goes on.
    pub fn record_continue(&mut self, msg: String) {
        if output_buffer::muted() {
            return;
        }
        self.continues.push(msg);
    }

    /// `$finish` in the design.
    pub fn record_finish(&mut self) {
        if output_buffer::muted() {
            return;
        }
        self.stop = true;
    }

    /// A component asked to finish or reported a failure; the component
    /// keeps the details.
    pub fn request_stop(&mut self) {
        self.stop = true;
    }

    /// Takes in what has been raised so far; called once an edge has been
    /// evaluated.
    #[inline]
    pub fn latch(&mut self) {
        self.latched_stop = self.stop;
    }

    /// Whether the run has to stop at the end of the current cycle.
    #[inline]
    pub fn stop_requested(&self) -> bool {
        self.latched_stop
    }

    /// Whether a fatal assertion failure has been recorded, latched or not.
    pub fn has_fatal(&self) -> bool {
        self.fatal.is_some()
    }

    /// The failure to report: the fatal message, or else the
    /// `$assert_continue` messages joined.
    pub fn take_failure(&mut self) -> Option<String> {
        if let Some(msg) = self.fatal.take() {
            self.continues.clear();
            return Some(msg);
        }
        if self.continues.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.continues).join("\n"))
        }
    }
}
