use std::error::Error;

use smithay::backend::renderer::vulkan::{Device, Instance, PhysicalDevice};

fn main() -> Result<(), Box<dyn Error>> {
    // First create an instance to load Vulkan.
    // The instance being created has no requested extensions or layers.
    let instance = Instance::with_extensions(std::iter::empty())?;

    // Enumerate over the available physical devices.
    let (_, physical_device) = PhysicalDevice::enumerate(&instance)?
        .enumerate()
        // Print information about each device.
        .inspect(|(index, device)| {
            println!("Device {}: {}", index, device.name());
            println!("Extensions:");

            // Print all supported extensions on the device.
            for extension in device.extensions() {
                println!("\t{}", extension);
            }
        })
        // Pick the first device for the sake of this example.
        .next()
        .expect("No devices available");

    // A raw handle may be obtained from Smithay's "PhysicalDevice" for checking if other properties or device capabilities are available.
    let _ = unsafe { physical_device.raw_handle() };

    // Create a device from the physical device.
    let device = Device::new(
        &physical_device,
        &mut ash::vk::PhysicalDeviceFeatures2::default(),
        std::iter::empty(),
    )?;

    // A raw handle may be obtained from Smithay's "Device" to allow creation of objects from extensions.
    let _ = unsafe { device.raw_handle() };

    Ok(())
}
