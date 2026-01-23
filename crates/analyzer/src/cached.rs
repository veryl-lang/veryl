use std::cell::{Cell, RefCell};
use std::{cmp, fmt, hash};

#[derive(Clone, Default)]
pub struct CachedCopy<T: Copy + Default> {
    payload: Cell<T>,
    cached: Cell<bool>,
}

impl<T: Copy + Default> CachedCopy<T> {
    pub fn clear(&self) {
        self.cached.replace(false);
    }

    pub fn get(&self) -> Option<T> {
        if self.cached.get() {
            Some(self.payload.get())
        } else {
            None
        }
    }

    pub fn set(&self, x: T) {
        self.payload.replace(x);
        self.cached.replace(true);
    }
}

impl<T: Copy + Default> fmt::Debug for CachedCopy<T> {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

impl<T: Copy + Default> PartialEq for CachedCopy<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T: Copy + Default> Eq for CachedCopy<T> {}

impl<T: Copy + Default> PartialOrd for CachedCopy<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Copy + Default> Ord for CachedCopy<T> {
    fn cmp(&self, _other: &Self) -> cmp::Ordering {
        cmp::Ordering::Equal
    }
}

impl<T: Copy + Default> hash::Hash for CachedCopy<T> {
    fn hash<H: hash::Hasher>(&self, _state: &mut H) {}
}

#[derive(Clone, Default)]
pub struct CachedRef<T: Default> {
    payload: RefCell<T>,
    cached: Cell<bool>,
}

impl<T: Default> CachedRef<T> {
    pub fn clear(&self) {
        self.cached.replace(false);
    }

    pub fn get(&self) -> Option<&T> {
        if self.cached.get() {
            let ret = unsafe { self.payload.try_borrow_unguarded().unwrap() };
            Some(ret)
        } else {
            None
        }
    }

    pub fn set(&self, x: T) {
        self.payload.replace(x);
        self.cached.replace(true);
    }
}

impl<T: Default> fmt::Debug for CachedRef<T> {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

impl<T: Default> PartialEq for CachedRef<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T: Default> Eq for CachedRef<T> {}

impl<T: Default> PartialOrd for CachedRef<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Default> Ord for CachedRef<T> {
    fn cmp(&self, _other: &Self) -> cmp::Ordering {
        cmp::Ordering::Equal
    }
}

impl<T: Default> hash::Hash for CachedRef<T> {
    fn hash<H: hash::Hasher>(&self, _state: &mut H) {}
}
