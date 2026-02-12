// This implementation refers the following book
// https://lo48576.gitlab.io/rust-custom-slice-book/

use crate::ir::{Expression, Op};
use crate::value::Value;
use std::borrow::{Borrow, BorrowMut, Cow, ToOwned};
use std::fmt;
use std::ops::{Deref, DerefMut, Index, IndexMut, Range, RangeBounds, RangeFrom};
use std::rc::Rc;
use std::sync::Arc;
use std::vec::Drain;
use veryl_parser::token_range::TokenRange;

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ShapeRef([Option<usize>]);

impl ShapeRef {
    #[inline]
    #[must_use]
    pub fn new(x: &[Option<usize>]) -> &Self {
        unsafe { &*(x as *const [Option<usize>] as *const Self) }
    }

    #[inline]
    #[must_use]
    pub fn new_mut(x: &mut [Option<usize>]) -> &mut Self {
        unsafe { &mut *(x as *mut [Option<usize>] as *mut Self) }
    }

    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[Option<usize>] {
        &self.0
    }

    #[inline]
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [Option<usize>] {
        &mut self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn dims(&self) -> usize {
        self.0.len()
    }

    pub fn total(&self) -> Option<usize> {
        if self.is_empty() {
            Some(1)
        } else {
            let mut ret = 1;
            for x in &self.0 {
                if let Some(x) = x {
                    ret *= x;
                } else {
                    return None;
                }
            }
            Some(ret)
        }
    }

    #[inline]
    pub fn get(&self, x: usize) -> Option<&Option<usize>> {
        self.0.get(x)
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize) -> Option<&mut Option<usize>> {
        self.0.get_mut(x)
    }

    #[inline]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Option<usize>> + DoubleEndedIterator {
        self.0.iter()
    }

    #[inline]
    pub fn iter_mut(
        &mut self,
    ) -> impl ExactSizeIterator<Item = &mut Option<usize>> + DoubleEndedIterator {
        self.0.iter_mut()
    }

    #[inline]
    pub fn first(&self) -> Option<&Option<usize>> {
        self.0.first()
    }

    #[inline]
    pub fn first_mut(&mut self) -> Option<&mut Option<usize>> {
        self.0.first_mut()
    }

    #[inline]
    pub fn last(&self) -> Option<&Option<usize>> {
        self.0.last()
    }

    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut Option<usize>> {
        self.0.last_mut()
    }

    pub fn calc_index(&self, index: &[usize]) -> Option<usize> {
        if self.is_empty() || (self.dims() == 1 && self[0] == Some(1) && index.is_empty()) {
            Some(0)
        } else if index.len() != self.dims() {
            None
        } else {
            let mut ret = 0;
            let mut base = 1;
            for (i, x) in self.iter().enumerate().rev() {
                if let Some(x) = x {
                    ret += index[i] * base;
                    base *= x;
                } else {
                    return None;
                }
            }
            Some(ret)
        }
    }

    pub fn calc_index_expr(&self, index: &[Expression]) -> Option<Expression> {
        if self.is_empty() || (self.dims() == 1 && self[0] == Some(1) && index.is_empty()) {
            let token = TokenRange::default();
            let expr = Expression::create_value(Value::new(0, 32, false), token);
            Some(expr)
        } else if index.len() != self.dims() {
            None
        } else {
            let mut ret = None;
            let mut base = 1;
            for (i, x) in self.iter().enumerate().rev() {
                if let Some(x) = x {
                    let index_expr = index[i].clone();
                    let token = index_expr.token_range();
                    let base_expr =
                        Expression::create_value(Value::new(base as u64, 32, false), token);
                    let expr =
                        Expression::Binary(Box::new(index_expr), Op::Mul, Box::new(base_expr));

                    if let Some(x) = ret {
                        ret = Some(Expression::Binary(Box::new(x), Op::Add, Box::new(expr)));
                    } else {
                        ret = Some(expr);
                    }

                    base *= x;
                } else {
                    return None;
                }
            }
            ret
        }
    }

