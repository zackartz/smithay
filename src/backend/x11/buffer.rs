//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use std::os::unix::prelude::RawFd;
use std::sync::atomic::Ordering;

use super::{Window, X11Error};
use nix::fcntl;
use x11rb::connection::Connection;
use x11rb::protocol::dri3::ConnectionExt as _;
use x11rb::protocol::present::{self, ConnectionExt};
use x11rb::protocol::xproto::PixmapWrapper;
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

pub trait PixmapWrapperExt<'c, C>
where
    C: Connection,
{
    /// Creates a new Pixmap using the supplied Dmabuf.
    ///
    /// The returned Pixmap is freed when dropped.
    fn with_dmabuf(
        connection: &'c C,
        window: &Window,
        dmabuf: &Dmabuf,
    ) -> Result<PixmapWrapper<'c, C>, CreatePixmapError>;

    fn present(self, connection: &C, window: &Window) -> Result<u32, X11Error>;
}

impl<'c, C> PixmapWrapperExt<'c, C> for PixmapWrapper<'c, C>
where
    C: Connection,
{
    fn with_dmabuf(
        connection: &'c C,
        window: &Window,
        dmabuf: &Dmabuf,
    ) -> Result<PixmapWrapper<'c, C>, CreatePixmapError> {
        if dmabuf.num_planes() > 4 {
            return Err(CreatePixmapError::TooManyPlanes);
        }

        let xid = connection.generate_id()?;
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

        // TODO: Use dri3_pixmap_from_buffers where appropriate.

        connection.dri3_pixmap_from_buffer(
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

        Ok(PixmapWrapper::for_pixmap(connection, xid))
    }

    fn present(
        self,
        connection: &C,
        window: &Window,
        // present_state: &mut PresentState,
    ) -> Result<u32, X11Error> {
        let window_inner = window.0.upgrade().unwrap(); // We have the connection and window alive.
        let next_serial = window_inner.next_serial.fetch_add(1, Ordering::SeqCst);
        let msc = window_inner.last_msc.load(Ordering::SeqCst) + 1;

        const OPTIONS: present::Option = present::Option::NONE;

        connection.present_pixmap(
            window.id(),
            self.pixmap(),
            next_serial,
            x11rb::NONE, // Update the entire window
            x11rb::NONE, // Update the entire window
            0,           // No offsets
            0,
            x11rb::NONE,    // Let the X server pick the most suitable crtc
            x11rb::NONE,    // Do not wait to present
            x11rb::NONE,    // We will wait for the X server to tell us when it is done with our pixmap.
            OPTIONS.into(), // No special presentation options.
            msc,            // TODO: Handle target msc
            0,
            0,
            &[], // We don't need to notify any other windows.
        )?;

        Ok(self.into_pixmap())
    }
}
