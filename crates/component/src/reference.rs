//! References to design variables passed to method arguments.
//!
//! A method argument typed [`ClockPort`], [`ResetPort`], [`InputPort`],
//! [`OutputPort`] or [`ModportRef`] takes a reference from the testbench
//! (`tb.probe("x", dut.x)`). The method gets a port bound to each referenced
//! variable, so the component keeps the handle and reads or writes through
//! it in later hooks like any connected port. A clock or a reset passed this
//! way fires `on_clock` or `on_reset` like a connected clock or reset port,
//! from the call on. [`SimCtx::port_info`] describes the referenced variable.

use crate::sys::{PortDesc, PortDescDir};
use crate::{ClockPort, InputPort, OutputPort, ResetPort, Result, SimCtx, Value, bail, sys};

/// An argument passed to a component method: a value, or a reference to
/// design variables, which the dispatch `#[component_impl]` generates turns
/// into a port handle or a [`ModportRef`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MethodArg {
    Value(Value),
    Ref(RefArg),
}

/// The ports the host bound for a reference argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefArg {
    /// One of `VRL_REF_*`.
    kind: u32,
    ports: Vec<u32>,
}

impl RefArg {
    pub(crate) fn new(kind: u32, ports: Vec<u32>) -> Self {
        Self { kind, ports }
    }
}

impl From<Value> for MethodArg {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

impl MethodArg {
    pub fn as_value(&self) -> Result<&Value> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Ref(_) => bail!("expected a value, got a reference"),
        }
    }

    /// The caller must supply valid pointers for the declared lengths.
    pub(crate) unsafe fn from_vrl(arg: &sys::VrlValue) -> Self {
        if arg.kind != sys::VRL_VALUE_REF {
            return Self::Value(unsafe { Value::from_vrl(arg) });
        }
        let ports = if arg.nwords == 0 || arg.words.is_null() {
            vec![]
        } else {
            unsafe { std::slice::from_raw_parts(arg.words, arg.nwords) }
                .iter()
                .map(|&port| port as u32)
                .collect()
        };
        Self::Ref(RefArg::new(arg.width, ports))
    }
}

/// `width` bits from `lsb` of a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitRange {
    pub lsb: u32,
    pub width: u32,
}

/// The shape of a port's variable, from [`SimCtx::port_info`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    width: u32,
    dims: Vec<u32>,
    path: String,
}

impl PortInfo {
    /// Width of one element.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Packed dimensions, outermost first: `[4, 8]` for `logic<4, 8>`. A
    /// connected port, whose type is not described, has one dimension of its
    /// width.
    pub fn dims(&self) -> &[u32] {
        &self.dims
    }

