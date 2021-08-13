//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor
//! directly as an X11 client.
//!

mod buffer;
mod event_source;
mod window;

use self::window::{Atoms, WindowInner};
use super::input::{
    Axis, AxisSource, ButtonState, Device, InputBackend, KeyState, KeyboardKeyEvent, MouseButton,
    PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, UnusedEvent,
};
use crate::backend::input::InputEvent;
use crate::backend::input::{DeviceCapability, Event as BackendEvent};
use crate::backend::x11::event_source::X11Source;
use crate::utils::{Logical, Size};
use calloop::LoopHandle;
use slog::{info, o, Logger};
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol as x11;
use x11rb::rust_connection::{ReplyOrIdError, RustConnection};
use x11rb::x11_utils::X11Error as ImplError;

/// An error emitted by the X11 backend.
#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    /// Connecting to the X server failed.
    #[error("Connecting to the X server failed")]
    ConnectionFailed(ConnectError),

    /// An error occurred with the connection to the X server.
    #[error("An error occurred with the connection to the X server.")]
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

impl From<ReplyOrIdError> for X11Error {
    fn from(_: ReplyOrIdError) -> Self {
        todo!()
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
    pub fn set_title(&self, title: &str) {
        if let Some(inner) = self.0.upgrade() {
            inner.set_title(title);
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

/// An event emitted by the X11 backend.
#[derive(Debug)]
pub enum X11Event {
    /// The window has received an Expose request, indicating the window is ready to accept new
    /// content to present.
    Expose,

    /// The window was resized.
    Resized(Size<u16, Logical>),

    /// The window was requested to be closed.
    CloseRequested,
}

/// An abstraction representing a connection to the X11 server.
#[derive(Debug)]
pub struct X11Backend {
    log: Logger,
    connection: Rc<RustConnection>,
    window: Rc<WindowInner>,
    queued_input_events: Rc<RefCell<Vec<x11::Event>>>,
}

impl X11Backend {
    /// Initializes the X11 backend, connecting to the X server and creating the window the compositor may output to.
    pub fn init<Data, F, L>(
        loop_handle: LoopHandle<'static, Data>,
        properties: WindowProperties<'_>,
        callback: F,
        logger: L,
    ) -> Result<X11Backend, X11Error>
    where
        L: Into<Option<slog::Logger>>,
        F: FnMut(Window, X11Event, &mut Data) + 'static,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));
        let callback = Rc::new(RefCell::new(callback));

        info!(log, "Connecting to the X server");
        let (connection, screen_number) = RustConnection::connect(None)?;
        let connection = Rc::new(connection);
        info!(log, "Connected to screen {}", screen_number);

        let screen = &connection.setup().roots[screen_number];
        let atoms = Atoms::new(connection.clone())?;
        let window = Rc::new(WindowInner::new(connection.clone(), screen, atoms, properties)?);
        let event_source = X11Source::new(connection.clone());
        let queued_input_events = Rc::new(RefCell::new(vec![]));

        // Clones to move into the source's callback
        let callback_connection = connection.clone();
        let callback_window = window.clone();
        let callback_callback = callback.clone();
        let callback_queued_input_events = queued_input_events.clone();

        loop_handle
            .insert_source(event_source, move |events, _, data| {
                let connection = callback_connection.clone();
                let window = callback_window.clone();
                let queued_input_events = callback_queued_input_events.clone();
                let callback = callback_callback.clone();

                for event in events {
                    match event {
                        // Input events need to be queued up:
                        x11::Event::ButtonPress(button_press) => {
                            if button_press.event == window.inner {
                                queued_input_events.borrow_mut().push(event);
                            }
                        }

                        x11::Event::ButtonRelease(button_release) => {
                            if button_release.event == window.inner {
                                queued_input_events.borrow_mut().push(event);
                            }
                        }

                        // TODO: Is it correct to directly cast the details of the event in? Or do we need to preprocess with xkbcommon
                        x11::Event::KeyPress(key_press) => {
                            if key_press.event == window.inner {
                                queued_input_events.borrow_mut().push(event);
                            }
                        }

                        x11::Event::KeyRelease(key_release) => {
                            if key_release.event == window.inner {
                                queued_input_events.borrow_mut().push(event);
                            }
                        }

                        x11::Event::MotionNotify(motion_notify) => {
                            if motion_notify.event == window.inner {
                                queued_input_events.borrow_mut().push(event);
                            }
                        }

                        // Events for immediate execution.
                        x11::Event::ResizeRequest(resized) => {
                            if resized.window == window.inner {
                                let size = (resized.width, resized.height).into();

                                (&mut callback.borrow_mut())(
                                    Window(Rc::downgrade(&window)),
                                    X11Event::Resized(size),
                                    data,
                                );
                            }
                        }

                        x11::Event::ClientMessage(client_message) => {
                            // Were we told to destroy the window?
                            if client_message.data.as_data32()[0] == window.atoms.wm_delete_window
                                && client_message.window == window.inner
                            {
                                (&mut callback.borrow_mut())(
                                    Window(Rc::downgrade(&window)),
                                    X11Event::CloseRequested,
                                    data,
                                );
                            }
                        }

                        x11::Event::Expose(expose) => {
                            if expose.window == window.inner {
                                (&mut callback.borrow_mut())(
                                    Window(Rc::downgrade(&window)),
                                    X11Event::Expose,
                                    data,
                                );
                            }
                        }

                        // TODO: What to do with errors?
                        x11::Event::Error(_) => (),

                        _ => (),
                    }
                }

                // Now flush requests to the clients.
                Ok(connection.flush()?)
            })
            .expect("TODO");

        info!(log, "Window created");

        Ok(X11Backend {
            log,
            connection,
            window,
            queued_input_events,
        })
    }

    /// Returns a handle to the X11 window this input backend handles inputs for.
    pub fn window(&self) -> Window {
        Window(Rc::downgrade(&self.window))
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
        let mut queued_events = self.queued_input_events.borrow_mut();

        queued_events.drain(..).for_each(|event| {
            match event {
                x11::Event::ButtonPress(event) => {
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

                                        _ => unreachable!(),
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

                x11::Event::ButtonRelease(event) => {
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

                // TODO: Is it correct to directly cast the details of the event in? Or do we need to preprocess with xkbcommon
                x11::Event::KeyPress(event) => {
                    callback(InputEvent::Keyboard {
                        event: X11KeyboardInputEvent {
                            time: event.time,
                            key: event.detail as u32,
                            count: 1, // TODO: Counter
                            state: KeyState::Pressed,
                        },
                    })
                }

                x11::Event::KeyRelease(event) => {
                    callback(InputEvent::Keyboard {
                        event: X11KeyboardInputEvent {
                            time: event.time,
                            key: event.detail as u32,
                            count: 1, // TODO: Counter
                            state: KeyState::Released,
                        },
                    })
                }

                x11::Event::MotionNotify(event) => {
                    // Only handle key events if the event occurred in our own window.
                    if event.event == self.window.inner {
                        // TODO: Fill in extra details.
                        let _ = event.event_x;
                        let _ = event.event_y;

                        callback(InputEvent::PointerMotionAbsolute {
                            event: X11MouseMovedEvent { time: event.time },
                        })

                        // Use event_x/y since those are relative the the window receiving events.
                    }
                }

                _ => (),
            }
        });

        Ok(())
    }
}