    pub fn calc_range(&self, index: &[usize]) -> Option<(usize, usize)> {
        if index.len() > self.dims() {
            None
        } else if index.len() < self.dims() {
            let mut beg = index.to_vec();
            let mut end = index.to_vec();
            for (i, x) in self.iter().enumerate() {
                if i >= index.len() {
                    if let Some(x) = x {
                        beg.push(0);
                        end.push(x.saturating_sub(1));
                    } else {
                        return None;
                    }
                }
            }
            let beg = self.calc_index(&beg)?;
            let end = self.calc_index(&end)?;
            Some((beg, end))
        } else {
            self.calc_index(index).map(|x| (x, x))
        }
    }
}

impl ToOwned for ShapeRef {
    type Owned = Shape;

    fn to_owned(&self) -> Self::Owned {
        Shape::new(self.as_slice().to_owned())
    }
}

impl<'a> From<&'a [Option<usize>]> for &'a ShapeRef {
    #[inline]
    fn from(value: &'a [Option<usize>]) -> Self {
        ShapeRef::new(value)
    }
}

impl<'a, const N: usize> From<&'a [Option<usize>; N]> for &'a ShapeRef {
    #[inline]
    fn from(value: &'a [Option<usize>; N]) -> Self {
        ShapeRef::new(value)
    }
}

impl<'a> From<&'a mut [Option<usize>]> for &'a mut ShapeRef {
    #[inline]
    fn from(value: &'a mut [Option<usize>]) -> Self {
        ShapeRef::new_mut(value)
    }
}

impl<'a> From<&'a ShapeRef> for &'a [Option<usize>] {
    #[inline]
    fn from(value: &'a ShapeRef) -> Self {
        value.as_slice()
    }
}

impl<'a> From<&'a mut ShapeRef> for &'a mut [Option<usize>] {
    #[inline]
    fn from(value: &'a mut ShapeRef) -> Self {
        value.as_mut_slice()
    }
}

impl From<&ShapeRef> for Box<ShapeRef> {
    fn from(value: &ShapeRef) -> Self {
        let inner_box: Box<[Option<usize>]> = Box::from(value.as_slice());
        let inner_boxed_ptr: *mut [Option<usize>] = Box::into_raw(inner_box);

        unsafe { Box::from_raw(inner_boxed_ptr as *mut ShapeRef) }
    }
}

impl From<&ShapeRef> for Rc<ShapeRef> {
    fn from(value: &ShapeRef) -> Self {
        let inner_box: Rc<[Option<usize>]> = Rc::from(value.as_slice());
        let inner_boxed_ptr: *const [Option<usize>] = Rc::into_raw(inner_box);

        unsafe { Rc::from_raw(inner_boxed_ptr as *const ShapeRef) }
    }
}

impl From<&ShapeRef> for Arc<ShapeRef> {
    fn from(value: &ShapeRef) -> Self {
        let inner_box: Arc<[Option<usize>]> = Arc::from(value.as_slice());
        let inner_boxed_ptr: *const [Option<usize>] = Arc::into_raw(inner_box);

        unsafe { Arc::from_raw(inner_boxed_ptr as *const ShapeRef) }
    }
}

impl From<Box<[Option<usize>]>> for Box<ShapeRef> {
    fn from(value: Box<[Option<usize>]>) -> Self {
        let boxed_ptr = Box::into_raw(value);
        unsafe { Box::from_raw(boxed_ptr as *mut ShapeRef) }
    }
}

impl From<Vec<Option<usize>>> for Box<ShapeRef> {
    fn from(value: Vec<Option<usize>>) -> Self {
        value.into_boxed_slice().into()
    }
}

impl From<Box<ShapeRef>> for Shape {
    fn from(value: Box<ShapeRef>) -> Self {
        let boxed_slice: Box<[Option<usize>]> = value.into();
        Self::new(Vec::from(boxed_slice))
    }
}

impl From<Box<ShapeRef>> for Box<[Option<usize>]> {
    fn from(value: Box<ShapeRef>) -> Self {
        let boxed_ptr = Box::into_raw(value);
        unsafe { Box::from_raw(boxed_ptr as *mut [Option<usize>]) }
    }
}

