//! Utilities for managing an X11 window.

use crate::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use crate::utils::{Logical, Size};

use super::buffer::{present, PixmapWrapperExt};
use super::{Atoms, WindowProperties, X11Error};
use drm_fourcc::DrmFourcc;
use gbm::BufferObjectFlags;
use std::mem;
use std::os::unix::prelude::RawFd;
use std::sync::{Arc, Mutex, Weak};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self as x11, AtomEnum, CreateGCAux, CreateWindowAux, Depth, EventMask, PixmapWrapper, PropMode, Screen,
    WindowClass,
};
use x11rb::protocol::xproto::{ConnectionExt as _, UnmapNotifyEvent};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt;

/// An X11 window.
#[derive(Debug)]
pub struct Window { pub(crate) inner: WindowInner }

impl Window {
    /// Sets the title of the window.
    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    /// Maps the window, making it visible.
    pub fn map(&self) {
        self.inner.map();
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) {
        self.inner.unmap();
    }

    /// Returns the size of this window.
    ///
    /// If the window has been destroyed, the size is `0 x 0`.
    pub fn size(&self) -> Size<u16, Logical> {
        self.inner.size()
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        self.inner.id
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        self.inner.depth.depth
    }

    /// Returns the graphics context used to draw to this window.
    pub fn gc(&self) -> u32 {
        self.inner.gc
    }

    /// Returns the GBM device the window uses to allocate buffers. `None` if the window is no
    /// longer valid.
    pub fn device(&self) -> gbm::Device<RawFd> {
        self.inner.device.clone()
    }

    /// Returns an object that may be used to
    // TODO: Error type
    pub fn present(&mut self) -> Result<Present<'_>, ()> {
        todo!()
    }
}

/// An RAII scoped object providing the next buffer that will be presented to the window.
///
/// A [Renderer](crate::backend::renderer::Renderer) may bind to the provided buffer to draw to the
/// window.
///
/// ```rust,no_run
/// # use crate::backend::renderer::Renderer;
/// # let window: Window = todo!();
/// # let renderer: Gles2Renderer = todo!();
/// let present = window.present();
/// renderer.bind(present.buffer())?;
///
/// // Use the renderer here.
///
/// // Make sure you unbind from the renderer.
/// renderer.unbind()?;
///
/// // Now when the Present is dropped, anything that was rendered will be presented to the window.
/// ```
#[derive(Debug)]
pub struct Present<'w> {
    window: &'w mut Window,
    buffer: Dmabuf
}

impl Present<'_> {
    /// Returns the next buffer that will be presented to the Window.
    ///
    /// You may bind this buffer to a renderer to render.
    pub fn buffer(&self) -> Dmabuf {
        self.buffer.clone()
    }
}

