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
use std::{cell::RefCell, rc::Rc};
use x11rb::protocol as x11;
use x11rb::protocol::xproto::Window;
use x11rb::{connection::Connection, xcb_ffi::XCBConnection};

#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    #[error("Connection to the X server failed")]
    ConnectionFailed,

    #[error("Connection to the X server was lost")]
    ConnectionLost,
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
}

/// Shared data between the X11 input and graphical backends.
#[derive(Debug)]
struct X11Backend {
    // The connection to the X server.
    connection: XCBConnection,
    /// The window used to display the compositor.
    window: Window,
}
