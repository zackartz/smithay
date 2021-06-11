//! Rendering functionality and abstractions
//!
//! Collection of common traits and implementations
//! to facilitate (possible hardware-accelerated) rendering.
//!
//! Supported rendering apis:
//!
//! - Raw OpenGL ES 2

use std::collections::HashSet;
use std::error::Error;

#[cfg(feature = "wayland_frontend")]
use crate::{utils::Rectangle, wayland::compositor::SurfaceAttributes};
use cgmath::{prelude::*, Matrix3, Vector2};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

#[cfg(feature = "renderer_gl")]
pub mod gles2;
#[cfg(feature = "renderer_vulkan")]
pub mod vulkan;

#[cfg(feature = "wayland_frontend")]
use crate::backend::allocator::{dmabuf::Dmabuf, Format};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::backend::egl::display::EGLBufferReader;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Possible transformations to two-dimensional planes
pub enum Transform {
    /// Identity transformation (plane is unaltered when applied)
    Normal,
    /// Plane is rotated by 90 degrees
    _90,
    /// Plane is rotated by 180 degrees
    _180,
    /// Plane is rotated by 270 degrees
    _270,
    /// Plane is flipped vertically
    Flipped,
    /// Plane is flipped vertically and rotated by 90 degrees
    Flipped90,
    /// Plane is flipped vertically and rotated by 180 degrees
    Flipped180,
    /// Plane is flipped vertically and rotated by 270 degrees
    Flipped270,
}

