//! Utilities for managing an X11 window.

use crate::utils::{Logical, Size};

use super::{WindowProperties, X11Error};
use std::cell::RefCell;
use std::rc::Rc;
use x11rb::atom_manager;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self as x11, AtomEnum, CreateWindowAux, Depth, EventMask, PropMode, Screen, WindowClass,
};
use x11rb::protocol::xproto::{ConnectionExt as _, UnmapNotifyEvent};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt;

atom_manager! {
    pub Atoms: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_CLASS,
        _NET_WM_NAME,
        UTF8_STRING,
    }
}

#[derive(Debug)]
pub(crate) struct WindowInner {
    pub connection: Rc<RustConnection>,
    pub inner: x11::Window,
    root: x11::Window,
    pub atoms: Atoms,
    pub size: RefCell<Size<u16, Logical>>,
    pub depth: Depth,
}

impl WindowInner {
    pub fn new(
        connection: Rc<RustConnection>,
        screen: &Screen,
        properties: WindowProperties<'_>,
        depth: Depth,
        visual_id: u32,
        colormap: u32,
    ) -> Result<WindowInner, X11Error> {
        let atoms = Atoms::new(&*connection)?.reply()?;

        // Generate the xid for the window
        let window = connection.generate_id()?;
        let window_aux = CreateWindowAux::new().event_mask(
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

        let cookie = connection.create_window(
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
        let window = WindowInner {
            connection: connection.clone(),
            inner: window,
            root: screen.root,
            atoms,
            size: RefCell::new((properties.width, properties.height).into()),
            depth,
        };

        // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
        connection.change_property32(
            PropMode::REPLACE,
            window.inner,
            atoms.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[atoms.WM_DELETE_WINDOW],
        )?;

        // Block until window creation is complete.
        cookie.check()?;
        window.set_title(properties.title);

        // Finally map the window
        connection.map_window(window.inner)?;

        // Flush requests to server so window is displayed.
        connection.flush()?;

        Ok(window)
    }

    pub fn map(&self) {
        let _ = self.connection.map_window(self.inner);
    }

    pub fn unmap(&self) {
        // ICCCM - Changing Window State
        //
        // Normal -> Withdrawn - The client should unmap the window and follow it with a synthetic
        // UnmapNotify event as described later in this section.
        let _ = self.connection.unmap_window(self.inner);

        // Send a synthetic UnmapNotify event to make the ICCCM happy
        let _ = self.connection.send_event(
            false,
            self.inner,
            EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY,
            UnmapNotifyEvent {
                response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
                sequence: 0,
                event: self.root,
                window: self.inner,
                from_configure: false,
            },
        );
    }

    pub fn size(&self) -> Size<u16, Logical> {
        *self.size.borrow()
    }

    pub fn set_title(&self, title: &str) {
        // _NET_WM_NAME should be preferred by window managers, but set both in case.
        let _ = self.connection.change_property8(
            PropMode::REPLACE,
            self.inner,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        );

        let _ = self.connection.change_property8(
            PropMode::REPLACE,
            self.inner,
            self.atoms._NET_WM_NAME,
            self.atoms.UTF8_STRING,
            title.as_bytes(),
        );

        // Set WM_CLASS
        let mut raw = Vec::new();
        raw.extend_from_slice(title.as_bytes());
        raw.extend_from_slice(b"\n");
        raw.extend_from_slice(title.as_bytes());
        raw.push(b'\n');

        let _ = self.connection.change_property8(
            PropMode::REPLACE,
            self.inner,
            self.atoms.WM_CLASS,
            AtomEnum::STRING,
            &raw[..],
        );
    }

    pub fn format(&self) {
        todo!()
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.inner);
    }
}
