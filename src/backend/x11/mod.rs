//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor
//! directly as an X11 client.
//!

mod buffer;
mod event_source;
pub mod input;
mod window;

use self::window::WindowInner;
use super::input::{Axis, ButtonState, KeyState, MouseButton};
use crate::backend::input::InputEvent;
use crate::backend::x11::event_source::X11Source;
use crate::backend::x11::input::*;
use crate::utils::{Logical, Size};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use slog::{error, info, o, Logger};
use x11_dl::xlib_xcb::{Xlib_xcb, XEventQueueOwner};
use x11rb::xcb_ffi::XCBConnection;
use std::{fmt, io, ptr};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Weak;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol::xproto::{ColormapAlloc, ConnectionExt, Depth, VisualClass};
use x11rb::rust_connection::ReplyOrIdError;
use x11rb::x11_utils::X11Error as ImplError;
use x11rb::{atom_manager, protocol as x11};
use x11_dl::xlib::Xlib;

/// An error that may occur when initializing the backend.
#[derive(Debug, thiserror::Error)]
pub enum InitializationError {}

/// An error emitted by the X11 backend.
#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    /// An error that may occur when initializing the backend.
    #[error("Error while initializing backend")]
    Initialization(InitializationError),

    /// Connecting to the X server failed.
    #[error("Connecting to the X server failed")]
    ConnectionFailed(ConnectError),

    /// An X11 error packet was encountered.
    #[error("An X11 error packet was encountered.")]
    Protocol(ReplyOrIdError),

    /// The window was destroyed.
    #[error("The window was destroyed")]
    WindowDestroyed,
}

impl From<InitializationError> for X11Error {
    fn from(e: InitializationError) -> Self {
        X11Error::Initialization(e)
    }
}

impl From<ConnectError> for X11Error {
    fn from(e: ConnectError) -> Self {
        X11Error::ConnectionFailed(e)
    }
}

impl From<ConnectionError> for X11Error {
    fn from(e: ConnectionError) -> Self {
        ReplyOrIdError::from(e).into()
    }
}

impl From<ImplError> for X11Error {
    fn from(e: ImplError) -> Self {
        ReplyOrIdError::from(e).into()
    }
}

impl From<ReplyError> for X11Error {
    fn from(e: ReplyError) -> Self {
        ReplyOrIdError::from(e).into()
    }
}

impl From<ReplyOrIdError> for X11Error {
    fn from(e: ReplyOrIdError) -> Self {
        X11Error::Protocol(e)
    }
}

/// Properties defining information about the window created by the X11 backend.
#[derive(Debug, Clone, Copy)]
#[allow(missing_docs)] // Self explanatory fields
pub struct WindowProperties<'a> {
    pub width: u16,
    pub height: u16,
    pub title: &'a str,
}

impl Default for WindowProperties<'_> {
    fn default() -> Self {
        WindowProperties {
            width: 1280,
            height: 800,
            title: "Smithay",
        }
    }
}

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

    pub fn depth(&self) -> u8 {
        if let Some(inner) = self.0.upgrade() {
            inner.depth.depth
        } else {
            0
        }
    }

    pub fn gc(&self) -> u32 {
        if let Some(inner) = self.0.upgrade() {
            inner.gc
        } else {
            0
        }
    }
}

/// An event emitted by the X11 backend.
#[derive(Debug)]
pub enum X11Event {
    /// The X server has sent an expose event, requiring the compositor to redraw the window.
    ///
    /// This is only called when redrawing is required, otherwise you should schedule drawing to
    /// the window yourself.
    Expose,

    /// An input event occurred.
    Input(InputEvent<X11Input>),

    /// The window was resized.
    Resized(Size<u16, Logical>),

    /// The window was requested to be closed.
    CloseRequested,
}

pub struct XConnection {
    pub xlib_library: Xlib,
    pub xlib_display: *mut x11_dl::xlib::Display,
    pub xcb_connection: XCBConnection,
}

impl XConnection {
    pub fn new() -> Result<(XConnection, usize), X11Error> {
        let xlib = Xlib::open().expect("Failed to open xlib library");
        let xlib_xcb = Xlib_xcb::open().expect("Failed to load xlib_libxcb");
        let (display, screen_number) = unsafe {
            let display = (xlib.XOpenDisplay)(ptr::null());

            if display.is_null() {
                todo!("Failed to open display");
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
                return Err(todo!("Must have Xlib_xcb"));
            }

            // Do not drop the connection upon closure since Xlib created the xcb_connection_t
            XCBConnection::from_raw_xcb_connection(xcb_connection_t, false)
        }?;

        Ok((XConnection {
            xlib_library: xlib,
            xlib_display: display,
            xcb_connection,
        }, screen_number as usize))
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

/// An abstraction representing a connection to the X11 server.
#[derive(Debug)]
pub struct X11Backend {
    log: Logger,
    connection: Arc<XConnection>,
    source: X11Source,
    window: Arc<WindowInner>,
    key_counter: Arc<AtomicU32>,
    depth: Depth,
    visual_id: u32,
}

unsafe impl Send for X11Backend {}

atom_manager! {
    pub(crate) Atoms: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_CLASS,
        _NET_WM_NAME,
        UTF8_STRING,
        _SMITHAY_X11_BACKEND_CLOSE,
    }
}

impl X11Backend {
    /// Initializes the X11 backend, connecting to the X server and creating the window the compositor may output to.
    pub fn new<L>(properties: WindowProperties<'_>, logger: L) -> Result<X11Backend, X11Error>
    where
        L: Into<Option<slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));

        

