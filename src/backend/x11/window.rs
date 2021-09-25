//! Utilities for managing an X11 window.

use crate::utils::{Logical, Size};

use super::{Atoms, WindowProperties, X11Error};
use std::sync::{Arc, Mutex, Weak};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self as x11, AtomEnum, CreateGCAux, CreateWindowAux, Depth, EventMask, PropMode, Screen, WindowClass,
};
use x11rb::protocol::xproto::{ConnectionExt as _, UnmapNotifyEvent};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt;

/// An X11 window.
#[derive(Debug)]
pub struct Window(Weak<WindowInner>);

impl Window {
    /// Sets the title of the window.
    pub fn set_title(&self, title: &str) {
        self.0.upgrade().map(|inner| inner.set_title(title));
    }

    /// Maps the window, making it visible.
    pub fn map(&self) {
        self.0.upgrade().map(|inner| inner.map());
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) {
        self.0.upgrade().map(|inner| inner.unmap());
    }

    /// Returns the size of this window.
    ///
    /// If the window has been destroyed, the size is `0 x 0`.
    pub fn size(&self) -> Size<u16, Logical> {
        self.0
            .upgrade()
            .map(|inner| inner.size())
            .unwrap_or((0, 0).into())
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        self.0.upgrade().map(|inner| inner.id).unwrap_or(0)
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        self.0.upgrade().map(|inner| inner.depth.depth).unwrap_or(0)
    }

    /// Returns the graphics context used to draw to this window.
    pub fn gc(&self) -> u32 {
        self.0.upgrade().map(|inner| inner.gc).unwrap_or(0)
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.upgrade(), other.0.upgrade()) {
            (Some(self_), Some(other)) => self_ == other,
            _ => false
        }
    }
}

impl From<Arc<WindowInner>> for Window {
    fn from(inner: Arc<WindowInner>) -> Self {
        Window(Arc::downgrade(&inner))
    }
}

#[derive(Debug)]
pub(crate) struct WindowInner {
    // TODO: Consider future x11rb WindowWrapper
    pub connection: Weak<RustConnection>,
    pub id: x11::Window,
    root: x11::Window,
    pub atoms: Atoms,
    pub size: Mutex<Size<u16, Logical>>,
    pub depth: Depth,
    pub gc: x11::Gcontext,
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

        // Send requests to change window properties while we wait for the window creation request to complete.
        let mut window = WindowInner {
            connection: weak,
            id: window,
            root: screen.root,
            atoms,
            size: Mutex::new((properties.width, properties.height).into()),
            depth,
            gc: 0,
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

impl PartialEq for WindowInner {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        self.connection.upgrade().map(|connection| {
            let _ = connection.destroy_window(self.id);
        });
    }
}
