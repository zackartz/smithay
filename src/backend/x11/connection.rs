//! Utilities to open a connection to an X server using [x11rb](https://github.com/psychon/x11rb).

use std::ptr;

use lazy_static::lazy_static;
use slog::Logger;
use x11_dl::{
    error::OpenError,
    xlib::Xlib,
    xlib_xcb::{XEventQueueOwner, Xlib_xcb},
};
use x11rb::{rust_connection::ConnectError, xcb_ffi::XCBConnection};

/// An error that may occur when connecting to the X server.
#[derive(Debug, thiserror::Error)]
pub enum ConnectToXError {
    /// An error occured while setting up XCB.
    #[error("XCB failed to connect to the X server")]
    Xcb(ConnectError),

    /// Some required libraries were not present.
    #[error("Required libraries (xlib and xlib_libxcb) were not loaded")]
    LibrariesNotLoaded(OpenError),

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

impl From<OpenError> for ConnectToXError {
    fn from(e: OpenError) -> Self {
        Self::LibrariesNotLoaded(e)
    }
}

pub(crate) struct X11Libraries {
    pub xlib: Xlib,
    pub xlib_xcb: Xlib_xcb,
}

lazy_static! {
    pub(crate) static ref LIBRARIES: Result<X11Libraries, ConnectToXError> = {
        let xlib = Xlib::open().map(|library| {
            // In order to make XConnection Send and Sync, we need to call this that any Displays
            // created by xlib are thread safe.
            unsafe { (library.XInitThreads)() };

            library
        })?;

        let xlib_xcb = Xlib_xcb::open()?;

        Ok(X11Libraries {
            xlib,
            xlib_xcb,
        })
    };
}

/// A connection to the X server.
///
/// This contains a way to access both the xcb connection and the xlib Display.
#[derive(Debug)]
pub struct XConnection {
    /// The xlib Display that initiated the XCBConnection
    ///
    /// If we want to allow creation of an EGL Context using X11, the XCB extensions are not
    /// implemented most drivers yet. So we also expose the Xlib types so an OpenGL context
    /// can be made in nearly every driver.
    xlib_display: *mut x11_dl::xlib::Display,
    xcb_connection: XCBConnection,
}

impl XConnection {
    /// Attempts to connect to the X server.
    pub(crate) fn new(_logger: &Logger) -> Result<(XConnection, usize), ConnectToXError> {
        let library = LIBRARIES.as_ref().expect("Handle this error");
        let xlib = &library.xlib;
        let xlib_xcb = &library.xlib_xcb;

        // TODO: Log setup process
        let (display, screen_number) = unsafe {
            let display = (xlib.XOpenDisplay)(ptr::null());

            if display.is_null() {
                return Err(ConnectToXError::XlibNoConnection);
            }

            (display, (xlib.XDefaultScreen)(display))
        };

        // Transfer ownership of the event queue to XCB since we use XCB to handle events.
        let xcb_connection_t = unsafe {
            let ptr = (xlib_xcb.XGetXCBConnection)(display);
            (xlib_xcb.XSetEventQueueOwner)(display, XEventQueueOwner::XCBOwnsEventQueue);
            ptr
        };

        let xcb_connection = unsafe {
            // Xlib implementation does not use XCB
            if xcb_connection_t.is_null() {
                (xlib.XCloseDisplay)(display);
                return Err(ConnectToXError::NoXlibXcb);
            }

            // Do not drop the connection upon closure since Xlib created the xcb_connection_t.
            // The Drop impl of `XConnection` will shutdown the Xlib Display
            XCBConnection::from_raw_xcb_connection(xcb_connection_t, false)
        };

        if xcb_connection.is_err() {
            unsafe { (xlib.XCloseDisplay)(display) };
            return Err(ConnectToXError::Xcb(xcb_connection.err().unwrap()));
        }

        Ok((
            XConnection {
                xlib_display: display,
                xcb_connection: xcb_connection.unwrap(),
            },
            screen_number as usize,
        ))
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

// Xlib (after calling XInitThreads) and libxcb are both thread safe.
unsafe impl Send for XConnection {}
unsafe impl Sync for XConnection {}

impl Drop for XConnection {
    fn drop(&mut self) {
        // Close the display
        unsafe {
            (LIBRARIES.as_ref().unwrap().xlib.XCloseDisplay)(self.xlib_display);
        }
    }
}
