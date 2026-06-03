// REQ-STL-005: WebGL/Audio fingerprint protection  @trace REQ-STL-005
#[derive(Debug, Clone)]
pub struct WebGLProfile {
    pub vendor: String,
    pub renderer: String,
    pub extensions: Vec<String>,
    pub max_texture_size: u32,
    pub max_renderbuffer_size: u32,
    pub max_viewport_dims: [u32; 2],
}

impl WebGLProfile {
    pub fn firefox() -> Self {
        WebGLProfile {
            vendor: "Mozilla".into(),
            renderer: "WebGL 1.0 (OpenGL ES 2.0 Chromium)".into(),
            extensions: vec![
                "ANGLE_instanced_arrays".into(),
                "EXT_blend_minmax".into(),
                "EXT_color_buffer_half_float".into(),
                "EXT_float_blend".into(),
                "EXT_frag_depth".into(),
                "EXT_shader_texture_lod".into(),
                "EXT_texture_compression_bptc".into(),
                "EXT_texture_filter_anisotropic".into(),
                "OES_element_index_uint".into(),
                "OES_fbo_render_mipmap".into(),
                "OES_standard_derivatives".into(),
                "OES_texture_float".into(),
                "OES_texture_float_linear".into(),
                "OES_texture_half_float".into(),
                "OES_texture_half_float_linear".into(),
                "OES_vertex_array_object".into(),
                "WEBGL_color_buffer_float".into(),
                "WEBGL_compressed_texture_etc".into(),
                "WEBGL_compressed_texture_s3tc".into(),
                "WEBGL_debug_renderer_info".into(),
                "WEBGL_debug_shaders".into(),
                "WEBGL_depth_texture".into(),
                "WEBGL_draw_buffers".into(),
                "WEBGL_lose_context".into(),
            ],
            max_texture_size: 16384,
            max_renderbuffer_size: 16384,
            max_viewport_dims: [16384, 16384],
        }
    }

