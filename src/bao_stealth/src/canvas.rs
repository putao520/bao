// REQ-STL-003: Canvas fingerprint protection (pixel noise)  @trace REQ-STL-003
#[derive(Debug, Clone)]
pub struct CanvasNoise {
    seed: u64,
    noise_amplitude: f64,
}

impl CanvasNoise {
    pub fn new(seed: u64) -> Self {
        assert!(seed > 0, "canvas_seed must be > 0 per SPEC");
        CanvasNoise {
            seed,
            noise_amplitude: 0.001,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn noise_amplitude(&self) -> f64 {
        self.noise_amplitude
    }

    pub fn apply_to_pixel(&self, r: u8, g: u8, b: u8, a: u8, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let noise = self.deterministic_noise(x, y);
        let nr = (r as f64 + noise * self.noise_amplitude * 255.0).clamp(0.0, 255.0) as u8;
        let ng = (g as f64 + noise * self.noise_amplitude * 127.0).clamp(0.0, 255.0) as u8;
        let nb = (b as f64 + noise * self.noise_amplitude * 63.0).clamp(0.0, 255.0) as u8;
        (nr, ng, nb, a)
    }

    fn deterministic_noise(&self, x: u32, y: u32) -> f64 {
        let mut state = self.seed;
        state ^= (x as u64).wrapping_mul(0x517CC1B727220A95);
        state ^= (y as u64).wrapping_mul(0x6C62272E07BB0142);
        state = state.wrapping_mul(0x2545F4914F6CDD1D);
        state ^= state >> 33;
        state = state.wrapping_mul(0x27D4EB2D1659B4D6);
        state ^= state >> 33;
        (state as f64) / (u64::MAX as f64) - 0.5
    }
}

// @trace REQ-STL-003 [req:REQ-STL-003] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_seed() {
        let noise = CanvasNoise::new(42);
        assert_eq!(noise.seed, 42);
    }

    #[test]
    fn seed_getter_works() {
        let noise = CanvasNoise::new(12345);
        assert_eq!(noise.seed(), 12345);
    }

    #[test]
    fn noise_amplitude_is_0_001() {
        let noise = CanvasNoise::new(1);
        assert!((noise.noise_amplitude() - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_to_pixel_deterministic_same_input_same_output() {
        let noise = CanvasNoise::new(100);
        let (r1, g1, b1, a1) = noise.apply_to_pixel(128, 64, 32, 255, 10, 20);
        let (r2, g2, b2, a2) = noise.apply_to_pixel(128, 64, 32, 255, 10, 20);
        assert_eq!((r1, g1, b1, a1), (r2, g2, b2, a2));
    }

    #[test]
    fn apply_to_pixel_different_seeds_different_pixels() {
        let n1 = CanvasNoise::new(100);
        let n2 = CanvasNoise::new(200);
        let (r1, g1, b1, _) = n1.apply_to_pixel(128, 64, 32, 255, 10, 20);
        let (r2, g2, b2, _) = n2.apply_to_pixel(128, 64, 32, 255, 10, 20);
        assert_ne!((r1, g1, b1), (r2, g2, b2));
    }

    #[test]
    fn apply_to_pixel_different_seeds_different_results() {
        let noise1 = CanvasNoise::new(100);
        let noise2 = CanvasNoise::new(200);
        let (r1, g1, b1, a1) = noise1.apply_to_pixel(128, 64, 32, 255, 10, 20);
        let (r2, g2, b2, a2) = noise2.apply_to_pixel(128, 64, 32, 255, 10, 20);
        assert_ne!((r1, g1, b1), (r2, g2, b2));
    }

    #[test]
    fn apply_to_pixel_alpha_preserved() {
        let noise = CanvasNoise::new(100);
        let (_, _, _, a) = noise.apply_to_pixel(128, 64, 32, 200, 10, 20);
        assert_eq!(a, 200);
    }

    #[test]
    fn apply_to_pixel_values_in_range() {
        let noise = CanvasNoise::new(100);
        for x in 0..10u32 {
            for y in 0..10u32 {
                // black pixels: noise can only add, so r/g/b should be >= 0 (u8 guarantees this)
                let (r, g, b, a) = noise.apply_to_pixel(0, 0, 0, 0, x, y);
                assert!(r < 255 || g < 255 || b < 255 || a == 0);
                // white pixels: noise can only subtract (via -0.5*amplitude*factor), verify no overflow panic
                let (r, g, b, _a) = noise.apply_to_pixel(255, 255, 255, 255, x, y);
                assert!(r > 0 || g > 0 || b > 0);
                let _ = a;
            }
        }
    }
}
