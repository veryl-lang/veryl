//! References to design variables passed to method arguments.
//!
//! A method argument typed [`ClockPort`], [`ResetPort`], [`InputPort`],
//! [`OutputPort`] or [`ModportRef`] takes a reference from the testbench
//! (`tb.probe("x", dut.x)`). The host binds a port to each referenced
//! variable when the method is called, so the component keeps the handle and
//! reads or writes through it in later hooks like any connected port. A
//! clock or a reset passed this way fires `on_clock` or `on_reset` like a
//! connected clock or reset port.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::{ClockPort, InputPort, OutputPort, ResetPort, Result, SimCtx, Value, bail, sys};

/// Shape of a port bound for a reference, from the host's description.
/// Interned, so a handle stays `Copy`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PortShape {
    pub(crate) output: bool,
    /// A clock firing `on_clock`; read like an input.
    pub(crate) clock: bool,
    /// A reset firing `on_reset`; read like an input.
    pub(crate) reset: bool,
    pub(crate) width: u32,
    pub(crate) dims: Vec<u32>,
    pub(crate) unpacked_dims: Vec<u32>,
    pub(crate) path: String,
    pub(crate) member: String,
    pub(crate) interface: String,
    pub(crate) modport: String,
}

impl PortShape {
    fn parse(desc: &str) -> Result<Self> {
        let fields: Vec<&str> = desc.split(sys::VRL_PORT_DESC_SEP).collect();
        let [dir, width, dims, unpacked, path, member, interface, modport] = fields[..] else {
            bail!("malformed port description `{desc}`");
        };
        let list = |s: &str| -> Result<Vec<u32>> {
            s.split(',')
                .filter(|x| !x.is_empty())
                .map(|x| Ok(x.parse::<u32>()?))
                .collect()
        };
        let (output, clock, reset) = match dir {
            "input" => (false, false, false),
            "output" => (true, false, false),
            "clock" => (false, true, false),
            "reset" => (false, false, true),
            _ => bail!("malformed port direction `{dir}`"),
        };
        Ok(Self {
            output,
            clock,
            reset,
            width: width.parse()?,
            dims: list(dims)?,
            unpacked_dims: list(unpacked)?,
            path: path.to_string(),
            member: member.to_string(),
            interface: interface.to_string(),
            modport: modport.to_string(),
        })
    }

    /// Interns the shape described by `desc`. The set of distinct shapes is
    /// bounded by the testbench's call sites, so the leak is too.
    pub(crate) fn intern(desc: &str) -> Result<&'static PortShape> {
        static SHAPES: OnceLock<Mutex<HashMap<String, &'static PortShape>>> = OnceLock::new();
        let mut shapes = SHAPES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(shape) = shapes.get(desc) {
            return Ok(shape);
        }
        let shape: &'static PortShape = Box::leak(Box::new(Self::parse(desc)?));
        shapes.insert(desc.to_string(), shape);
        Ok(shape)
    }

    /// Bit range `(lsb, width)` of the element at packed indices `index`,
    /// outermost first: `[2]` of `logic<4, 8>` is bits `[23:16]`, as in
    /// SystemVerilog. Fewer indices than dimensions select a whole
    /// sub-element.
    pub(crate) fn packed_range(dims: &[u32], width: u32, index: &[u32]) -> Result<(u32, u32)> {
        if index.len() > dims.len() {
            bail!(
                "{} indices for {} packed dimension(s)",
                index.len(),
                dims.len()
            );
        }
        let mut lsb = 0;
        let mut size = width;
        for (i, (&x, &dim)) in index.iter().zip(dims).enumerate() {
            if x >= dim {
                bail!("index {x} out of range of dimension {i} of size {dim}");
            }
            size /= dim;
            lsb += x * size;
        }
        Ok((lsb, size))
    }
}

/// Packed dimensions of a port without a description: one dimension of its
/// width.
fn plain_dims(width: u32) -> Vec<u32> {
    vec![width]
}

impl InputPort {
    /// Packed dimensions, outermost first: `[4, 8]` for `logic<4, 8>`. A
    /// connected port, whose type is not described, reports its width.
    pub fn dims(&self) -> Vec<u32> {
        self.shape
            .map_or_else(|| plain_dims(self.width), |s| s.dims.clone())
    }

