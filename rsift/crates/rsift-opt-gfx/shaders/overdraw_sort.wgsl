// Overdraw reduction (WGSL). The actual sort runs on CPU; this records the in-shader
// contract: render opaque draws front-to-back and keep depth writes ON in the color
// pass so early-Z rejects hidden fragments without running the pixel shader.

fn DepthPassBenefit() -> bool {
    // True when front-to-back order + early-Z is enabled.
    return true;
}
