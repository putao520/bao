// REQ-STL-003: Canvas fingerprint protection (pixel noise)
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
