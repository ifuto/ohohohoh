//! AO precompute at mesh time (Tier 1) — Minecraft smooth-lighting corner AO.
//! Bakes 0..=3 into vertex so fragment shader stays cheap (Sodium LightPipeline idea).

/// 3×3×3 neighborhood centered on the face's outward cell.
/// `true` = opaque occluder.
pub type AoNeighborhood = [[[bool; 3]; 3]; 3];

/// Classic Minecraft corner AO: side1 + side2 + corner, clamped 0..=3.
#[inline]
pub fn corner_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        return 0;
    }
    let mut v = 0u8;
    if side1 {
        v += 1;
    }
    if side2 {
        v += 1;
    }
    if corner {
        v += 1;
    }
    3 - v.min(3)
}

/// Bake four corners for a face. `face`: 0=+X 1=-X 2=+Y 3=-Y 4=+Z 5=-Z.
/// Neighborhood index: [x+1][y+1][z+1] is center (the face origin block).
pub fn bake_face_ao(n: &AoNeighborhood, face: u8) -> [u8; 4] {
    // Helper: opaque at offset from center
    let o = |dx: i32, dy: i32, dz: i32| -> bool {
        n[(1 + dx) as usize][(1 + dy) as usize][(1 + dz) as usize]
    };
    match face {
        0 => {
            // +X
            [
                corner_ao(o(1, 0, -1), o(1, -1, 0), o(1, -1, -1)),
                corner_ao(o(1, 0, 1), o(1, -1, 0), o(1, -1, 1)),
                corner_ao(o(1, 0, 1), o(1, 1, 0), o(1, 1, 1)),
                corner_ao(o(1, 0, -1), o(1, 1, 0), o(1, 1, -1)),
            ]
        }
        1 => {
            // -X
            [
                corner_ao(o(-1, 0, 1), o(-1, -1, 0), o(-1, -1, 1)),
                corner_ao(o(-1, 0, -1), o(-1, -1, 0), o(-1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(-1, 1, 0), o(-1, 1, -1)),
                corner_ao(o(-1, 0, 1), o(-1, 1, 0), o(-1, 1, 1)),
            ]
        }
        2 => {
            // +Y
            [
                corner_ao(o(-1, 1, 0), o(0, 1, -1), o(-1, 1, -1)),
                corner_ao(o(1, 1, 0), o(0, 1, -1), o(1, 1, -1)),
                corner_ao(o(1, 1, 0), o(0, 1, 1), o(1, 1, 1)),
                corner_ao(o(-1, 1, 0), o(0, 1, 1), o(-1, 1, 1)),
            ]
        }
        3 => {
            // -Y
            [
                corner_ao(o(-1, -1, 0), o(0, -1, 1), o(-1, -1, 1)),
                corner_ao(o(1, -1, 0), o(0, -1, 1), o(1, -1, 1)),
                corner_ao(o(1, -1, 0), o(0, -1, -1), o(1, -1, -1)),
                corner_ao(o(-1, -1, 0), o(0, -1, -1), o(-1, -1, -1)),
            ]
        }
        4 => {
            // +Z
            [
                corner_ao(o(-1, 0, 1), o(0, -1, 1), o(-1, -1, 1)),
                corner_ao(o(1, 0, 1), o(0, -1, 1), o(1, -1, 1)),
                corner_ao(o(1, 0, 1), o(0, 1, 1), o(1, 1, 1)),
                corner_ao(o(-1, 0, 1), o(0, 1, 1), o(-1, 1, 1)),
            ]
        }
        _ => {
            // -Z
            [
                corner_ao(o(1, 0, -1), o(0, -1, -1), o(1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(0, -1, -1), o(-1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(0, 1, -1), o(-1, 1, -1)),
                corner_ao(o(1, 0, -1), o(0, 1, -1), o(1, 1, -1)),
            ]
        }
    }
}

/// Average corner AO as light multiplier 0.2..=1.0 (shader-friendly).
#[inline]
pub fn ao_to_shade(ao: u8) -> f32 {
    match ao {
        0 => 0.2,
        1 => 0.45,
        2 => 0.7,
        _ => 1.0,
    }
}

/// Pack 4 corner AO (2 bits each) into one byte.
#[inline]
pub fn pack_ao4(corners: [u8; 4]) -> u8 {
    (corners[0] & 3)
        | ((corners[1] & 3) << 2)
        | ((corners[2] & 3) << 4)
        | ((corners[3] & 3) << 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosed_corner_is_dark() {
        assert_eq!(corner_ao(true, true, true), 0);
        assert_eq!(corner_ao(false, false, false), 3);
    }

    #[test]
    fn face_ao_open_sky() {
        let n = [[[false; 3]; 3]; 3];
        let ao = bake_face_ao(&n, 2);
        assert_eq!(ao, [3, 3, 3, 3]);
    }
}
