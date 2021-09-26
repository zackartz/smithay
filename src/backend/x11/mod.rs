//! Implementation of the backend types using X11.
//!
//! This backend provides the appropriate backend implementations to run a Wayland compositor as an
//! X11 client.
//!
//! The backend is initialized using [X11Backend::new](self::X11Backend::new). The function will
//! return two objects:
//!
//! - an [`X11Backend`], which you will insert into an [EventLoop](calloop::EventLoop) to process input events.
//! - an [`X11Surface`], which represents a surface that buffers are presented to for display.
//!

/*
A note from i509 for future maintainers and contributors:

Grab yourself the nearest copy of the ICCCM.

You should follow this document religiously, or else you will easily get shot in the foot when doing anything window related.
Specifically look out for "Section 4: Client to Window Manager Communication"

A link to the ICCCM Section 4: https://tronche.com/gui/x/icccm/sec-4.html

Useful reading:

DRI3 protocol documentation: https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/dri3proto.txt
*/

mod buffer;
mod drm;
mod error;
mod input;
mod window_inner;

use self::{
    buffer::{present, PixmapWrapperExt},
    window_inner::WindowInner,
};
use super::{
    allocator::dmabuf::{AsDmabuf, Dmabuf},
    input::{Axis, ButtonState, KeyState, MouseButton},
};
use crate::{
    backend::{
        input::InputEvent,
        x11::drm::{get_drm_node_type_from_fd, DRM_NODE_RENDER},
    },
    utils::{x11rb::X11Source, Logical, Size},
};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use drm_fourcc::DrmFourcc;
use gbm::BufferObjectFlags;
use nix::fcntl;
use slog::{error, info, o, Logger};
use std::{
    io, mem,
    os::unix::prelude::{AsRawFd, RawFd},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Weak,
    },
};
use x11rb::{
    atom_manager,
    connection::{Connection, RequestConnection},
    protocol::{
        self as x11,
        dri3::{self, ConnectionExt},
        xfixes::{self, ConnectionExt as _},
        xproto::{ColormapAlloc, ConnectionExt as _, Depth, PixmapWrapper, VisualClass},
    },
    rust_connection::RustConnection,
};

pub use self::error::*;
pub use self::input::*;

/// Properties defining initial information about the window created by the X11 backend.
#[derive(Debug, Clone, Copy)]
#[allow(missing_docs)] // Self explanatory fields
pub struct WindowProperties<'a> {
    pub size: Size<u16, Logical>,
    pub title: &'a str,
}

impl Default for WindowProperties<'_> {
    fn default() -> Self {
        WindowProperties {
            size: (1280, 800).into(),
            title: "Smithay",
        }
    }
}

/// An event emitted by the X11 backend.
#[derive(Debug)]
pub enum X11Event {
    /// The X server has required the compositor to redraw the window.
    Refresh,

    /// An input event occurred.
    Input(InputEvent<X11Input>),

    /// The window was resized.
    Resized(Size<u16, Logical>),

    /// The window has received a request to be closed.
    CloseRequested,
}

