//! Phase 1/2 — FerriteCore-inspired memory: palette, SoA ECS, generational handles, arena, intern.

use hashbrown::HashMap;
use smallvec::SmallVec;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Single-value / linear / hashmap palette (Minecraft chunk section).
#[derive(Debug, Clone)]
pub enum Palette {
    Single(u32),
    Linear(SmallVec<[u32; 16]>),
    Hash(HashMap<u32, u16, rustc_hash::FxBuildHasher>),
}

#[derive(Debug, Clone)]
pub struct PalettedContainer {
    pub palette: Palette,
    pub storage: Vec<u64>,
    pub bits: u8,
    pub size: usize,
}

impl PalettedContainer {
    pub fn single(value: u32, size: usize) -> Self {
        Self {
            palette: Palette::Single(value),
            storage: Vec::new(),
            bits: 0,
            size,
        }
    }

    pub fn get(&self, index: usize) -> u32 {
        match &self.palette {
            Palette::Single(v) => *v,
            Palette::Linear(list) => {
                let pi = self.read_index(index) as usize;
                list.get(pi).copied().unwrap_or(0)
            }
            Palette::Hash(map) => {
                let pi = self.read_index(index);
                map.iter()
                    .find(|(_, &v)| v == pi)
                    .map(|(k, _)| *k)
                    .unwrap_or(0)
            }
        }
    }

    pub fn set(&mut self, index: usize, value: u32) {
        match &mut self.palette {
            Palette::Single(v) if *v == value => {}
            Palette::Single(old) => {
                let mut list = SmallVec::new();
                list.push(*old);
                list.push(value);
                self.palette = Palette::Linear(list);
                self.bits = 1;
                self.storage = vec![0u64; (self.size + 63) / 64];
                self.write_index(index, 1);
            }
            Palette::Linear(list) => {
                let pi = if let Some(i) = list.iter().position(|&x| x == value) {
                    i as u16
                } else if list.len() >= 16 {
                    let mut map = HashMap::with_hasher(rustc_hash::FxBuildHasher);
                    for (i, v) in list.iter().enumerate() {
                        map.insert(*v, i as u16);
                    }
                    let id = map.len() as u16;
                    map.insert(value, id);
                    self.palette = Palette::Hash(map);
                    self.ensure_bits(id);
                    self.write_index(index, id);
                    return;
                } else {
                    list.push(value);
                    (list.len() - 1) as u16
                };
                self.ensure_bits(pi);
                self.write_index(index, pi);
            }
            Palette::Hash(map) => {
                let pi = if let Some(&id) = map.get(&value) {
                    id
                } else {
                    let id = map.len() as u16;
                    map.insert(value, id);
                    id
                };
                self.ensure_bits(pi);
                self.write_index(index, pi);
            }
        }
    }

    fn ensure_bits(&mut self, max_id: u16) {
        let need = (16 - max_id.leading_zeros()).max(1) as u8;
        if need > self.bits {
            self.bits = need;
            let words = (self.size * self.bits as usize + 63) / 64;
            self.storage.resize(words, 0);
        }
    }

    fn read_index(&self, index: usize) -> u16 {
        if self.bits == 0 {
            return 0;
        }
        let bit = index * self.bits as usize;
        let word = bit / 64;
        let shift = bit % 64;
        let mask = (1u64 << self.bits) - 1;
        ((self.storage.get(word).copied().unwrap_or(0) >> shift) & mask) as u16
    }

    fn write_index(&mut self, index: usize, value: u16) {
        if self.bits == 0 {
            return;
        }
        let bit = index * self.bits as usize;
        let word = bit / 64;
        let shift = bit % 64;
        let mask = (1u64 << self.bits) - 1;
        if word >= self.storage.len() {
            self.storage.resize(word + 1, 0);
        }
        self.storage[word] =
            (self.storage[word] & !(mask << shift)) | ((value as u64 & mask) << shift);
    }