    /// Unpacked dimensions, outermost first; empty for a single variable.
    pub fn unpacked_dims(&self) -> &[u32] {
        self.shape.map_or(&[], |s| &s.unpacked_dims)
    }

    /// Hierarchical path of the referenced variable; empty for a connected
    /// port.
    pub fn path(&self) -> &str {
        self.shape.map_or("", |s| &s.path)
    }
}

impl OutputPort {
    /// Packed dimensions, outermost first: `[4, 8]` for `logic<4, 8>`. A
    /// connected port, whose type is not described, reports its width.
    pub fn dims(&self) -> Vec<u32> {
        self.shape
            .map_or_else(|| plain_dims(self.width), |s| s.dims.clone())
    }

    /// Unpacked dimensions, outermost first; empty for a single variable.
    pub fn unpacked_dims(&self) -> &[u32] {
        self.shape.map_or(&[], |s| &s.unpacked_dims)
    }

    /// Hierarchical path of the referenced variable; empty for a connected
    /// port.
    pub fn path(&self) -> &str {
        self.shape.map_or("", |s| &s.path)
    }
}

/// A member port of a [`ModportRef`], with the direction the modport gives
/// it from the component's side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberPort {
    Input(InputPort),
    Output(OutputPort),
}

/// A member of a [`ModportRef`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModportMember {
    pub name: String,
    pub port: MemberPort,
}

/// A modport of any interface, passed to a method argument
/// (`tb.set_vif("bus", dut.bus.tb)`). Its members are bound like the
/// members of an `#[interface]` port.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModportRef {
    interface: String,
    modport: String,
    path: String,
    members: Vec<ModportMember>,
}

impl ModportRef {
    /// The interface, as the testbench's project names it.
    pub fn interface(&self) -> &str {
        &self.interface
    }

    pub fn modport(&self) -> &str {
        &self.modport
    }

    /// Hierarchical path of the interface instance.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn members(&self) -> &[ModportMember] {
        &self.members
    }

    /// The member `name`, if the modport has it.
    pub fn member(&self, name: &str) -> Option<MemberPort> {
        self.members.iter().find(|m| m.name == name).map(|m| m.port)
    }

    /// The member `name`, which the modport must give as an input.
    pub fn input(&self, name: &str) -> Result<InputPort> {
        match self.member(name) {
            Some(MemberPort::Input(port)) => Ok(port),
            Some(MemberPort::Output(_)) => bail!("member `{name}` of `{}` is an output", self.path),
            None => bail!("`{}` has no member `{name}`", self.path),
        }
    }

    /// The member `name`, which the modport must give as an output.
    pub fn output(&self, name: &str) -> Result<OutputPort> {
        match self.member(name) {
            Some(MemberPort::Output(port)) => Ok(port),
            Some(MemberPort::Input(_)) => bail!("member `{name}` of `{}` is an input", self.path),
            None => bail!("`{}` has no member `{name}`", self.path),
        }
    }
}

/// Reference conversions used by the code `#[component_impl]` generates for
/// reference arguments.
impl SimCtx {
    fn ref_ports(value: &Value, kind: u32) -> Result<&[u32]> {
        match value {
            Value::Ref { kind: k, ports } if *k == kind => Ok(ports),
            Value::Ref { .. } => bail!("reference of the wrong kind"),
            _ => bail!("not a reference"),
        }
    }

    fn ref_port(&mut self, idx: u32) -> Result<(&'static PortShape, crate::sys::VrlPortDirect)> {
        let Some(desc) = self.raw.port_desc(idx) else {
            bail!("port {idx} is not bound for a reference");
        };
        Ok((PortShape::intern(&desc)?, self.raw.port_direct(idx)))
    }

    fn member_port(&mut self, idx: u32) -> Result<(MemberPort, &'static PortShape)> {
        let (shape, direct) = self.ref_port(idx)?;
        let port = if shape.output {
            MemberPort::Output(OutputPort {
                idx,
                width: shape.width,
                words_ptr: direct.words,
                mask_ptr: direct.mask_xz,
                dirty_ptr: direct.dirty,
                shape: Some(shape),
            })
        } else {
            MemberPort::Input(InputPort {
                idx,
                width: shape.width,
                words_ptr: direct.words.cast_const(),
                shape: Some(shape),
            })
        };
        Ok((port, shape))
    }

