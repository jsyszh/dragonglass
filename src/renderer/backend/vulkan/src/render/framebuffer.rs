use crate::core::{SwapchainProperties, VulkanContext};
use ash::{version::DeviceV1_0, vk};
use std::sync::Arc;

// TODO: Add snafu errors

pub struct Framebuffer {
    framebuffer: vk::Framebuffer,
    context: Arc<VulkanContext>,
}

impl Framebuffer {
    // TODO: Refactor this to use less parameters
    pub fn new(
        context: Arc<VulkanContext>,
        swapchain_properties: &SwapchainProperties,
        render_pass: vk::RenderPass,
        attachments: &[vk::ImageView],
    ) -> Self {
        // TODO: Take creation info as a parameter
        let framebuffer_info = vk::FramebufferCreateInfo::builder()
            .render_pass(render_pass)
            .attachments(attachments)
            .width(swapchain_properties.extent.width)
            .height(swapchain_properties.extent.height)
            .layers(1)
            .build();
        let framebuffer = unsafe {
            context
                .logical_device()
                .logical_device()
                .create_framebuffer(&framebuffer_info, None)
                .expect("Failed to create framebuffer!")
        };

        Framebuffer {
            framebuffer,
            context,
        }
    }

    pub fn framebuffer(&self) -> vk::Framebuffer {
        self.framebuffer
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe {
            self.context
                .logical_device()
                .logical_device()
                .destroy_framebuffer(self.framebuffer, None);
        }
    }
}