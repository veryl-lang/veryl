//! Thread-local queue table backing the `$tb::queue` testbench handles.
//!
//! Like `random_table`, queues are keyed by the declaring variable's name
//! (`StrId`) and cleared before each test.

use crate::ir::Value;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use veryl_analyzer::ir::QueueOp;
use veryl_parser::resource_table::StrId;

/// Element type of a queue.
#[derive(Clone, Copy, Debug)]
pub struct Element {
    pub width: u32,
    pub signed: bool,
    pub four_state: bool,
}

impl Element {
    /// Value returned when reading an empty queue: X for a 4-state element,
    /// 0 for a 2-state element.
    fn default_value(&self) -> Value {
        if self.four_state {
            Value::new_x(self.width as usize, self.signed)
        } else {
            Value::new(0, self.width as usize, self.signed)
        }
    }

    /// Adjusts `value` to the element width, extending by its own signedness.
    fn fit(&self, value: Value) -> Value {
        let width = self.width as usize;
        let use_sign = value.signed();
        let mut value = value.expand(width, use_sign).into_owned();
        value.trunc(width);
        value.set_signed(self.signed);
        value
    }
}

thread_local! {
    static TABLE: RefCell<HashMap<StrId, VecDeque<Value>>> = RefCell::new(HashMap::new());
}

/// Clear all queues. Call before a test.
pub fn reset() {
    TABLE.with(|t| t.borrow_mut().clear());
}

/// Executes `op` on queue `key`. `value` is the pushed value of
/// `push_back`/`push_front`, and `capacity` is the bound (`0` for unbounded).
/// Returns the method's return value, if any.
pub fn exec(
    key: StrId,
    op: QueueOp,
    value: Option<Value>,
    elem: Element,
    capacity: u32,
) -> Option<Value> {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let queue = t.entry(key).or_default();
        let is_full = capacity != 0 && queue.len() >= capacity as usize;
        match op {
            QueueOp::PushBack | QueueOp::PushFront => {
                if !is_full && let Some(value) = value {
                    let value = elem.fit(value);
                    if op == QueueOp::PushBack {
                        queue.push_back(value);
                    } else {
                        queue.push_front(value);
                    }
                }
                None
            }
            QueueOp::PopFront => Some(queue.pop_front().unwrap_or_else(|| elem.default_value())),
            QueueOp::PopBack => Some(queue.pop_back().unwrap_or_else(|| elem.default_value())),
            QueueOp::DropFront => {
                queue.pop_front();
                None
            }
            QueueOp::DropBack => {
                queue.pop_back();
                None
            }
            QueueOp::Front => Some(
                queue
                    .front()
                    .cloned()
                    .unwrap_or_else(|| elem.default_value()),
            ),
            QueueOp::Back => Some(
                queue
                    .back()
                    .cloned()
                    .unwrap_or_else(|| elem.default_value()),
            ),
            QueueOp::Clear => {
                queue.clear();
                None
            }
            QueueOp::Size => Some(Value::new(queue.len() as u64, 32, false)),
            QueueOp::IsEmpty => Some(Value::new(queue.is_empty() as u64, 1, false)),
            QueueOp::IsFull => Some(Value::new(is_full as u64, 1, false)),
        }
    })
}
