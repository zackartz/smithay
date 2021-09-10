//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use std::os::unix::prelude::RawFd;
use std::sync::Arc;

use super::connection::XConnection;
use super::{Window, X11Error};
use nix::fcntl;
use x11rb::connection::Connection;
use x11rb::protocol::dri3::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::{ConnectionError, ReplyOrIdError};
use x11rb::utils::RawFdContainer;

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
    // TODO: Consider future x11rb WindowWrapper
    connection: Arc<XConnection>,
    inner: u32,
    width: u16,
    height: u16,
}

impl Pixmap {
    #[allow(dead_code)]
    /// Creates a pixmap from a Dmabuf.
    pub fn from_dmabuf(
        connection: Arc<XConnection>,
        window: &Window,
        dmabuf: &Dmabuf,
    ) -> Result<Pixmap, CreatePixmapError> {
        if dmabuf.num_planes() > 4 {
            return Err(CreatePixmapError::TooManyPlanes);
        }

        let xcb = connection.xcb_connection();

        let xid = xcb.generate_id()?;
        let mut strides = dmabuf.strides();

        let mut fds = Vec::new();

        for handle in dmabuf.handles() {
            // XCB closes the file descriptor after sending, so duplicate the file descriptor.
            let fd: RawFd = fcntl::fcntl(
                handle,
                fcntl::FcntlArg::F_DUPFD_CLOEXEC(3), // Set to 3 so the fd cannot become stdin, stdout or stderr
            )
            .map_err(|e| CreatePixmapError::DupFailed(e.to_string()))?;

            fds.push(RawFdContainer::new(fd))
        }

        let stride = strides.next().unwrap();

        xcb.dri3_pixmap_from_buffer(
            xid,
            window.id(),
            dmabuf.height() * stride,
            dmabuf.width() as u16,
            dmabuf.height() as u16,
            stride as u16,
            window.depth(),
            32, // TODO: Stop hardcoding this
            fds.remove(0),
        )?;

        Ok(Pixmap {
            connection,
            inner: xid,
            width: dmabuf.width() as u16,
            height: dmabuf.height() as u16,
        })
    }

    #[allow(dead_code)]
    pub fn present(&self, window: &Window) -> Result<(), X11Error> {
        self.connection.xcb_connection().copy_area(
            self.inner,
            window.id(),
            window.gc(),
            0,
            0,
            0,
            0,
            self.width,
            self.height,
        )?;

        Ok(())
    }
}

impl Drop for Pixmap {
    fn drop(&mut self) {
        let _ = self.connection.xcb_connection().free_pixmap(self.inner);
    }
}