    pub fn chrome() -> Self {
        WebGLProfile {
            vendor: "Google Inc. (NVIDIA)".into(),
            renderer: "ANGLE (NVIDIA, NVIDIA GeForce GTX 1060, OpenGL 4.5)".into(),
            extensions: vec![
                "ANGLE_instanced_arrays".into(),
                "EXT_blend_minmax".into(),
                "EXT_color_buffer_half_float".into(),
                "EXT_float_blend".into(),
                "EXT_texture_filter_anisotropic".into(),
                "OES_element_index_uint".into(),
                "OES_standard_derivatives".into(),
                "OES_texture_float".into(),
                "OES_texture_float_linear".into(),
                "OES_texture_half_float".into(),
                "OES_texture_half_float_linear".into(),
                "OES_vertex_array_object".into(),
                "WEBGL_color_buffer_float".into(),
                "WEBGL_compressed_texture_s3tc".into(),
                "WEBGL_debug_renderer_info".into(),
                "WEBGL_depth_texture".into(),
                "WEBGL_draw_buffers".into(),
                "WEBGL_lose_context".into(),
            ],
            max_texture_size: 16384,
            max_renderbuffer_size: 16384,
            max_viewport_dims: [16384, 16384],
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioProfile {
    seed: u64,
    noise_amplitude: f64,
    sample_rate: u32,
}

impl AudioProfile {
    pub fn new(seed: u64) -> Self {
        AudioProfile {
            seed,
            noise_amplitude: 1e-7,
            sample_rate: 44100,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn noise_amplitude(&self) -> f64 {
        self.noise_amplitude
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn apply_noise(&self, sample: f64, index: u32) -> f64 {
        let noise = self.deterministic_noise(index);
        sample + noise * self.noise_amplitude
    }

    fn deterministic_noise(&self, index: u32) -> f64 {
        let mut state = self.seed;
        state ^= (index as u64).wrapping_mul(0x517CC1B727220A95);
        state = state.wrapping_mul(0x2545F4914F6CDD1D);
        state ^= state >> 33;
        (state as f64) / (u64::MAX as f64) - 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── AudioProfile ──────────────────────────────────────────────
    // @trace REQ-STL-005 [req:REQ-STL-005] [level:unit]

    #[test]
    fn test_audio_profile_new() {
        let ap = AudioProfile::new(42);
        assert_eq!(ap.seed(), 42);
        assert_eq!(ap.noise_amplitude(), 1e-7);
        assert_eq!(ap.sample_rate(), 44100);
    }

    #[test]
    fn test_audio_profile_different_seeds() {
        let ap1 = AudioProfile::new(0);
        let ap2 = AudioProfile::new(999);
        assert_ne!(ap1.seed(), ap2.seed());
    }

    #[test]
    fn test_deterministic_noise_same_seed_same_result() {
        let ap = AudioProfile::new(12345);
        let n1 = ap.deterministic_noise(100);
        let n2 = ap.deterministic_noise(100);
        assert_eq!(n1, n2);
    }

    #[test]
    fn test_deterministic_noise_range() {
        let ap = AudioProfile::new(42);
        for i in 0..1000u32 {
            let n = ap.deterministic_noise(i);
            assert!(n >= -0.5 && n <= 0.5, "noise at index {} is {}", i, n);
        }
    }

    #[test]
    fn test_deterministic_noise_different_indices() {
        let ap = AudioProfile::new(42);
        let n0 = ap.deterministic_noise(0);
        let n1 = ap.deterministic_noise(1);
        // Different indices almost always produce different noise
        assert_ne!(n0, n1);
    }

    #[test]
    fn test_deterministic_noise_different_seeds() {
        let ap1 = AudioProfile::new(100);
        let ap2 = AudioProfile::new(200);
        let n1 = ap1.deterministic_noise(50);
        let n2 = ap2.deterministic_noise(50);
        assert_ne!(n1, n2);
    }

    #[test]
    fn test_apply_noise_adds_deterministic_offset() {
        let ap = AudioProfile::new(42);
        let sample = 1.0;
        let index = 10u32;
        let result = ap.apply_noise(sample, index);
        let noise = ap.deterministic_noise(index);
        // Result should be sample + noise * amplitude
        let expected = sample + noise * ap.noise_amplitude();
        assert!((result - expected).abs() < 1e-15);
    }

    #[test]
    fn test_apply_noise_preserves_signal() {
        let ap = AudioProfile::new(42);
        let sample = 0.5;
        let result = ap.apply_noise(sample, 0);
        // Noise amplitude is 1e-7, so result is within ±1e-7 of sample
        assert!((result - sample).abs() < 1e-6);
    }

    #[test]
    fn test_apply_noise_different_indices_different_results() {
        let ap = AudioProfile::new(42);
        let r0 = ap.apply_noise(1.0, 0);
        let r1 = ap.apply_noise(1.0, 1);
        assert_ne!(r0, r1);
    }

    #[test]
    fn test_audio_profile_clone() {
        let ap = AudioProfile::new(42);
        let cloned = ap.clone();
        assert_eq!(ap.seed(), cloned.seed());
        assert_eq!(ap.noise_amplitude(), cloned.noise_amplitude());
        assert_eq!(ap.sample_rate(), cloned.sample_rate());
    }

    #[test]
    fn test_audio_profile_debug_format() {
        let ap = AudioProfile::new(42);
        let debug_str = format!("{:?}", ap);
        assert!(debug_str.contains("AudioProfile"));
    }

    // ─── WebGLProfile ──────────────────────────────────────────────
    // @trace REQ-STL-005 [req:REQ-STL-005] [level:unit]

    #[test]
    fn test_webgl_firefox_vendor() {
        let p = WebGLProfile::firefox();
        assert_eq!(p.vendor, "Mozilla");
    }

    #[test]
    fn test_webgl_firefox_extensions_nonempty() {
        let p = WebGLProfile::firefox();
        assert!(!p.extensions.is_empty());
        assert!(p.extensions.contains(&"WEBGL_debug_renderer_info".to_string()));
    }

    #[test]
    fn test_webgl_firefox_max_texture_size() {
        let p = WebGLProfile::firefox();
        assert_eq!(p.max_texture_size, 16384);
    }

    #[test]
    fn test_webgl_chrome_vendor() {
        let p = WebGLProfile::chrome();
        assert_eq!(p.vendor, "Google Inc. (NVIDIA)");
    }

    #[test]
    fn test_webgl_chrome_extensions_nonempty() {
        let p = WebGLProfile::chrome();
        assert!(!p.extensions.is_empty());
        assert!(p.extensions.contains(&"WEBGL_debug_renderer_info".to_string()));
    }

    #[test]
    fn test_webgl_firefox_more_extensions_than_chrome() {
        let ff = WebGLProfile::firefox();
        let ch = WebGLProfile::chrome();
        assert!(ff.extensions.len() > ch.extensions.len());
    }

    #[test]
    fn test_webgl_same_max_viewport_dims() {
        let ff = WebGLProfile::firefox();
        let ch = WebGLProfile::chrome();
        assert_eq!(ff.max_viewport_dims, ch.max_viewport_dims);
        assert_eq!(ff.max_viewport_dims, [16384, 16384]);
    }

    #[test]
    fn test_webgl_profile_clone() {
        let p = WebGLProfile::firefox();
        let cloned = p.clone();
        assert_eq!(p.vendor, cloned.vendor);
        assert_eq!(p.extensions, cloned.extensions);
    }

    #[test]
    fn test_webgl_profile_debug_format() {
        let p = WebGLProfile::chrome();
        let debug = format!("{:?}", p);
        assert!(debug.contains("WebGLProfile"));
        assert!(debug.contains("vendor"));
    }
}
