//! XDG Decoration protocol.
//!
//! This module provides a compositor with the ability to announce support for server-side
//! decorations.
//!
//! This module allows the client to negotiate how decorations are drawn and the compositor to
//! dictate to the client whether to use the client's decorations or the server's decorations.
//!
//! For clients which do not support this protocol or do not wish to use server side decorations,
//! these clients will continue to self-decorate.
//!
//! ## Supported surfaces
//!
//! Note this protocol is only supported on XDG toplevel surfaces.

use crate::wayland::shell::xdg::ToplevelSurface;
use slog::o;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use wayland_commons::filter::Filter;
use wayland_protocols::unstable::xdg_decoration::v1::server::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, zxdg_toplevel_decoration_v1::Mode,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
};
use wayland_server::{Display, Global, Main};

/// Tracks the current state of decoration modes.
#[derive(Debug)]
pub struct DecorationManager {
    decorations: Vec<ToplevelDecoration>,
}

impl DecorationManager {
    /// Returns all decoration settings for toplevel surfaces which understand this protocol.
    pub fn decorations(&self) -> &[ToplevelDecoration] {
        &self.decorations[..]
    }

    /// Returns the decoration state for a toplevel surface.
    ///
    /// Returns none if the client providing the surface does not declare the ability to understand
    /// the decoration protocol.
    ///
    /// This is only supported if the toplevel surface is provided by the stable XDG protocol.
    /// Otherwise `None` is returned for the incompatible ZXDG V6 toplevel surface.
    pub fn get_decoration(&self, toplevel: &ToplevelSurface) -> Option<&ToplevelDecoration> {
        self.decorations
            .iter()
            .find(|decoration| decoration.surface() == toplevel)
    }
}

/// A decoration object for a toplevel surface.
#[derive(Debug, Clone)]
pub struct ToplevelDecoration {
    inner: ZxdgToplevelDecorationV1,
    surface: ToplevelSurface,
    mode: Cell<DecorationMode>,
}

impl ToplevelDecoration {
    /// Returns the surface this decoration state belongs to.
    pub fn surface(&self) -> &ToplevelSurface {
        &self.surface
    }

    /// Returns the current decoration mode.
    pub fn mode(&self) -> DecorationMode {
        self.mode.get()
    }

    /// Asks the client to change the decoration mode of the toplevel surface.
    pub fn set_mode(&self, mode: DecorationMode) {
        self.mode.set(mode);
        self.inner.configure(mode.into());
    }
}

/// The decoration mode of a surface.
#[derive(Debug, Copy, Clone)]
pub enum DecorationMode {
    /// Decorations should be drawn by the client.
    ClientSide,

    /// Decorations should be drawn by the server.
    ServerSide,
}

impl From<DecorationMode> for Mode {
    fn from(mode: DecorationMode) -> Self {
        match mode {
            DecorationMode::ClientSide => Mode::ClientSide,
            DecorationMode::ServerSide => Mode::ServerSide,
        }
    }
}

impl From<Mode> for DecorationMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::ClientSide => DecorationMode::ClientSide,
            Mode::ServerSide => DecorationMode::ServerSide,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
/// A request a client may send regarding the current state of decorations.
pub enum DecorationRequest {
    /// The client has requested that the decoration mode of the surface should be changed.
    ///
    /// The compositor should respond to the surface with an ack-configure event.
    SetMode {
        decoration: ToplevelDecoration,
        /// The new decoration mode.
        mode: DecorationMode,
    },

    /// The client has indicated it does not prefer a particular decoration mode.
    ///
    /// The compositor should respond to the surface with an ack-configure event.
    UnsetMode { decoration: ToplevelDecoration },

    /// The client has switched back to a mode with no server side decorations for the next commit.
    Destroy { decoration: ToplevelDecoration },
}

/// Creates a new `zxdg_decoration_manager_v1` global.
pub fn init_decoration_manager<Impl, L>(
    display: &mut Display,
    implementation: Impl,
    logger: L,
) -> (Arc<Mutex<DecorationManager>>, Global<ZxdgDecorationManagerV1>)
where
    Impl: FnMut(ToplevelSurface, DecorationMode),
    L: Into<Option<::slog::Logger>>,
{
    let implementation = Rc::new(RefCell::new(implementation));

    let _log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "zxdg_decoration_manager"));
    let manager = Arc::new(Mutex::new(DecorationManager { decorations: vec![] }));

    let global = display.create_global(
        1,
        Filter::new(
            move |(manager, _version): (Main<ZxdgDecorationManagerV1>, _), _, _| {
                manager.quick_assign(|manager, request, _| {
                    //
                    //
                })
            },
        ),
    );

    (manager, global)
}
