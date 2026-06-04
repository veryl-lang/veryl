//! Resolve a constant partial-index reference (`arr[0]` against
//! `logic [N, M]`) into a base offset for inst-port and function-call
//! argument expansion.

/// Base index into `parent_dims`-shaped flat storage that `idx_vals`
/// selects, when the remaining un-indexed dims match `child_element_count`.
/// `None` if no such slice exists.
pub(crate) fn partial_index_base(
    parent_dims: &[Option<usize>],
    idx_vals: &[usize],
    child_element_count: usize,
    parent_element_count: usize,
) -> Option<usize> {
    if idx_vals.len() >= parent_dims.len() {
        return None;
    }
    if !(0..idx_vals.len()).all(|i| parent_dims[i].is_some()) {
        return None;
    }
    let mut strides = vec![1usize; parent_dims.len()];
    for i in (0..parent_dims.len().saturating_sub(1)).rev() {
        strides[i] = strides[i + 1] * parent_dims[i + 1].unwrap_or(1);
    }
    let base: usize = idx_vals
        .iter()
        .enumerate()
        .map(|(i, v)| v * strides[i])
        .sum();
    let remaining: usize = parent_dims
        .iter()
        .skip(idx_vals.len())
        .map(|d| d.unwrap_or(1))
        .product();
    if remaining != child_element_count || base + remaining > parent_element_count {
        return None;
    }
    Some(base)
}
