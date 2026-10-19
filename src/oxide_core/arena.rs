//! Typed arena used by the staged core.
//!
//! Records are stored in a dense `Vec`; liveness and mark bits are side
//! bitmaps, and a free list recycles slots. This is the quickjs-oxide heap
//! layout without the VM integration yet.

use std::marker::PhantomData;

/// A dense arena with side liveness and mark bitmaps.
pub struct Arena<T> {
    records: Vec<Option<T>>,
    live: Vec<u64>,
    marks: Vec<u64>,
    free: Vec<u32>,
    _marker: PhantomData<T>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            live: Vec::new(),
            marks: Vec::new(),
            free: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
            live: vec![0; capacity.div_ceil(64)],
            marks: vec![0; capacity.div_ceil(64)],
            free: Vec::new(),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[inline]
    pub fn is_live(&self, index: u32) -> bool {
        let index = index as usize;
        index < self.records.len()
            && self.live.get(index / 64).is_some_and(|word| {
                word & (1u64 << (index % 64)) != 0
            })
    }

    #[inline]
    pub fn get(&self, index: u32) -> Option<&T> {
        if !self.is_live(index) {
            return None;
        }
        self.records[index as usize].as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, index: u32) -> Option<&mut T> {
        if !self.is_live(index) {
            return None;
        }
        self.records[index as usize].as_mut()
    }

    pub fn alloc(&mut self, value: T) -> u32 {
        let index = if let Some(index) = self.free.pop() {
            self.records[index as usize] = Some(value);
            index
        } else {
            let index = self.records.len() as u32;
            self.records.push(Some(value));
            index
        };
        self.ensure_bits(index as usize);
        self.set_live(index as usize);
        self.clear_mark(index as usize);
        index
    }

    pub fn alloc_with(&mut self, create: impl FnOnce(u32) -> T) -> u32 {
        let index = self.free.pop().unwrap_or(self.records.len() as u32);
        if index as usize == self.records.len() {
            self.records.push(Some(create(index)));
        } else {
            self.records[index as usize] = Some(create(index));
        }
        self.ensure_bits(index as usize);
        self.set_live(index as usize);
        self.clear_mark(index as usize);
        index
    }

    /// Drop the record and return its slot to the free list. The caller must
    /// clear outgoing references before calling this; the arena only owns the
    /// record's storage.
    pub fn free(&mut self, index: u32) -> Option<T> {
        if !self.is_live(index) {
            return None;
        }
        self.clear_live(index as usize);
        self.clear_mark(index as usize);
        self.free.push(index);
        self.records[index as usize].take()
    }

    #[inline]
    pub fn mark(&mut self, index: u32) {
        if self.is_live(index) {
            let index = index as usize;
            self.marks[index / 64] |= 1u64 << (index % 64);
        }
    }

    #[inline]
    pub fn is_marked(&self, index: u32) -> bool {
        let index = index as usize;
        index < self.records.len()
            && self
                .marks
                .get(index / 64)
                .is_some_and(|word| word & (1u64 << (index % 64)) != 0)
    }

    /// Free every live record that was not marked during this cycle.
    pub fn sweep(&mut self) -> Vec<u32> {
        let mut dead = Vec::new();
        for index in 0..self.records.len() {
            if self.is_live(index as u32) && !self.is_marked(index as u32) {
                self.clear_live(index);
                self.free.push(index as u32);
                self.records[index].take();
                dead.push(index as u32);
            }
            self.clear_mark(index);
        }
        dead
    }

    fn ensure_bits(&mut self, index: usize) {
        let words = index / 64 + 1;
        if self.live.len() < words {
            self.live.resize(words, 0);
            self.marks.resize(words, 0);
        }
    }

    fn set_live(&mut self, index: usize) {
        self.live[index / 64] |= 1u64 << (index % 64);
    }

    fn clear_live(&mut self, index: usize) {
        self.live[index / 64] &= !(1u64 << (index % 64));
    }

    fn clear_mark(&mut self, index: usize) {
        self.marks[index / 64] &= !(1u64 << (index % 64));
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Arena;

    #[test]
    fn alloc_free_and_reuse() {
        let mut arena = Arena::new();
        let a = arena.alloc(10);
        let b = arena.alloc(20);
        assert_eq!(arena.get(a), Some(&10));
        assert_eq!(arena.get(b), Some(&20));
        arena.free(a);
        assert!(!arena.is_live(a));
        let c = arena.alloc(30);
        assert_eq!(c, a);
        assert_eq!(arena.get(c), Some(&30));
    }

    #[test]
    fn marks_and_sweep() {
        let mut arena = Arena::new();
        let a = arena.alloc(1);
        let b = arena.alloc(2);
        arena.mark(a);
        let dead = arena.sweep();
        assert_eq!(dead, vec![b]);
        assert!(arena.is_live(a));
        assert!(!arena.is_live(b));
    }
}