/// Represents an active connection to the X to manage events on the Window provided by the backend.
#[derive(Debug)]
pub struct X11Backend {
    log: Logger,
    connection: Arc<RustConnection>,
    source: X11Source,
    screen_number: usize,
    window: Arc<WindowInner>,
    resize: Sender<Size<u16, Logical>>,
    key_counter: Arc<AtomicU32>,
    depth: Depth,
    visual_id: u32,
}

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
    pub fn new<L>(properties: WindowProperties<'_>, logger: L) -> Result<(X11Backend, X11Surface), X11Error>
    where
        L: Into<Option<slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));

        info!(logger, "Connecting to the X server");

        let (connection, screen_number) = RustConnection::connect(None)?;
        let connection = Arc::new(connection);
        info!(logger, "Connected to screen {}", screen_number);

        if connection
            .extension_information(xfixes::X11_EXTENSION_NAME)?
            .is_none()
        {
            // TODO: Emit error in log
            return Err(MissingExtensionError::NotFound {
                name: xfixes::X11_EXTENSION_NAME,
                major: 4,
                minor: 0,
            }
            .into());
        }

        let xfixes_extension = connection.xfixes_query_version(4, 0)?.reply()?;

        if xfixes_extension.major_version < 4 {
            // TODO: Emit error in log
            return Err(MissingExtensionError::WrongVersion {
                name: xfixes::X11_EXTENSION_NAME,
                required_major: 4,
                required_minor: 0,
                available_major: xfixes_extension.major_version,
                available_minor: xfixes_extension.minor_version,
            }
            .into());
        }

        let screen = &connection.setup().roots[screen_number];

        // We want 32 bit color
        let depth = screen
            .allowed_depths
            .iter()
            .find(|depth| depth.depth == 32)
            .cloned()
            .ok_or(CreateWindowError::NoDepth)?;

        // Next find a visual using the supported depth
        let visual_id = depth
            .visuals
            .iter()
            .find(|visual| visual.class == VisualClass::TRUE_COLOR)
            .ok_or(CreateWindowError::NoVisual)?
            .visual_id;

        // Find a supported format.
        // TODO

        // Make a colormap
        let colormap = connection.generate_id()?;
        connection.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)?;

        let atoms = Atoms::new(&*connection)?.reply()?;

        let window = Arc::new(WindowInner::new(
            Arc::downgrade(&connection),
            screen,
            properties,
            atoms,
            depth.clone(),
            visual_id,
            colormap,
        )?);

        let source = X11Source::new(
            connection.clone(),
            window.id,
            atoms._SMITHAY_X11_BACKEND_CLOSE,
            logger.clone(),
        );

        info!(logger, "Window created");

        let (resize_send, resize_recv) = mpsc::channel();

        let backend = X11Backend {
            log: logger,
            source,
            connection,
            window,
            key_counter: Arc::new(AtomicU32::new(0)),
            depth,
            visual_id,
            screen_number,
            resize: resize_send,
        };

        let surface = X11Surface::new(&backend, resize_recv)?;

        Ok((backend, surface))
    }

    /// Returns the default screen number of the X server.
    pub fn screen(&self) -> usize {
        self.screen_number
    }

    /// Returns the underlying connection to the X server.
    pub fn connection(&self) -> &RustConnection {
        &*self.connection
    }

    /// Returns a handle to the X11 window this input backend handles inputs for.
    pub fn window(&self) -> Window {
        self.window.clone().into()
    }
}

/// An X11 surface which uses GBM to allocate and present buffers.
#[derive(Debug)]
pub struct X11Surface {
    connection: Weak<RustConnection>,
    window: Window,
    resize: Receiver<Size<u16, Logical>>,
    device: gbm::Device<RawFd>,
    width: u16,
    height: u16,
    current: Dmabuf,
    next: Dmabuf,
}

impl X11Surface {
    fn new(backend: &X11Backend, resize: Receiver<Size<u16, Logical>>) -> Result<X11Surface, X11Error> {
        let connection = &backend.connection;
        let window = backend.window();

        if connection
            .extension_information(dri3::X11_EXTENSION_NAME)?
            .is_none()
        {
            // TODO: Emit error in log
            return Err(MissingExtensionError::NotFound {
                name: dri3::X11_EXTENSION_NAME,
                major: 1,
                minor: 2,
            }
            .into());
        }

        // Does the X server support dri3?
        let (dri3_major, dri3_minor) = {
            // DRI3 will only return the highest version we request.
            // TODO: We might need to request a higher version?
            let version = connection.dri3_query_version(1, 2)?.reply()?;

            if version.minor_version < 2 {
                // TODO: Emit error in log
                return Err(MissingExtensionError::WrongVersion {
                    name: dri3::X11_EXTENSION_NAME,
                    required_major: 1,
                    required_minor: 2,
                    available_major: version.major_version,
                    available_minor: version.minor_version,
                }
                .into());
            }

            (version.major_version, version.minor_version)
        };

        // Determine which drm-device the Display is using.
        let screen = &connection.setup().roots[backend.screen()];
        let dri3 = connection.dri3_open(screen.root, 0)?.reply()?;

        let drm_device_fd = dri3.device_fd;
        // Duplicate the drm_device_fd
        let drm_device_fd: RawFd = fcntl::fcntl(
            drm_device_fd.as_raw_fd(),
            fcntl::FcntlArg::F_DUPFD_CLOEXEC(3), // Set to 3 so the fd cannot become stdin, stdout or stderr
        )
        .map_err(AllocateBuffersError::from)?;

        let fd_flags =
            nix::fcntl::fcntl(drm_device_fd.as_raw_fd(), nix::fcntl::F_GETFD).expect("Handle this error");
        // No need to check if ret == 1 since nix handles that.

        // Enable the close-on-exec flag.
        nix::fcntl::fcntl(
            drm_device_fd.as_raw_fd(),
            nix::fcntl::F_SETFD(
                nix::fcntl::FdFlag::from_bits_truncate(fd_flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
            ),
        )
        .map_err(AllocateBuffersError::from)?;

        if get_drm_node_type_from_fd(drm_device_fd.as_raw_fd())? != DRM_NODE_RENDER {
            todo!("Attempt to get the render device by name for the DRM node that isn't a render node")
        }

        // Finally create a GBMDevice to manage the buffers.
        let device = gbm::Device::new(drm_device_fd.as_raw_fd()).expect("Failed to create gbm device");

        let size = backend.window().size();
        // TODO: Dont hardcode format.
        let current = device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .map_err(Into::<AllocateBuffersError>::into)?
            .export()
            .map_err(Into::<AllocateBuffersError>::into)?;

        let next = device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )
            .map_err(Into::<AllocateBuffersError>::into)?
            .export()
            .map_err(Into::<AllocateBuffersError>::into)?;

        Ok(X11Surface {
            connection: Arc::downgrade(connection),
            window,
            device,
            width: size.w,
            height: size.h,
            current,
            next,
            resize,
        })
    }

