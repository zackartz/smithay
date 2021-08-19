use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

use slog::Logger;
use smithay::{
    backend::{
        input::InputBackend,
        x11::{WindowProperties, X11Backend, X11Event},
    },
    reexports::{calloop::EventLoop, wayland_server::Display},
};

use crate::{state::Backend, AnvilState};

#[derive(Debug)]
struct X11Data;

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
}

pub fn run_x11(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let window_properties = WindowProperties {
        width: 1280,
        height: 800,
        title: "Anvil",
    };

    let backend = X11Backend::new(window_properties, log.clone()).expect("Failed to initialize X11 backend");

    // TODO: Renderer?
    let data = X11Data;

    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    event_loop
        .handle()
        .insert_source(backend, |event, _window, state| {
            if let X11Event::CloseRequested = event {
                state.running.store(false, Ordering::SeqCst);
            }

            println!("{:?}", event);
        })
        .expect("Failed to insert X11 Backend into event loop");

    let start_time = std::time::Instant::now();
    let mut cursor_visible = true;

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        // // drawing logic
        // {
        //     let mut renderer = renderer.borrow_mut();
        //     // This is safe to do as with winit we are guaranteed to have exactly one output
        //     let (output_geometry, output_scale) = state
        //         .output_map
        //         .borrow()
        //         .find_by_name(OUTPUT_NAME)
        //         .map(|output| (output.geometry(), output.scale()))
        //         .unwrap();

        //     let result = renderer
        //         .render(|renderer, frame| {
        //             frame.clear([0.8, 0.8, 0.9, 1.0])?;

        //             let window_map = &*state.window_map.borrow();

        //             for layer in [Layer::Background, Layer::Bottom] {
        //                 draw_layers(
        //                     renderer,
        //                     frame,
        //                     window_map,
        //                     layer,
        //                     output_geometry,
        //                     output_scale,
        //                     &log,
        //                 )?;
        //             }

        //             // draw the windows
        //             draw_windows(renderer, frame, window_map, output_geometry, output_scale, &log)?;

        //             for layer in [Layer::Top, Layer::Overlay] {
        //                 draw_layers(
        //                     renderer,
        //                     frame,
        //                     window_map,
        //                     layer,
        //                     output_geometry,
        //                     output_scale,
        //                     &log,
        //                 )?;
        //             }

        //             let (x, y) = state.pointer_location.into();
        //             // draw the dnd icon if any
        //             {
        //                 let guard = state.dnd_icon.lock().unwrap();
        //                 if let Some(ref surface) = *guard {
        //                     if surface.as_ref().is_alive() {
        //                         draw_dnd_icon(
        //                             renderer,
        //                             frame,
        //                             surface,
        //                             (x as i32, y as i32).into(),
        //                             output_scale,
        //                             &log,
        //                         )?;
        //                     }
        //                 }
        //             }
        //             // draw the cursor as relevant
        //             {
        //                 let mut guard = state.cursor_status.lock().unwrap();
        //                 // reset the cursor if the surface is no longer alive
        //                 let mut reset = false;
        //                 if let CursorImageStatus::Image(ref surface) = *guard {
        //                     reset = !surface.as_ref().is_alive();
        //                 }
        //                 if reset {
        //                     *guard = CursorImageStatus::Default;
        //                 }

        //                 // draw as relevant
        //                 if let CursorImageStatus::Image(ref surface) = *guard {
        //                     cursor_visible = false;
        //                     draw_cursor(
        //                         renderer,
        //                         frame,
        //                         surface,
        //                         (x as i32, y as i32).into(),
        //                         output_scale,
        //                         &log,
        //                     )?;
        //                 } else {
        //                     cursor_visible = true;
        //                 }
        //             }

        //             #[cfg(feature = "debug")]
        //             {
        //                 let fps = state.backend_data.fps.avg().round() as u32;
        //                 draw_fps(
        //                     renderer,
        //                     frame,
        //                     &state.backend_data.fps_texture,
        //                     output_scale as f64,
        //                     fps,
        //                 )?;
        //             }

        //             Ok(())
        //         })
        //         .map_err(Into::<SwapBuffersError>::into)
        //         .and_then(|x| x);

        //     renderer.window().set_cursor_visible(cursor_visible);

        //     if let Err(SwapBuffersError::ContextLost(err)) = result {
        //         error!(log, "Critical Rendering Error: {}", err);
        //         state.running.store(false, Ordering::SeqCst);
        //     }
        // }

        // Send frame events so that client start drawing their next frame
        state
            .window_map
            .borrow()
            .send_frames(start_time.elapsed().as_millis() as u32);
        display.borrow_mut().flush_clients(&mut state);

        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            state.running.store(false, Ordering::SeqCst);
        } else {
            display.borrow_mut().flush_clients(&mut state);
            state.window_map.borrow_mut().refresh();
            state.output_map.borrow_mut().refresh();
        }

        #[cfg(feature = "debug")]
        state.backend_data.fps.tick();
    }

    // Cleanup stuff
    state.window_map.borrow_mut().clear();
}
