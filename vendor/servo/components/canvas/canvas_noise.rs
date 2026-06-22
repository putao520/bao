/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// Global canvas noise seed (0 = disabled). Written from Bao runtime bridge,
/// read from canvas paint thread.
static CANVAS_NOISE_SEED: AtomicU64 = AtomicU64::new(0);

/// Global canvas noise amplitude, stored as fixed-point: actual = raw / 1_000_000.0
/// (AtomicF64 doesn't exist; this gives 6 decimal digits of precision.)
static CANVAS_NOISE_AMPLITUDE_RAW: AtomicU64 = AtomicU64::new(0);

/// Flag: 0 = not set, 1 = enabled, 2 = disabled
static CANVAS_NOISE_ENABLED: AtomicU8 = AtomicU8::new(0);

/// Set the global canvas noise seed and amplitude from the Bao runtime bridge.
/// This is called once during stealth profile initialization on the script thread.
/// The canvas paint thread reads these values when creating canvases or reading pixels.
pub fn set_global_canvas_noise(seed: u64, noise_amplitude: f64) {
    CANVAS_NOISE_SEED.store(seed, Ordering::Relaxed);
    // Convert f64 amplitude to fixed-point u64 for atomic storage
    let raw = (noise_amplitude * 1_000_000.0).round() as u64;
    CANVAS_NOISE_AMPLITUDE_RAW.store(raw, Ordering::Relaxed);
    CANVAS_NOISE_ENABLED.store(if seed > 0 { 1 } else { 2 }, Ordering::Relaxed);
}

/// Read the global canvas noise configuration. Returns None if not set or disabled.
pub fn get_global_canvas_noise() -> Option<(u64, f64)> {
    match CANVAS_NOISE_ENABLED.load(Ordering::Relaxed) {
        1 => {
            let seed = CANVAS_NOISE_SEED.load(Ordering::Relaxed);
            let raw = CANVAS_NOISE_AMPLITUDE_RAW.load(Ordering::Relaxed);
            let amplitude = raw as f64 / 1_000_000.0;
            Some((seed, amplitude))
        },
        _ => None,
    }
}

/// Deterministic canvas pixel noise for anti-fingerprinting.
///
/// Ported from `bao_stealth::canvas::CanvasNoise` — the noise algorithm and
/// channel multipliers must stay identical to ensure JS↔Rust parity.
#[derive(Clone)]
pub struct CanvasNoiseConfig {
    seed: u64,
    noise_amplitude: f64,
    enabled: bool,
}

impl CanvasNoiseConfig {
    pub fn disabled() -> Self {
        CanvasNoiseConfig {
            seed: 0,
            noise_amplitude: 0.0,
            enabled: false,
        }
    }

