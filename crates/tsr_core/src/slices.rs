//! Retained slice headers for ports where identity, aliasing and nil are part
//! of the API. Ordinary slices/Vec remain preferable when those properties
//! are not observable. Cloning this handle retains the backing and range.
use std::{
    ops::{Deref, Range},
    sync::{Arc, RwLock, RwLockReadGuard},
};

#[derive(Debug)]
pub struct SharedSlice<T> {
    backing: Option<Arc<RwLock<Vec<T>>>>,
    start: usize,
    len: usize,
    capacity: usize,
}
impl<T> Default for SharedSlice<T> {
    fn default() -> Self {
        Self {
            backing: None,
            start: 0,
            len: 0,
            capacity: 0,
        }
    }
}
impl<T> Clone for SharedSlice<T> {
    fn clone(&self) -> Self {
        Self {
            backing: self.backing.clone(),
            start: self.start,
            len: self.len,
            capacity: self.capacity,
        }
    }
}
pub struct SliceRead<'a, T> {
    guard: Option<RwLockReadGuard<'a, Vec<T>>>,
    start: usize,
    len: usize,
}
impl<T> Deref for SliceRead<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.guard
            .as_ref()
            .map_or(&[], |values| &values[self.start..self.start + self.len])
    }
}
impl<T: PartialEq + Clone> PartialEq for SharedSlice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}
impl<T: Eq + Clone> Eq for SharedSlice<T> {}
impl<T> From<Vec<T>> for SharedSlice<T> {
    fn from(values: Vec<T>) -> Self {
        Self::from_vec(values)
    }
}
impl<T> FromIterator<T> for SharedSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Self::from_vec(items.into_iter().collect())
    }
}
pub struct SliceIter<'a, T> {
    slice: &'a SharedSlice<T>,
    index: usize,
}
impl<T: Clone> Iterator for SliceIter<'_, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        let value = self.slice.get(self.index)?;
        self.index += 1;
        Some(value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.slice.len() - self.index;
        (n, Some(n))
    }
}
impl<T: Clone> ExactSizeIterator for SliceIter<'_, T> {}
impl<'a, T: Clone> IntoIterator for &'a SharedSlice<T> {
    type Item = T;
    type IntoIter = SliceIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<T: Clone> IntoIterator for SharedSlice<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.read().to_vec().into_iter()
    }
}
impl<T> SharedSlice<T> {
    pub fn from_vec(values: Vec<T>) -> Self {
        let len = values.len();
        Self {
            backing: Some(Arc::new(RwLock::new(values))),
            start: 0,
            len,
            capacity: len,
        }
    }
    pub fn is_nil(&self) -> bool {
        self.backing.is_none()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn read(&self) -> SliceRead<'_, T> {
        SliceRead {
            guard: self.backing.as_ref().map(|values| {
                values
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            }),
            start: self.start,
            len: self.len,
        }
    }
    /// Mutation callbacks must not recursively read or write this backing.
    pub fn update<R>(&mut self, update: impl FnOnce(&mut [T]) -> R) -> R {
        if let Some(backing) = &self.backing {
            let mut values = backing
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            update(&mut values[self.start..self.start + self.len])
        } else {
            update(&mut [])
        }
    }
    pub fn update_each(&mut self, mut update: impl FnMut(&mut T)) {
        if let Some(backing) = &self.backing {
            let mut values = backing
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for value in &mut values[self.start..self.start + self.len] {
                update(value);
            }
        }
    }
    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Self {
        assert!(
            range.start <= range.end && range.end <= self.len,
            "slice bounds out of range"
        );
        Self {
            backing: self.backing.clone(),
            start: self.start + range.start,
            len: range.len(),
            capacity: self.capacity - range.start,
        }
    }
    pub fn set(&mut self, index: usize, value: T) {
        assert!(index < self.len, "index out of range");
        self.backing
            .as_ref()
            .unwrap()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[self.start + index] = value;
    }
    /// Empty slices compare identically, including nil and distinct backings.
    /// port: tsc/internal/core/core.go:Same
    pub fn same(&self, other: &Self) -> bool {
        self.len == other.len
            && (self.len == 0
                || self.start == other.start
                    && matches!((&self.backing,&other.backing),(Some(a),Some(b)) if Arc::ptr_eq(a,b)))
    }
}
impl<T: Clone> SharedSlice<T> {
    pub fn get(&self, index: usize) -> Option<T> {
        self.read().get(index).cloned()
    }
    pub fn first(&self) -> Option<T> {
        self.get(0)
    }
    pub fn iter(&self) -> SliceIter<'_, T> {
        SliceIter {
            slice: self,
            index: 0,
        }
    }
    pub fn to_vec(&self) -> Vec<T> {
        self.read().to_vec()
    }

    pub(crate) fn remove_preserving_tail(&mut self, index: usize) {
        assert!(index < self.len, "index out of range");
        let mut values = self
            .backing
            .as_ref()
            .unwrap()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for i in self.start + index..self.start + self.len - 1 {
            values[i] = values[i + 1].clone();
        }
        self.len -= 1;
    }
    pub fn push(&mut self, value: T)
    where
        T: GoSliceElement,
    {
        self.extend_from_slice(&[value]);
    }
    /// Append one batch, retaining Go's growth decision for the whole batch.
    pub fn extend_from_slice(&mut self, added: &[T])
    where
        T: GoSliceElement,
    {
        let len = self
            .len
            .checked_add(added.len())
            .expect("slice length overflow");
        if len > self.capacity {
            let double = self
                .capacity
                .checked_mul(2)
                .expect("slice capacity overflow");
            let capacity = if len > double {
                len
            } else if self.capacity < 256 {
                double
            } else {
                let mut capacity = self.capacity;
                while capacity < len {
                    capacity = capacity
                        .checked_add((capacity + 3 * 256) / 4)
                        .expect("slice capacity overflow");
                }
                capacity
            };
            let capacity = rounded_capacity::<T>(capacity);
            let mut values = Vec::with_capacity(capacity);
            values.extend_from_slice(&self.read());
            values.extend_from_slice(added);
            self.backing = Some(Arc::new(RwLock::new(values)));
            self.start = 0;
            self.len = len;
            self.capacity = capacity;
        } else if !added.is_empty() {
            let mut values = self
                .backing
                .as_ref()
                .unwrap()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (offset, value) in added.iter().enumerate() {
                let index = self.start + self.len + offset;
                if index < values.len() {
                    values[index] = value.clone();
                } else {
                    values.push(value.clone());
                }
            }
            self.len = len;
        }
    }
}

