use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering, time::Duration};

use slog::Logger;
use smithay::{
    backend::{
        egl::{EGLContext, EGLDisplay},
        renderer::{gles2::Gles2Renderer, Bind, ImportEgl, Renderer, Transform, Unbind},
        x11::{surface::X11Surface, WindowProperties, X11Backend, X11Event},
        SwapBuffersError,
    },
    reexports::{
        calloop::EventLoop,
        wayland_server::{protocol::wl_output, Display},
    },
    wayland::output::{Mode, PhysicalProperties},
};

use crate::{render::render_layers_and_windows, state::Backend, AnvilState};

#[cfg(feature = "debug")]
use smithay::backend::renderer::gles2::Gles2Texture;

pub const OUTPUT_NAME: &str = "x11";

#[derive(Debug)]
pub struct X11Data {
    mode: Mode,
    surface: X11Surface,
    #[cfg(feature = "debug")]
    fps_texture: Gles2Texture,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
}

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
}

pub fn run_x11(log: Logger) {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::new()));

    let window_properties = WindowProperties {
        title: "Anvil",
        ..WindowProperties::default()
    };

    let (backend, surface) = X11Backend::new(window_properties, log.clone()).expect("Failed to initialize X11 backend");

    // Initialize EGL using the GBM device setup earlier.
    let egl = EGLDisplay::new(&surface.device(), log.clone()).expect("TODO");
    let context = EGLContext::new(&egl, log.clone()).expect("TODO");
    let mut renderer =
        unsafe { Gles2Renderer::new(context, log.clone()) }.expect("Failed to initialize renderer");

    #[cfg(feature = "egl")]
    {
        if renderer.bind_wl_display(&*display.borrow()).is_ok() {
            info!(log, "EGL hardware-acceleration enabled");
        }
    }

    let size = {
        let s = backend.window().size();

        (s.w as i32, s.h as i32).into()
    };

    let mode = Mode {
        size,
        refresh: 60_000,
    };

    let data = X11Data {
        mode,
        surface,
        #[cfg(feature = "debug")]
        fps_texture: {
            use crate::drawing::{import_bitmap, FPS_NUMBERS_PNG};

            import_bitmap(
                &mut renderer,
                &image::io::Reader::with_format(
                    std::io::Cursor::new(FPS_NUMBERS_PNG),
                    image::ImageFormat::Png,
                )
                .decode()
                .unwrap()
                .to_rgba8(),
            )
            .expect("Unable to upload FPS texture")
        },
        #[cfg(feature = "debug")]
        fps: fps_ticker::Fps::default(),
    };

    let mut state = AnvilState::init(display.clone(), event_loop.handle(), data, log.clone(), true);

    state.output_map.borrow_mut().add(
        OUTPUT_NAME,
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11".into(),
        },
        mode,
    );

    event_loop
        .handle()
        .insert_source(backend, |event, _window, state| match event {
            X11Event::CloseRequested => {
                state.running.store(false, Ordering::SeqCst);
            }

            X11Event::Resized(size) => {
                let size = { (size.w as i32, size.h as i32).into() };

                state.backend_data.mode = Mode {
                    size,
                    refresh: 60_000,
                };
                state.output_map.borrow_mut().update_mode_by_name(
                    Mode {
                        size,
                        refresh: 60_000,
                    },
                    OUTPUT_NAME,
                );

                let output_mut = state.output_map.borrow();
                let output = output_mut.find_by_name(OUTPUT_NAME).unwrap();

                state.window_map.borrow_mut().layers.arange_layers(output);
            }

            X11Event::Input(event) => state.process_input_event(event),

            _ => (),
        })
        .expect("Failed to insert X11 Backend into event loop");

    let start_time = std::time::Instant::now();
    let mut cursor_visible = true;

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    info!(log, "Initialization completed, starting the main loop.");

    while state.running.load(Ordering::SeqCst) {
        let (output_geometry, output_scale) = state
            .output_map
            .borrow()
            .find_by_name(OUTPUT_NAME)
            .map(|output| (output.geometry(), output.scale()))
            .unwrap();

        {
            let backend_data = &mut state.backend_data;
            let present = backend_data.surface.present().expect("TODO");
            let window_map = state.window_map.borrow();
            #[cfg(feature = "debug")]
            let fps = backend_data.fps.avg().round() as u32;
            #[cfg(feature = "debug")]
            let fps_texture = &backend_data.fps_texture;

            renderer.bind(present.buffer()).expect("TODO");

            // drawing logic
            match renderer
                // Apparently X11 is upside down
                .render(
                    backend_data.mode.size,
                    Transform::Flipped180,
                    |renderer, frame| {
                        render_layers_and_windows(
                            renderer,
                            frame,
                            &*window_map,
                            output_geometry,
                            output_scale,
                            &log,
                        )?;

                        #[cfg(feature = "debug")]
                        {
                            use crate::drawing::draw_fps;

                            draw_fps(renderer, frame, fps_texture, output_scale as f64, fps)?;
                        }

                        Ok(())
                    },
                )
                .map_err(Into::<SwapBuffersError>::into)
                .and_then(|x| x)
                .map_err(Into::<SwapBuffersError>::into)
            {
                Ok(()) => {
                    // Unbind the buffer and now let the scope end to present.
                    renderer.unbind().expect("Unbind");
                }

                Err(err) => {
                    // TODO:
                    panic!("Swap buffers");
                }
            }
        }

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

    // TODO: Figure out why the renderer is dropped later than everything else and therefore segfaults?
    drop(renderer);
}
