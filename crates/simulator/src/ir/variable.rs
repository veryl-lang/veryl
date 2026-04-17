use crate::HashMap;
use std::fmt;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::{Type, VarId, VarPath};
use veryl_analyzer::value::Value;
use veryl_parser::resource_table::StrId;

/// Typed variable offset that encodes buffer identity (FF or Comb).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarOffset {
    Ff(isize),
    Comb(isize),
}

impl VarOffset {
    #[inline]
    pub fn is_ff(&self) -> bool {
        matches!(self, VarOffset::Ff(_))
    }
    #[inline]
    pub fn raw(&self) -> isize {
        match self {
            VarOffset::Ff(o) | VarOffset::Comb(o) => *o,
        }
    }
    #[inline]
    pub fn adjust(&self, ff_delta: isize, comb_delta: isize) -> Self {
        match self {
            VarOffset::Ff(o) => VarOffset::Ff(o + ff_delta),
            VarOffset::Comb(o) => VarOffset::Comb(o + comb_delta),
        }
    }
    #[inline]
    pub fn new(is_ff: bool, offset: isize) -> Self {
        if is_ff {
            VarOffset::Ff(offset)
        } else {
            VarOffset::Comb(offset)
        }
    }
    #[inline]
    pub fn to_pair(&self) -> (bool, isize) {
        (self.is_ff(), self.raw())
    }
}

/// Returns native storage width in bytes: 4 for width <= 32, 8 for 33-64, 16 for 65-128,
/// and 8-byte aligned for >128.
pub fn native_bytes(width: usize) -> usize {
    if width <= 32 {
        4
    } else if width <= 64 {
        8
    } else if width <= 128 {
        16
    } else {
        width.div_ceil(64) * 8
    }
}

/// Returns the byte size of a single value slot (payload + optional mask_xz).
pub fn value_size(native_bytes: usize, use_4state: bool) -> usize {
    if use_4state {
        native_bytes * 2
    } else {
        native_bytes
    }
}

/// Read a native-width payload from a byte buffer pointer.
#[inline(always)]
pub fn read_payload(ptr: *const u8, nb: usize) -> u64 {
    unsafe {
        match nb {
            4 => (ptr as *const u32).read_unaligned() as u64,
            8 => (ptr as *const u64).read_unaligned(),
            _ => unreachable!("read_payload called with nb={}, expected 4 or 8", nb),
        }
    }
}

/// Read a 128-bit native-width payload from a byte buffer pointer.
#[inline(always)]
pub fn read_payload_128(ptr: *const u8) -> u128 {
    unsafe { (ptr as *const u128).read_unaligned() }
}

/// Write a native-width payload to a byte buffer pointer.
#[inline(always)]
pub fn write_payload(ptr: *mut u8, nb: usize, val: u64) {
    unsafe {
        match nb {
            4 => (ptr as *mut u32).write_unaligned(val as u32),
            8 => (ptr as *mut u64).write_unaligned(val),
            _ => unreachable!("write_payload called with nb={}, expected 4 or 8", nb),
        }
    }
}

/// Write a 128-bit native-width payload to a byte buffer pointer.
#[inline(always)]
pub fn write_payload_128(ptr: *mut u8, val: u128) {
    unsafe { (ptr as *mut u128).write_unaligned(val) }
}

/// Read a full Value from native byte storage.
///
/// # Safety
/// `ptr` must point to a valid buffer of at least `nb * (1 + use_4state as usize)` bytes.
pub unsafe fn read_native_value(
    ptr: *const u8,
    nb: usize,
    use_4state: bool,
    width: u32,
    signed: bool,
) -> Value {
    unsafe {
        if nb > 16 {
            let payload = std::slice::from_raw_parts(ptr, nb);
            let mask_xz_slice: &[u8];
            let zeros;
            if use_4state {
                mask_xz_slice = std::slice::from_raw_parts(ptr.add(nb), nb);
            } else {
                zeros = vec![0u8; nb];
                mask_xz_slice = &zeros;
            }
            Value::from_le_bytes(payload, mask_xz_slice, width as usize, signed)
        } else if nb == 16 {
            let payload = read_payload_128(ptr);
            let mask_xz = if use_4state {
                read_payload_128(ptr.add(nb))
            } else {
                0u128
            };
            Value::from_u128(payload, mask_xz, width as usize, signed)
        } else {
            let payload = read_payload(ptr, nb);
            let mask_xz = if use_4state {
                read_payload(ptr.add(nb), nb)
            } else {
                0
            };
            Value::U64(veryl_analyzer::value::ValueU64 {
                payload,
                mask_xz,
                width,
                signed,
            })
        }
    }
}

