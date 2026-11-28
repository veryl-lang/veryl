//! Typed values and signals: the conversion between Veryl's packed bits
//! and Rust types ([`Packed`]), signals read and driven as such types
//! ([`InputSignal`], [`OutputSignal`]), and modports declared as structs of
//! them ([`Modport`], implemented by `#[modport]` and `import!`).
//!
//! Values are two-state: X and Z read as 0, as SystemVerilog converts
//! four-state values to two-state types, and nothing drives X or Z.

use crate::value::words_for;
use crate::{ClockPort, InputPort, ModportRef, OutputPort, ResetPort, Result, SimCtx, Value, bail};
use num_bigint::{BigInt, BigUint, Sign};
use smallvec::SmallVec;
use std::fmt;

/// The bits `[offset, offset + width)` of `words` (least significant word
/// first); `width` is 64 at most. Bits beyond `words` read as 0.
pub fn get_bits(words: &[u64], offset: u32, width: u32) -> u64 {
    debug_assert!(width <= 64);
    if width == 0 {
        return 0;
    }
    let (index, shift) = ((offset / 64) as usize, offset % 64);
    let mut ret = words.get(index).copied().unwrap_or(0) >> shift;
    if shift != 0 && shift + width > 64 {
        ret |= words.get(index + 1).copied().unwrap_or(0) << (64 - shift);
    }
    ret & mask(width)
}

/// Sets the bits `[offset, offset + width)` of `words` to `value`, truncated
/// to `width` bits (64 at most). Bits beyond `words` are dropped.
pub fn set_bits(words: &mut [u64], offset: u32, width: u32, value: u64) {
    debug_assert!(width <= 64);
    if width == 0 {
        return;
    }
    let mask = mask(width);
    let value = value & mask;
    let (index, shift) = ((offset / 64) as usize, offset % 64);
    if let Some(word) = words.get_mut(index) {
        *word = (*word & !(mask << shift)) | (value << shift);
    }
    if shift != 0
        && shift + width > 64
        && let Some(word) = words.get_mut(index + 1)
    {
        let rest = 64 - shift;
        *word = (*word & !(mask >> rest)) | (value >> rest);
    }
}

fn mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1 << width) - 1
    }
}

/// A Rust type a Veryl value of a packed type converts to and from: a
/// component method's argument, a signal's value, a field of a struct of an
/// exported package.
///
/// Implemented for `bool`, the integers up to 128 bits, `BigUint` and
/// `BigInt` (any width), [`Value`], arrays of them, and the types
/// `import!` generates. A value is truncated to the width it is packed
/// into.
pub trait Packed: Sized {
    /// Whether a value of `width` bits converts to this type.
    fn fits(width: u32) -> bool;

    /// The value of the bits `[offset, offset + width)` of `words` (least
    /// significant word first). It fails when the bits are no value of the
    /// type (an enum's).
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self>;

    /// Writes the value into the bits `[offset, offset + width)` of `words`.
    fn pack(&self, words: &mut [u64], offset: u32, width: u32);

    /// The value of a [`Value`], whose X and Z bits read as 0.
    fn from_value(value: &Value) -> Result<Self> {
        let Value::Bits {
            words,
            mask_xz,
            width,
        } = value
        else {
            bail!(
                "a value that is not bits cannot be converted to `{}`",
                type_name::<Self>()
            );
        };
        check_width::<Self>("a value", *width)?;
        let words: SmallVec<[u64; 4]> = words
            .iter()
            .enumerate()
            .map(|(i, w)| w & !mask_xz.get(i).copied().unwrap_or(0))
            .collect();
        Self::unpack(&words, 0, *width)
    }

    /// The value as a [`Value`] of `width` bits.
    fn to_value(&self, width: u32) -> Value {
        let mut words: SmallVec<[u64; 2]> = SmallVec::from_elem(0, words_for(width));
        self.pack(&mut words, 0, width);
        Value::from_bits(words, SmallVec::new(), width)
    }

    /// The value of the input `port` before the current edge.
    fn read(ctx: &mut SimCtx, port: InputPort) -> Result<Self> {
        let mut words: SmallVec<[u64; 4]> = SmallVec::from_elem(0, port.words());
        ctx.read_two_state(port, &mut words);
        Self::unpack(&words, 0, port.width())
    }

