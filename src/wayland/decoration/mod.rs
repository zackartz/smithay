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

use wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_server::{Display, Global};

pub fn init_decoration_manager<L>(display: &mut Display, logger: L) -> Global<ZxdgDecorationManagerV1>
where
    L: Into<Option<::slog::Logger>>,
{
    todo!()
}
