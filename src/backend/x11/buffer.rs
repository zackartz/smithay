//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use std::rc::Rc;

use super::X11Error;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::utils::RawFdContainer;
use x11rb::{protocol::dri3::ConnectionExt as _, rust_connection::RustConnection};

use crate::backend::allocator::Buffer;
use crate::backend::allocator::dmabuf::Dmabuf;

// Plan here is to support dmabufs via the dri3 extensions, xcb_dri3_pixmap_from_buffer.
// Shm can also be supported easily, through xcb_shm_create_pixmap.

#[derive(Debug)]
pub struct Pixmap {
    connection: Rc<RustConnection>,
    inner: u32,
}

impl Drop for Pixmap {
    fn drop(&mut self) {
        let _ = self.connection.free_pixmap(self.inner);
    }
}

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
            dmabuf.strides().nth(0).unwrap(),
            dmabuf.offsets().nth(0).unwrap(),
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
            dmabuf.handles().map(|fd| RawFdContainer::new(fd)).collect(),
        )?
        .check()?;

    Ok(todo!())
}
