//! Front-to-back / back-to-front object sorting to cut overdraw.
//!
//! Opaque geometry drawn front-to-back lets the depth buffer reject
//! (early-Z) hidden fragments, so far fewer pixel-shader invocations are
//! needed. That directly helps integrated GPUs with limited fill rate.

#[derive(Debug, Clone, Copy)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Vec3 {
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderObject {
    pub id: u32,
    pub position: Vec3,
    pub bounding_radius: f32,
}

pub struct OverdrawSorter;
impl OverdrawSorter {
    /// Distance from the camera to an object's center.
    pub fn distance(cam: Vec3, obj: &RenderObject) -> f32 {
        (obj.position - cam).length()
    }

    /// Front-to-back (nearest first) — for opaque geometry / early-Z.
    pub fn sort_front_to_back(cam: Vec3, objects: &[RenderObject]) -> Vec<RenderObject> {
        let mut v = objects.to_vec();
        v.sort_by(|a, b| {
            Self::distance(cam, a)
                .partial_cmp(&Self::distance(cam, b))
                .unwrap()
        });
        v
    }

    /// Back-to-front (farthest first) — for alpha-blended transparency.
    pub fn sort_back_to_front(cam: Vec3, objects: &[RenderObject]) -> Vec<RenderObject> {
        let mut v = Self::sort_front_to_back(cam, objects);
        v.reverse();
        v
    }

    pub fn wgsl_source(&self) -> &'static str {
        OVERDRAW_SORT_WGSL
    }
}

pub const OVERDRAW_SORT_WGSL: &str = include_str!("../shaders/overdraw_sort.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(id: u32, x: f32) -> RenderObject {
        RenderObject {
            id,
            position: Vec3::new(x, 0.0, 0.0),
            bounding_radius: 1.0,
        }
    }

    #[test]
    fn front_to_back_nearest_first() {
        let cam = Vec3::new(0.0, 0.0, 0.0);
        let objs = vec![obj(1, 10.0), obj(2, 1.0), obj(3, 5.0)];
        let s = OverdrawSorter::sort_front_to_back(cam, &objs);
        assert_eq!(s[0].id, 2);
        assert_eq!(s[1].id, 3);
        assert_eq!(s[2].id, 1);
    }

    #[test]
    fn back_to_front_farthest_first() {
        let cam = Vec3::new(0.0, 0.0, 0.0);
        let objs = vec![obj(1, 10.0), obj(2, 1.0), obj(3, 5.0)];
        let s = OverdrawSorter::sort_back_to_front(cam, &objs);
        assert_eq!(s[0].id, 1);
        assert_eq!(s[2].id, 2);
    }
}
