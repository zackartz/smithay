//! Implementation of the rendering traits using Vulkan.

mod vulkano_ext;

use crate::backend::drm::DrmDevice;
use crate::backend::renderer::{Frame, Renderer, Texture, Transform};
use image::{ImageBuffer, Rgba};
use std::error::Error;
use std::marker::PhantomData;
use std::sync::Arc;
use vulkano::device::{Device, DeviceExtensions};
use vulkano::instance::PhysicalDevice;

// TODO: Questions to ask
//  How to handle winit: swapchain presentation needs to occur somewhere
//  Shaders: What library, or do we make the library user provide their own shader modules.
//  Libraries: Vulkano initially, but ash should also be done.
//   The fact vulkano is now built on ash gives us some opportunities for reducing duplicate code,
//   but I'd tend towards keeping the vulkano implementation as safe as possible.
//  Exposition of types.
//   Some things like the device and instance is obvious. However things related to the render pass,
//   command buffers, fences, shader modules need to be decided.
//  Textures.
//  Implement smithay traits.
//  Allocation of memory. We probably will need to allocate to the drm buf.
pub struct VulkanRenderer;

impl VulkanRenderer {
    pub fn with_drm() -> Result<VulkanRenderer, VulkanRendererCreateError> {
        todo!()
    }

    pub fn device(&self) -> &Arc<Device> {
        todo!()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum VulkanRendererCreateError {}
