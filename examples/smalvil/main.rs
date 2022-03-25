//! Smalvil, the small Wayland compositor.
//!
//! Smalvil is a functional starting point from which larger compositors may be built. Smalvil prioritizes the
//! following:
//! - Small and easy to reason with.
//! - Implements the core Wayland protocols and the XDG shell, meaning Smalvil works with the vast majority of
//! clients.
//! - Follows recommended usage patterns for Smithay.
//!
//! Smalvil being an example compositor does not prioritize the following:
//! - Aesthetics
//! - Configuration
//! - Plentiful features
//!
//! Smalvil is only designed to be run inside an existing session (inside a window) for the sake of
//! simplicity.
//!
//! Smalvil is obviously not complete, there are some future changes that will be made such as:
//! - Fully migrate to using the EventLoop to drive the compositor.
//! - Migrate from winit to a common windowing backend that is easier to integrate with calloop (abstraction
//!   over X11 and Wayland).

use std::time::Duration;

use calloop::EventLoop;
use smithay::{
    backend::renderer::{Frame, Renderer},
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_output,
    delegate_xdg_shell,
    utils::{Rectangle, Transform},
    wayland::{
        compositor::{CompositorHandler, CompositorState},
        output::{Output, OutputManagerState, PhysicalProperties},
        seat::{KeyboardHandle, PointerHandle, Seat, SeatHandler, SeatState, XkbConfig},
        shell::xdg::{XdgRequest, XdgShellHandler, XdgShellState},
        shm::ShmState,
    },
};
use wayland_server::{
    protocol::{wl_output::Subpixel, wl_surface},
    socket::ListeningSocket,
    Display, DisplayHandle,
};

/// The main function.
///
/// TODO: High level overview.
fn main() {
    // A compositor is a process which needs to manage events from multiple sources. This includes clients,
    // display hardware, input hardware, etc. A compositor is generally spending most of it's time waiting for
    // some events.
    //
    // Smithay is primarily designed around "calloop". Of course this isn't a calloop tutorial, but we will
    // discuss some parts of calloop as necessary. Information about calloop can be found here:
    // https://github.com/Smithay/calloop
    //
    // Calloop provides an event loop which the compositor may register sources using. A callback is invoked
    // when an event arrives from a source.
    let event_loop = EventLoop::<CalloopData>::try_new().expect("Failed to create event loop");

    // Let's prepare for the compositor setup by creating the necessary output backends.
    //
    // In the Smalvil, we use the winit for the windowing and input backend.
    // TODO: When ready, migrate to a common windowing backend over x11 and wayland.
    let (mut graphics_backend, mut winit_event_loop) =
        smithay::backend::winit::init(None).expect("could not initialize winit");

    // TODO: winit setup

    // The first step of the compositor setup is creating the display.
    //
    // The display handles queuing and dispatch of events from clients.
    let mut display = Display::<Smalvil>::new().expect("failed to create display");

    // Next the globals for the core wayland protocol and the xdg shell are created. In particular, this
    // creates instances of "delegate type"s which are responsible for processing some group of Wayland
    // protocols.
    //
    // Smalvil needs to initialize delegate types to handle the core Wayland protocols and the XDG Shell
    // protocol.

    let protocols = ProtocolStates {
        // Delegate type for the compositor
        compositor_state: CompositorState::new(&mut display, None),
        // Delegate type for the xdg shell.
        //
        // The xdg shell is the primary windowing shell used in the Wayland ecosystem.
        xdg_shell_state: XdgShellState::new(&mut display, None).0, // TODO: Make GlobalId a member of XdgShellState.
        shm_state: ShmState::new(&mut display, Vec::new(), None),
        _output_manager: OutputManagerState::new(),
        seat_state: SeatState::new(),
    };

    let mut seat = Seat::new(&mut display, "smalvil", None);
    let keyboard = seat
        .add_keyboard(
            &mut display.handle(),
            XkbConfig::default(),
            50,
            300,
            |_seat, _surface| (),
        )
        .expect("could not add keyboard");
    let pointer = seat.add_pointer(&mut display.handle(), |_cursor_image| ());

    // TODO: GlobalId behavior is not nice
    // TODO: Take impl Into<String> for output name.
    let output = Output::new(
        &mut display,
        "smalvil".into(),
        PhysicalProperties {
            size: (0, 0).into(), // TODO: Size needs to be initialized to something proper.
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Smalvil".into(),
        },
        None,
    )
    .0;

    let smalvil = Smalvil {
        protocols,
        seat,
        keyboard,
        pointer,
        output,
        running: true,
    };

    // TODO: Socket setup.
    let listening_socket =
        ListeningSocket::bind_auto("wayland", 1..32).expect("failed to find free socket name");

    // TODO: Run loop
    let mut calloop_data = CalloopData { smalvil, display };

    // And here is the run loop.
    //
    // TODO: Some changes to be made:
    // - Move to driving the compositor with the event loop when winit has a new release.
    while calloop_data.smalvil.running {
        // Dispatch windowing events
        if winit_event_loop
            .dispatch_new_events(|event| {
                // TODO: Input event
            })
            .is_err()
        {
            calloop_data.smalvil.running = false;
            break;
        }

        // Rendering
        graphics_backend
            .bind()
            .expect("could not bind winit graphics backend");

        let size = graphics_backend.window_size().physical_size;
        // For now hardcode the damage box to cover the entire window.
        let damage = Rectangle::from_loc_and_size((0, 0), size);

        graphics_backend
            .renderer()
            // TODO: Transform is not required?
            .render(size, Transform::Flipped180, |renderer, frame| {
                frame.clear([0.1, 0.0, 0.0, 1.0], &[damage])
            })
            // TODO: This is kinda ugly?
            .expect("rendering error")
            .expect("rendering error");

        // Accept new clients
        // TODO: Use ListeningSocketSource (see pull request)
        while let Some(_stream) = listening_socket.accept().expect("socket") {
            // TODO: better client infrastructure.
        }

        // TODO: Source to listen to all registered clients, dispatch their events and flush clients.
        calloop_data
            .display
            .dispatch_clients(&mut calloop_data.smalvil)
            .expect("dispatch clients");
        calloop_data.display.flush_clients().expect("flush clients");

        // Finally submit the buffer to winit to ensure the next round of events is received by winit.
        graphics_backend
            .submit(Some(&[damage.to_logical(1.0)]), 1.0)
            .expect("swap buffers failed");
    }
}

