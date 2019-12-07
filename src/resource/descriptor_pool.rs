// TODO: Make a type alias for the current device version (DeviceV1_0)
use crate::{resource::Buffer, VulkanContext};
use ash::{version::DeviceV1_0, vk};
use std::sync::Arc;

// TODO: Add snafu errors

pub struct DescriptorPool {
    pool: vk::DescriptorPool,
    context: Arc<VulkanContext>,
}

impl DescriptorPool {
    pub fn new(context: Arc<VulkanContext>, size: u32) -> Self {
        let pool_size = vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: size,
        };
        let pool_sizes = [pool_size];

        let pool_info = vk::DescriptorPoolCreateInfo::builder()
            .pool_sizes(&pool_sizes)
            .max_sets(size)
            .build();

        let pool = unsafe {
            context
                .logical_device()
                .create_descriptor_pool(&pool_info, None)
                .unwrap()
        };

        DescriptorPool { pool, context }
    }

    pub fn allocate_descriptor_sets(
        &self,
        layout: vk::DescriptorSetLayout,
        number_of_sets: u32,
    ) -> Vec<vk::DescriptorSet> {
        let layouts = (0..number_of_sets).map(|_| layout).collect::<Vec<_>>();
        let allocation_info = vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts)
            .build();
        unsafe {
            self.context
                .logical_device()
                .allocate_descriptor_sets(&allocation_info)
                .unwrap()
        }
    }

    // TODO: Refactor this to use less parameters and make it smaller
    pub fn update_descriptor_sets(
        &self,
        descriptor_sets: &[vk::DescriptorSet],
        descriptor_type: vk::DescriptorType,
        buffers: &[Buffer],
        range: vk::DeviceSize,
    ) {
        descriptor_sets
            .iter()
            .zip(buffers.iter())
            .for_each(|(set, buffer)| {
                let buffer_info = vk::DescriptorBufferInfo::builder()
                    .buffer(buffer.buffer())
                    .offset(0)
                    .range(range)
                    .build();
                let buffer_infos = [buffer_info];

                let descriptor_write = vk::WriteDescriptorSet::builder()
                    .dst_set(*set)
                    .dst_binding(0)
                    .dst_array_element(0)
                    .descriptor_type(descriptor_type)
                    .buffer_info(&buffer_infos)
                    .build();
                let descriptor_writes = [descriptor_write];
                let null = [];

                unsafe {
                    self.context
                        .logical_device()
                        .update_descriptor_sets(&descriptor_writes, &null)
                }
            })
    }
}

impl Drop for DescriptorPool {
    fn drop(&mut self) {
        unsafe {
            self.context
                .logical_device()
                .destroy_descriptor_pool(self.pool, None);
        }
    }
}