impl From<Box<ShapeRef>> for Vec<Option<usize>> {
    fn from(value: Box<ShapeRef>) -> Self {
        let boxed_slice: Box<[Option<usize>]> = value.into();
        boxed_slice.into()
    }
}

impl<'a> From<&'a ShapeRef> for Cow<'a, ShapeRef> {
    #[inline]
    fn from(value: &'a ShapeRef) -> Self {
        Cow::Borrowed(value)
    }
}

impl From<Cow<'_, ShapeRef>> for Box<ShapeRef> {
    fn from(value: Cow<'_, ShapeRef>) -> Self {
        match value {
            Cow::Borrowed(x) => x.into(),
            Cow::Owned(x) => x.into(),
        }
    }
}

impl AsRef<ShapeRef> for ShapeRef {
    #[inline]
    fn as_ref(&self) -> &ShapeRef {
        self
    }
}

impl AsMut<ShapeRef> for ShapeRef {
    #[inline]
    fn as_mut(&mut self) -> &mut ShapeRef {
        self
    }
}

impl AsRef<[Option<usize>]> for ShapeRef {
    #[inline]
    fn as_ref(&self) -> &[Option<usize>] {
        self.as_slice()
    }
}

impl AsMut<[Option<usize>]> for ShapeRef {
    #[inline]
    fn as_mut(&mut self) -> &mut [Option<usize>] {
        self.as_mut_slice()
    }
}

impl AsRef<[Option<usize>]> for Box<ShapeRef> {
    #[inline]
    fn as_ref(&self) -> &[Option<usize>] {
        self.as_slice()
    }
}

impl AsMut<[Option<usize>]> for Box<ShapeRef> {
    #[inline]
    fn as_mut(&mut self) -> &mut [Option<usize>] {
        self.as_mut_slice()
    }
}

impl fmt::Debug for ShapeRef {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for ShapeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, x) in self.0.iter().enumerate() {
            if i != 0 {
                ret.push_str(", ");
            }
            if let Some(x) = x {
                ret.push_str(&x.to_string());
            } else {
                ret.push_str("unknown");
            }
        }

        ret.fmt(f)
    }
}

impl Default for &ShapeRef {
    fn default() -> Self {
        ShapeRef::new(<&[Option<usize>]>::default())
    }
}

impl Default for &mut ShapeRef {
    fn default() -> Self {
        ShapeRef::new_mut(<&mut [Option<usize>]>::default())
    }
}

impl Default for Box<ShapeRef> {
    fn default() -> Self {
        <&ShapeRef>::default().into()
    }
}