/// The source element layout is explicit because Rust strings and handles have
/// different sizes from their Go counterparts. Only append's *observable
/// sharing* uses this layout; Rust still owns and allocates ordinary T values.
pub trait GoSliceElement: Clone {
    const GO_SIZE: usize;
    const GO_POINTERS: bool;
}
macro_rules! plain_element { ($($ty:ty),*) => {$(impl GoSliceElement for $ty {
    const GO_SIZE: usize = std::mem::size_of::<Self>();
    const GO_POINTERS: bool = false;
})*}; }
plain_element!(u8, i8, u16, i16, u32, i32, u64, i64, usize, isize, bool, f32, f64);
impl GoSliceElement for String {
    const GO_SIZE: usize = 2 * std::mem::size_of::<usize>();
    const GO_POINTERS: bool = true;
}
impl GoSliceElement for tsr_jsstring::JsString {
    const GO_SIZE: usize = 2 * std::mem::size_of::<usize>();
    const GO_POINTERS: bool = true;
}
impl<T> GoSliceElement for Arc<T> {
    const GO_SIZE: usize = std::mem::size_of::<usize>();
    const GO_POINTERS: bool = true;
}
impl<T: GoSliceElement> SharedSlice<T> {
    /// Copy via Go's append-to-empty allocation policy (slices.Clone/Concat).
    pub fn copied(values: &[T]) -> Self {
        let mut out = Self::from_vec(values.to_vec());
        if !values.is_empty() {
            out.capacity = rounded_capacity::<T>(values.len());
        }
        out
    }
}
fn rounded_capacity<T: GoSliceElement>(count: usize) -> usize {
    // runtime.growslice + roundupsize in the pinned Go 1.27.1. Size-class
    // rounding affects when a later append detaches an aliased slice header.
    const CLASSES: &[usize] = &[
        8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320,
        352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792,
        2048, 2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472,
        9728, 10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264,
        28672, 32768,
    ];
    assert!(
        T::GO_SIZE != 0,
        "zero-sized Go slice elements need a separate identity contract"
    );
    let bytes = count
        .checked_mul(T::GO_SIZE)
        .expect("slice capacity overflow");
    let rounded = if bytes <= 32768 - 8 {
        let header =
            if T::GO_POINTERS && bytes > std::mem::size_of::<usize>() * usize::BITS as usize {
                8
            } else {
                0
            };
        let index = CLASSES.partition_point(|size| *size < bytes + header);
        CLASSES[index] - header
    } else {
        bytes.checked_add(8191).expect("slice capacity overflow") & !8191
    };
    rounded / T::GO_SIZE
}
