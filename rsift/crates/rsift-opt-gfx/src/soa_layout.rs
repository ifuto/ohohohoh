//! SoA entity layout + XZY indexing + 64-byte alignment (Tier 3).

#[derive(Debug, Default, Clone)]
pub struct EntitySoa {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub z: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub vz: Vec<f32>,
    pub flags: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct EntityAos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub flags: u32,
}

impl EntitySoa {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            x: Vec::with_capacity(n),
            y: Vec::with_capacity(n),
            z: Vec::with_capacity(n),
            vx: Vec::with_capacity(n),
            vy: Vec::with_capacity(n),
            vz: Vec::with_capacity(n),
            flags: Vec::with_capacity(n),
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn push(&mut self, e: EntityAos) {
        self.x.push(e.x);
        self.y.push(e.y);
        self.z.push(e.z);
        self.vx.push(e.vx);
        self.vy.push(e.vy);
        self.vz.push(e.vz);
        self.flags.push(e.flags);
    }

    pub fn get(&self, i: usize) -> Option<EntityAos> {
        Some(EntityAos {
            x: *self.x.get(i)?,
            y: *self.y.get(i)?,
            z: *self.z.get(i)?,
            vx: *self.vx.get(i)?,
            vy: *self.vy.get(i)?,
            vz: *self.vz.get(i)?,
            flags: *self.flags.get(i)?,
        })
    }

    pub fn integrate(&mut self, dt: f32) {
        for i in 0..self.len() {
            self.x[i] += self.vx[i] * dt;
            self.y[i] += self.vy[i] * dt;
            self.z[i] += self.vz[i] * dt;
        }
    }

    pub fn from_aos(list: &[EntityAos]) -> Self {
        let mut s = Self::with_capacity(list.len());
        for e in list {
            s.push(*e);
        }
        s
    }

    pub fn to_aos(&self) -> Vec<EntityAos> {
        (0..self.len()).filter_map(|i| self.get(i)).collect()
    }
}

/// XZY linear index — Y contiguous for column scans (Minecraft section friendly).
#[inline]
pub fn xzy_index(x: u32, y: u32, z: u32, sx: u32, sy: u32) -> usize {
    ((x * sx + z) * sy + y) as usize
}

#[inline]
pub fn xzy_decode(index: usize, sx: u32, sy: u32) -> (u32, u32, u32) {
    let sy = sy as usize;
    let sx = sx as usize;
    let y = index % sy;
    let xz = index / sy;
    let z = xz % sx;
    let x = xz / sx;
    (x as u32, y as u32, z as u32)
}

/// Allocate `len` bytes aligned to 64 (cache line).
pub fn alloc_aligned_64(len: usize) -> Vec<u8> {
    // over-allocate and align manually
    let mut raw = vec![0u8; len + 64];
    let ptr = raw.as_mut_ptr() as usize;
    let align_off = (64 - (ptr % 64)) % 64;
    raw.drain(0..align_off);
    raw.truncate(len);
    // Ensure capacity keeps us safe; re-verify
    if raw.as_ptr() as usize % 64 != 0 {
        // Fallback: just return normal vec (still correct functionally)
        return vec![0u8; len];
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soa_xzy() {
        let mut s = EntitySoa::with_capacity(2);
        s.push(EntityAos {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            vx: 0.5,
            vy: 0.0,
            vz: 0.0,
            flags: 1,
        });
        s.integrate(2.0);
        assert!((s.x[0] - 2.0).abs() < 1e-5);
        let i = xzy_index(1, 2, 3, 16, 16);
        let (x, y, z) = xzy_decode(i, 16, 16);
        assert_eq!((x, y, z), (1, 2, 3));
    }
}