    /// Drives the output `port`; the value reaches the design at the edge.
    fn write(&self, ctx: &mut SimCtx, port: OutputPort) {
        let mut words: SmallVec<[u64; 4]> = SmallVec::from_elem(0, port.words());
        self.pack(&mut words, 0, port.width());
        if port.width() <= 64 {
            ctx.write_u64(port, words[0]);
        } else {
            ctx.write_words(port, &words);
        }
    }
}

/// A [`Packed`] type of a fixed width: an enum or a struct of an exported
/// package.
pub trait FixedWidth: Packed {
    const WIDTH: u32;
}

fn type_name<T>() -> &'static str {
    std::any::type_name::<T>()
}

fn check_width<T: Packed>(what: &str, width: u32) -> Result<()> {
    if T::fits(width) {
        Ok(())
    } else {
        bail!("{what} of {width} bits does not fit `{}`", type_name::<T>())
    }
}

impl Packed for bool {
    fn fits(width: u32) -> bool {
        width == 1
    }
    fn unpack(words: &[u64], offset: u32, _width: u32) -> Result<Self> {
        Ok(get_bits(words, offset, 1) != 0)
    }
    fn pack(&self, words: &mut [u64], offset: u32, _width: u32) {
        set_bits(words, offset, 1, u64::from(*self));
    }
}

macro_rules! packed_unsigned {
    ($($ty:ty),*) => {$(
        impl Packed for $ty {
            fn fits(width: u32) -> bool {
                (1..=<$ty>::BITS).contains(&width)
            }
            fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
                Ok(get_bits(words, offset, width) as $ty)
            }
            fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
                set_bits(words, offset, width, *self as u64);
            }
        }
    )*};
}

packed_unsigned!(u8, u16, u32, u64);

macro_rules! packed_signed {
    ($($ty:ty),*) => {$(
        impl Packed for $ty {
            fn fits(width: u32) -> bool {
                (1..=<$ty>::BITS).contains(&width)
            }
            fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
                let shift = 64 - width;
                Ok((((get_bits(words, offset, width) << shift) as i64) >> shift) as $ty)
            }
            fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
                set_bits(words, offset, width, *self as i64 as u64);
            }
        }
    )*};
}

packed_signed!(i8, i16, i32, i64);

impl Packed for u128 {
    fn fits(width: u32) -> bool {
        (1..=128).contains(&width)
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let lo = get_bits(words, offset, width.min(64));
        let hi = get_bits(words, offset + 64, width.saturating_sub(64));
        Ok(u128::from(lo) | (u128::from(hi) << 64))
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        set_bits(words, offset, width.min(64), *self as u64);
        set_bits(
            words,
            offset + 64,
            width.saturating_sub(64),
            (*self >> 64) as u64,
        );
    }
}

impl Packed for i128 {
    fn fits(width: u32) -> bool {
        (1..=128).contains(&width)
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let shift = 128 - width;
        Ok(((u128::unpack(words, offset, width)? << shift) as i128) >> shift)
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        (*self as u128).pack(words, offset, width);
    }
}

impl Packed for BigUint {
    fn fits(width: u32) -> bool {
        width >= 1
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let mut digits: SmallVec<[u32; 8]> = SmallVec::new();
        let mut pos = 0;
        while pos < width {
            let word = get_bits(words, offset + pos, (width - pos).min(64));
            digits.push(word as u32);
            digits.push((word >> 32) as u32);
            pos += 64;
        }
        Ok(BigUint::from_slice(&digits))
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        let mut digits = self.iter_u64_digits();
        let mut pos = 0;
        while pos < width {
            let word = digits.next().unwrap_or(0);
            set_bits(words, offset + pos, (width - pos).min(64), word);
            pos += 64;
        }
    }
}

impl Packed for BigInt {
    fn fits(width: u32) -> bool {
        width >= 1
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let raw = BigInt::from(BigUint::unpack(words, offset, width)?);
        if get_bits(words, offset + width - 1, 1) != 0 {
            Ok(raw - (BigInt::from(1) << width))
        } else {
            Ok(raw)
        }
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        let raw = if self.sign() == Sign::Minus {
            self + (BigInt::from(1) << width)
        } else {
            self.clone()
        };
        raw.magnitude().pack(words, offset, width);
    }
}