/// The primary compositor state data type.
///
/// This struct contains all the moving parts of the compositor and other data you need to track. This data
/// type is passed around to most parts of the compositor, meaning this is a reliable place to store data you
/// may need to access later.
pub struct Smalvil {
    protocols: ProtocolStates,

    seat: Seat<Self>,
    keyboard: KeyboardHandle,
    pointer: PointerHandle<Self>,
    output: Output,

    /// Whether the compositor event loop should continue.
    running: bool,
}

/// All the protocol delegate types Smalvil uses.
pub struct ProtocolStates {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _output_manager: OutputManagerState,
    seat_state: SeatState<Smalvil>,
}

/// Data passed to the event loop.
pub struct CalloopData {
    smalvil: Smalvil,
    display: Display<Smalvil>,
}

/*
  Trait implementations and delegate macros
*/

// In order to use the delegate types we have defined in the `Smalvil` type and created in our main function,
// we need to implement some traits and use some macros.
//
// The trait bounds on `D` required by `CompositorState::new` indicate that the Smalvil type needs to
// implement the `CompositorHandler` trait.
impl CompositorHandler for Smalvil {
    // Many wayland frontend abstractions require a way to get the delegate type from your data type.
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor_state
    }

    // This function is called when a surface has sent a commit to indicate the state has changed.
    //
    // In this case Smalvil delegates the handling to the "desktop" abstractions. A compositor can use this
    // function to perform other tasks as well.
    fn commit(&mut self, _dh: &mut DisplayHandle<'_>, _surface: &wl_surface::WlSurface) {
        todo!("desktop")
    }
}

// In order to complete implementing everything needed for the compositor state, we need to use the
// "delegate_compositor" macro to implement the Dispatch and GlobalDispatch traits on Smalvil for all the
// compositor protocol types. This macro ensures that the compositor protocols are handled by the
// CompositorState delegate type.
delegate_compositor!(Smalvil);

// Xdg shell trait and delegate
impl XdgShellHandler for Smalvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.xdg_shell_state
    }

    /// Called when an event generated by the xdg shell is received.
    fn request(&mut self, _dh: &mut DisplayHandle<'_>, _request: XdgRequest) {
        todo!("desktop")
    }
}

// Implement Dispatch and GlobalDispatch for Smalvil to handle the xdg shell.
delegate_xdg_shell!(Smalvil);

// Shm
impl AsRef<ShmState> for Smalvil {
    fn as_ref(&self) -> &ShmState {
        &self.protocols.shm_state
    }
}

delegate_shm!(Smalvil);

// Seat
impl SeatHandler<Self> for Smalvil {
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.protocols.seat_state
    }
}

delegate_seat!(Smalvil);

// Output(s)
delegate_output!(Smalvil);
