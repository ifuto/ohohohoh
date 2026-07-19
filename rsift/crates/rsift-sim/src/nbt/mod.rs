//! Phase 2 — zero-copy NBT view over borrowed bytes + registry helpers.

use rustc_hash::FxHashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NbtError {
    #[error("truncated")]
    Truncated,
    #[error("bad type")]
    BadType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TagId {
    End = 0,
    Byte = 1,
    Short = 2,
    Int = 3,
    Long = 4,
    Float = 5,
    Double = 6,
    ByteArray = 7,
    String = 8,
    List = 9,
    Compound = 10,
    IntArray = 11,
    LongArray = 12,
}

/// Zero-copy cursor over NBT payload (does not allocate for primitives / string slices).
pub struct NbtCursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> NbtCursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], NbtError> {
        if self.pos + n > self.data.len() {
            return Err(NbtError::Truncated);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn read_u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }

    pub fn read_i16(&mut self) -> Result<i16, NbtError> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_i32(&mut self) -> Result<i32, NbtError> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i64(&mut self) -> Result<i64, NbtError> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes(b.try_into().unwrap()))
    }

    pub fn read_f32(&mut self) -> Result<f32, NbtError> {
        Ok(f32::from_bits(self.read_i32()? as u32))
    }

    pub fn read_string(&mut self) -> Result<&'a str, NbtError> {
        let len = self.read_i16()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).map_err(|_| NbtError::BadType)
    }

    /// Skip a full named tag payload starting at type byte (for lazy walk).
    pub fn skip_payload(&mut self, tag: TagId) -> Result<(), NbtError> {
        match tag {
            TagId::End => Ok(()),
            TagId::Byte => {
                self.take(1)?;
                Ok(())
            }
            TagId::Short => {
                self.take(2)?;
                Ok(())
            }
            TagId::Int | TagId::Float => {
                self.take(4)?;
                Ok(())
            }
            TagId::Long | TagId::Double => {
                self.take(8)?;
                Ok(())
            }
            TagId::ByteArray => {
                let n = self.read_i32()? as usize;
                self.take(n)?;
                Ok(())
            }
            TagId::String => {
                let _ = self.read_string()?;
                Ok(())
            }
            TagId::List => {
                let inner = self.read_u8()?;
                let n = self.read_i32()? as usize;
                let tid = tag_from_u8(inner)?;
                for _ in 0..n {
                    self.skip_payload(tid)?;
                }
                Ok(())
            }
            TagId::Compound => loop {
                let t = self.read_u8()?;
                if t == 0 {
                    break Ok(());
                }
                let _name = self.read_string()?;
                self.skip_payload(tag_from_u8(t)?)?;
            },
            TagId::IntArray => {
                let n = self.read_i32()? as usize;
                self.take(n * 4)?;
                Ok(())
            }
            TagId::LongArray => {
                let n = self.read_i32()? as usize;
                self.take(n * 8)?;
                Ok(())
            }
        }
    }
}

fn tag_from_u8(v: u8) -> Result<TagId, NbtError> {
    Ok(match v {
        0 => TagId::End,
        1 => TagId::Byte,
        2 => TagId::Short,
        3 => TagId::Int,
        4 => TagId::Long,
        5 => TagId::Float,
        6 => TagId::Double,
        7 => TagId::ByteArray,
        8 => TagId::String,
        9 => TagId::List,
        10 => TagId::Compound,
        11 => TagId::IntArray,
        12 => TagId::LongArray,
        _ => return Err(NbtError::BadType),
    })
}

/// Registry: string → dense u32 id.
#[derive(Debug, Default)]
pub struct RegistryU32 {
    to_id: FxHashMap<String, u32>,
    to_name: Vec<String>,
}

impl RegistryU32 {
    pub fn register(&mut self, name: impl Into<String>) -> u32 {
        let name = name.into();
        if let Some(&id) = self.to_id.get(&name) {
            return id;
        }
        let id = self.to_name.len() as u32;
        self.to_id.insert(name.clone(), id);
        self.to_name.push(name);
        id
    }

    pub fn get_id(&self, name: &str) -> Option<u32> {
        self.to_id.get(name).copied()
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.to_name.get(id as usize).map(|s| s.as_str())
    }
}

/// Tag membership as roaring-ish bitset over registry ids (compact Vec<u64>).
#[derive(Debug, Clone, Default)]
pub struct TagBitset {
    bits: Vec<u64>,
}

impl TagBitset {
    pub fn insert(&mut self, id: u32) {
        let w = (id / 64) as usize;
        let b = id % 64;
        if w >= self.bits.len() {
            self.bits.resize(w + 1, 0);
        }
        self.bits[w] |= 1u64 << b;
    }

    pub fn contains(&self, id: u32) -> bool {
        let w = (id / 64) as usize;
        let b = id % 64;
        self.bits.get(w).map(|x| (x >> b) & 1 != 0).unwrap_or(false)
    }
}

/// Recipe index by ingredient id → recipe ids.
#[derive(Debug, Default)]
pub struct RecipeIndex {
    by_ingredient: FxHashMap<u32, Vec<u32>>,
    recipes: FxHashMap<u32, Vec<u32>>, // recipe → ingredients
}

impl RecipeIndex {
    pub fn add_recipe(&mut self, recipe_id: u32, ingredients: &[u32]) {
        self.recipes.insert(recipe_id, ingredients.to_vec());
        for &ing in ingredients {
            self.by_ingredient.entry(ing).or_default().push(recipe_id);
        }
    }

    pub fn find_by_ingredient(&self, ingredient: u32) -> &[u32] {
        self.by_ingredient
            .get(&ingredient)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_tags() {
        let mut r = RegistryU32::default();
        let stone = r.register("minecraft:stone");
        let mut tags = TagBitset::default();
        tags.insert(stone);
        assert!(tags.contains(stone));
        let mut recipes = RecipeIndex::default();
        recipes.add_recipe(1, &[stone]);
        assert!(recipes.find_by_ingredient(stone).contains(&1));
    }
}
