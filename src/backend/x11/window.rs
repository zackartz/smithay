//! Utilities for managing an X11 window.

use crate::utils::{Logical, Size};

use super::{Atoms, WindowProperties, X11Error, XConnection};
use std::sync::{Arc, Mutex, Weak};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self as x11, AtomEnum, CreateGCAux, CreateWindowAux, Depth, EventMask, PropMode, Screen, WindowClass,
};
use x11rb::protocol::xproto::{ConnectionExt as _, UnmapNotifyEvent};
use x11rb::wrapper::ConnectionExt;

/// An X11 window.
#[derive(Debug)]
pub struct Window(Weak<WindowInner>);

impl Window {
    /// Sets the title of the window.
    pub fn set_title(&self, title: &str) -> Result<(), X11Error> {
        if let Some(inner) = self.0.upgrade() {
            inner.set_title(title)
        } else {
            Err(X11Error::WindowDestroyed)
        }
    }

    /// Maps the window, making it visible.
    pub fn map(&self) -> Result<(), X11Error> {
        if let Some(inner) = self.0.upgrade() {
            inner.map()
        } else {
            Err(X11Error::WindowDestroyed)
        }
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) -> Result<(), X11Error> {
        if let Some(inner) = self.0.upgrade() {
            inner.unmap()
        } else {
            Err(X11Error::WindowDestroyed)
        }
    }

    /// Returns the size of this window.
    pub fn size(&self) -> Result<Size<u16, Logical>, X11Error> {
        if let Some(inner) = self.0.upgrade() {
            Ok(inner.size())
        } else {
            Err(X11Error::WindowDestroyed)
        }
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        if let Some(inner) = self.0.upgrade() {
            inner.inner
        } else {
            0
        }
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        if let Some(inner) = self.0.upgrade() {
            inner.depth.depth
        } else {
            0
        }
    }

    /// Returns the graphics context used to draw to this window.
    pub fn gc(&self) -> u32 {
        if let Some(inner) = self.0.upgrade() {
            inner.gc
        } else {
            0
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
    pub connection: Arc<XConnection>,
    pub inner: x11::Window,
    root: x11::Window,
    pub atoms: Atoms,
    pub size: Mutex<Size<u16, Logical>>,
    pub depth: Depth,
    pub gc: x11::Gcontext,
}

impl WindowInner {
    pub fn new(
        connection: Arc<XConnection>,
        screen: &Screen,
        properties: WindowProperties<'_>,
        atoms: Atoms,
        depth: Depth,
        visual_id: u32,
        colormap: u32,
    ) -> Result<WindowInner, X11Error> {
        let xcb = connection.xcb_connection();

        // Generate the xid for the window
        let window = xcb.generate_id()?;
        let window_aux = CreateWindowAux::new()
            .event_mask(
                EventMask::EXPOSURE // Be told when the window is exposed
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::KEY_PRESS // Key press and release
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS // Mouse button press and release
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION // Mouse movement
            | EventMask::RESIZE_REDIRECT // Handling resizes
            | EventMask::NO_EVENT,
            )
            // Border pixel and color map need to be set if our depth may differ from the root depth.
            .border_pixel(0)
            .colormap(colormap);

        let cookie = xcb.create_window(
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
            connection: connection.clone(),
            inner: window,
            root: screen.root,
            atoms,
            size: Mutex::new((properties.width, properties.height).into()),
            depth,
            gc: 0,
        };

        let gc = xcb.generate_id()?;
        xcb.create_gc(gc, window.inner, &CreateGCAux::new())?;
        window.gc = gc;

        // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
        xcb.change_property32(
            PropMode::REPLACE,
            window.inner,
            atoms.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[atoms.WM_DELETE_WINDOW],
        )?;

        // Block until window creation is complete.
        cookie.check()?;
        window.set_title(properties.title)?;

        // Finally map the window
        xcb.map_window(window.inner)?;

        // Flush requests to server so window is displayed.
        xcb.flush()?;

        Ok(window)
    }

    pub fn map(&self) -> Result<(), X11Error> {
        self.connection.xcb_connection().map_window(self.inner)?;
        Ok(())
    }

    pub fn unmap(&self) -> Result<(), X11Error> {
        let xcb = self.connection.xcb_connection();

        // ICCCM - Changing Window State
        //
        // Normal -> Withdrawn - The client should unmap the window and follow it with a synthetic
        // UnmapNotify event as described later in this section.
        xcb.unmap_window(self.inner)?;

        // Send a synthetic UnmapNotify event to make the ICCCM happy
        xcb.send_event(
            false,
            self.inner,
            EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY,
            UnmapNotifyEvent {
                response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
                sequence: 0, // Ignored by X server
                event: self.root,
                window: self.inner,
                from_configure: false,
            },
        )?;

        Ok(())
    }

    pub fn size(&self) -> Size<u16, Logical> {
        *self.size.lock().unwrap()
    }

    pub fn set_title(&self, title: &str) -> Result<(), X11Error> {
        let xcb = self.connection.xcb_connection();

        // _NET_WM_NAME should be preferred by window managers, but set both in case.
        xcb.change_property8(
            PropMode::REPLACE,
            self.inner,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )?;

        xcb.change_property8(
            PropMode::REPLACE,
            self.inner,
            self.atoms._NET_WM_NAME,
            self.atoms.UTF8_STRING,
            title.as_bytes(),
        )?;

        // Set WM_CLASS
        let mut raw = Vec::new();
        raw.extend_from_slice(title.as_bytes());
        raw.extend_from_slice(b"\n");
        raw.extend_from_slice(title.as_bytes());
        raw.push(b'\n');

        let _ = xcb.change_property8(
            PropMode::REPLACE,
            self.inner,
            self.atoms.WM_CLASS,
            AtomEnum::STRING,
            &raw[..],
        )?;

        Ok(())
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        let _ = self.connection.xcb_connection().destroy_window(self.inner);
    }
}