    /// Returns a handle to the GBM device used to allocate buffers.
    pub fn device(&self) -> gbm::Device<RawFd> {
        self.device.clone()
    }

    /// Returns an RAII scoped object which provides the next buffer.
    ///
    /// When the object is dropped, the contents of the buffer are swapped and then presented.
    // TODO: Error type
    pub fn present(&mut self) -> Result<Present<'_>, AllocateBuffersError> {
        if let Some(new_size) = self.resize.try_iter().last() {
            self.resize(new_size)?;
        }

        Ok(Present { surface: self })
    }

    // TODO: Error type.
    fn resize(&mut self, size: Size<u16, Logical>) -> Result<(), AllocateBuffersError> {
        self.width = size.w;
        self.height = size.h;

        // Create new buffers
        let current = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )?
            .export()?;

        let next = self
            .device
            .create_buffer_object::<()>(
                size.w as u32,
                size.h as u32,
                DrmFourcc::Argb8888,
                BufferObjectFlags::empty(),
            )?
            .export()?;

        self.current = current;
        self.next = next;

        Ok(())
    }
}

/// An RAII scope containing the next buffer that will be presented to the window. Presentation
/// occurs when the `Present` is dropped.
///
/// The provided buffer may be bound to a [Renderer](crate::backend::renderer::Renderer) to draw to
/// the window.
///
/// ```rust,no_run
/// # use crate::backend::renderer::gles2::Gles2Renderer;
/// # let mut renderer: Gles2Renderer = ...;
/// # let mut surface: X11Surface = ...;
/// let present = surface.present()?;
///
/// // Bind the buffer to the renderer in order to render.
/// renderer.bind(present.buffer())?;
///
/// // Rendering here!
///
/// // Make sure to unbind the buffer when done.
/// renderer.unbind()?;
///
/// // When the `present` is dropped, whatever was rendered will be shown in the window.
/// ```
#[derive(Debug)]
pub struct Present<'a> {
    surface: &'a mut X11Surface,
}

impl Present<'_> {
    /// Returns the next buffer that will be presented to the Window.
    ///
    /// You may bind this buffer to a renderer to render.
    pub fn buffer(&self) -> Dmabuf {
        self.surface.next.clone()
    }
}

impl Drop for Present<'_> {
    fn drop(&mut self) {
        let surface = &mut self.surface;

        if let Some(connection) = surface.connection.upgrade() {
            // Swap the buffers
            mem::swap(&mut surface.next, &mut surface.current);

            if let Ok(pixmap) = PixmapWrapper::with_dmabuf(&*connection, &surface.window, &surface.current) {
                // Now present the current buffer
                let _ = present(
                    &*connection,
                    &pixmap,
                    &surface.window,
                    surface.width,
                    surface.height,
                );
            }
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

    /// Maps the window, making it visible.
    pub fn map(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.map();
        }
    }

    /// Unmaps the window, making it invisible.
    pub fn unmap(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.unmap();
        }
    }

    /// Returns the size of this window.
    ///
    /// If the window has been destroyed, the size is `0 x 0`.
    pub fn size(&self) -> Size<u16, Logical> {
        self.0
            .upgrade()
            .map(|inner| inner.size())
            .unwrap_or_else(|| (0, 0).into())
    }

    /// Changes the visibility of the cursor within the confines of the window.
    ///
    /// If `false`, this will hide the cursor. If `true`, this will show the cursor.
    pub fn set_cursor_visible(&self, visible: bool) {
        if let Some(inner) = self.0.upgrade() {
            inner.set_cursor_visible(visible);
        }
    }

    /// Returns the XID of the window.
    pub fn id(&self) -> u32 {
        self.0.upgrade().map(|inner| inner.id).unwrap_or(0)
    }

    /// Returns the depth id of this window.
    pub fn depth(&self) -> u8 {
        self.0.upgrade().map(|inner| inner.depth.depth).unwrap_or(0)
    }

    /// Returns the graphics context used to draw to this window.
    pub fn gc(&self) -> u32 {
        self.0.upgrade().map(|inner| inner.gc).unwrap_or(0)
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.upgrade(), other.0.upgrade()) {
            (Some(self_), Some(other)) => self_ == other,
            _ => false,
        }
    }
}