    #[doc(hidden)]
    pub fn __input_ref(&mut self, value: &Value) -> Result<InputPort> {
        let &[idx] = Self::ref_ports(value, sys::VRL_REF_SIGNAL)? else {
            bail!("malformed signal reference");
        };
        match self.member_port(idx)?.0 {
            MemberPort::Input(port) => Ok(port),
            MemberPort::Output(_) => bail!("expected an input reference"),
        }
    }

    #[doc(hidden)]
    pub fn __output_ref(&mut self, value: &Value) -> Result<OutputPort> {
        let &[idx] = Self::ref_ports(value, sys::VRL_REF_SIGNAL)? else {
            bail!("malformed signal reference");
        };
        match self.member_port(idx)?.0 {
            MemberPort::Output(port) => Ok(port),
            MemberPort::Input(_) => bail!("expected an output reference"),
        }
    }

    #[doc(hidden)]
    pub fn __clock_ref(&mut self, value: &Value) -> Result<ClockPort> {
        let &[idx] = Self::ref_ports(value, sys::VRL_REF_SIGNAL)? else {
            bail!("malformed clock reference");
        };
        let (shape, _) = self.ref_port(idx)?;
        if !shape.clock {
            bail!("expected a clock reference");
        }
        Ok(ClockPort { idx })
    }

    #[doc(hidden)]
    pub fn __reset_ref(&mut self, value: &Value) -> Result<ResetPort> {
        let &[idx] = Self::ref_ports(value, sys::VRL_REF_SIGNAL)? else {
            bail!("malformed reset reference");
        };
        let (shape, _) = self.ref_port(idx)?;
        if !shape.reset {
            bail!("expected a reset reference");
        }
        Ok(ResetPort { idx })
    }

    #[doc(hidden)]
    pub fn __modport_ref(&mut self, value: &Value) -> Result<ModportRef> {
        let ports = Self::ref_ports(value, sys::VRL_REF_MODPORT)?.to_vec();
        let mut members = Vec::with_capacity(ports.len());
        let mut info = None;
        for idx in ports {
            let (port, shape) = self.member_port(idx)?;
            info.get_or_insert((
                shape.interface.clone(),
                shape.modport.clone(),
                shape.path.clone(),
            ));
            members.push(ModportMember {
                name: shape.member.clone(),
                port,
            });
        }
        let (interface, modport, path) = info.unwrap_or_default();
        // The path of a member names the member; the modport's is its
        // instance.
        let path = match path.rsplit_once('.') {
            Some((instance, _)) if !members.is_empty() => instance.to_string(),
            _ => path,
        };
        Ok(ModportRef {
            interface,
            modport,
            path,
            members,
        })
    }
}

/// Bits `[lsb, lsb + width)` of `value`.
pub(crate) fn slice_bits(value: &Value, lsb: u32, width: u32) -> Value {
    let Value::Bits { words, mask_xz, .. } = value else {
        return Value::from_u64(0, width);
    };
    let get = |w: &[u64], i: u32| -> bool {
        let bit = lsb + i;
        w.get((bit / 64) as usize)
            .is_some_and(|x| x >> (bit % 64) & 1 != 0)
    };
    let n = crate::value::words_for(width);
    let mut out_words = smallvec::SmallVec::from_elem(0u64, n);
    let mut out_mask = smallvec::SmallVec::from_elem(0u64, n);
    for i in 0..width {
        if get(words, i) {
            out_words[(i / 64) as usize] |= 1 << (i % 64);
        }
        if get(mask_xz, i) {
            out_mask[(i / 64) as usize] |= 1 << (i % 64);
        }
    }
    Value::from_bits(out_words, out_mask, width)
}

/// Element and bit-range access of ports.
impl SimCtx {
    /// The pre-edge value of the element of `port` at `index`: unpacked
    /// indices first (all of them), then packed indices, outermost first and
    /// possibly fewer than the packed dimensions. `[2]` of `logic<4, 8>` is
    /// bits `[23:16]`.
    pub fn read_elem(&mut self, port: InputPort, index: &[u32]) -> Result<Value> {
        let packed = Self::packed_index(port.unpacked_dims(), index)?;
        let (lsb, width) = PortShape::packed_range(&port.dims(), port.width, packed)?;
        let value = self.read(port);
        Ok(slice_bits(&value, lsb, width))
    }

