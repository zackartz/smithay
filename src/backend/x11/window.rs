//! Utilities for managing an X11 window.

use super::{WindowProperties, X11Error};
use std::rc::Rc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, UnmapNotifyEvent};
use x11rb::protocol::xproto::{
    self as x11, Atom, AtomEnum, CreateWindowAux, EventMask, PropMode, Screen, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Atoms {
    pub wm_protocols: Atom,
    pub wm_delete_window: Atom,
    pub net_wm_name: Atom,
    pub utf8_string: Atom,
}

impl Atoms {
    pub fn new(connection: Rc<RustConnection>) -> Result<Atoms, X11Error> {
        // Stagger intern requests and checking the reply in each cookie as not to block during each request.
        let wm_protocols = connection.intern_atom(false, b"WM_PROTOCOLS")?;
        let wm_delete_window = connection.intern_atom(false, b"WM_DELETE_WINDOW")?;
        let net_wm_name = connection.intern_atom(false, b"_NET_WM_NAME")?;
        let utf8_string = connection.intern_atom(false, b"UTF8_STRING")?;
        let wm_protocols = wm_protocols.reply().unwrap().atom;
        let wm_delete_window = wm_delete_window.reply().unwrap().atom;
        let net_wm_name = net_wm_name.reply().unwrap().atom;
        let utf8_string = utf8_string.reply().unwrap().atom;

        Ok(Atoms {
            wm_protocols,
            wm_delete_window,
            net_wm_name,
            utf8_string,
        })
    }
}

#[derive(Debug)]
pub(crate) struct WindowInner {
    pub connection: Rc<RustConnection>,
    pub inner: x11::Window,
    root: x11::Window,
    pub atoms: Atoms,
}

impl WindowInner {
    pub fn new(
        connection: Rc<RustConnection>,
        screen: &Screen,
        atoms: Atoms,
        properties: WindowProperties<'_>,
    ) -> Result<WindowInner, X11Error> {
        // Generate the xid for the window
        let window = connection.generate_id()?;
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
            );

        let cookie = connection.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            properties.width,
            properties.height,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &window_aux,
        )?;

        // Send requests to change window properties while we wait for the window creation request to complete.
        let window = WindowInner {
            connection: connection.clone(),
            inner: window,
            root: screen.root,
            atoms,
        };

        // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
        connection.change_property32(
            PropMode::REPLACE,
            window.inner,
            atoms.wm_protocols,
            AtomEnum::ATOM,
            &[atoms.wm_delete_window],
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
            self.atoms.net_wm_name,
            self.atoms.utf8_string,
            title.as_bytes(),
        );
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.inner);
    }
}