impl Packed for Value {
    fn fits(width: u32) -> bool {
        width >= 1
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let n = words_for(width);
        let mut ret: SmallVec<[u64; 2]> = SmallVec::from_elem(0, n);
        for (i, word) in ret.iter_mut().enumerate() {
            let pos = i as u32 * 64;
            *word = get_bits(words, offset + pos, (width - pos).min(64));
        }
        Ok(Value::from_bits(ret, SmallVec::new(), width))
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        let src = self.as_slice();
        let mut pos = 0;
        while pos < width {
            let word = src.get((pos / 64) as usize).copied().unwrap_or(0);
            set_bits(words, offset + pos, (width - pos).min(64), word);
            pos += 64;
        }
    }
}

/// An array packs its elements evenly over the width, the first element
/// the most significant, as a packed array of Veryl does.
impl<T: Packed, const N: usize> Packed for [T; N] {
    fn fits(width: u32) -> bool {
        N != 0 && width.is_multiple_of(N as u32) && T::fits(width / N as u32)
    }
    fn unpack(words: &[u64], offset: u32, width: u32) -> Result<Self> {
        let elem = width / N as u32;
        let mut ret = Vec::with_capacity(N);
        for i in 0..N {
            let pos = (N - 1 - i) as u32 * elem;
            ret.push(T::unpack(words, offset + pos, elem)?);
        }
        Ok(ret.try_into().unwrap_or_else(|_| unreachable!()))
    }
    fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
        let elem = width / N as u32;
        for (i, x) in self.iter().enumerate() {
            let pos = (N - 1 - i) as u32 * elem;
            x.pack(words, offset + pos, elem);
        }
    }
}

/// What supplies a modport's members: a [`ModportRef`] a test module
/// passed to a method, or anything else naming ports by member.
pub trait MemberSource {
    /// The path naming the source in messages.
    fn path(&self) -> &str;
    fn input(&self, name: &str) -> Result<InputPort>;
    fn output(&self, name: &str) -> Result<OutputPort>;
    fn clock(&self, name: &str) -> Result<ClockPort>;
    fn reset(&self, name: &str) -> Result<ResetPort>;
}

impl MemberSource for ModportRef {
    fn path(&self) -> &str {
        ModportRef::path(self)
    }
    fn input(&self, name: &str) -> Result<InputPort> {
        ModportRef::input(self, name)
    }
    fn output(&self, name: &str) -> Result<OutputPort> {
        ModportRef::output(self, name)
    }
    fn clock(&self, name: &str) -> Result<ClockPort> {
        ModportRef::clock(self, name)
    }
    fn reset(&self, name: &str) -> Result<ResetPort> {
        ModportRef::reset(self, name)
    }
}

/// An input signal read as `T`; its width is checked against `T` when
/// bound.
pub struct InputSignal<T> {
    port: InputPort,
    value: std::marker::PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for InputSignal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputSignal")
            .field("port", &self.port)
            .finish()
    }
}

impl<T: Packed> InputSignal<T> {
    /// Binds the input `port`; `what` names it in an error.
    pub fn bind(port: InputPort, what: &str) -> Result<Self> {
        check_width::<T>(&format!("`{what}`"), port.width())?;
        Ok(Self {
            port,
            value: std::marker::PhantomData,
        })
    }

    /// Binds the input member `name` of `source`.
    pub fn member<M: MemberSource + ?Sized>(source: &M, name: &str) -> Result<Self> {
        Self::bind(source.input(name)?, &format!("{}.{name}", source.path()))
    }

    pub fn port(&self) -> InputPort {
        self.port
    }

    /// The value before the current edge.
    pub fn read(&self, ctx: &mut SimCtx) -> Result<T> {
        T::read(ctx, self.port)
    }
}

/// An output signal driven from `T`; its width is checked against `T` when
/// bound. It remembers the value last driven, so that [`Self::store`]
/// drives only a change: an output never assigned is never driven, and the
/// design's value stands.
pub struct OutputSignal<T> {
    port: OutputPort,
    last: T,
}

impl<T> fmt::Debug for OutputSignal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputSignal")
            .field("port", &self.port)
            .finish()
    }
}

impl<T: Packed + Clone + PartialEq + Default> OutputSignal<T> {
    /// Binds the output `port`; `what` names it in an error. The value last
    /// driven is taken as `T::default()`.
    pub fn bind(port: OutputPort, what: &str) -> Result<Self> {
        check_width::<T>(&format!("`{what}`"), port.width())?;
        Ok(Self {
            port,
            last: T::default(),
        })
    }