    /// Hierarchical path of the referenced variable; empty for a connected
    /// port.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The bits of the element at packed indices `index`, outermost first:
    /// `[2]` of `logic<4, 8>` is bits `[23:16]`, as in SystemVerilog. Fewer
    /// indices than dimensions select a whole sub-element.
    pub fn elem(&self, index: &[u32]) -> Result<BitRange> {
        if index.len() > self.dims.len() {
            bail!(
                "{} indices for {} packed dimension(s)",
                index.len(),
                self.dims.len()
            );
        }
        let mut lsb = 0;
        let mut width = self.width;
        for (i, (&x, &dim)) in index.iter().zip(&self.dims).enumerate() {
            if x >= dim {
                bail!("index {x} out of range of dimension {i} of size {dim}");
            }
            width /= dim;
            lsb += x * width;
        }
        Ok(BitRange { lsb, width })
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::InputPort {}
    impl Sealed for crate::OutputPort {}
    impl Sealed for crate::ClockPort {}
    impl Sealed for crate::ResetPort {}
}

/// A port handle, as [`SimCtx::port_info`] takes it.
pub trait Port: Copy + sealed::Sealed {
    #[doc(hidden)]
    fn port_index(&self) -> u32;
}

impl Port for InputPort {
    fn port_index(&self) -> u32 {
        self.idx
    }
}

impl Port for OutputPort {
    fn port_index(&self) -> u32 {
        self.idx
    }
}

impl Port for ClockPort {
    fn port_index(&self) -> u32 {
        self.idx
    }
}

impl Port for ResetPort {
    fn port_index(&self) -> u32 {
        self.idx
    }
}

/// The clock or reset event a port hears. Ports hearing one event, connected
/// or passed as references, have equal ids; ids compare only within one
/// component instance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EventId(pub(crate) u32);

/// A clock or reset port handle, as [`SimCtx::fired`] takes it.
pub trait EventPort: Port {
    /// The event the port hears.
    fn event(&self) -> EventId;
}

impl EventPort for ClockPort {
    fn event(&self) -> EventId {
        EventId(self.event)
    }
}

impl EventPort for ResetPort {
    fn event(&self) -> EventId {
        EventId(self.event)
    }
}

/// A member port of a [`ModportRef`], with the direction the modport gives
/// it from the component's side. An input member of a clock or a reset
/// type is a [`ClockPort`] or a [`ResetPort`], firing `on_clock` or
/// `on_reset` as a clock or reset passed on its own does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberPort {
    Input(InputPort),
    Output(OutputPort),
    Clock(ClockPort),
    Reset(ResetPort),
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
            other => self.mismatch(name, other, "an input"),
        }
    }

    /// The member `name`, which the modport must give as an output.
    pub fn output(&self, name: &str) -> Result<OutputPort> {
        match self.member(name) {
            Some(MemberPort::Output(port)) => Ok(port),
            other => self.mismatch(name, other, "an output"),
        }
    }

    /// The member `name`, an input of a clock type.
    pub fn clock(&self, name: &str) -> Result<ClockPort> {
        match self.member(name) {
            Some(MemberPort::Clock(port)) => Ok(port),
            other => self.mismatch(name, other, "a clock"),
        }
    }

    /// The member `name`, an input of a reset type.
    pub fn reset(&self, name: &str) -> Result<ResetPort> {
        match self.member(name) {
            Some(MemberPort::Reset(port)) => Ok(port),
            other => self.mismatch(name, other, "a reset"),
        }
    }

    fn mismatch<T>(&self, name: &str, found: Option<MemberPort>, expected: &str) -> Result<T> {
        let is = match found {
            None => bail!("`{}` has no member `{name}`", self.path),
            Some(MemberPort::Input(_)) => "an input",
            Some(MemberPort::Output(_)) => "an output",
            Some(MemberPort::Clock(_)) => "a clock",
            Some(MemberPort::Reset(_)) => "a reset",
        };
        bail!("member `{name}` of `{}` is {is}, not {expected}", self.path)
    }
}

/// Conversions of reference arguments into port handles, which the dispatch
/// `#[component_impl]` generates calls for each reference argument; a
/// hand-written [`crate::Component::method`] calls them itself.
impl SimCtx {
    fn ref_ports(value: &MethodArg, kind: u32) -> Result<&[u32]> {
        match value {
            MethodArg::Ref(r) if r.kind == kind => Ok(&r.ports),
            MethodArg::Ref(_) => bail!("reference of the wrong kind"),
            MethodArg::Value(_) => bail!("not a reference"),
        }
    }

    fn ref_desc(&mut self, idx: u32) -> Result<PortDesc> {
        match self.port_desc(idx)? {
            Some(desc) => Ok(desc),
            None => bail!("port {idx} is not bound for a reference"),
        }
    }

    fn port_desc(&mut self, idx: u32) -> Result<Option<PortDesc>> {
        let Some(text) = self.raw.port_desc(idx) else {
            return Ok(None);
        };
        match PortDesc::decode(&text) {
            Some(desc) => Ok(Some(desc)),
            None => bail!("malformed port description `{text}`"),
        }
    }

    fn member_port(&mut self, idx: u32) -> Result<(MemberPort, PortDesc)> {
        let desc = self.ref_desc(idx)?;
        let direct = self.raw.port_direct(idx);
        let port = match desc.dir {
            PortDescDir::Clock => MemberPort::Clock(ClockPort {
                idx,
                event: self.raw.port_event(idx),
            }),
            PortDescDir::Reset => MemberPort::Reset(ResetPort {
                idx,
                event: self.raw.port_event(idx),
            }),
            PortDescDir::Output => MemberPort::Output(OutputPort {
                idx,
                width: desc.width,
                words_ptr: direct.words,
                mask_ptr: direct.mask_xz,
                dirty_ptr: direct.dirty,
            }),
            PortDescDir::Input => MemberPort::Input(InputPort {
                idx,
                width: desc.width,
                words_ptr: direct.words.cast_const(),
            }),
        };
        Ok((port, desc))
    }

    fn signal_ref(&mut self, value: &MethodArg) -> Result<MemberPort> {
        let &[idx] = Self::ref_ports(value, sys::VRL_REF_SIGNAL)? else {
            bail!("malformed signal reference");
        };
        Ok(self.member_port(idx)?.0)
    }

    /// The input port of a signal reference.
    pub fn input_ref(&mut self, value: &MethodArg) -> Result<InputPort> {
        match self.signal_ref(value)? {
            MemberPort::Input(port) => Ok(port),
            _ => bail!("expected an input reference"),
        }
    }