impl Transform {
    /// A projection matrix to apply this transformation
    pub fn matrix(&self) -> Matrix3<f32> {
        match self {
            Transform::Normal => Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_90 => Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_180 => Matrix3::new(-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_270 => Matrix3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped => Matrix3::new(-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped90 => Matrix3::new(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped180 => Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped270 => Matrix3::new(0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        }
    }

    /// Inverts any 90-degree transformation into 270-degree transformations and vise versa.
    ///
    /// Flipping is preserved and 180/Normal transformation are uneffected.
    pub fn invert(&self) -> Transform {
        match self {
            Transform::Normal => Transform::Normal,
            Transform::Flipped => Transform::Flipped,
            Transform::_90 => Transform::_270,
            Transform::_180 => Transform::_180,
            Transform::_270 => Transform::_90,
            Transform::Flipped90 => Transform::Flipped270,
            Transform::Flipped180 => Transform::Flipped180,
            Transform::Flipped270 => Transform::Flipped90,
        }
    }

    /// Transformed size after applying this transformation.
    pub fn transform_size(&self, width: u32, height: u32) -> (u32, u32) {
        if *self == Transform::_90
            || *self == Transform::_270
            || *self == Transform::Flipped90
            || *self == Transform::Flipped270
        {
            (height, width)
        } else {
            (width, height)
        }
    }
}

#[cfg(feature = "wayland-frontend")]
impl From<wayland_server::protocol::wl_output::Transform> for Transform {
    fn from(transform: wayland_server::protocol::wl_output::Transform) -> Transform {
        use wayland_server::protocol::wl_output::Transform::*;
        match transform {
            Normal => Transform::Normal,
            _90 => Transform::_90,
            _180 => Transform::_180,
            _270 => Transform::_270,
            Flipped => Transform::Flipped,
            Flipped90 => Transform::Flipped90,
            Flipped180 => Transform::Flipped180,
            Flipped270 => Transform::Flipped270,
        }
    }
}

/// Abstraction for Renderers, that can render into different targets
pub trait Bind<Target>: Unbind {
    /// Bind a given rendering target, which will contain the rendering results until `unbind` is called.
    ///
    /// Binding to target, while another one is already bound, is rendering defined.
    /// Some renderers might happily replace the current target, while other might drop the call
    /// or throw an error.
    fn bind(&mut self, target: Target) -> Result<(), <Self as Renderer>::Error>;
    /// Supported pixel formats for given targets, if applicable.
    fn supported_formats(&self) -> Option<HashSet<crate::backend::allocator::Format>> {
        None
    }
}

/// Functionality to unbind the current rendering target
pub trait Unbind: Renderer {
    /// Unbind the current rendering target.
    ///
    /// May fall back to a default target, if defined by the implementation.
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error>;
}

/// A two dimensional texture
pub trait Texture {
    /// Size of the texture plane (w x h)
    fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    /// Width of the texture plane
    fn width(&self) -> u32;
    /// Height of the texture plane
    fn height(&self) -> u32;
}

/// Helper trait for [`Renderer`], which defines a rendering api for a currently in-progress frame during [`Renderer::render`].
pub trait Frame {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;

    /// Clear the complete current target with a single given color.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error>;
    /// Render a texture to the current target using given projection matrix and alpha.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn render_texture(
        &mut self,
        texture: &Self::TextureId,
        matrix: Matrix3<f32>,
        alpha: f32,
    ) -> Result<(), Self::Error>;
    /// Render a texture to the current target as a flat 2d-plane at a given
    /// position, applying the given transformation with the given alpha value.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: (i32, i32),
        transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        let mut mat = Matrix3::<f32>::identity();

        // position and scale
        let size = texture.size();
        mat = mat * Matrix3::from_translation(Vector2::new(pos.0 as f32, pos.1 as f32));
        mat = mat * Matrix3::from_nonuniform_scale(size.0 as f32, size.1 as f32);

        //apply surface transformation
        mat = mat * Matrix3::from_translation(Vector2::new(0.5, 0.5));
        if transform == Transform::Normal {
            assert_eq!(mat, mat * transform.invert().matrix());
            assert_eq!(transform.matrix(), Matrix3::<f32>::identity());
        }
        mat = mat * transform.invert().matrix();
        mat = mat * Matrix3::from_translation(Vector2::new(-0.5, -0.5));

        self.render_texture(texture, mat, alpha)
    }
}

/// Abstraction of commonly used rendering operations for compositors.
pub trait Renderer {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;
    /// Type representing a currently in-progress frame during the [`Renderer::render`]-call
    type Frame: Frame<Error = Self::Error, TextureId = Self::TextureId>;

    /// Import a given bitmap into the renderer.
    ///
    /// Returns a texture_id, which can be used with `render_texture(_at)` or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it,
    /// and needs to be freed by calling `destroy_texture` on this renderer to avoid a resource leak.
    ///
    /// This operation needs no bound or default rendering target.
    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target = [u8]>>(
        &mut self,
        image: &image::ImageBuffer<image::Rgba<u8>, C>,
    ) -> Result<Self::TextureId, Self::Error>;

    /// Initialize a rendering context on the current rendering target with given dimensions and transformation.
    ///
    /// This function *may* error, if:
    /// - The given dimensions are unsuppored (too large) for this renderer
    /// - The given Transformation is not supported by the renderer (`Transform::Normal` is always supported).
    /// - This renderer implements `Bind`, no target was bound *and* has no default target.
    /// - (Renderers not implementing `Bind` always have a default target.)
    fn render<F, R>(
        &mut self,
        width: u32,
        height: u32,
        transform: Transform,
        rendering: F,
    ) -> Result<R, Self::Error>
    where
        F: FnOnce(&mut Self, &mut Self::Frame) -> R;
}

#[cfg(feature = "wayland_frontend")]
/// Trait for Renderers supporting importing shm-based buffers.
pub trait ImportShm: Renderer {
    /// Import a given shm-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    ///
    /// If provided the `SurfaceAttributes` can be used to do caching of rendering resources and is generally recommended.
    ///
    /// The `damage` argument provides a list of rectangle locating parts of the buffer that need to be updated. When provided
    /// with an empty list `&[]`, the renderer is allowed to not update the texture at all.
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceAttributes>,
        damage: &[Rectangle],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;

    /// Returns supported formats for shared memory buffers.
    ///
    /// Will always contain At least `Argb8888` and `Xrgb8888`.
    fn shm_formats(&self) -> &[wl_shm::Format] {
        // Mandatory
        &[wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
/// Trait for Renderers supporting importing wl_drm-based buffers.
pub trait ImportEgl: Renderer {
    /// Import a given wl_drm-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        egl: &EGLBufferReader,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;
}

#[cfg(feature = "wayland_frontend")]
/// Trait for Renderers supporting importing dmabuf-based buffers.
pub trait ImportDma: Renderer {
    /// Returns supported formats for dmabufs.
    fn dmabuf_formats<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Format> + 'a> {
        Box::new([].iter())
    }

    /// Import a given dmabuf-based buffer into the renderer (see [`buffer_type`]).
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_dma_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        let dmabuf = buffer
            .as_ref()
            .user_data()
            .get::<Dmabuf>()
            .expect("import_dma_buffer without checking buffer type?");
        self.import_dmabuf(dmabuf)
    }

    /// Import a given raw dmabuf into the renderer.
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>;
}

// TODO: Replace this with a trait_alias, once that is stabilized.
// pub type ImportAll = Renderer + ImportShm + ImportEgl;

/// Common trait for renderers of any wayland buffer type
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
pub trait ImportAll: Renderer + ImportShm + ImportEgl {
    /// Import a given buffer into the renderer.
    ///
    /// Returns a texture_id, which can be used with [`Frame::render_texture`] (or [`Frame::render_texture_at`])
    /// or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    ///
    /// If provided the `SurfaceAttributes` can be used to do caching of rendering resources and is generally recommended.
    ///
    /// The `damage` argument provides a list of rectangle locating parts of the buffer that need to be updated. When provided
    /// with an empty list `&[]`, the renderer is allowed to not update the texture at all.
    ///
    /// Returns `None`, if the buffer type cannot be determined.
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceAttributes>,
        damage: &[Rectangle],
        egl: Option<&EGLBufferReader>,
    ) -> Option<Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error>> {
        match buffer_type(buffer, egl) {
            Some(BufferType::Shm) => Some(self.import_shm_buffer(buffer, surface, damage)),
            Some(BufferType::Egl) => Some(self.import_egl_buffer(buffer, egl.unwrap())),
            _ => None,
        }
    }
}
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl<R: Renderer + ImportShm + ImportEgl> ImportAll for R {}

#[cfg(feature = "wayland_frontend")]
#[non_exhaustive]
/// Buffer type of a given wl_buffer, if managed by smithay
pub enum BufferType {
    /// Buffer is managed by the [`crate::wayland::shm`] global
    Shm,
    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    /// Buffer is managed by a currently initialized [`crate::backend::egl::display::EGLBufferReader`]
    Egl,
    /// Buffer is managed by the [`crate::wayland::dmabuf`] global
    Dma,
}

/// Returns the *type* of a wl_buffer
///
/// Returns `None` if the type is not known to smithay
/// or otherwise not supported (e.g. not initialized using one of smithays [`crate::wayland`]-handlers).
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
pub fn buffer_type(
    buffer: &wl_buffer::WlBuffer,
    egl_buffer_reader: Option<&EGLBufferReader>,
) -> Option<BufferType> {
    if buffer.as_ref().user_data().get::<Dmabuf>().is_some() {
        Some(BufferType::Dma)
    } else if egl_buffer_reader
        .as_ref()
        .and_then(|x| x.egl_buffer_dimensions(&buffer))
        .is_some()
    {
        Some(BufferType::Egl)
    } else if crate::wayland::shm::with_buffer_contents(&buffer, |_, _| ()).is_ok() {
        Some(BufferType::Shm)
    } else {
        None
    }
}

/// Returns the *type* of a wl_buffer
///
/// Returns `None` if the type is not recognized by smithay or otherwise not supported.
#[cfg(all(
    feature = "wayland_frontend",
    not(all(feature = "backend_egl", feature = "use_system_lib"))
))]
pub fn buffer_type(buffer: &wl_buffer::WlBuffer) -> Option<BufferType> {
    if buffer.as_ref().user_data().get::<Dmabuf>().is_some() {
        Some(BufferType::Dma)
    } else if crate::wayland::shm::with_buffer_contents(&buffer, |_, _| ()).is_ok() {
        Some(BufferType::Shm)
    } else {
        None
    }
}