        info!(log, "Connecting to the X server");

        let (connection, screen_number) = XConnection::new()?;
        let connection = Arc::new(connection);
        info!(log, "Connected to screen {}", screen_number);

        let xcb = &connection.xcb_connection;
        let screen = &xcb.setup().roots[screen_number];

        // We want 32 bit color
        let depth = screen
            .allowed_depths
            .iter()
            .find(|depth| depth.depth == 32)
            .cloned()
            .expect("TODO");

        // Next find a visual using the supported depth
        let visual_id = depth
            .visuals
            .iter()
            .find(|visual| visual.class == VisualClass::TRUE_COLOR)
            .expect("TODO")
            .visual_id;

        // Find a supported format.
        // TODO

        // Make a colormap
        let colormap = xcb.generate_id()?;
        &xcb.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)?;

        let atoms = Atoms::new(xcb)?.reply()?;

        let window = Arc::new(WindowInner::new(
            connection.clone(),
            screen,
            properties,
            atoms,
            depth.clone(),
            visual_id,
            colormap,
        )?);
        let source = X11Source::new(
            connection.clone(),
            window.inner,
            atoms._SMITHAY_X11_BACKEND_CLOSE,
            log.clone(),
        );

        info!(log, "Window created");

        Ok(X11Backend {
            log,
            source,
            connection,
            window,
            key_counter: Arc::new(AtomicU32::new(0)),
            depth,
            visual_id,
        })
    }

    pub fn connection(&self) -> &XConnection {
        &self.connection
    }

    /// Returns a handle to the X11 window this input backend handles inputs for.
    pub fn window(&self) -> Window {
        Window(Arc::downgrade(&self.window))
    }
}

impl EventSource for X11Backend {
    type Event = X11Event;

    type Metadata = Window;

    type Ret = ();

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        use self::X11Event::Input;

        let connection = self.connection.clone();
        let window = self.window.clone();
        let key_counter = self.key_counter.clone();
        let log = self.log.clone();
        let mut event_window = Window(Arc::downgrade(&window));