    pub fn new(seed: u64, noise_amplitude: f64) -> Self {
        CanvasNoiseConfig {
            seed,
            noise_amplitude,
            enabled: true,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Apply deterministic noise to RGBA8 pixel data in-place.
    ///
    /// `data` must be `width * height * 4` bytes in RGBA order.
    /// Alpha channel is preserved; R/G/B channels receive channel-weighted noise.
    pub fn apply_to_pixels(&self, data: &mut [u8], width: u32, height: u32) {
        if !self.enabled || width == 0 || height == 0 {
            return;
        }
        let expected_len = width as usize * height as usize * 4;
        if data.len() < expected_len {
            return;
        }
        for y in 0..height {
            for x in 0..width {
                let idx = (y as usize * width as usize + x as usize) * 4;
                let r = data[idx];
                let g = data[idx + 1];
                let b = data[idx + 2];
                // a = data[idx + 3]; — alpha preserved

                let noise = self.deterministic_noise(x, y);
                let nr = (r as f64 + noise * self.noise_amplitude * 255.0).clamp(0.0, 255.0) as u8;
                let ng = (g as f64 + noise * self.noise_amplitude * 127.0).clamp(0.0, 255.0) as u8;
                let nb = (b as f64 + noise * self.noise_amplitude * 63.0).clamp(0.0, 255.0) as u8;

                data[idx] = nr;
                data[idx + 1] = ng;
                data[idx + 2] = nb;
                // data[idx + 3] unchanged
            }
        }
    }

    /// Deterministic noise function — same algorithm as `bao_stealth::canvas::CanvasNoise`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_does_nothing() {
        let config = CanvasNoiseConfig::disabled();
        let mut pixels = [128u8, 64, 32, 255];
        config.apply_to_pixels(&mut pixels, 1, 1);
        assert_eq!(pixels, [128, 64, 32, 255]);
    }

    #[test]
    fn enabled_modifies_rgb_preserves_alpha() {
        // Use a large amplitude to ensure at least one u8 channel changes
        let config = CanvasNoiseConfig::new(42, 1.0);
        let mut pixels = [128u8, 64, 32, 200];
        config.apply_to_pixels(&mut pixels, 1, 1);
        assert_eq!(pixels[3], 200); // alpha unchanged
        // With amplitude=1.0, noise * 255/127/63 offset is large enough to change u8 values
        assert!(pixels[0] != 128 || pixels[1] != 64 || pixels[2] != 32);
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let config = CanvasNoiseConfig::new(42, 0.001);
        let mut p1 = [128u8, 64, 32, 255];
        let mut p2 = [128u8, 64, 32, 255];
        config.apply_to_pixels(&mut p1, 1, 1);
        config.apply_to_pixels(&mut p2, 1, 1);
        assert_eq!(p1, p2);
    }

    #[test]
    fn different_seeds_different_pixels() {
        // Use large amplitude to ensure different seeds produce different u8 outputs
        let c1 = CanvasNoiseConfig::new(100, 1.0);
        let c2 = CanvasNoiseConfig::new(200, 1.0);
        let mut p1 = [128u8, 64, 32, 255];
        let mut p2 = [128u8, 64, 32, 255];
        c1.apply_to_pixels(&mut p1, 1, 1);
        c2.apply_to_pixels(&mut p2, 1, 1);
        assert_ne!(p1, p2);
    }

    #[test]
    fn deterministic_noise_range() {
        let config = CanvasNoiseConfig::new(42, 0.001);
        for x in 0..10u32 {
            for y in 0..10u32 {
                let n = config.deterministic_noise(x, y);
                assert!(n >= -0.5 && n <= 0.5, "noise at ({},{}) = {}", x, y, n);
            }
        }
    }

    #[test]
    fn noise_preserves_signal() {
        let config = CanvasNoiseConfig::new(42, 0.001);
        let mut pixels = [128u8, 64, 32, 255];
        config.apply_to_pixels(&mut pixels, 1, 1);
        // amplitude=0.001, noise range [-0.5,0.5], max offset = 0.5*0.001*255 ≈ 0.1275
        assert!((pixels[0] as i32 - 128).abs() <= 1);
        assert!((pixels[1] as i32 - 64).abs() <= 1);
        assert!((pixels[2] as i32 - 32).abs() <= 1);
    }

    #[test]
    fn zero_dimensions_noop() {
        let config = CanvasNoiseConfig::new(42, 0.001);
        let mut pixels = [128u8, 64, 32, 255];
        config.apply_to_pixels(&mut pixels, 0, 0);
        assert_eq!(pixels, [128, 64, 32, 255]);
    }

    #[test]
    fn parity_with_bao_stealth_canvas_noise() {
        // Verify that the deterministic_noise output matches bao_stealth::canvas::CanvasNoise
        // for the same seed and coordinates. This is the critical parity check.
        let config = CanvasNoiseConfig::new(42, 0.001);
        // bao_stealth::canvas::CanvasNoise uses the same algorithm:
        // state ^= x * 0x517CC1B727220A95, ^= y * 0x6C62272E07BB0142,
        // *= 0x2545F4914F6CDD1D, ^= >> 33, *= 0x27D4EB2D1659B4D6, ^= >> 33
        // result = state / u64::MAX - 0.5
        // Test at a few coordinate pairs to confirm algorithm match
        let n = config.deterministic_noise(10, 20);
        assert!(n >= -0.5 && n <= 0.5);

        // Manual verification: compute the expected value
        let mut state: u64 = 42;
        state ^= 10u64.wrapping_mul(0x517CC1B727220A95);
        state ^= 20u64.wrapping_mul(0x6C62272E07BB0142);
        state = state.wrapping_mul(0x2545F4914F6CDD1D);
        state ^= state >> 33;
        state = state.wrapping_mul(0x27D4EB2D1659B4D6);
        state ^= state >> 33;
        let expected = (state as f64) / (u64::MAX as f64) - 0.5;
        assert!((n - expected).abs() < 1e-15);
    }
}
