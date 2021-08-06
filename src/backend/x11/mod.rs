//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor
//! directly as an X11 client.
//!

mod buffer;

use super::input::{
    ButtonState, Device, InputBackend, KeyState, KeyboardKeyEvent, MouseButton, PointerAxisEvent,
    PointerButtonEvent, PointerMotionAbsoluteEvent, UnusedEvent,
};
use crate::backend::input::InputEvent;
use crate::backend::input::{DeviceCapability, Event as BackendEvent};
use slog::Logger;
use std::{cell::RefCell, rc::Rc};
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError};
use x11rb::protocol as x11;
use x11rb::protocol::xproto::{AtomEnum, CreateWindowAux, EventMask, PropMode, Screen, Window, WindowClass};
use x11rb::rust_connection::ReplyError;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    #[error("Connection to the X server failed")]
    ConnectionFailed(ConnectError),

    #[error("An X error occurred during the connection")]
    ConnectionError(ConnectionError),

    #[error("An X protocol error packet was returned")]
    Protocol(x11rb::x11_utils::X11Error),
}

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

pub struct X11KeyboardInputEvent {
    time: u32,
    key: u32,
    count: u32,
    state: KeyState,
}

impl BackendEvent<X11InputBackend> for X11KeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl KeyboardKeyEvent<X11InputBackend> for X11KeyboardInputEvent {
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

pub struct X11MouseWheelEvent {
    time: u32,
}

impl BackendEvent<X11InputBackend> for X11MouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerAxisEvent<X11InputBackend> for X11MouseWheelEvent {
    fn amount(&self, axis: super::input::Axis) -> Option<f64> {
        todo!()
    }

    fn amount_discrete(&self, axis: super::input::Axis) -> Option<f64> {
        todo!()
    }

    fn source(&self) -> super::input::AxisSource {
        todo!()
    }
}

pub struct X11MouseInputEvent {
    time: u32,
    button: MouseButton,
    state: ButtonState,
}

impl BackendEvent<X11InputBackend> for X11MouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerButtonEvent<X11InputBackend> for X11MouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

pub struct X11MouseMovedEvent {
    time: u32,
}

impl BackendEvent<X11InputBackend> for X11MouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<X11InputBackend> for X11MouseMovedEvent {
    fn x(&self) -> f64 {
        todo!()
    }

    fn y(&self) -> f64 {
        todo!()
    }

    fn x_transformed(&self, width: i32) -> f64 {
        todo!()
    }

    fn y_transformed(&self, height: i32) -> f64 {
        todo!()
    }
}

#[derive(Debug)]
pub struct X11InputBackend {
    x11: Rc<RefCell<X11Backend>>,
    window: Window,
}

impl X11InputBackend {
    /// Returns the XID of the window this input backend is bound to.
    pub fn window(&self) -> u32 {
        self.window
    }
}

impl InputBackend for X11InputBackend {
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
        let x11 = self.x11.borrow();

        while let Some(event) = x11.connection.poll_for_event().expect("TODO: Error") {
            match event {
                x11::Event::Error(_) => todo!("Handle error"),
                x11::Event::ButtonPress(_) => todo!("Handle button press"),
                x11::Event::ButtonRelease(_) => todo!("Handle button release"),
                x11::Event::Expose(_) => todo!("Handle expose"),

                // TODO: Client message to handle WM_DELETE_WINDOW

                // TODO: Is it correct to directly cast the details of the event in? Or do we need to preprocess with xkbcommon
                x11::Event::KeyPress(event) => {
                    // Only handle key events if the event occurred in our own window.
                    if event.event == x11.window {
                        callback(InputEvent::Keyboard {
                            event: X11KeyboardInputEvent {
                                time: event.time,
                                key: event.detail as u32,
                                count: 1,
                                state: KeyState::Pressed,
                            },
                        })
                    }
                }

                x11::Event::KeyRelease(event) => {
                    // Only handle key events if the event occurred in our own window.
                    if event.event == x11.window {
                        callback(InputEvent::Keyboard {
                            event: X11KeyboardInputEvent {
                                time: event.time,
                                key: event.detail as u32,
                                count: 1,
                                state: KeyState::Released,
                            },
                        })
                    }
                }

                x11::Event::ResizeRequest(_) => todo!("Handle resize"),
                x11::Event::UnmapNotify(_) => todo!("Handle shutdown"),

                // TODO: Where are cursors handled?
                _ => (),
            }
        }