    /// The output port of a signal reference.
    pub fn output_ref(&mut self, value: &MethodArg) -> Result<OutputPort> {
        match self.signal_ref(value)? {
            MemberPort::Output(port) => Ok(port),
            _ => bail!("expected an output reference"),
        }
    }

    /// The clock port of a clock reference, firing `on_clock` from the call
    /// on.
    pub fn clock_ref(&mut self, value: &MethodArg) -> Result<ClockPort> {
        match self.signal_ref(value)? {
            MemberPort::Clock(port) => Ok(port),
            _ => bail!("expected a clock reference"),
        }
    }

    /// The reset port of a reset reference, firing `on_reset` from the call
    /// on.
    pub fn reset_ref(&mut self, value: &MethodArg) -> Result<ResetPort> {
        match self.signal_ref(value)? {
            MemberPort::Reset(port) => Ok(port),
            _ => bail!("expected a reset reference"),
        }
    }

    /// The members of a modport reference.
    pub fn modport_ref(&mut self, value: &MethodArg) -> Result<ModportRef> {
        let ports = Self::ref_ports(value, sys::VRL_REF_MODPORT)?.to_vec();
        let mut members = Vec::with_capacity(ports.len());
        let mut info = None;
        for idx in ports {
            let (port, desc) = self.member_port(idx)?;
            members.push(ModportMember {
                name: desc.member.clone(),
                port,
            });
            info.get_or_insert((desc.interface, desc.modport, desc.path));
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
fn slice_bits(value: &Value, range: BitRange) -> Value {
    let BitRange { lsb, width } = range;
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

fn check_range(port_width: u32, range: BitRange) -> Result<()> {
    let BitRange { lsb, width } = range;
    if width == 0 || lsb.checked_add(width).is_none_or(|end| end > port_width) {
        bail!("bits [{lsb} +: {width}] out of a {port_width}-bit port");
    }
    Ok(())
}

/// Port descriptions and bit-range access.
impl SimCtx {
    /// The shape of `port`'s variable. A port bound for a reference gets its
    /// variable's dimensions and path; a connected port, whose type is not
    /// described, one dimension of its width and no path.
    pub fn port_info(&mut self, port: impl Port) -> Result<PortInfo> {
        let idx = port.port_index();
        if let Some(desc) = self.port_desc(idx)? {
            return Ok(PortInfo {
                width: desc.width,
                dims: desc.dims,
                path: desc.path,
            });
        }
        let width = self.raw.port_width(idx);
        Ok(PortInfo {
            width,
            dims: vec![width],
            path: String::new(),
        })
    }

    /// The pre-edge value of bits `range` of `port`.
    pub fn read_bits(&mut self, port: impl Into<InputPort>, range: BitRange) -> Result<Value> {
        let port = port.into();
        check_range(port.width, range)?;
        let value = self.read(port);
        Ok(slice_bits(&value, range))
    }

    /// Writes bits `range` of `port`, leaving the other bits as written
    /// before. Commits with FFs.
    pub fn write_bits(
        &mut self,
        port: OutputPort,
        range: BitRange,
        value: impl Into<Value>,
    ) -> Result<()> {
        check_range(port.width, range)?;
        let value = value.into();
        let words = value.to_port_words(range.width)?;
        let mask_xz = if self.is_4state() {
            Some(value.to_port_mask_xz(range.width)?)
        } else {
            None
        };
        if !self
            .raw
            .write_output_bits(port.idx, range.lsb, range.width, &words, mask_xz.as_deref())
        {
            bail!(
                "cannot write bits [{} +: {}] of port {}",
                range.lsb,
                range.width,
                port.idx
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elem_follows_sv() {
        let info = PortInfo {
            width: 32,
            dims: vec![4, 8],
            path: String::new(),
        };
        let range = |lsb, width| BitRange { lsb, width };
        assert_eq!(info.elem(&[]).unwrap(), range(0, 32));
        assert_eq!(info.elem(&[2]).unwrap(), range(16, 8));
        assert_eq!(info.elem(&[2, 5]).unwrap(), range(21, 1));
        assert!(info.elem(&[4]).is_err());
        assert!(info.elem(&[0, 0, 0]).is_err());
    }

    #[test]
    fn slices_bits() {
        let v = Value::from_u64(0xab_cd, 16);
        let range = |lsb, width| BitRange { lsb, width };
        assert_eq!(slice_bits(&v, range(8, 8)), Value::from_u64(0xab, 8));
        assert_eq!(slice_bits(&v, range(4, 4)), Value::from_u64(0xc, 4));
    }
}
