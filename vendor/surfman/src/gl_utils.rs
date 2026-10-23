//! Various OpenGL utilities used by the different backends.

use log::warn;
use glow::{HasContext, NativeFramebuffer};

use crate::gl;
use crate::Gl;

#[allow(dead_code)]
pub(crate) fn create_and_bind_framebuffer(
    gl: &Gl,
    texture_target: u32,
    texture_object: Option<glow::NativeTexture>,
) -> Option<NativeFramebuffer> {
    unsafe {
        // BAO patch (REQ-BRW-051, gl-oom-defense-001, 2026-10-20): return the
        // allocation failure instead of unwrapping; callers degrade to a
        // color-attachment-less surface or propagate `Err` (process survival
        // under GL_OUT_OF_MEMORY beats a hard abort).
        match gl.create_framebuffer() {
            Ok(framebuffer_object) => {
                gl.bind_framebuffer(gl::FRAMEBUFFER, Some(framebuffer_object));
                gl.framebuffer_texture_2d(
                    gl::FRAMEBUFFER,
                    gl::COLOR_ATTACHMENT0,
                    texture_target,
                    texture_object,
                    0,
                );
                Some(framebuffer_object)
            },
            Err(error) => {
                warn!("surfman: framebuffer allocation failed ({error})");
                None
            },
        }
    }
}

pub(crate) fn unbind_framebuffer_if_necessary(gl: &Gl, framebuffer_object: NativeFramebuffer) {
    unsafe {
        // Unbind the framebuffer if it's bound.
        let current_draw_framebuffer = gl.get_parameter_framebuffer(gl::DRAW_FRAMEBUFFER_BINDING);
        let current_read_framebuffer = gl.get_parameter_framebuffer(gl::READ_FRAMEBUFFER_BINDING);
        if current_draw_framebuffer == Some(framebuffer_object) {
            gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, None);
        }
        if current_read_framebuffer == Some(framebuffer_object) {
            gl.bind_framebuffer(gl::READ_FRAMEBUFFER, None);
        }
    }
}

#[allow(dead_code)]
pub(crate) fn destroy_framebuffer(gl: &Gl, framebuffer_object: NativeFramebuffer) {
    unbind_framebuffer_if_necessary(gl, framebuffer_object);
    unsafe {
        gl.delete_framebuffer(framebuffer_object);
    }
}