impl Index<usize> for ShapeRef {
    type Output = Option<usize>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for ShapeRef {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<RangeFrom<usize>> for ShapeRef {
    type Output = [Option<usize>];

    fn index(&self, index: RangeFrom<usize>) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<RangeFrom<usize>> for ShapeRef {
    fn index_mut(&mut self, index: RangeFrom<usize>) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<Range<usize>> for ShapeRef {
    type Output = [Option<usize>];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<Range<usize>> for ShapeRef {
    fn index_mut(&mut self, index: Range<usize>) -> &mut Self::Output {
        &mut self.0[index]
    }
}

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Shape(Vec<Option<usize>>);

impl Shape {
    #[inline]
    #[must_use]
    pub fn new(x: Vec<Option<usize>>) -> Self {
        Self(x)
    }

    #[inline]
    #[must_use]
    pub fn as_shape_ref(&self) -> &ShapeRef {
        ShapeRef::new(self.0.as_slice())
    }

    #[inline]
    #[must_use]
    pub fn as_shape_ref_mut(&mut self) -> &mut ShapeRef {
        ShapeRef::new_mut(self.0.as_mut_slice())
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_shape_ref().is_empty()
    }

    #[inline]
    pub fn dims(&self) -> usize {
        self.as_shape_ref().dims()
    }

    #[inline]
    pub fn total(&self) -> Option<usize> {
        self.as_shape_ref().total()
    }

    #[inline]
    pub fn get(&self, x: usize) -> Option<&Option<usize>> {
        self.as_shape_ref().get(x)
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize) -> Option<&mut Option<usize>> {
        self.as_shape_ref_mut().get_mut(x)
    }

    #[inline]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Option<usize>> + DoubleEndedIterator {
        self.as_shape_ref().iter()
    }

    #[inline]
    pub fn iter_mut(
        &mut self,
    ) -> impl ExactSizeIterator<Item = &mut Option<usize>> + DoubleEndedIterator {
        self.as_shape_ref_mut().iter_mut()
    }

    #[inline]
    pub fn first(&self) -> Option<&Option<usize>> {
        self.as_shape_ref().first()
    }

    #[inline]
    pub fn first_mut(&mut self) -> Option<&mut Option<usize>> {
        self.as_shape_ref_mut().first_mut()
    }

    #[inline]
    pub fn last(&self) -> Option<&Option<usize>> {
        self.as_shape_ref().last()
    }

    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut Option<usize>> {
        self.as_shape_ref_mut().last_mut()
    }

    #[inline]
    pub fn calc_index(&self, index: &[usize]) -> Option<usize> {
        self.as_shape_ref().calc_index(index)
    }

    #[inline]
    pub fn calc_index_expr(&self, index: &[Expression]) -> Option<Expression> {
        self.as_shape_ref().calc_index_expr(index)
    }

    #[inline]
    pub fn calc_range(&self, index: &[usize]) -> Option<(usize, usize)> {
        self.as_shape_ref().calc_range(index)
    }

    #[inline]
    pub fn append(&mut self, x: &mut Shape) {
        self.0.append(&mut x.0)
    }

    #[inline]
    pub fn remove(&mut self, x: usize) -> Option<usize> {
        self.0.remove(x)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.0.clear()
    }

    #[inline]
    pub fn push(&mut self, x: Option<usize>) {
        self.0.push(x)
    }

    #[inline]
    pub fn pop(&mut self) -> Option<Option<usize>> {
        self.0.pop()
    }

    #[inline]
    pub fn replace(&mut self, i: usize, x: Option<usize>) {
        if i < self.dims() {
            self.0[i] = x;
        }
    }

    #[inline]
    pub fn drain<R: RangeBounds<usize>>(&mut self, x: R) -> Drain<'_, Option<usize>> {
        self.0.drain(x)
    }
}

impl Borrow<ShapeRef> for Shape {
    #[inline]
    fn borrow(&self) -> &ShapeRef {
        self.as_shape_ref()
    }
}

impl BorrowMut<ShapeRef> for Shape {
    #[inline]
    fn borrow_mut(&mut self) -> &mut ShapeRef {
        self.as_shape_ref_mut()
    }
}

impl From<Vec<Option<usize>>> for Shape {
    #[inline]
    fn from(value: Vec<Option<usize>>) -> Self {
        Self::new(value)
    }
}

impl From<&[Option<usize>]> for Shape {
    #[inline]
    fn from(value: &[Option<usize>]) -> Self {
        Self::new(value.to_owned())
    }
}

impl From<Shape> for Vec<Option<usize>> {
    #[inline]
    fn from(value: Shape) -> Self {
        value.0
    }
}

impl From<&ShapeRef> for Shape {
    #[inline]
    fn from(value: &ShapeRef) -> Self {
        value.to_owned()
    }
}

impl From<&mut ShapeRef> for Shape {
    #[inline]
    fn from(value: &mut ShapeRef) -> Self {
        value.to_owned()
    }
}

impl From<&Shape> for Shape {
    #[inline]
    fn from(value: &Shape) -> Self {
        value.clone()
    }
}

impl From<Shape> for Box<ShapeRef> {
    fn from(value: Shape) -> Self {
        let inner_box: Box<[Option<usize>]> = Box::from(value.0);
        let inner_boxed_ptr: *mut [Option<usize>] = Box::into_raw(inner_box);

        unsafe { Box::from_raw(inner_boxed_ptr as *mut ShapeRef) }
    }
}

impl From<Shape> for Rc<ShapeRef> {
    fn from(value: Shape) -> Self {
        let inner_box: Rc<[Option<usize>]> = Rc::from(value.0);
        let inner_boxed_ptr: *const [Option<usize>] = Rc::into_raw(inner_box);

        unsafe { Rc::from_raw(inner_boxed_ptr as *const ShapeRef) }
    }
}

impl From<Shape> for Arc<ShapeRef> {
    fn from(value: Shape) -> Self {
        let inner_box: Arc<[Option<usize>]> = Arc::from(value.0);
        let inner_boxed_ptr: *const [Option<usize>] = Arc::into_raw(inner_box);

        unsafe { Arc::from_raw(inner_boxed_ptr as *const ShapeRef) }
    }
}

impl From<Shape> for Cow<'_, ShapeRef> {
    #[inline]
    fn from(value: Shape) -> Self {
        Cow::Owned(value)
    }
}

impl<'a> From<&'a Shape> for Cow<'a, ShapeRef> {
    #[inline]
    fn from(value: &'a Shape) -> Self {
        Cow::Borrowed(value.as_shape_ref())
    }
}

impl From<Cow<'_, ShapeRef>> for Shape {
    fn from(value: Cow<'_, ShapeRef>) -> Self {
        match value {
            Cow::Borrowed(x) => x.to_owned(),
            Cow::Owned(x) => x,
        }
    }
}

