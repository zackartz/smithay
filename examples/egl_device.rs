use std::error::Error;

use smithay::{
    backend::egl::{
        ffi,
        native::{EGLNativeDisplay, EGLPlatform},
        EGLDevice, EGLDisplay,
    },
    egl_platform,
};

fn main() -> Result<(), Box<dyn Error>> {
    let display = EGLDisplay::new(&DriverPlatform, None)?;

    for (device_number, device) in EGLDevice::enumerate(&display)?.enumerate() {
        if let Ok(path) = device.drm_device_path() {
            println!("{} => {}", device_number, path.display());
        } else {
            println!("{} => No DRM device", device_number);
        }
    }

    Ok(())
}

struct DriverPlatform;

impl EGLNativeDisplay for DriverPlatform {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![egl_platform!(
            PLATFORM_X11_EXT,
            // We pass DEFAULT_DISPLAY (null pointer) because the driver should open a connection to the X server.
            ffi::egl::DEFAULT_DISPLAY,
            &["EGL_EXT_platform_x11"]
        )]
    }
}
