use std::{
    mem,
    os::unix::prelude::{AsRawFd, RawFd},
    sync::Arc,
};

use drm_fourcc::DrmFourcc;
use gbm::{BufferObjectFlags, Device};
use nix::fcntl;
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        dri3::{self, ConnectionExt},
        xproto::PixmapWrapper,
    },
};

use crate::{
    backend::{
        allocator::dmabuf::{AsDmabuf, Dmabuf},
        x11::drm::{get_drm_node_type, DRM_NODE_RENDER},
    },
    utils::{Logical, Size},
};

use super::{
    buffer::{present, PixmapWrapperExt},
    connection::XConnection,
    window::Window,
    X11Backend, X11Error,
};

/// An X11 surface which uses GBM to allocate and present buffers.
#[derive(Debug)]
pub struct GbmBufferingX11Surface {
    connection: Arc<XConnection>,
    window: Window,
    device: Device<RawFd>,
    width: u16,
    height: u16,
    current: Dmabuf,
    next: Dmabuf,
}

impl GbmBufferingX11Surface {
    /// Returns a new surface which allows allocating Dmabufs and presenting them to an X11 window.
    pub fn new(backend: &X11Backend) -> Result<GbmBufferingX11Surface, X11Error> {
        let connection = backend.connection();
        let window = backend.window();
        let xcb = connection.xcb_connection();

        if xcb.extension_information(dri3::X11_EXTENSION_NAME)?.is_none() {
            todo!("DRI3 is not present")
        }

        // Does the X server support dri3?
        let (dri3_major, dri3_minor) = {
            // DRI3 will only return the highest version we request.
            // TODO: We might need to request a higher version?
            let version = xcb.dri3_query_version(1, 2)?.reply()?;

            if version.minor_version < 2 {
                todo!("DRI3 version too low")
            }

            (version.major_version, version.minor_version)
        };

        dbg!("DRI3 {}.{}", dri3_major, dri3_minor);

        // Determine which drm-device the Display is using.
        let screen = &xcb.setup().roots[backend.screen()];
        let dri3 = xcb.dri3_open(screen.root, 0)?.reply()?;

        let drm_device_fd = dri3.device_fd;
        // Duplicate the drm_device_fd
        let drm_device_fd: RawFd = fcntl::fcntl(
            drm_device_fd.as_raw_fd(),
            fcntl::FcntlArg::F_DUPFD_CLOEXEC(3), // Set to 3 so the fd cannot become stdin, stdout or stderr
        )
        .expect("TODO");

        let fd_flags =
            nix::fcntl::fcntl(drm_device_fd.as_raw_fd(), nix::fcntl::F_GETFD).expect("Handle this error");
        // No need to check if ret == 1 since nix handles that.

        // Enable the close-on-exec flag.
        nix::fcntl::fcntl(
            drm_device_fd.as_raw_fd(),
            nix::fcntl::F_SETFD(
                nix::fcntl::FdFlag::from_bits_truncate(fd_flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
            ),
        )
        .expect("Handle this result");

        if get_drm_node_type(drm_device_fd.as_raw_fd()).expect("TODO") != DRM_NODE_RENDER {
            todo!("Attempt to get the render device by name for the DRM node that isn't a render node")
        }

        // Finally create a GBMDevice to manage the buffers.
        let device = crate::backend::allocator::gbm::GbmDevice::new(drm_device_fd.as_raw_fd())
            .expect("Failed to create gbm device");

        let size = backend.window().size().expect("TODO");
        // TODO: Dont hardcode format.
        let current = device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .expect("Failed to allocate presented buffer")
            .export()
            .unwrap();
        let next = device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .expect("Failed to allocate back buffer")
            .export()
            .unwrap();

        Ok(GbmBufferingX11Surface {
            connection,
            window,
            device,
            width: size.w,
            height: size.h,
            current,
            next,
        })
    }

    /// Returns a handle to the GBM device used to allocate buffers.
    pub fn device(&self) -> Device<RawFd> {
        self.device.clone()
    }

    /// Returns an RAII scoped object which provides the next buffer.
    ///
    /// When the object is dropped, the contents of the buffer are swapped and then presented.
    // TODO: Error type
    pub fn present(&mut self) -> Result<Present<'_>, ()> {
        Ok(Present { surface: self })
    }

    /// Resizes the surface, and recreates the internal buffers to match the new size.
    // TODO: Error type, cannot resize while presenting.
    pub fn resize(&mut self, size: Size<u16, Logical>) -> Result<(), ()> {
        self.width = size.w;
        self.height = size.h;

        // Create new buffers
        let current = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .expect("Failed to allocate presented buffer")
            .export()
            .unwrap();

        let next = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .expect("Failed to allocate back buffer")
            .export()
            .unwrap();

        self.current = current;
        self.next = next;

        Ok(())
    }
}

/// An RAII scope holding a Dmabuf to be bound to a renderer.
///
/// Upon dropping this object, the contents of the Dmabuf are immediately presented to the window.
#[derive(Debug)]
pub struct Present<'a> {
    surface: &'a mut GbmBufferingX11Surface,
}

impl Present<'_> {
    /// Returns the next buffer that will be presented to the Window.
    ///
    /// You may bind this buffer to a renderer to render.
    pub fn buffer(&self) -> Dmabuf {
        self.surface.next.clone()
    }
}

impl Drop for Present<'_> {
    fn drop(&mut self) {
        let surface = &mut self.surface;

        // Swap the buffers
        mem::swap(&mut surface.next, &mut surface.current);

        if let Ok(pixmap) = PixmapWrapper::create_with_dmabuf(
            surface.connection.xcb_connection(),
            &surface.window,
            &surface.current,
        ) {
            // Now present the current buffer
            let _ = present(
                surface.connection.xcb_connection(),
                &pixmap,
                &surface.window,
                surface.width,
                surface.height,
            );
        }
    }
}
