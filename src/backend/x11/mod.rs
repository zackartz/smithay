//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor
//! directly as an X11 client.
//!

mod buffer;
mod window;

use self::window::{Atoms, WindowInner};
use super::input::{
    Axis, AxisSource, ButtonState, Device, InputBackend, KeyState, KeyboardKeyEvent, MouseButton,
    PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, UnusedEvent,
};
use crate::backend::input::InputEvent;
use crate::backend::input::{DeviceCapability, Event as BackendEvent};
use slog::{info, o, Logger};
use std::sync::Arc;
use std::sync::Weak;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol as x11;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::x11_utils::X11Error as ImplError;
use x11rb::xcb_ffi::XCBConnection;

/// An error emitted by the X11 backend.
#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    /// Connecting to the X server failed.
    #[error("Connecting to the X server failed")]
    ConnectionFailed(ConnectError),

    /// An error occured with the connection to the X server.
    #[error("An error occured with the connection to the X server.")]
    ConnectionError(ConnectionError),

    /// An X11 error packet was encountered.
    #[error("An X11 error packet was encountered.")]
    Protocol(ImplError),

    /// The window was destroyed.
    #[error("The window was destroyed")]
    WindowDestroyed,
}

impl From<ConnectError> for X11Error {
    fn from(e: ConnectError) -> Self {
        X11Error::ConnectionFailed(e)
    }
}

impl From<ConnectionError> for X11Error {
    fn from(e: ConnectionError) -> Self {
        X11Error::ConnectionError(e)
    }
}

impl From<ImplError> for X11Error {
    fn from(e: ImplError) -> Self {
        X11Error::Protocol(e)
    }
}

impl From<ReplyError> for X11Error {
    fn from(e: ReplyError) -> Self {
        match e {
            ReplyError::ConnectionError(e) => e.into(),
            ReplyError::X11Error(e) => e.into(),
        }
    }
}

/// Properties defining information about the Window created by the X11 backend.
// TODO:
// - Rendering? I guess we allow binding buffers for this?
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

    /// Returns the XID of the window.
    ///
    /// Returns `None` if the window has been destroyed.
    pub fn id(&self) -> Option<u32> {
        self.0.upgrade().map(|w| w.inner)
    }

    // TODO: Window size?
}

/// An abstraction representing a connection to the X11 server.
#[derive(Debug)]
pub struct X11Backend {
    log: Logger,
    connection: Arc<XCBConnection>,
    window: Arc<WindowInner>,
}

impl X11Backend {
    /// Initializes the X11 backend, connecting to the X server and creating the window the compositor may output to.
    pub fn init<L>(properties: WindowProperties<'_>, logger: L) -> Result<X11Backend, X11Error>
    where
        L: Into<Option<slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));

        info!(log, "Connecting to the X server");
        let (connection, screen_number) = XCBConnection::connect(None)?;
        let connection = Arc::new(connection);
        info!(log, "Connected to screen {}", screen_number);

        let screen = &connection.setup().roots[screen_number];
        let atoms = Atoms::new(connection.clone())?;
        let window = Arc::new(WindowInner::new(connection.clone(), screen, atoms, properties)?);
        info!(log, "Window created");

        Ok(X11Backend {
            log,
            connection: connection,
            window,
        })
    }

    /// Returns a handle to the X11 window this input backend handles inputs for.
    pub fn window(&self) -> Window {
        Window(Arc::downgrade(&self.window))
    }
}

/// Virtual input device used by the backend to associate input events.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct X11VirtualDevice;

impl Device for X11VirtualDevice {
    fn id(&self) -> String {
        "x11".to_owned()
    }

    fn name(&self) -> String {
        "x11 virtual input".to_owned()
    }

    fn has_capability(&self, capability: super::input::DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer | DeviceCapability::Touch
        )
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`KeyboardKeyEvent`].
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11KeyboardInputEvent {
    time: u32,
    key: u32,
    count: u32,
    state: KeyState,
}