    /// Writes the element of `port` at `index` (see [`Self::read_elem`]),
    /// leaving the other bits as written before. Commits with FFs.
    pub fn write_elem(
        &mut self,
        port: OutputPort,
        index: &[u32],
        value: impl Into<Value>,
    ) -> Result<()> {
        let packed = Self::packed_index(port.unpacked_dims(), index)?;
        let (lsb, width) = PortShape::packed_range(&port.dims(), port.width, packed)?;
        self.write_bits_at(port, lsb, width, value.into())
    }

    /// The pre-edge value of `width` bits from `lsb` of the element at
    /// unpacked indices `unpacked` (empty for a single variable).
    pub fn read_bits(
        &mut self,
        port: InputPort,
        unpacked: &[u32],
        lsb: u32,
        width: u32,
    ) -> Result<Value> {
        Self::packed_index(port.unpacked_dims(), unpacked)?;
        Self::check_range(port.width, lsb, width)?;
        let value = self.read(port);
        Ok(slice_bits(&value, lsb, width))
    }

    /// Writes `width` bits from `lsb` of the element at unpacked indices
    /// `unpacked` (empty for a single variable), leaving the other bits as
    /// written before. Commits with FFs.
    pub fn write_bits(
        &mut self,
        port: OutputPort,
        unpacked: &[u32],
        lsb: u32,
        width: u32,
        value: impl Into<Value>,
    ) -> Result<()> {
        Self::packed_index(port.unpacked_dims(), unpacked)?;
        Self::check_range(port.width, lsb, width)?;
        self.write_bits_at(port, lsb, width, value.into())
    }

    /// Splits `index` past the unpacked indices. Unpacked arrays are not
    /// bound yet, so there are none.
    fn packed_index<'a>(unpacked_dims: &[u32], index: &'a [u32]) -> Result<&'a [u32]> {
        if !unpacked_dims.is_empty() {
            bail!("unpacked array references are not supported");
        }
        Ok(index)
    }

    fn check_range(port_width: u32, lsb: u32, width: u32) -> Result<()> {
        if width == 0 || lsb.checked_add(width).is_none_or(|end| end > port_width) {
            bail!("bits [{lsb} +: {width}] out of a {port_width}-bit port");
        }
        Ok(())
    }

    fn write_bits_at(
        &mut self,
        port: OutputPort,
        lsb: u32,
        width: u32,
        value: Value,
    ) -> Result<()> {
        let words = value.to_port_words(width)?;
        let mask_xz = if self.is_4state() {
            Some(value.to_port_mask_xz(width)?)
        } else {
            None
        };
        if !self
            .raw
            .write_output_bits(port.idx, lsb, width, &words, mask_xz.as_deref())
        {
            bail!("cannot write bits [{lsb} +: {width}] of port {}", port.idx);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_range_follows_sv() {
        // logic<4, 8>
        assert_eq!(PortShape::packed_range(&[4, 8], 32, &[]).unwrap(), (0, 32));
        assert_eq!(PortShape::packed_range(&[4, 8], 32, &[2]).unwrap(), (16, 8));
        assert_eq!(
            PortShape::packed_range(&[4, 8], 32, &[2, 5]).unwrap(),
            (21, 1)
        );
        assert!(PortShape::packed_range(&[4, 8], 32, &[4]).is_err());
        assert!(PortShape::packed_range(&[4, 8], 32, &[0, 0, 0]).is_err());
    }

    #[test]
    fn parses_a_description() {
        let shape = PortShape::parse("output\t32\t4,8\t\ttop.bus.data\tdata\tbus_if\ttb").unwrap();
        assert!(shape.output);
        assert_eq!(shape.width, 32);
        assert_eq!(shape.dims, vec![4, 8]);
        assert!(shape.unpacked_dims.is_empty());
        assert_eq!(shape.member, "data");
        assert!(PortShape::parse("sideways\t1\t\t\t\t\t\t").is_err());
    }

    #[test]
    fn slices_bits() {
        let v = Value::from_u64(0xab_cd, 16);
        assert_eq!(slice_bits(&v, 8, 8), Value::from_u64(0xab, 8));
        assert_eq!(slice_bits(&v, 4, 4), Value::from_u64(0xc, 4));
    }
}