impl EventSource for X11Backend {
    type Event = X11Event;

    /// The window the incoming events are applicable to.
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
        let mut event_window = window.clone().into();
        let resize = &self.resize;

        self.source.process_events(readiness, token, |event, _| {
            match event {
                x11::Event::ButtonPress(button_press) => {
                    if button_press.event == window.id {
                        // X11 decided to associate scroll wheel with a button, 4, 5, 6 and 7 for
                        // up, down, right and left. For scrolling, a press event is emitted and a
                        // release is them immediately followed for scrolling. This means we can
                        // ignore release for scrolling.

                        // Ideally we would use `ButtonIndex` from XCB, however it does not cover 6 and 7
                        // for horizontal scroll and does not work nicely in match statements, so we
                        // use magic constants here:
                        //
                        // 1 => MouseButton::Left
                        // 2 => MouseButton::Middle
                        // 3 => MouseButton::Right
                        // 4 => Axis::Vertical +1.0
                        // 5 => Axis::Vertical -1.0
                        // 6 => Axis::Horizontal -1.0
                        // 7 => Axis::Horizontal +1.0
                        // Others => ??
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

                            // Unknown mouse button
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
                    if button_release.event == window.id {
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
                    if key_press.event == window.id {
                        callback(
                            Input(InputEvent::Keyboard {
                                event: X11KeyboardInputEvent {
                                    time: key_press.time,
                                    // X11's keycodes are +8 relative to the libinput keycodes
                                    // that are expected, so subtract 8 from each keycode to
                                    // match libinput.
                                    //
                                    // https://github.com/freedesktop/xorg-xf86-input-libinput/blob/master/src/xf86libinput.c#L54
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
                    if key_release.event == window.id {
                        // atomic u32 has no checked_sub, so load and store to do the same.
                        let mut key_counter_val = key_counter.load(Ordering::SeqCst);
                        key_counter_val = key_counter_val.saturating_sub(1);
                        key_counter.store(key_counter_val, Ordering::SeqCst);

                        callback(
                            Input(InputEvent::Keyboard {
                                event: X11KeyboardInputEvent {
                                    time: key_release.time,
                                    // X11's keycodes are +8 relative to the libinput keycodes
                                    // that are expected, so subtract 8 from each keycode to
                                    // match libinput.
                                    //
                                    // https://github.com/freedesktop/xorg-xf86-input-libinput/blob/master/src/xf86libinput.c#L54
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
                    if motion_notify.event == window.id {
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

                x11::Event::ConfigureNotify(configure_notify) => {
                    if configure_notify.window == window.id {
                        let previous_size = { *window.size.lock().unwrap() };

                        // Did the size of the window change?
                        let configure_notify_size: Size<u16, Logical> =
                            (configure_notify.width, configure_notify.height).into();

                        if configure_notify_size != previous_size {
                            // Intentionally drop the lock on the size mutex incase a user
                            // requests a resize or does something which causes a resize
                            // inside the callback.
                            {
                                *window.size.lock().unwrap() = configure_notify_size;
                            }

                            (callback)(X11Event::Resized(configure_notify_size), &mut event_window);
                            let _ = resize.send(configure_notify_size);
                        }
                    }
                }

                x11::Event::ClientMessage(client_message) => {
                    if client_message.data.as_data32()[0] == window.atoms.WM_DELETE_WINDOW // Destroy the window?
                            && client_message.window == window.id
                    // Same window
                    {
                        (callback)(X11Event::CloseRequested, &mut event_window);
                    }
                }

                x11::Event::Expose(expose) => {
                    // TODO: Use details of expose event to tell the the compositor what has been damaged?
                    if expose.window == window.id && expose.count == 0 {
                        (callback)(X11Event::Refresh, &mut event_window);
                    }
                }

                x11::Event::Error(e) => {
                    error!(log, "X11 error: {:?}", e);
                }

                _ => (),
            }

            // Flush the connection so changes to the window state during callbacks can be emitted.
            let _ = connection.flush();
        })
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
