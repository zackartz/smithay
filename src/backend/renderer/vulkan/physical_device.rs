#[cfg(feature = "backend_drm")]
use crate::backend::drm::{DrmDevice, DrmNode, NodeType};
#[cfg(feature = "backend_drm")]
use std::os::unix::prelude::AsRawFd;
use std::{ffi::CStr, slice::Iter};

use ash::vk;

use super::Instance;

/// An error that may occur when enumerating physical devices.
#[derive(Debug, thiserror::Error)]
pub enum EnumerateDevicesError {
    /// Some Vulkan error occurred while enumerating devices.
    #[error("{0}")]
    Vulkan(#[from] vk::Result),
}

/// Represents a Vulkan physical device.
///
/// A physical device in a loose sense refers to some software or hardware implementation of the Vulkan APIs.
/// Multiple physical devices may exist for one real device in the case that there are multiple installable
/// client drivers (ICDs).
#[derive(Debug)]
pub struct PhysicalDevice<'inst> {
    inner: vk::PhysicalDevice,
    instance: &'inst Instance,
    extensions: Vec<String>,
    features: vk::PhysicalDeviceFeatures2,
    name: String,
}

impl PhysicalDevice<'_> {
    /// Returns an iterator which enumerates over all available physical devices.
    ///
    /// The returned physical devices may be used to find the desired device you wish to use in Vulkan. One use of a
    /// physical device is testing if a device supports a specific device extension. You may also query the
    /// capabilities of a device.
    ///
    /// A set of enumerated devices is only valid for the lifetime of the parent instance. This means after destroying
    /// the instance, there is no guarantee the same amount physical devices will be enumerated or in the same order.
    pub fn enumerate(
        instance: &Instance,
    ) -> Result<impl Iterator<Item = PhysicalDevice<'_>>, EnumerateDevicesError> {
        // SAFETY: The created objects borrow the instance, therefore ensuring the physical devices are destroyed
        // before the instance is destroyed.
        let handle = unsafe { instance.raw_handle() };

        // SAFETY: Ash handles the safety of enumerate_physical_devices.
        let devices = unsafe { handle.enumerate_physical_devices() }?
            .iter()
            .map(|inner| {
                // SAFETY: Ash handles the safety of enumerate_device_extension_properties and we guarantee the
                // physical device is valid.
                let extensions = unsafe { handle.enumerate_device_extension_properties(*inner) }?
                    .iter()
                    .map(|extension|
                        // SAFETY: Vulkan always returns null terminated strings with a maximum length of 256
                        unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) })
                    .map(|extension| {
                        extension
                            .to_str()
                            .expect("Vulkan reported a non-UTF8 extension name")
                            .to_owned()
                    })
                    .collect::<Vec<_>>();

                let mut properties = vk::PhysicalDeviceProperties2::default();
                unsafe { handle.get_physical_device_properties2(*inner, &mut properties) };

                let mut features = vk::PhysicalDeviceFeatures2::default();
                unsafe { handle.get_physical_device_features2(*inner, &mut features) };

                // SAFETY: Vulkan always returns null terminated strings with a maximum length of 256
                let name = unsafe { CStr::from_ptr(properties.properties.device_name.as_ptr()) }
                    .to_str()
                    .expect("Vulkan reported a non-UTF8 device name")
                    .to_owned();

                Ok::<_, EnumerateDevicesError>(PhysicalDevice {
                    inner: *inner,
                    instance,
                    extensions,
                    features,
                    name,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(devices.into_iter())
    }

    /// Enumerates over all available devices and finds the device associated with the specified DRM node.
    #[cfg(feature = "backend_drm")]
    pub fn with_drm_node<'inst>(
        instance: &'inst Instance,
        node: &DrmNode,
    ) -> Result<Option<PhysicalDevice<'inst>>, EnumerateDevicesError> {
        Self::with_major_and_minor(instance, node.major(), node.minor(), node.ty())
    }

    /// Enumerates over all available devices and finds the device associated with the specified DRM device.
    #[cfg(feature = "backend_drm")]
    pub fn with_drm_device<'inst, A: AsRawFd>(
        instance: &'inst Instance,
        device: &DrmDevice<A>,
    ) -> Result<Option<PhysicalDevice<'inst>>, EnumerateDevicesError> {
        // Since a DRM device is capable of modesetting, it must be a primary node.
        Self::with_major_and_minor(instance, device.major(), device.minor(), NodeType::Primary)
    }

    #[cfg(feature = "backend_drm")]
    fn with_major_and_minor(
        instance: &Instance,
        major: u64,
        minor: u64,
        ty: NodeType,
    ) -> Result<Option<PhysicalDevice<'_>>, EnumerateDevicesError> {
        use ash::extensions::ext;

        let device = Self::enumerate(instance)?.find(|device| {
            // SAFETY: both raw handles are only used in scope.
            let instance = unsafe { device.instance().raw_handle() };
            let handle = unsafe { device.raw_handle() };

            if device
                .extensions()
                .any(|extension| *extension == "VK_EXT_physical_device_drm")
            {
                // SAFETY: The extension defining this function is present and the handles are valid.
                let drm_info = unsafe { ext::PhysicalDeviceDrm::get_properties(instance, *handle) };

                match ty {
                    NodeType::Primary if drm_info.has_primary == vk::TRUE => {
                        drm_info.primary_major == major as i64 && drm_info.primary_minor == minor as i64
                    }

                    NodeType::Render if drm_info.has_render == vk::TRUE => {
                        drm_info.render_major == major as i64 && drm_info.render_minor == minor as i64
                    }

                    // VK_EXT_physical_device_drm does not provide info about control nodes.
                    // Also handles the scenario where the device does not have a primary or render node but supports
                    // the extension which is a possible valid outcome.
                    _ => false,
                }
            } else {
                false // Device does not support the extension.
            }
        });

        Ok(device)
    }

    /// Returns the instance that owns this physical device.
    pub fn instance(&self) -> &Instance {
        self.instance
    }

    /// Returns an iterator over all the extensions this device supports.
    pub fn extensions(&self) -> Iter<'_, String> {
        self.extensions.iter()
    }

    /// Returns the features supported by the device.
    ///
    /// This only reports the core device features supported in the Vulkan specification. To check if features added
    /// through some extension, use the [raw handle](PhysicalDevice::raw_handle) of the physical device to check check
    /// for extended features.
    pub fn features(&self) -> vk::PhysicalDeviceFeatures {
        self.features.features
    }

    /// Returns the name of this device.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a handle to ash's Vulkan physical device type.
    ///
    /// ## Safety
    ///
    /// The caller responsible for ensuring the returned handle is only used while the physical device and the owning
    /// instance are valid.
    ///
    /// The caller is also responsible for ensuring any child objects created by the physical device are destroyed
    /// before the owning instance is dropped.
    pub unsafe fn raw_handle(&self) -> &vk::PhysicalDevice {
        &self.inner
    }
}