impl BackendEvent<X11Backend> for X11KeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl KeyboardKeyEvent<X11Backend> for X11KeyboardInputEvent {
    fn key_code(&self) -> u32 {
        self.key
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        self.count
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerAxisEvent`]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct X11MouseWheelEvent {
    time: u32,
    axis: Axis,
    amount: f64,
}

impl BackendEvent<X11Backend> for X11MouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerAxisEvent<X11Backend> for X11MouseWheelEvent {
    fn amount(&self, _axis: Axis) -> Option<f64> {
        None
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        // TODO: Is this proper?
        if self.axis == axis {
            Some(self.amount)
        } else {
            None
        }
    }

    fn source(&self) -> AxisSource {
        // X11 seems to act within the scope of individual rachets of a scroll wheel.
        AxisSource::Wheel
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerButtonEvent`]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11MouseInputEvent {
    time: u32,
    button: MouseButton,
    state: ButtonState,
}

impl BackendEvent<X11Backend> for X11MouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerButtonEvent<X11Backend> for X11MouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerMotionAbsoluteEvent`]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11MouseMovedEvent {
    time: u32,
}

impl BackendEvent<X11Backend> for X11MouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<X11Backend> for X11MouseMovedEvent {
    fn x(&self) -> f64 {
        todo!()
    }

    fn y(&self) -> f64 {
        todo!()
    }

    fn x_transformed(&self, _width: i32) -> f64 {
        todo!()
    }

    fn y_transformed(&self, _height: i32) -> f64 {
        todo!()
    }
}

impl InputBackend for X11Backend {
    type EventError = X11Error;

    type Device = X11VirtualDevice;
    type KeyboardKeyEvent = X11KeyboardInputEvent;
    type PointerAxisEvent = X11MouseWheelEvent;
    type PointerButtonEvent = X11MouseInputEvent;

    type PointerMotionEvent = UnusedEvent;

    type PointerMotionAbsoluteEvent = X11MouseMovedEvent;

    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;

    fn dispatch_new_events<F>(&mut self, mut callback: F) -> Result<(), Self::EventError>
    where
        F: FnMut(super::input::InputEvent<Self>),
    {
        while let Some(event) = self.connection.poll_for_event().expect("TODO: Error") {
            match event {
                x11::Event::Error(_) => (), // todo!("Handle error"),

                x11::Event::ButtonPress(event) => {
                    if event.event == self.window.inner {
                        // X11 decided to associate scroll wheel with a button, 4, 5, 6 and 7 for
                        // up, down, right and left. For scrolling, a press event is emitted and a
                        // release is them immediately followed for scrolling. This means we can
                        // ignore release for scrolling.

                        match event.detail {
                            1..=3 => {
                                // Clicking a button.
                                callback(InputEvent::PointerButton {
                                    event: X11MouseInputEvent {
                                        time: event.time,
                                        button: match event.detail {
                                            1 => MouseButton::Left,

                                            2 => MouseButton::Middle,

                                            3 => MouseButton::Right,

                                            _ => unreachable!(),
                                        },
                                        state: ButtonState::Pressed,
                                    },
                                })
                            }

                            4..=7 => {
                                // Scrolling
                                callback(InputEvent::PointerAxis {
                                    event: X11MouseWheelEvent {
                                        time: event.time,
                                        axis: match event.detail {
                                            // Up | Down
                                            4 | 5 => Axis::Vertical,

                                            // Right | Left
                                            6 | 7 => Axis::Horizontal,

                                            _ => unreachable!(),
                                        },
                                        amount: match event.detail {
                                            // Up | Right
                                            4 | 7 => 1.0,

                                            // Down | Left
                                            5 | 6 => -1.0,

                                            _ => unreachable!()
                                        },
                                    },
                                })
                            }

                            // Unknown button?
                            _ => callback(InputEvent::PointerButton {
                                event: X11MouseInputEvent {
                                    time: event.time,
                                    button: MouseButton::Other(event.detail),
                                    state: ButtonState::Pressed,
                                },
                            }),
                        }
                    }
                }

                x11::Event::ButtonRelease(event) => {
                    if event.event == self.window.inner {
                        match event.detail {
                            1..=3 => {
                                // Releasing a button.
                                callback(InputEvent::PointerButton {
                                    event: X11MouseInputEvent {
                                        time: event.time,
                                        button: match event.detail {
                                            1 => MouseButton::Left,

                                            2 => MouseButton::Middle,

                                            3 => MouseButton::Right,

                                            _ => unreachable!(),
                                        },
                                        state: ButtonState::Released,
                                    },
                                })
                            }

                            // We may ignore the release tick for scrolling, as the X server will
                            // always emit this immediately after press.
                            4..=7 => (),

                            _ => callback(InputEvent::PointerButton {
                                event: X11MouseInputEvent {
                                    time: event.time,
                                    button: MouseButton::Other(event.detail),
                                    state: ButtonState::Released,
                                },
                            }),
                        }
                    }
                }

                x11::Event::Expose(_) => (), // todo!("Handle expose"),

                // TODO: Is it correct to directly cast the details of the event in? Or do we need to preprocess with xkbcommon
                x11::Event::KeyPress(event) => {
                    // Only handle key events if the event occurred in our own window.
                    if event.event == self.window.inner {
                        callback(InputEvent::Keyboard {
                            event: X11KeyboardInputEvent {
                                time: event.time,
                                key: event.detail as u32,
                                count: 1, // TODO: Counter
                                state: KeyState::Pressed,
                            },
                        })
                    }
                }

                x11::Event::KeyRelease(event) => {
                    // Only handle key events if the event occurred in our own window.
                    if event.event == self.window.inner {
                        callback(InputEvent::Keyboard {
                            event: X11KeyboardInputEvent {
                                time: event.time,
                                key: event.detail as u32,
                                count: 1, // TODO: Counter
                                state: KeyState::Released,
                            },
                        })
                    }
                }

                x11::Event::ResizeRequest(_) => (), // todo!("Handle resize"),

                // TODO: Is this fired after the client message stuff?
                x11::Event::UnmapNotify(_) => (), // todo!("Handle shutdown"),

                x11::Event::ClientMessage(event) => {
                    // Were we told to destroy the window?

                    // TODO: May be worth changing this to "close requested" and let the compositor impl choose what to do?
                    if event.data.as_data32()[0] == self.window.atoms.wm_delete_window
                        && event.window == self.window.inner
                    {
                        self.connection.unmap_window(self.window.inner)?;
                        return Err(X11Error::WindowDestroyed);
                    }
                }

                // TODO: Where are cursors handled?
                _ => (),
            }
        }

        Ok(())
    }
}
