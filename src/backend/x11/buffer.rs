//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.

use super::X11Error;
use x11rb::connection::Connection;
use x11rb::utils::RawFdContainer;
use x11rb::{protocol::dri3::ConnectionExt, rust_connection::RustConnection};

use crate::backend::allocator::dmabuf::Dmabuf;

// Plan here is to support dmabufs via the dri3 extensions, xcb_dri3_pixmap_from_buffer.
// Shm can also be supported easily, through xcb_shm_create_pixmap.

pub fn new_dma_pixbuf(
    dmabuf: Dmabuf,
    connection: &RustConnection,
    window: u32,
    width: u16,
    height: u16,
    depth: u8,
    bpp: u8,
    modifier: u64,
) -> Result<(), X11Error> {
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
            modifier,
            // TODO: Duplicate attributes, as raw fd container takes ownership
            dmabuf.handles().map(|fd| RawFdContainer::new(fd)).collect(),
        )?
        .check()?;

    Ok(())
}