/// Write a Value into native byte storage.
///
/// # Safety
/// `ptr` must point to a valid buffer of at least `nb * (1 + use_4state as usize)` bytes.
pub unsafe fn write_native_value(ptr: *mut u8, nb: usize, use_4state: bool, val: &Value) {
    unsafe {
        if nb > 16 {
            let payload_buf = std::slice::from_raw_parts_mut(ptr, nb);
            val.write_payload_to_bytes(payload_buf);
            if use_4state {
                let mask_xz_buf = std::slice::from_raw_parts_mut(ptr.add(nb), nb);
                val.write_mask_xz_to_bytes(mask_xz_buf);
            }
        } else if nb == 16 {
            let payload = val.payload_u128();
            write_payload_128(ptr, payload);
            if use_4state {
                let mask_xz = val.mask_xz_u128();
                write_payload_128(ptr.add(nb), mask_xz);
            }
        } else {
            match val {
                Value::U64(v) => {
                    write_payload(ptr, nb, v.payload);
                    if use_4state {
                        write_payload(ptr.add(nb), nb, v.mask_xz);
                    }
                }
                Value::BigUint(_) => {
                    unreachable!("BigUint with nb < 16");
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub path: VarPath,
    pub r#type: Type,
    pub width: usize,
    pub native_bytes: usize,
    pub current_values: Vec<*mut u8>,
    pub next_values: Vec<*mut u8>,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, &ptr) in self.current_values.iter().enumerate() {
            let value = unsafe {
                read_native_value(ptr, self.native_bytes, false, self.width as u32, false)
            };
            ret.push_str(&format!("{}[{}] = {:x};\n", self.path, i, value));
        }

        ret.trim_end().fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct VariableElement {
    /// Native storage width in bytes (4 or 8)
    pub native_bytes: usize,
    /// Typed byte offset of current value from ff_buf[0] (Ff) or comb_buf[0] (Comb)
    pub current: VarOffset,
    /// byte offset of next value from ff_buf[0]; meaningful only when is_ff == true
    pub next_offset: isize,
}

impl VariableElement {
    #[inline]
    pub fn is_ff(&self) -> bool {
        self.current.is_ff()
    }
    #[inline]
    pub fn current_offset(&self) -> isize {
        self.current.raw()
    }
}

#[derive(Clone, Debug)]
pub struct VariableMeta {
    pub path: VarPath,
    pub r#type: Type,
    pub width: usize,
    pub native_bytes: usize,
    pub elements: Vec<VariableElement>,
    /// initial value for each element; used when instantiating
    pub initial_values: Vec<Value>,
}

impl VariableMeta {
    /// Returns (base_current_offset, base_next_offset, stride, is_ff) for dynamic indexing.
    pub fn dynamic_index_info(&self) -> Option<(isize, isize, isize, bool)> {
        let first = self.elements.first()?;
        let is_ff = first.is_ff();
        #[cfg(debug_assertions)]
        for (i, elem) in self.elements.iter().enumerate() {
            debug_assert_eq!(
                elem.is_ff(),
                is_ff,
                "dynamic_index_info: mixed FF/comb in array, elem[{}] is_ff={} but elem[0] is_ff={} (path={:?})",
                i,
                elem.is_ff(),
                is_ff,
                self.path,
            );
        }
        let stride = if self.elements.len() > 1 {
            self.elements[1].current_offset() - self.elements[0].current_offset()
        } else {
            // Single-element: compute stride from native_bytes
            // FF: [current vs][next vs] → stride = 2 * vs
            // Comb: [vs] → stride = vs
            // vs is already accounted for in the layout, but for single elements
            // we need to provide a sensible stride for bounds checking.
            // Use the offset gap between current and next as the half-stride for FF.
            if is_ff {
                (first.next_offset - first.current_offset()) * 2
            } else {
                // Cannot determine from single element; use value_size as stride
                // This only matters for dynamic index bounds, so any positive value works.
                first.native_bytes as isize
            }
        };
        Some((first.current_offset(), first.next_offset, stride, is_ff))
    }
}

/// Compute offset-based VariableMeta for each variable using native-width byte storage.
///
/// Returns `(variable_meta, ff_bytes, comb_bytes)`.
/// `ff_bytes` / `comb_bytes` are the number of bytes allocated by this module only
/// (not including the start offset).
/// Iterates variables sorted by VarId so the iteration order is deterministic
/// and matches the buffer allocation order in `fill_buffers`.
/// Returns (variables, ff_bytes, comb_bytes, comb_hot_bytes).
/// comb_hot_bytes is the size of the "hot" comb region before large arrays.
pub fn create_variable_meta(
    src: &HashMap<VarId, air::Variable>,
    ff_table: &air::FfTable,
    use_4state: bool,
    ff_start_bytes: isize,
    comb_start_bytes: isize,
) -> Option<(HashMap<VarId, VariableMeta>, usize, usize, usize)> {
    let mut ff_pos: isize = ff_start_bytes;
    let mut comb_pos: isize = comb_start_bytes;
    let mut comb_hot_end: isize = 0; // will be set after small vars allocated

    let mut src_sorted: Vec<_> = src.iter().collect();
    // Sort small variables first, large arrays last.
    // This puts hot comb signals (pipeline regs, control flags) at low
    // offsets in comb_values where they fit in L1 cache (~32KB).
    // Large arrays (e.g., testbench DRAM) are placed at high offsets
    // and accessed only via dynamic indexing (cache miss is unavoidable).
    src_sorted.sort_by_key(|(k, v)| (v.value.len() > 256, **k));

    let mut variables = HashMap::default();
    let mut seen_large = false;

    for (k, v) in src_sorted {
        // `string`-typed params/consts have no well-defined native byte
        // width; they are always comptime-inlined so no runtime storage
        // is needed.
        if matches!(v.kind, air::VarKind::Param | air::VarKind::Const)
            && v.r#type.kind == air::TypeKind::String
        {
            continue;
        }
        // Record comb_hot_end: position before first large array
        if !seen_large && v.value.len() > 256 {
            comb_hot_end = comb_pos - comb_start_bytes;
            seen_large = true;
        }
        let width = v.r#type.total_width()?;
        let nb = native_bytes(width);
        let vs = value_size(nb, use_4state);

        // For multi-element variables (arrays), all elements must have the
        // same FF/comb classification. DynamicVariable expressions assume
        // uniform stride in a single buffer; mixed placement is invalid.
        let any_ff = v
            .value
            .iter()
            .enumerate()
            .any(|(i, _)| ff_table.is_ff(v.id, i));
        let force_ff = any_ff && v.value.len() > 1;

        let mut elements = vec![];
        let mut initial_values = vec![];

        for (i, val) in v.value.iter().enumerate() {
            let mut val = val.clone();
            if !use_4state {
                val.clear_xz();
            }

            if force_ff || ff_table.is_ff(v.id, i) {
                let current_offset = ff_pos;
                let next_offset = ff_pos + vs as isize;
                elements.push(VariableElement {
                    native_bytes: nb,
                    current: VarOffset::Ff(current_offset),
                    next_offset,
                });
                ff_pos += (vs * 2) as isize; // current + next
            } else {
                let current_offset = comb_pos;
                elements.push(VariableElement {
                    native_bytes: nb,
                    current: VarOffset::Comb(current_offset),
                    next_offset: 0,
                });
                comb_pos += vs as isize;
            }

            initial_values.push(val);
        }

        let meta = VariableMeta {
            path: v.path.clone(),
            r#type: v.r#type.clone(),
            width,
            native_bytes: nb,
            elements,
            initial_values,
        };
        variables.insert(*k, meta);
    }

    #[cfg(debug_assertions)]
    {
        let ff_end = ff_pos;
        let comb_end = comb_pos;
        for meta in variables.values() {
            for elem in &meta.elements {
                let off = elem.current_offset();
                match elem.current {
                    VarOffset::Ff(_) => debug_assert!(
                        off >= ff_start_bytes && off < ff_end,
                        "FF offset {} out of range [{}, {})",
                        off,
                        ff_start_bytes,
                        ff_end
                    ),
                    VarOffset::Comb(_) => debug_assert!(
                        off >= comb_start_bytes && off < comb_end,
                        "Comb offset {} out of range [{}, {})",
                        off,
                        comb_start_bytes,
                        comb_end
                    ),
                }
            }
        }
    }

    // If no large arrays were seen, hot region = entire comb
    if !seen_large {
        comb_hot_end = comb_pos - comb_start_bytes;
    }

    Some((
        variables,
        (ff_pos - ff_start_bytes) as usize,
        (comb_pos - comb_start_bytes) as usize,
        comb_hot_end as usize,
    ))
}

/// Hierarchical variable metadata tree: each module has its own variable_meta
/// and a list of child modules.
#[derive(Clone, Debug)]
pub struct ModuleVariableMeta {
    pub name: StrId,
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub children: Vec<ModuleVariableMeta>,
}

/// Hierarchical variable tree with resolved pointers into the flat buffers.
#[derive(Clone, Debug)]
pub struct ModuleVariables {
    pub name: StrId,
    pub variables: HashMap<VarId, Variable>,
    pub children: Vec<ModuleVariables>,
}

impl fmt::Display for ModuleVariables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_indent(f, 0)
    }
}

impl ModuleVariables {
    fn fmt_with_indent(&self, f: &mut fmt::Formatter<'_>, indent: usize) -> fmt::Result {
        let prefix = "  ".repeat(indent);
        writeln!(f, "{}module {}:", prefix, self.name)?;
        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by(|a, b| a.0.cmp(b.0));
        for (_, x) in variables {
            for line in format!("{}", x).lines() {
                writeln!(f, "{}  {}", prefix, line)?;
            }
        }
        for child in &self.children {
            child.fmt_with_indent(f, indent + 1)?;
        }
        Ok(())
    }
}
