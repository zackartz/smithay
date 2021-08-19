//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use std::os::unix::prelude::RawFd;
use std::rc::Rc;

use super::{Window, X11Error};
use nix::fcntl;
use wayland_server::protocol::wl_buffer::WlBuffer;
use x11rb::connection::Connection;
use x11rb::protocol::shm::ConnectionExt;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::utils::RawFdContainer;
use x11rb::{protocol::dri3::ConnectionExt as _, rust_connection::RustConnection};

use crate::backend::allocator::dmabuf::Dmabuf;
use crate::backend::allocator::Buffer;

// Plan here is to support dmabufs via the dri3 extensions, xcb_dri3_pixmap_from_buffer.
// Shm can also be supported easily, through xcb_shm_create_pixmap.

#[derive(Debug)]
pub struct Pixmap {
    connection: Rc<RustConnection>,
    inner: u32,
}

impl Pixmap {
    pub fn from_shm(
        connection: Rc<RustConnection>,
        window: &Window,
        buffer: &WlBuffer,
    ) -> Result<Pixmap, X11Error> {
        use crate::wayland::shm::with_buffer_contents;

        let (fd, buffer_data) = with_buffer_contents(buffer, |slice, data, fd| (fd, data)).expect("TODO");

        // XCB closes the file descriptor after sending, so duplicate the file descriptor.
        let fd: RawFd = fcntl::fcntl(
            fd,
            fcntl::FcntlArg::F_DUPFD_CLOEXEC(0), // Why is this 0?
        )
        .expect("TODO");

        let shm_seg_xid = connection.generate_id()?;
        connection.shm_attach_fd(shm_seg_xid, RawFdContainer::new(fd), false)?;

        let pixmap_xid = connection.generate_id()?;
        connection.shm_create_pixmap(
            pixmap_xid,
            window.id().expect("TODO?"),
            buffer_data.width as u16,
            buffer_data.height as u16,
            window.0.upgrade().expect("TODO").depth.depth,
            shm_seg_xid,
            buffer_data.offset as u32,
        )?;

        connection.shm_detach(shm_seg_xid)?.check()?;

        Ok(Pixmap {
            connection,
            inner: pixmap_xid,
        })
    }

    pub fn present(&self, window: &Window) -> Result<(), X11Error> {
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