impl Deref for Shape {
    type Target = ShapeRef;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_shape_ref()
    }
}

impl DerefMut for Shape {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_shape_ref_mut()
    }
}

impl AsRef<ShapeRef> for Shape {
    #[inline]
    fn as_ref(&self) -> &ShapeRef {
        self.as_shape_ref()
    }
}

impl AsMut<ShapeRef> for Shape {
    #[inline]
    fn as_mut(&mut self) -> &mut ShapeRef {
        self.as_shape_ref_mut()
    }
}

impl AsRef<[Option<usize>]> for Shape {
    #[inline]
    fn as_ref(&self) -> &[Option<usize>] {
        self.as_slice()
    }
}

impl AsMut<[Option<usize>]> for Shape {
    #[inline]
    fn as_mut(&mut self) -> &mut [Option<usize>] {
        self.as_mut_slice()
    }
}

impl fmt::Debug for Shape {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_shape_ref().fmt(f)
    }
}

impl fmt::Display for Shape {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_shape_ref().fmt(f)
    }
}

impl PartialEq<&ShapeRef> for &Shape {
    fn eq(&self, other: &&ShapeRef) -> bool {
        self.as_shape_ref().eq(other)
    }
}

impl Index<usize> for Shape {
    type Output = Option<usize>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for Shape {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<RangeFrom<usize>> for Shape {
    type Output = [Option<usize>];
    fn index(&self, index: RangeFrom<usize>) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<RangeFrom<usize>> for Shape {
    fn index_mut(&mut self, index: RangeFrom<usize>) -> &mut Self::Output {
        &mut self.0[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_index() {
        let array = Shape::new(vec![Some(2), Some(3), Some(4)]);
        assert_eq!(array.calc_index(&[0, 0, 0]), Some(0));
        assert_eq!(array.calc_index(&[0, 0, 1]), Some(1));
        assert_eq!(array.calc_index(&[0, 0, 2]), Some(2));
        assert_eq!(array.calc_index(&[0, 0, 3]), Some(3));
        assert_eq!(array.calc_index(&[0, 1, 0]), Some(4));
        assert_eq!(array.calc_index(&[0, 1, 1]), Some(5));
        assert_eq!(array.calc_index(&[0, 1, 2]), Some(6));
        assert_eq!(array.calc_index(&[0, 1, 3]), Some(7));
        assert_eq!(array.calc_index(&[0, 2, 0]), Some(8));
        assert_eq!(array.calc_index(&[0, 2, 1]), Some(9));
        assert_eq!(array.calc_index(&[0, 2, 2]), Some(10));
        assert_eq!(array.calc_index(&[0, 2, 3]), Some(11));
        assert_eq!(array.calc_index(&[1, 0, 0]), Some(12));
        assert_eq!(array.calc_index(&[1, 0, 1]), Some(13));
        assert_eq!(array.calc_index(&[1, 0, 2]), Some(14));
        assert_eq!(array.calc_index(&[1, 0, 3]), Some(15));
        assert_eq!(array.calc_index(&[1, 1, 0]), Some(16));
        assert_eq!(array.calc_index(&[1, 1, 1]), Some(17));
        assert_eq!(array.calc_index(&[1, 1, 2]), Some(18));
        assert_eq!(array.calc_index(&[1, 1, 3]), Some(19));
        assert_eq!(array.calc_index(&[1, 2, 0]), Some(20));
        assert_eq!(array.calc_index(&[1, 2, 1]), Some(21));
        assert_eq!(array.calc_index(&[1, 2, 2]), Some(22));
        assert_eq!(array.calc_index(&[1, 2, 3]), Some(23));
    }

    #[test]
    fn test_calc_range() {
        let array = Shape::new(vec![Some(2), Some(3), Some(4)]);
        assert_eq!(array.calc_range(&[0, 0, 0]), Some((0, 0)));
        assert_eq!(array.calc_range(&[0, 0, 1]), Some((1, 1)));
        assert_eq!(array.calc_range(&[0, 0, 2]), Some((2, 2)));
        assert_eq!(array.calc_range(&[0, 0, 3]), Some((3, 3)));
        assert_eq!(array.calc_range(&[0, 1, 0]), Some((4, 4)));
        assert_eq!(array.calc_range(&[0, 1, 1]), Some((5, 5)));
        assert_eq!(array.calc_range(&[0, 1, 2]), Some((6, 6)));
        assert_eq!(array.calc_range(&[0, 1, 3]), Some((7, 7)));
        assert_eq!(array.calc_range(&[0, 2, 0]), Some((8, 8)));
        assert_eq!(array.calc_range(&[0, 2, 1]), Some((9, 9)));
        assert_eq!(array.calc_range(&[0, 2, 2]), Some((10, 10)));
        assert_eq!(array.calc_range(&[0, 2, 3]), Some((11, 11)));
        assert_eq!(array.calc_range(&[1, 0, 0]), Some((12, 12)));
        assert_eq!(array.calc_range(&[1, 0, 1]), Some((13, 13)));
        assert_eq!(array.calc_range(&[1, 0, 2]), Some((14, 14)));
        assert_eq!(array.calc_range(&[1, 0, 3]), Some((15, 15)));
        assert_eq!(array.calc_range(&[1, 1, 0]), Some((16, 16)));
        assert_eq!(array.calc_range(&[1, 1, 1]), Some((17, 17)));
        assert_eq!(array.calc_range(&[1, 1, 2]), Some((18, 18)));
        assert_eq!(array.calc_range(&[1, 1, 3]), Some((19, 19)));
        assert_eq!(array.calc_range(&[1, 2, 0]), Some((20, 20)));
        assert_eq!(array.calc_range(&[1, 2, 1]), Some((21, 21)));
        assert_eq!(array.calc_range(&[1, 2, 2]), Some((22, 22)));
        assert_eq!(array.calc_range(&[1, 2, 3]), Some((23, 23)));
        assert_eq!(array.calc_range(&[0, 0]), Some((0, 3)));
        assert_eq!(array.calc_range(&[0, 1]), Some((4, 7)));
        assert_eq!(array.calc_range(&[0, 2]), Some((8, 11)));
        assert_eq!(array.calc_range(&[1, 0]), Some((12, 15)));
        assert_eq!(array.calc_range(&[1, 1]), Some((16, 19)));
        assert_eq!(array.calc_range(&[1, 2]), Some((20, 23)));
        assert_eq!(array.calc_range(&[0]), Some((0, 11)));
        assert_eq!(array.calc_range(&[1]), Some((12, 23)));
        assert_eq!(array.calc_range(&[]), Some((0, 23)));
    }
}
