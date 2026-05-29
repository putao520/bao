// REQ-STL-005: WebGL/Audio fingerprint protection
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
