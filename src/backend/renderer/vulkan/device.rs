use std::{
    ffi::{CString, NulError},
    fmt,
    sync::Arc,
};

use ash::vk;

use super::{Instance, PhysicalDevice};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// Some extensions were missing when creating a device.
    #[error("The following extensions device extensions were missing: {}", .0.join(", "))]
    MissingExtensions(Vec<String>),

    /// An extension has an invalid name.
    #[error("The extension string \"{0}\" contains a null terminator at {}", .1.nul_position())]
    InvalidExtension(String, NulError),

    /// Some Vulkan error occurred while creating a device.
    #[error("{0}")]
    Vulkan(#[from] vk::Result),
}

#[derive(Debug)]
pub struct Device {
    inner: Arc<DeviceInner>,
}

impl Device {
    /// Creates a new device using the specified physical device, enabled features and extensions.
    pub fn new<'a, E>(
        physical_device: &PhysicalDevice<'_>,
        // Not actually modified by the implementation.
        enabled_features: &mut vk::PhysicalDeviceFeatures2,
        extensions: E,
    ) -> Result<Device, DeviceError>
    // TODO: Queue
    where
        E: IntoIterator<Item = &'a str>,
    {
        // TODO: Check missing extensions

        let instance = physical_device.instance().impl_clone();
        // SAFETY: Device holds a strong reference to the instance.
        let instance_handle = unsafe { instance.raw_handle() };
        // SAFETY: Only used while in scope
        let physical_handle = unsafe { physical_device.raw_handle() };

        let extensions = {
            let extensions = extensions.into_iter();
            let extensions_count = {
                let size_hint = extensions.size_hint();

                size_hint.1.unwrap_or(size_hint.0)
            };

            let mut c_extensions = Vec::with_capacity(extensions_count);

            for extension in extensions {
                c_extensions.push(
                    CString::new(extension)
                        .map_err(|err| DeviceError::InvalidExtension(extension.to_owned(), err))?,
                );
            }

            c_extensions
        };

        let extensions = &extensions
            .into_iter()
            .map(|extension| extension.as_ptr())
            .collect::<Vec<_>>();

        // We are told in ash to not call `build()` but there is no way to use the queue create into without calling build...
        let queue_info = [vk::DeviceQueueCreateInfo::builder()
            // TODO:
            .build()];

        let create_info = vk::DeviceCreateInfo::builder()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(extensions)
            .push_next(enabled_features);

        // SAFETY: The device will hold a strong reference to the parent instance, ensuring the device is destroyed
        // before the instance.
        let device = unsafe { instance_handle.create_device(*physical_handle, &create_info, None) }?;

        Ok(Device {
            inner: Arc::new(DeviceInner {
                handle: device,
                instance,
            }),
        })
    }

    /// Returns the instance that owns this device.
    pub fn instance(&self) -> &Instance {
        &self.inner.instance
    }

    /// Returns a handle to ash's Vulkan device type.
    ///
    /// ## Safety
    ///
    /// The caller responsible for ensuring the returned handle is only used while the device is valid.
    ///
    /// The caller is also responsible for ensuring any child objects created by the device are destroyed before the
    /// owning device is dropped.
    pub unsafe fn raw_handle(&self) -> &ash::Device {
        &self.inner.handle
    }
}

pub(crate) struct DeviceInner {
    pub handle: ash::Device,
    /// Strong reference to the instance that owns this device.
    ///
    /// This ensures that the device will be destroyed before the instance, meeting the requirements of the Vulkan
    /// specification.
    pub instance: Instance,
}

impl fmt::Debug for DeviceInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInner")
            .field("handle", &self.handle.handle())
            .field("instance", &self.instance)
            .finish()
    }
}

impl Drop for DeviceInner {
    fn drop(&mut self) {
        // SAFETY (Synchronization):
        // - Externally synchronized because DeviceInner is always inside an `Arc`.
        // SAFETY (Usage):
        // - All child objects of the instance hold strong references to the `Device`, therefore all child
        //   objects will be dropped before the device is.
        unsafe {
            self.handle.destroy_device(None);
        }
    }
}