        todo!()
    }
}

#[derive(Debug)]
pub struct X11GraphicsBackend {
    x11: Rc<RefCell<X11Backend>>,
    window: Window,
}

impl X11GraphicsBackend {
    /// Returns the XID of the window this graphics backend presents to.
    pub fn window(&self) -> u32 {
        self.window
    }
}

/// Shared data between the X11 input and graphical backends.
#[derive(Debug)]
struct X11Backend {
    log: Logger,
    // The connection to the X server.
    connection: XCBConnection,
    /// The window used to display the compositor.
    window: Window,
}

impl X11Backend {
    fn new(log: Logger) -> Result<X11Backend, X11Error> {
        use x11rb::protocol::xproto::ConnectionExt;

        // Connect to the X server and reserve an ID for our window.
        let (connection, screen_number) = XCBConnection::connect(None)?;
        let window = connection.generate_id().expect("TODO: Error");
        let screen = &connection.setup().roots[screen_number];

        // Stagger intern requests and checking the reply in each cookie as not to block during each request.
        let wm_protocols = connection.intern_atom(false, b"WM_PROTOCOLS")?;
        let wm_delete_window = connection.intern_atom(false, b"WM_DELETE_WINDOW")?;
        let net_wm_name = connection.intern_atom(false, b"_NET_WM_NAME")?;
        let utf8_string = connection.intern_atom(false, b"UTF8_STRING")?;
        let wm_protocols = wm_protocols.reply().unwrap().atom;
        let wm_delete_window = wm_delete_window.reply().unwrap().atom;
        let net_wm_name = net_wm_name.reply().unwrap().atom;
        let utf8_string = utf8_string.reply().unwrap().atom;

        create_window(
            &connection,
            screen,
            window,
            1280,
            800,
            wm_protocols,
            wm_delete_window,
            net_wm_name,
            utf8_string,
        )?;

        Ok(X11Backend {
            log,
            connection,
            window,
        })
    }
}

fn create_window(
    connection: &XCBConnection,
    screen: &Screen,
    window: Window,
    height: u16,
    width: u16,
    wm_protocols: u32,
    wm_delete_window: u32,
    net_wm_name: u32,
    utf8_string: u32,
) -> Result<(), X11Error> {
    use x11rb::protocol::xproto::ConnectionExt;

    let window_aux = CreateWindowAux::new()
        .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::NO_EVENT)
        .background_pixel(screen.black_pixel);

    let cookie = connection.create_window(
        screen.root_depth,
        window,
        screen.root,
        0,
        0,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &window_aux,
    )?;

    let title = "Smithay";

    // _NET_WM_NAME should be preferred by window managers, but set both in case.
    connection.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )?;

    connection.change_property8(
        PropMode::REPLACE,
        window,
        net_wm_name,
        utf8_string,
        title.as_bytes(),
    )?;

    // Enable WM_DELETE_WINDOW so our client is not disconnected upon our toplevel window being destroyed.
    connection.change_property32(
        PropMode::REPLACE,
        window,
        wm_protocols,
        AtomEnum::ATOM,
        &[wm_delete_window],
    )?;

    // WM_CLASS is in the format of `instance\0class\0`
    let mut class = Vec::new();
    class.extend_from_slice(title.as_bytes());
    class.extend_from_slice(b"\0wayland_compositor\0");

    connection.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        &class[..],
    )?;

    cookie.check()?;

    Ok(())
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

impl From<x11rb::x11_utils::X11Error> for X11Error {
    fn from(e: x11rb::x11_utils::X11Error) -> Self {
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
