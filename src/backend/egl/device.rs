use std::{ffi::CStr, mem::MaybeUninit, os::raw::c_void, path::PathBuf, ptr};

use super::{
    ffi::{self, egl::types::EGLDeviceEXT},
    wrap_egl_call, EGLDisplay, EGLError, Error,
};

/// safe EGLDevice wrapper
#[derive(Debug)]
pub struct EGLDevice {
    pub(super) inner: EGLDeviceEXT,
    device_extensions: Vec<String>,
}

unsafe impl Send for EGLDevice {}

impl EGLDevice {
    /// Returns an iterator which enumerates over the available [`EGLDevices`](EGLDevice) on the system.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_base.txt)
    /// - [`EGL_EXT_device_enumeration`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_enumeration.txt)
    /// - [`EGL_EXT_device_query`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_query.txt)
    pub fn enumerate() -> Result<impl Iterator<Item = EGLDevice>, Error> {
        // Check the required extensions are present:
        let extensions = ffi::make_sure_egl_is_loaded()?;

        if !extensions.iter().any(|s| s == "EGL_EXT_device_base") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_base"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_enumeration") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_enumeration"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_query") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_query"]));
        }

        // Yes, this is marked as `mut` even though the value is never mutated. EGL expects a mutable pointer
        // for num_devices and will not modify the value if we are asking for pointers to some EGLDeviceEXT.
        let mut device_amount = match wrap_egl_call(|| {
            let mut amount: MaybeUninit<ffi::egl::types::EGLint> = MaybeUninit::uninit();

            // Passing 0 for max devices and a null-pointer for devices is safe because we indicate we only
            // want the number of devices.
            if unsafe { ffi::egl::QueryDevicesEXT(0, ptr::null_mut(), amount.as_mut_ptr()) } != ffi::egl::TRUE
            {
                0
            } else {
                // Safe because EGL returned a valid value.
                unsafe { amount.assume_init() }
            }
        }) {
            Ok(number) => number,
            Err(err) => return Err(Error::QueryDevices(err)),
        };

        let mut devices = Vec::with_capacity(device_amount as usize);

        if let Err(err) = wrap_egl_call(|| unsafe {
            // This is safe because the vector is the same size as `device_amount` and we were told earlier
            // how many devices are available.
            ffi::egl::QueryDevicesEXT(device_amount, devices.as_mut_ptr(), &mut device_amount);

            // Set the length of the vec so that rust does not think it is still empty.
            // This is safe because:
            // 1) the vector is pre-allocated to the same size as the amount of returned devices.
            // 2) EGL will initialize every value in the pointer we give
            devices.set_len(device_amount as usize);
        }) {
            return Err(Error::QueryDevices(err));
        }

        Ok(devices
            .into_iter()
            .map(|device| {
                let device_extensions = device_extensions(device)?;
                Ok(EGLDevice {
                    inner: device,
                    device_extensions,
                })
            })
            .collect::<Result<Vec<_>, EGLError>>()
            .map_err(Error::QueryDevices)?
            .into_iter())
    }

    /// Returns the [`EGLDevices`](EGLDevice) related to the given `EGLDisplay`.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_base.txt)
    /// - [`EGL_EXT_device_query`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_query.txt)
    pub fn device_for_display(display: &EGLDisplay) -> Result<EGLDevice, Error> {
        // Check the required extensions are present:
        let extensions = ffi::make_sure_egl_is_loaded()?;

        if !extensions.iter().any(|s| s == "EGL_EXT_device_base") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_base"]));
        }

        if !extensions.iter().any(|s| s == "EGL_EXT_device_query") {
            return Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_query"]));
        }

        let mut device: ffi::egl::types::EGLAttrib = 0;
        if unsafe {
            ffi::egl::QueryDisplayAttribEXT(
                display.get_display_handle().handle,
                ffi::egl::DEVICE_EXT as i32,
                &mut device as *mut _,
            )
        } != ffi::egl::TRUE
        {
            return Err(Error::DisplayNotSupported);
        }

        let device = device as EGLDeviceEXT;
        let device_extensions = device_extensions(device).map_err(Error::QueryDevices)?;
        Ok(EGLDevice {
            inner: device,
            device_extensions,
        })
    }

    /// Returns a list of extensions the device supports.
    pub fn extensions(&self) -> Vec<String> {
        self.device_extensions.clone()
    }

    /// Returns the path to the drm node of this EGLDevice.
    ///
    /// This function will return an error if the following extensions are not available:
    /// - [`EGL_EXT_device_drm`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_device_drm.txt)
    pub fn drm_device_path(&self) -> Result<PathBuf, Error> {
        if !self.extensions().contains(&"EGL_EXT_device_drm".to_owned()) {
            Err(Error::EglExtensionNotSupported(&["EGL_EXT_device_drm"]))
        } else {
            let raw_path = wrap_egl_call(|| unsafe {
                ffi::egl::QueryDeviceStringEXT(
                    self.inner,
                    ffi::egl::DRM_DEVICE_FILE_EXT as ffi::egl::types::EGLint,
                )
            })
            .expect("TODO: Add error variant");

            // FIXME: Ensure EGL_FALSE is not returned.

            // This is safe because of the following:
            // 1) The string returned by `eglQueryDeviceStringEXT` is string which will exist as long
            //    as the EGLDisplay is valid. Since the pointer is only used in this function, the
            //    lifetime of the pointer will fulfil Rust's CStr requirements on lifetime.
            // 2) The string returned by EGL is null terminated.
            let device_path = unsafe { CStr::from_ptr(raw_path) }
                .to_str()
                // EGL ensures the string is valid UTF-8
                .expect("Non-UTF8 device path name");

            Ok(PathBuf::from(device_path))
        }
    }

    /// Returns the pointer to the raw [`EGLDevice`].
    pub fn inner(&self) -> *const c_void {
        self.inner
    }
}

fn device_extensions(device: EGLDeviceEXT) -> Result<Vec<String>, EGLError> {
    let raw_extensions = wrap_egl_call(|| unsafe {
        ffi::egl::QueryDeviceStringEXT(device, ffi::egl::EXTENSIONS as ffi::egl::types::EGLint)
    })?;

    // This is safe because of the following:
    // 1) The string returned by `eglQueryDeviceStringEXT` is string which will exist as long
    //    as the EGLDisplay is valid. Since the pointer is only used in this function, the
    //    lifetime of the pointer will fulfil Rust's CStr requirements on lifetime.
    // 2) The string returned by EGL is null terminated.
    // 3) Each extension is space separated (0x20) in the pointer, so strlen cannot return an
    //    improper length.
    let c_extensions = unsafe { CStr::from_ptr(raw_extensions) };

    Ok(c_extensions
        .to_str()
        // EGL ensures the string is valid UTF-8
        .expect("Non-UTF8 device extension name")
        .split_ascii_whitespace()
        // Take an owned copy so we do not point to garbage if EGL somehow vanishes.
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>())
}
