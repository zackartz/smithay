//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use std::os::unix::prelude::RawFd;
use std::rc::Rc;
use std::sync::Arc;

use super::{Window, X11Error};
use nix::fcntl;
use wayland_server::protocol::wl_buffer::WlBuffer;
use x11rb::connection::Connection;
use x11rb::protocol::shm::ConnectionExt;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::{ConnectionError, ReplyOrIdError};
use x11rb::utils::RawFdContainer;
use x11rb::{protocol::dri3::ConnectionExt as _, rust_connection::RustConnection};

use crate::backend::allocator::dmabuf::Dmabuf;
use crate::backend::allocator::Buffer;

// Plan here is to support dmabufs via the dri3 extensions, xcb_dri3_pixmap_from_buffer.
// Shm can also be supported easily, through xcb_shm_create_pixmap.

#[derive(Debug, thiserror::Error)]
pub enum CreatePixmapError {
    #[error("An x11 protocol error occured")]
    Protocol(X11Error),

    #[error("The Dmabuf had too many planes")]
    TooManyPlanes,

    #[error("Duplicating the file descriptors for the dmabuf handles failed")]
    DupFailed(String),
}

impl From<X11Error> for CreatePixmapError {
    fn from(e: X11Error) -> Self {
        CreatePixmapError::Protocol(e)
    }
}

impl From<ReplyOrIdError> for CreatePixmapError {
    fn from(e: ReplyOrIdError) -> Self {
        X11Error::from(e).into()
    }
}

impl From<ConnectionError> for CreatePixmapError {
    fn from(e: ConnectionError) -> Self {
        X11Error::from(e).into()
    }
}

#[derive(Debug)]
pub struct Pixmap {
    connection: Arc<RustConnection>,
    inner: u32,
}

impl Pixmap {
    /// Creates a pixmap from a Dmabuf.
    pub fn from_dmabuf(
        connection: Arc<RustConnection>,
        window: &Window,
        dmabuf: &Dmabuf,
    ) -> Result<Pixmap, CreatePixmapError> {
        if dmabuf.num_planes() > 4 {
            return Err(CreatePixmapError::TooManyPlanes);
        }

        let xid = connection.generate_id()?;
        let mut strides = dmabuf.strides();
        let mut offsets = dmabuf.offsets();
        let mut fds = Vec::new();

        for handle in dmabuf.handles() {
            // XCB closes the file descriptor after sending, so duplicate the file descriptor.
            let fd: RawFd = fcntl::fcntl(
                handle,
                fcntl::FcntlArg::F_DUPFD_CLOEXEC(0), // Why is this 0?
            )
            .map_err(|e| CreatePixmapError::DupFailed(e.to_string()))?;

            fds.push(RawFdContainer::new(fd))
        }

        connection.dri3_pixmap_from_buffers(
            xid,
            window.id(),
            dmabuf.width() as u16,
            dmabuf.height() as u16,
            strides.next().unwrap(),
            offsets.next().unwrap(),
            strides.next().unwrap(),
            offsets.next().unwrap(),
            strides.next().unwrap(),
            offsets.next().unwrap(),
            strides.next().unwrap(),
            offsets.next().unwrap(),
            window.depth(),
            todo!("bpp"),
            dmabuf.format().modifier.into(),
            fds,
        )?;

        Ok(Pixmap {
            connection,
            inner: xid,
        })
    }

    pub fn present(&self, _window: &Window) -> Result<(), X11Error> {
        todo!()
    }
}

impl Drop for Pixmap {
    fn drop(&mut self) {
        let _ = self.connection.free_pixmap(self.inner);
    }
}

#[allow(dead_code)]
pub fn new_dma_pixbuf(
    dmabuf: Dmabuf,
    connection: Rc<RustConnection>,
    window: u32,
    width: u16,
    height: u16,
    depth: u8,
    bpp: u8,
) -> Result<Pixmap, X11Error> {
    // Dup FDs since XCB will close the FDs after sending them.
    // TODO:

    let pixmap = connection.generate_id()?;
    connection
        .dri3_pixmap_from_buffers(
            pixmap,
            window,
            width,
            height,
            dmabuf.strides().next().unwrap(),
            dmabuf.offsets().next().unwrap(),
            dmabuf.strides().nth(1).unwrap(),
            dmabuf.offsets().nth(1).unwrap(),
            dmabuf.strides().nth(2).unwrap(),
            dmabuf.offsets().nth(2).unwrap(),
            dmabuf.strides().nth(3).unwrap(),
            dmabuf.offsets().nth(3).unwrap(),
            depth,
            bpp,
            dmabuf.format().modifier.into(),
            // TODO: Duplicate attributes, as raw fd container takes ownership
            dmabuf.handles().map(RawFdContainer::new).collect(),
        )?
        .check()?;

    todo!()
}