/// Returns the dimensions of a wl_buffer
///
/// *Note*: This will only return dimensions for buffer types known to smithay (see [`buffer_type`])
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
pub fn buffer_dimensions(
    buffer: &wl_buffer::WlBuffer,
    egl_buffer_reader: Option<&EGLBufferReader>,
) -> Option<(i32, i32)> {
    use crate::backend::allocator::Buffer;

    if let Some(buf) = buffer.as_ref().user_data().get::<Dmabuf>() {
        Some((buf.width() as i32, buf.height() as i32))
    } else if let Some((w, h)) = egl_buffer_reader
        .as_ref()
        .and_then(|x| x.egl_buffer_dimensions(&buffer))
    {
        Some((w, h))
    } else if let Ok((w, h)) =
        crate::wayland::shm::with_buffer_contents(&buffer, |_, data| (data.width, data.height))
    {
        Some((w, h))
    } else {
        None
    }
}

/// Returns the dimensions of a wl_buffer
///
/// *Note*: This will only return dimensions for buffer types known to smithay (see [`buffer_type`])
#[cfg(all(
    feature = "wayland_frontend",
    not(all(feature = "backend_egl", feature = "use_system_lib"))
))]
pub fn buffer_dimensions(buffer: &wl_buffer::WlBuffer) -> Option<(i32, i32)> {
    use crate::backend::allocator::Buffer;

    if let Some(buf) = buffer.as_ref().user_data().get::<Dmabuf>() {
        Some((buf.width() as i32, buf.height() as i32))
    } else if let Ok((w, h)) =
        crate::wayland::shm::with_buffer_contents(&buffer, |_, data| (data.width, data.height))
    {
        Some((w, h))
    } else {
        None
    }
}
