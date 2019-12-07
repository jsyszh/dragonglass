// TODO: Make a type alias for the current device version (DeviceV1_0)
use crate::{core::SwapchainProperties, VulkanContext};
use ash::{version::DeviceV1_0, vk};
use std::sync::Arc;

// TODO: Add snafu errors

pub struct ImageView {
    view: vk::ImageView,
    context: Arc<VulkanContext>,
}

impl ImageView {
    pub fn new(
        context: Arc<VulkanContext>,
        image: vk::Image,
        swapchain_properties: &SwapchainProperties,
    ) -> Self {
        let create_info = vk::ImageViewCreateInfo::builder()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(swapchain_properties.format.format)
            .components(vk::ComponentMapping {
                r: vk::ComponentSwizzle::IDENTITY,
                g: vk::ComponentSwizzle::IDENTITY,
                b: vk::ComponentSwizzle::IDENTITY,
                a: vk::ComponentSwizzle::IDENTITY,
            })
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .build();

        let view = unsafe {
            context
                .logical_device()
                .logical_device()
                .create_image_view(&create_info, None)
                .unwrap()
        };

        ImageView { view, context }
    }

    pub fn view(&self) -> vk::ImageView {
        self.view
    }
}

impl Drop for ImageView {
    fn drop(&mut self) {
        unsafe {
            self.context
                .logical_device()
                .logical_device()
                .destroy_image_view(self.view, None);
        }
    }
}