        self.source
            .process_events(readiness, token, |event, _| {
                match event {
                    // Input events need to be queued up:
                    x11::Event::ButtonPress(button_press) => {
                        if button_press.event == window.inner {
                            // X11 decided to associate scroll wheel with a button, 4, 5, 6 and 7 for
                            // up, down, right and left. For scrolling, a press event is emitted and a
                            // release is them immediately followed for scrolling. This means we can
                            // ignore release for scrolling.

                            // Ideally we would use `ButtonIndex` from XCB, however it does not cover 6 and 7
                            // for horizontal scroll and does not work nicely in match statements, so we
                            // use magic constants here.
                            match button_press.detail {
                                1..=3 => {
                                    // Clicking a button.
                                    callback(
                                        Input(InputEvent::PointerButton {
                                            event: X11MouseInputEvent {
                                                time: button_press.time,
                                                button: match button_press.detail {
                                                    1 => MouseButton::Left,

                                                    // Confusion: XCB docs for ButtonIndex and what plasma does don't match?
                                                    2 => MouseButton::Middle,

                                                    3 => MouseButton::Right,

                                                    _ => unreachable!(),
                                                },
                                                state: ButtonState::Pressed,
                                            },
                                        }),
                                        &mut event_window,
                                    )
                                }

                                4..=7 => {
                                    // Scrolling
                                    callback(
                                        Input(InputEvent::PointerAxis {
                                            event: X11MouseWheelEvent {
                                                time: button_press.time,
                                                axis: match button_press.detail {
                                                    // Up | Down
                                                    4 | 5 => Axis::Vertical,

                                                    // Right | Left
                                                    6 | 7 => Axis::Horizontal,

                                                    _ => unreachable!(),
                                                },
                                                amount: match button_press.detail {
                                                    // Up | Right
                                                    4 | 7 => 1.0,

                                                    // Down | Left
                                                    5 | 6 => -1.0,

                                                    _ => unreachable!(),
                                                },
                                            },
                                        }),
                                        &mut event_window,
                                    )
                                }

                                // Unknown button?
                                _ => callback(
                                    Input(InputEvent::PointerButton {
                                        event: X11MouseInputEvent {
                                            time: button_press.time,
                                            button: MouseButton::Other(button_press.detail),
                                            state: ButtonState::Pressed,
                                        },
                                    }),
                                    &mut event_window,
                                ),
                            }
                        }
                    }

                    x11::Event::ButtonRelease(button_release) => {
                        if button_release.event == window.inner {
                            match button_release.detail {
                                1..=3 => {
                                    // Releasing a button.
                                    callback(
                                        Input(InputEvent::PointerButton {
                                            event: X11MouseInputEvent {
                                                time: button_release.time,
                                                button: match button_release.detail {
                                                    1 => MouseButton::Left,

                                                    2 => MouseButton::Middle,

                                                    3 => MouseButton::Right,

                                                    _ => unreachable!(),
                                                },
                                                state: ButtonState::Released,
                                            },
                                        }),
                                        &mut event_window,
                                    )
                                }

                                // We may ignore the release tick for scrolling, as the X server will
                                // always emit this immediately after press.
                                4..=7 => (),

                                _ => callback(
                                    Input(InputEvent::PointerButton {
                                        event: X11MouseInputEvent {
                                            time: button_release.time,
                                            button: MouseButton::Other(button_release.detail),
                                            state: ButtonState::Released,
                                        },
                                    }),
                                    &mut event_window,
                                ),
                            }
                        }
                    }

                    x11::Event::KeyPress(key_press) => {
                        if key_press.event == window.inner {
                            callback(
                                Input(InputEvent::Keyboard {
                                    event: X11KeyboardInputEvent {
                                        time: key_press.time,
                                        // It seems as if X11's keycodes are +8 relative to the libinput
                                        // keycodes that are expected, so subtract 8 from each keycode
                                        // to be correct.
                                        key: key_press.detail as u32 - 8,
                                        count: key_counter.fetch_add(1, Ordering::SeqCst) + 1,
                                        state: KeyState::Pressed,
                                    },
                                }),
                                &mut event_window,
                            )
                        }
                    }

                    x11::Event::KeyRelease(key_release) => {
                        if key_release.event == window.inner {
                            // atomic u32 has no checked_sub, so load and store to do the same.
                            let mut key_counter_val = key_counter.load(Ordering::SeqCst);
                            key_counter_val = key_counter_val.saturating_sub(1);
                            key_counter.store(key_counter_val, Ordering::SeqCst);

                            callback(
                                Input(InputEvent::Keyboard {
                                    event: X11KeyboardInputEvent {
                                        time: key_release.time,
                                        // It seems as if X11's keycodes are +8 relative to the libinput
                                        // keycodes that are expected, so subtract 8 from each keycode
                                        // to be correct.
                                        key: key_release.detail as u32 - 8,
                                        count: key_counter_val,
                                        state: KeyState::Released,
                                    },
                                }),
                                &mut event_window,
                            );
                        }
                    }

                    x11::Event::MotionNotify(motion_notify) => {
                        if motion_notify.event == window.inner {
                            // Use event_x/y since those are relative the the window receiving events.
                            let x = motion_notify.event_x as f64;
                            let y = motion_notify.event_y as f64;

                            callback(
                                Input(InputEvent::PointerMotionAbsolute {
                                    event: X11MouseMovedEvent {
                                        time: motion_notify.time,
                                        x,
                                        y,
                                        size: window.size(),
                                    },
                                }),
                                &mut event_window,
                            )
                        }
                    }

                    x11::Event::ResizeRequest(resized) => {
                        if resized.window == window.inner {
                            let size: Size<u16, Logical> = (resized.width, resized.height).into();

                            {
                                *window.size.lock().unwrap() = size;
                            }

                            (callback)(X11Event::Resized(size), &mut event_window);
                        }
                    }

                    x11::Event::ClientMessage(client_message) => {
                        // Were we told to destroy the window?
                        if client_message.data.as_data32()[0] == window.atoms.WM_DELETE_WINDOW
                            && client_message.window == window.inner
                        {
                            (callback)(X11Event::CloseRequested, &mut event_window);
                        }
                    }

                    x11::Event::Expose(expose) => {
                        // TODO: We would ideally use this to determine damage and render more efficiently that way.
                        if expose.window == window.inner && expose.count == 0 {
                            (callback)(X11Event::Expose, &mut event_window);
                        }
                    }

                    x11::Event::Error(e) => {
                        error!(log, "X11 error: {:?}", e);
                    }

                    _ => (),
                }

                // Now flush requests to the clients.
                let _ = connection.xcb_connection.flush();
            })
            .expect("TODO");

        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.source.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        self.source.unregister(poll)
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        todo!()
    }
}
