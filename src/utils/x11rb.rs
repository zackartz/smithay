//! Utilities to open a connection to an X server using [x11rb](https://github.com/psychon/x11rb).

use std::{fmt, ptr};

use x11_dl::{xlib::Xlib, xlib_xcb::{XEventQueueOwner, Xlib_xcb}};
use x11rb::{rust_connection::ConnectError, xcb_ffi::XCBConnection};

/// An error that may occur when connecting to the X server.
#[derive(Debug, thiserror::Error)]
pub enum ConnectToXError {
    /// An error occured while setting up XCB.
    #[error("XCB failed to connect to the X server")]
    Xcb(ConnectError),

    /// Some required libraries were not present.
    #[error("Required libraries (xlib and xlib_libxcb) were not loaded")]
    LibrariesNotLoaded,

    /// Xlib failed to initialize it's connection.
    #[error("An xlib connection could not be established")]
    XlibNoConnection,

    /// The Xlib implementation is not xcb based.
    #[error("Xlib implementation does not support getting the inner xcb connection")]
    NoXlibXcb,
}

impl From<ConnectError> for ConnectToXError {
    fn from(inner: ConnectError) -> Self {
        Self::Xcb(inner)
    }
}

/// A connection to the X server.
///
/// This contains a way to access both the xcb connection and the xlib Display.
pub struct XConnection {
    xlib_library: Xlib,
    xlib_display: *mut x11_dl::xlib::Display,
    xcb_connection: XCBConnection,
}

impl XConnection {
    /// Attempts to connect to the X server.
    pub fn new() -> Result<(XConnection, usize), ConnectToXError> {
        let xlib = Xlib::open().expect("Failed to open xlib library");
        let xlib_xcb = Xlib_xcb::open().expect("Failed to load xlib_libxcb");
        let (display, screen_number) = unsafe {
            let display = (xlib.XOpenDisplay)(ptr::null());

            if display.is_null() {
                return Err(ConnectToXError::XlibNoConnection);
            }

            (display, (xlib.XDefaultScreen)(display))
        };

        // Transfer ownership of the event queue to XCB
        let xcb_connection_t = unsafe {
            let ptr = (xlib_xcb.XGetXCBConnection)(display);
            (xlib_xcb.XSetEventQueueOwner)(display, XEventQueueOwner::XCBOwnsEventQueue);
            ptr
        };

        let xcb_connection = unsafe {
            if xcb_connection_t.is_null() {
                (xlib.XCloseDisplay)(display);
                return Err(ConnectToXError::NoXlibXcb);
            }

            // Do not drop the connection upon closure since Xlib created the xcb_connection_t
            XCBConnection::from_raw_xcb_connection(xcb_connection_t, false)
        };

        if xcb_connection.is_err() {
            unsafe { (xlib.XCloseDisplay)(display) };
            return Err(ConnectToXError::Xcb(xcb_connection.err().unwrap()));
        }

        Ok((XConnection {
            xlib_library: xlib,
            xlib_display: display,
            xcb_connection: xcb_connection.unwrap(),
        }, screen_number as usize))
    }

    /// Returns the xcb connection
    pub fn xcb_connection(&self) -> &XCBConnection {
        &self.xcb_connection
    }

    /// Returns a pointer to the xlib display.
    pub fn xlib_display(&self) -> *mut x11_dl::xlib::Display {
        self.xlib_display
    }
}

// Xlib and libxcb are both thread safe.
unsafe impl Send for XConnection {}
unsafe impl Sync for XConnection {}

impl fmt::Debug for XConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XConnection")
            .field("xlib_library", &"xlib")
            .field("xlib_display", &self.xlib_display)
            .field("xcb_connection", &self.xcb_connection)
            .finish()
    }
}

impl Drop for XConnection {
    fn drop(&mut self) {
        // Close the display
        unsafe {
            (self.xlib_library.XCloseDisplay)(self.xlib_display);
        }
    }
}
