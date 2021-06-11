use crate::backend::drm::DrmDevice;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use vulkano::device::DeviceExtensions;
use vulkano::instance::{Instance, PhysicalDevice};

pub trait PhysicalDeviceExt {
    /// Returns the physical device associated with the provided drm file descriptor.
    fn from_fd<A>(instance: &Arc<Instance>, _fd: A) -> Result<Option<PhysicalDevice>, ()>
    where
        A: AsRawFd,
    {
        let _required = DeviceExtensions {
            // ext_physical_device_drm: true, // TODO: When in spec, check if supported.
            ..DeviceExtensions::none()
        };

        for physical_device in PhysicalDevice::enumerate(&instance) {
            // TODO: Return error when checking for supported extensions by a device.
            let _supported = DeviceExtensions::supported_by_device_raw(physical_device).unwrap();

            // TODO
            // if _supported.intersection(&_required).ext_physical_device_drm {
            //     // TODO: Compare fd and render nodes/etc.
            //     //  Add these things to extended properties.
            //     return Ok(Some(physical_device));
            // }
        }

        Ok(None)
    }
}