    pub fn memory_bytes(&self) -> usize {
        let pal = match &self.palette {
            Palette::Single(_) => 4,
            Palette::Linear(l) => l.len() * 4,
            Palette::Hash(m) => m.capacity() * 8,
        };
        pal + self.storage.len() * 8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenHandle {
    pub index: u32,
    pub generation: u32,
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

pub struct GenArena<T> {
    entries: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for GenArena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> GenArena<T> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn insert(&mut self, value: T) -> GenHandle {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.entries[index as usize];
            let gen = slot.generation.wrapping_add(1).max(1);
            slot.generation = gen;
            slot.value = Some(value);
            GenHandle {
                index,
                generation: gen,
            }
        } else {
            let index = self.entries.len() as u32;
            self.entries.push(Slot {
                generation: 1,
                value: Some(value),
            });
            GenHandle {
                index,
                generation: 1,
            }
        }
    }

    pub fn get(&self, h: GenHandle) -> Option<&T> {
        let slot = self.entries.get(h.index as usize)?;
        if slot.generation != h.generation {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn remove(&mut self, h: GenHandle) -> Option<T> {
        let slot = self.entries.get_mut(h.index as usize)?;
        if slot.generation != h.generation {
            return None;
        }
        let v = slot.value.take()?;
        self.free.push(h.index);
        Some(v)
    }
}

/// Bump arena with capacity cap (subsystem scoped).
pub struct BumpArena {
    buf: Vec<u8>,
    offset: usize,
    cap: usize,
}

impl BumpArena {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: vec![0u8; cap],
            offset: 0,
            cap,
        }
    }

    pub fn alloc_bytes(&mut self, len: usize, align: usize) -> Option<&mut [u8]> {
        let align = align.max(1);
        let aligned = (self.offset + align - 1) & !(align - 1);
        let end = aligned.checked_add(len)?;
        if end > self.cap {
            return None;
        }
        self.offset = end;
        Some(&mut self.buf[aligned..end])
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }

    pub fn used(&self) -> usize {
        self.offset
    }
}

/// String interning → u32 ids (Phase 1).
#[derive(Debug, Default)]
pub struct StringInterner {
    to_id: HashMap<Arc<str>, u32, rustc_hash::FxBuildHasher>,
    to_str: Vec<Arc<str>>,
}

impl StringInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.to_id.get(s) {
            return id;
        }
        let id = self.to_str.len() as u32;
        let arc: Arc<str> = Arc::from(s);
        self.to_id.insert(Arc::clone(&arc), id);
        self.to_str.push(arc);
        id
    }

    pub fn resolve(&self, id: u32) -> Option<&str> {
        self.to_str.get(id as usize).map(|s| s.as_ref())
    }
}

/// Minimal archetype ECS SoA store.
#[derive(Debug, Default)]
pub struct ArchetypeStore {
    pub positions_x: Vec<f32>,
    pub positions_y: Vec<f32>,
    pub positions_z: Vec<f32>,
    pub entity_ids: Vec<u32>,
}

impl ArchetypeStore {
    pub fn push(&mut self, id: u32, x: f32, y: f32, z: f32) {
        self.entity_ids.push(id);
        self.positions_x.push(x);
        self.positions_y.push(y);
        self.positions_z.push(z);
    }

    pub fn len(&self) -> usize {
        self.entity_ids.len()
    }
}

pub fn fx_hash_u64(bytes: &[u8]) -> u64 {
    let mut h = FxHasherLite(0);
    bytes.hash(&mut h);
    h.0
}

struct FxHasherLite(u64);
impl Hasher for FxHasherLite {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(5).wrapping_add(b as u64).wrapping_mul(0x517c_c1b7);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_and_intern() {
        let mut p = PalettedContainer::single(1, 4096);
        p.set(0, 2);
        assert_eq!(p.get(0), 2);
        assert_eq!(p.get(1), 1);
        let mut intern = StringInterner::new();
        let a = intern.intern("minecraft:stone");
        let b = intern.intern("minecraft:stone");
        assert_eq!(a, b);
        let mut arena = GenArena::new();
        let h = arena.insert(42u32);
        assert_eq!(arena.get(h), Some(&42));
        assert_eq!(arena.remove(h), Some(42));
        assert!(arena.get(h).is_none());
    }
}