impl Drop for Present<'_> {
    fn drop(&mut self) {
        let window = &mut self.window;

        if let Some(connection) = window.inner.connection.upgrade() {
            let mut buffers = window
                .inner
                .buffers
                .lock()
                .expect("WindowInner buffer mutex poisoned");
            buffers.swap();

            if let Ok(pixmap) = PixmapWrapper::with_dmabuf(&*connection, &window, &buffers.current) {
                // Now present the current buffer
                let _ = present(&*connection, &pixmap, &window);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WindowInner {
    // TODO: Consider future x11rb WindowWrapper
    pub connection: Weak<RustConnection>,
    pub id: x11::Window,
    root: x11::Window,
    pub depth: Depth,
    pub gc: x11::Gcontext,
    pub atoms: Atoms,

    device: gbm::Device<RawFd>,
    pub size: Arc<Mutex<Size<u16, Logical>>>,
    pub buffers: Arc<Mutex<Buffers>>,
}

#[derive(Debug)]
pub(crate) struct Buffers {
    current: Dmabuf,
    next: Dmabuf,
}

impl Buffers {
    fn swap(&mut self) {
        mem::swap(&mut self.next, &mut self.current);
    }
}

impl WindowInner {
    pub fn new(
        connection: Weak<RustConnection>,
        screen: &Screen,
        properties: WindowProperties<'_>,
        atoms: Atoms,
        depth: Depth,
        visual_id: u32,
        colormap: u32,
        device: gbm::Device<RawFd>,
    ) -> Result<WindowInner, X11Error> {
        let weak = connection;
        let connection = weak.upgrade().unwrap();

        // Generate the xid for the window
        let window = connection.generate_id()?;

        // The event mask never include `EventMask::RESIZE_REDIRECT`.
        //
        // The reason is twofold:
        // - We are not a window manager
        // - Makes our window impossible to resize.
        //
        // On the resizing aspect, KWin and some other WMs would allow resizing, but those
        // compositors rely on putting this window in another window for drawing decorations,
        // so visibly in KWin it would look like using the RESIZE_REDIRECT event mask would work,
        // but a tiling window manager would be sad and the tiling window manager devs mad because
        // this window would refuse to listen to the tiling WM.
        //
        // For resizing we use ConfigureNotify events from the STRUCTURE_NOTIFY event mask.

        let window_aux = CreateWindowAux::new()
            .event_mask(
                EventMask::EXPOSURE // Be told when the window is exposed
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::KEY_PRESS // Key press and release
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS // Mouse button press and release
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION // Mouse movement
            | EventMask::NO_EVENT,
            )
            // Border pixel and color map need to be set if our depth may differ from the root depth.
            .border_pixel(0)
            .colormap(colormap);

        let _ = connection.create_window(
            depth.depth,
            window,
            screen.root,
            0,
            0,
            properties.width,
            properties.height,
            0,
            WindowClass::INPUT_OUTPUT,
            visual_id,
            &window_aux,
        )?;

        let (current, next) =
            allocate_buffers(&device, (properties.width, properties.height).into()).expect("TODO");

        // Send requests to change window properties while we wait for the window creation request to complete.
        let mut window = WindowInner {
            connection: weak,
            id: window,
            root: screen.root,
            atoms,
            size: Arc::new(Mutex::new((properties.width, properties.height).into())),
            depth,
            gc: 0,
            device,
            buffers: Arc::new(Mutex::new(Buffers { current, next })),
        };

        let gc = connection.generate_id()?;
        connection.create_gc(gc, window.id, &CreateGCAux::new())?;
        window.gc = gc;

        // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
        connection.change_property32(
            PropMode::REPLACE,
            window.id,
            atoms.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[atoms.WM_DELETE_WINDOW],
        )?;

        // WM class cannot be safely changed later.
        let _ = connection.change_property8(
            PropMode::REPLACE,
            window.id,
            atoms.WM_CLASS,
            AtomEnum::STRING,
            b"Smithay\0Wayland_Compositor\0",
        )?;

        window.set_title(properties.title);
        window.map();

        // Flush requests to server so window is displayed.
        connection.flush()?;

        Ok(window)
    }

    pub fn map(&self) {
        self.connection.upgrade().map(|connection| {
            let _ = connection.map_window(self.id);
        });
    }

    pub fn unmap(&self) {
        if let Some(connection) = self.connection.upgrade() {
            // ICCCM - Changing Window State
            //
            // Normal -> Withdrawn - The client should unmap the window and follow it with a synthetic
            // UnmapNotify event as described later in this section.
            let _ = connection.unmap_window(self.id);

            // Send a synthetic UnmapNotify event to make the ICCCM happy
            let _ = connection.send_event(
                false,
                self.id,
                EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY,
                UnmapNotifyEvent {
                    response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
                    sequence: 0, // Ignored by X server
                    event: self.root,
                    window: self.id,
                    from_configure: false,
                },
            );
        }
    }

    pub fn size(&self) -> Size<u16, Logical> {
        *self.size.lock().unwrap()
    }

    pub fn set_title(&self, title: &str) {
        if let Some(connection) = self.connection.upgrade() {
            // _NET_WM_NAME should be preferred by window managers, but set both properties.
            let _ = connection.change_property8(
                PropMode::REPLACE,
                self.id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            );

            let _ = connection.change_property8(
                PropMode::REPLACE,
                self.id,
                self.atoms._NET_WM_NAME,
                self.atoms.UTF8_STRING,
                title.as_bytes(),
            );
        }
    }
}

// TODO: Error type
fn allocate_buffers(device: &gbm::Device<RawFd>, size: Size<u16, Logical>) -> Result<(Dmabuf, Dmabuf), ()> {
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

    Ok((current, next))
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        let _ = self.connection.upgrade().map(|connection| {
            let _ = connection.destroy_window(self.id);
        });
    }
}
