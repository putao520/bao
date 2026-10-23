//! A utility module for backends that wrap surfaces in OpenGL FBOs.

use crate::context::{ContextAttributeFlags, ContextAttributes};
use crate::gl;
use crate::Gl;
use std::thread;

use euclid::default::Size2D;
use gl::Renderbuffer;
use glow::HasContext;
use log::warn;

pub(crate) enum Renderbuffers {
    IndividualDepthStencil {
        depth: Option<Renderbuffer>,
        stencil: Option<Renderbuffer>,
    },
    CombinedDepthStencil(Option<Renderbuffer>),
}

impl Drop for Renderbuffers {
    fn drop(&mut self) {
        match *self {
            Renderbuffers::IndividualDepthStencil {
                depth: None,
                stencil: None,
            }
            | Renderbuffers::CombinedDepthStencil(None) => {}
            _ => {
                if !thread::panicking() {
                    panic!("Should have destroyed the FBO renderbuffers with `destroy()`!")
                }
            }
        }
    }
}

impl Renderbuffers {
    pub(crate) fn new(
        gl: &Gl,
        size: &Size2D<i32>,
        attributes: &ContextAttributes,
    ) -> Renderbuffers {
        unsafe {
            if attributes
                .flags
                .contains(ContextAttributeFlags::DEPTH | ContextAttributeFlags::STENCIL)
            {
                // BAO patch (REQ-BRW-051, gl-oom-defense-001, 2026-10-20): GL
                // allocation can fail under resource exhaustion; degrade to a
                // depth/stencil-less surface (logged) instead of panicking the
                // whole process.
                let renderbuffer = match gl.create_renderbuffer() {
                    Ok(renderbuffer) => renderbuffer,
                    Err(error) => {
                        warn!("surfman: renderbuffer allocation failed ({error}); continuing without depth/stencil");
                        return Renderbuffers::CombinedDepthStencil(None);
                    },
                };
                gl.bind_renderbuffer(gl::RENDERBUFFER, Some(renderbuffer));
                gl.renderbuffer_storage(
                    gl::RENDERBUFFER,
                    gl::DEPTH24_STENCIL8,
                    size.width,
                    size.height,
                );
                gl.bind_renderbuffer(gl::RENDERBUFFER, None);
                return Renderbuffers::CombinedDepthStencil(Some(renderbuffer));
            }

            let (mut depth_renderbuffer, mut stencil_renderbuffer) = (None, None);
            if attributes.flags.contains(ContextAttributeFlags::DEPTH) {
                depth_renderbuffer = match gl.create_renderbuffer() {
                    Ok(renderbuffer) => Some(renderbuffer),
                    Err(error) => {
                        warn!("surfman: depth renderbuffer allocation failed ({error}); continuing without depth");
                        None
                    },
                };
                gl.bind_renderbuffer(gl::RENDERBUFFER, depth_renderbuffer);
                gl.renderbuffer_storage(
                    gl::RENDERBUFFER,
                    gl::DEPTH_COMPONENT24,
                    size.width,
                    size.height,
                );
            }
            if attributes.flags.contains(ContextAttributeFlags::STENCIL) {
                stencil_renderbuffer = match gl.create_renderbuffer() {
                    Ok(renderbuffer) => Some(renderbuffer),
                    Err(error) => {
                        warn!("surfman: stencil renderbuffer allocation failed ({error}); continuing without stencil");
                        None
                    },
                };
                gl.bind_renderbuffer(gl::RENDERBUFFER, stencil_renderbuffer);
                gl.renderbuffer_storage(
                    gl::RENDERBUFFER,
                    gl::STENCIL_INDEX8,
                    size.width,
                    size.height,
                );
            }
            gl.bind_renderbuffer(gl::RENDERBUFFER, None);

            Renderbuffers::IndividualDepthStencil {
                depth: depth_renderbuffer,
                stencil: stencil_renderbuffer,
            }
        }
    }

    pub(crate) fn bind_to_current_framebuffer(&self, gl: &Gl) {
        unsafe {
            match *self {
                Renderbuffers::CombinedDepthStencil(renderbuffer) => {
                    if renderbuffer.is_some() {
                        gl.framebuffer_renderbuffer(
                            gl::FRAMEBUFFER,
                            gl::DEPTH_STENCIL_ATTACHMENT,
                            gl::RENDERBUFFER,
                            renderbuffer,
                        );
                    }
                }
                Renderbuffers::IndividualDepthStencil {
                    depth: depth_renderbuffer,
                    stencil: stencil_renderbuffer,
                } => {
                    if depth_renderbuffer.is_some() {
                        gl.framebuffer_renderbuffer(
                            gl::FRAMEBUFFER,
                            gl::DEPTH_ATTACHMENT,
                            gl::RENDERBUFFER,
                            depth_renderbuffer,
                        );
                    }
                    if stencil_renderbuffer.is_some() {
                        gl.framebuffer_renderbuffer(
                            gl::FRAMEBUFFER,
                            gl::STENCIL_ATTACHMENT,
                            gl::RENDERBUFFER,
                            stencil_renderbuffer,
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn destroy(&mut self, gl: &Gl) {
        unsafe {
            gl.bind_renderbuffer(gl::RENDERBUFFER, None);

            match *self {
                Renderbuffers::CombinedDepthStencil(ref mut renderbuffer) => {
                    if let Some(renderbuffer) = renderbuffer.take() {
                        gl.delete_renderbuffer(renderbuffer);
                    }
                }
                Renderbuffers::IndividualDepthStencil {
                    depth: ref mut depth_renderbuffer,
                    stencil: ref mut stencil_renderbuffer,
                } => {
                    if let Some(stencil_renderbuffer) = stencil_renderbuffer.take() {
                        gl.delete_renderbuffer(stencil_renderbuffer);
                    }
                    if let Some(depth_renderbuffer) = depth_renderbuffer.take() {
                        gl.delete_renderbuffer(depth_renderbuffer);
                    }
                }
            }
        }
    }
}