    /// Binds the output member `name` of `source`.
    pub fn member<M: MemberSource + ?Sized>(source: &M, name: &str) -> Result<Self> {
        Self::bind(source.output(name)?, &format!("{}.{name}", source.path()))
    }

    pub fn port(&self) -> OutputPort {
        self.port
    }

    /// Drives `value` if it differs from the value last driven.
    pub fn store(&mut self, ctx: &mut SimCtx, value: &T) {
        if *value != self.last {
            self.force(ctx, value);
        }
    }

    /// Drives `value` whatever was last driven.
    pub fn force(&mut self, ctx: &mut SimCtx, value: &T) {
        value.write(ctx, self.port);
        self.last = value.clone();
    }

    /// Takes `value` as the value last driven, without driving it.
    pub fn assume(&mut self, value: &T) {
        self.last = value.clone();
    }
}

/// A modport of an interface declared as a struct whose fields are its
/// members' values: inputs read by [`Self::load`], outputs driven by
/// [`Self::store`]. Implemented by `#[modport]`, and by `import!` for each
/// modport of an exported interface.
///
/// For each field `x`, the struct also has methods handling that member
/// alone: `load_x(ctx)` for an input, and `force_x(ctx)` and `assume_x()`
/// for an output (see [`OutputSignal::force`] and [`OutputSignal::assume`]).
pub trait Modport: Sized {
    /// Binds every member by its name in `source`, checking its width. The
    /// fields start at their default values.
    fn bind<M: MemberSource + ?Sized>(source: &M) -> Result<Self>;

    /// Reads every input into its field.
    fn load(&mut self, ctx: &mut SimCtx) -> Result<()>;

    /// Drives every output whose field changed since it was last driven.
    fn store(&mut self, ctx: &mut SimCtx);
}

/// Implemented for a [`Modport`] with no output: it only reads.
pub trait InputOnly: Modport {}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Packed + PartialEq + fmt::Debug>(value: T, offset: u32, width: u32) {
        let mut words = [u64::MAX; 4];
        value.pack(&mut words, offset, width);
        assert_eq!(T::unpack(&words, offset, width).unwrap(), value);
        // The bits around are left alone.
        if offset > 0 {
            assert_eq!(get_bits(&words, 0, offset.min(64)), mask(offset.min(64)));
        }
    }

    #[test]
    fn bits_across_words() {
        let mut words = [0u64; 2];
        set_bits(&mut words, 60, 8, 0xab);
        assert_eq!(words, [0xb << 60, 0xa]);
        assert_eq!(get_bits(&words, 60, 8), 0xab);
        set_bits(&mut words, 60, 8, 0x1ff);
        assert_eq!(get_bits(&words, 60, 8), 0xff);
    }

    #[test]
    fn primitives_roundtrip() {
        roundtrip(true, 3, 1);
        roundtrip(0x5u8, 61, 4);
        roundtrip(0xbeefu16, 10, 16);
        roundtrip(-3i8, 5, 4);
        roundtrip(-1234i32, 40, 20);
        roundtrip(0x1234_5678_9abc_def0_1122_3344u128, 7, 100);
        roundtrip(-5i128, 7, 100);
        roundtrip(
            BigUint::from(0x1234_5678_9abc_def0_1122_3344u128) << 40,
            3,
            180,
        );
        roundtrip(BigInt::from(-77), 3, 180);
        roundtrip([1u8, 2, 3], 5, 12);
    }

    #[test]
    fn signed_extends_the_top_bit() {
        let words = [0b1110u64];
        assert_eq!(i8::unpack(&words, 0, 4).unwrap(), -2);
        assert_eq!(u8::unpack(&words, 0, 4).unwrap(), 14);
    }

    #[test]
    fn array_puts_the_first_element_at_the_top() {
        let mut words = [0u64];
        [0x1u8, 0x2, 0x3].pack(&mut words, 0, 12);
        assert_eq!(words[0], 0x123);
        assert!(<[u8; 3]>::fits(12));
        assert!(!<[u8; 3]>::fits(13));
    }

    #[test]
    fn value_conversion_reads_xz_as_zero() {
        let value = Value::from_bits(
            SmallVec::from_slice(&[0b1111]),
            SmallVec::from_slice(&[0b0101]),
            4,
        );
        assert_eq!(u8::from_value(&value).unwrap(), 0b1010);
        assert!(bool::from_value(&value).is_err());
        assert_eq!(0xau8.to_value(4), Value::from_u64(0xa, 4));
    }
}
