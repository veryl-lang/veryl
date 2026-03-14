use crate::HashMap;
use std::fmt;
use std::mem::{offset_of, size_of};
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::{Type, VarId, VarPath};
use veryl_analyzer::value::Value;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug)]
pub struct Variable {
    pub path: VarPath,
    pub r#type: Type,
    pub width: usize,
    pub current_values: Vec<*mut Value>,
    pub next_values: Vec<*mut Value>,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, value) in self.current_values.iter().enumerate() {
            let value = unsafe { &**value };
            ret.push_str(&format!("{}[{}] = {:x};\n", self.path, i, value));
        }

        ret.trim_end().fmt(f)
    }
}

#[derive(Clone)]
pub struct FfValue {
    pub current: Value,
    pub next: Value,
}

impl FfValue {
    pub fn as_ptr(&self) -> *const Value {
        &self.current
    }

    pub fn as_mut_ptr(&mut self) -> *mut Value {
        &mut self.current
    }

    pub fn as_next_ptr(&self) -> *const Value {
        &self.next
    }

    pub fn as_next_mut_ptr(&mut self) -> *mut Value {
        &mut self.next
    }

    pub fn swap(&mut self) {
        std::mem::swap(&mut self.current, &mut self.next);
    }
}

#[derive(Clone)]
pub struct CombValue(pub Value);

impl CombValue {
    pub fn as_ptr(&self) -> *const Value {
        &self.0
    }

    pub fn as_mut_ptr(&mut self) -> *mut Value {
        &mut self.0
    }
}

#[derive(Clone, Debug)]
pub struct VariableElement {
    pub is_ff: bool,
    /// byte offset of current Value from ff_values[0] (is_ff) or comb_values[0] (!is_ff)
    pub current_offset: isize,
    /// byte offset of next Value from ff_values[0]; meaningful only when is_ff == true
    pub next_offset: isize,
}

#[derive(Clone, Debug)]
pub struct VariableMeta {
    pub path: VarPath,
    pub r#type: Type,
    pub width: usize,
    pub elements: Vec<VariableElement>,
    /// initial value for each element; used when instantiating
    pub initial_values: Vec<Value>,
}

impl VariableMeta {
    /// Returns (base_current_offset, base_next_offset, stride, is_ff) for dynamic indexing.
    pub fn dynamic_index_info(&self) -> Option<(isize, isize, isize, bool)> {
        let first = self.elements.first()?;
        let is_ff = first.is_ff;
        let stride = if self.elements.len() > 1 {
            self.elements[1].current_offset - self.elements[0].current_offset
        } else if is_ff {
            size_of::<FfValue>() as isize
        } else {
            size_of::<CombValue>() as isize
        };
        Some((first.current_offset, first.next_offset, stride, is_ff))
    }
}

/// Compute offset-based VariableMeta for each variable.
///
/// Returns `(variable_meta, ff_count, comb_count)`.
/// `ff_count` / `comb_count` are the number of entries allocated by this module only
/// (not including the start offset).
/// Iterates variables sorted by VarId so the iteration order is deterministic
/// and matches the buffer allocation order in `create_values`.
pub fn create_variable_meta(
    src: &HashMap<VarId, air::Variable>,
    ff_table: &air::FfTable,
    use_4state: bool,
    ff_start_index: isize,
    comb_start_index: isize,
) -> Option<(HashMap<VarId, VariableMeta>, usize, usize)> {
    let ff_stride = size_of::<FfValue>() as isize;
    let comb_stride = size_of::<CombValue>() as isize;
    let ff_current = offset_of!(FfValue, current) as isize;
    let ff_next = offset_of!(FfValue, next) as isize;
    let comb_current = offset_of!(CombValue, 0) as isize;

    let mut ff_index: isize = ff_start_index;
    let mut comb_index: isize = comb_start_index;

    let mut src_sorted: Vec<_> = src.iter().collect();
    src_sorted.sort_by_key(|(k, _)| **k);

    let mut variables = HashMap::default();

    for (k, v) in src_sorted {
        let mut elements = vec![];
        let mut initial_values = vec![];

        for (i, val) in v.value.iter().enumerate() {
            let mut val = val.clone();
            if !use_4state {
                val.clear_xz();
            }

            if ff_table.is_ff(v.id, i) {
                let current_offset = ff_index * ff_stride + ff_current;
                let next_offset = ff_index * ff_stride + ff_next;
                elements.push(VariableElement {
                    is_ff: true,
                    current_offset,
                    next_offset,
                });
                ff_index += 1;
            } else {
                let current_offset = comb_index * comb_stride + comb_current;
                elements.push(VariableElement {
                    is_ff: false,
                    current_offset,
                    next_offset: 0,
                });
                comb_index += 1;
            }

            initial_values.push(val);
        }

        let meta = VariableMeta {
            path: v.path.clone(),
            r#type: v.r#type.clone(),
            width: v.r#type.total_width()?,
            elements,
            initial_values,
        };
        variables.insert(*k, meta);
    }

    Some((
        variables,
        (ff_index - ff_start_index) as usize,
        (comb_index - comb_start_index) as usize,
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
