use crate::{
    core::VulkanContext,
    resource::{Buffer, CommandPool, ImageView, Sampler},
};
use ash::{version::DeviceV1_0, vk};
use gltf::image::Format;
use image::{DynamicImage, ImageBuffer, Pixel, RgbImage};
use std::{iter, sync::Arc};

// TODO: Add snafu errors

pub struct TextureDescription {
    pub format: vk::Format,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub mip_levels: u32,
}

impl TextureDescription {
    pub fn from_file(path: &str) -> Self {
        let image = image::open(path).expect("Failed to open image path!");
        Self::from_image(&image)
    }

    pub fn from_image(image: &DynamicImage) -> Self {
        let (format, (width, height)) = match image {
            DynamicImage::ImageRgb8(buffer) => (vk::Format::R8G8B8_UNORM, buffer.dimensions()),
            DynamicImage::ImageRgba8(buffer) => (vk::Format::R8G8B8A8_UNORM, buffer.dimensions()),
            DynamicImage::ImageBgr8(buffer) => (vk::Format::B8G8R8_UNORM, buffer.dimensions()),
            DynamicImage::ImageBgra8(buffer) => (vk::Format::B8G8R8A8_UNORM, buffer.dimensions()),
            DynamicImage::ImageRgb16(buffer) => (vk::Format::R16G16B16_UNORM, buffer.dimensions()),
            DynamicImage::ImageRgba16(buffer) => {
                (vk::Format::R16G16B16A16_UNORM, buffer.dimensions())
            }
            _ => panic!("Failed to match the provided image format to a vulkan format!"),
        };

        let mut description = Self {
            format,
            width,
            height,
            pixels: image.to_bytes(),
            mip_levels: Self::calculate_mip_levels(width, height),
        };
        description.convert_24bit_formats();
        description
    }

    pub fn from_gltf(data: &gltf::image::Data) -> Self {
        let format = Self::convert_to_vulkan_format(data.format);
        let mut description = Self {
            format,
            width: data.width,
            height: data.height,
            pixels: data.pixels.to_vec(),
            mip_levels: Self::calculate_mip_levels(data.width, data.height),
        };
        description.convert_24bit_formats();
        description
    }

    pub fn calculate_mip_levels(width: u32, height: u32) -> u32 {
        ((width.min(height) as f32).log2().floor() + 1.0) as u32
    }

    fn convert_24bit_formats(&mut self) {
        // 24-bit formats are unsupported, so they
        // need to have an alpha channel added to make them 32-bit
        match self.format {
            vk::Format::R8G8B8_UNORM => {
                self.format = vk::Format::R8G8B8A8_UNORM;
                self.attach_alpha_channel();
            }
            vk::Format::B8G8R8_UNORM => {
                self.format = vk::Format::B8G8R8A8_UNORM;
                self.attach_alpha_channel();
            }
            _ => {}
        };
    }

    fn attach_alpha_channel(&mut self) {
        let image_buffer: RgbImage =
            ImageBuffer::from_raw(self.width, self.height, self.pixels.to_vec())
                .expect("Failed to create an image buffer");

        self.pixels = image_buffer
            .pixels()
            .flat_map(|pixel| pixel.to_rgba().channels().to_vec())
            .collect::<Vec<_>>();
    }

    fn convert_to_vulkan_format(format: Format) -> vk::Format {
        match format {
            Format::R8 => vk::Format::R8_UNORM,
            Format::R8G8 => vk::Format::R8G8_UNORM,
            Format::R8G8B8A8 => vk::Format::R8G8B8A8_UNORM,
            Format::B8G8R8A8 => vk::Format::B8G8R8A8_UNORM,
            Format::R8G8B8 => vk::Format::R8G8B8_UNORM,
            Format::B8G8R8 => vk::Format::B8G8R8_UNORM,
        }
    }
}

// The order of the struct fields matters here
// because it determines drop order
pub struct Texture {
    image: vk::Image,
    allocation: vk_mem::Allocation,
    allocation_info: vk_mem::AllocationInfo,
    context: Arc<VulkanContext>,
}

impl Texture {
    pub fn new(
        context: Arc<VulkanContext>,
        allocation_create_info: &vk_mem::AllocationCreateInfo,
        image_create_info: &vk::ImageCreateInfo,
    ) -> Self {
        let (image, allocation, allocation_info) = context
            .allocator()
            .create_image(&image_create_info, &allocation_create_info)
            .expect("Failed to create image!");

        Self {
            image,
            allocation,
            allocation_info,
            context,
        }
    }

    pub fn upload_texture_data(
        &self,
        command_pool: &CommandPool,
        description: &TextureDescription,
    ) {
        let region = vk::BufferImageCopy::builder()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: description.width,
                height: description.height,
                depth: 1,
            })
            .build();
        let regions = [region];
        let buffer = Buffer::new_mapped_basic(
            self.context.clone(),
            self.allocation_info().get_size() as _,
            vk::BufferUsageFlags::TRANSFER_SRC,
            vk_mem::MemoryUsage::CpuToGpu,
        );
        buffer.upload_to_buffer(&description.pixels, 0, std::mem::align_of::<u8>() as _);

        let barrier = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image())
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: description.mip_levels,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .build();
        let barriers = [barrier];

        command_pool.transition_image_layout(
            &barriers,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
        );

        command_pool.copy_buffer_to_image(
            self.context.graphics_queue(),
            buffer.buffer(),
            self.image(),
            &regions,
        );

        self.generate_mipmaps(&command_pool, &description);
    }

    pub fn generate_mipmaps(
        &self,
        command_pool: &CommandPool,
        texture_description: &TextureDescription,
    ) {
        let format_properties = self
            .context
            .physical_device_format_properties(texture_description.format);

        if !format_properties
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR)
        {
            panic!(
                "Linear blitting is not supported for format: {:?}",
                texture_description.format
            );
        }

        let mut mip_width = texture_description.width as i32;
        let mut mip_height = texture_description.height as i32;
        for level in 1..texture_description.mip_levels {
            let next_mip_width = if mip_width > 1 {
                mip_width / 2
            } else {
                mip_width
            };

            let next_mip_height = if mip_height > 1 {
                mip_height / 2
            } else {
                mip_height
            };

            let barrier = vk::ImageMemoryBarrier::builder()
                .image(self.image())
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_array_layer: 0,
                    layer_count: 1,
                    level_count: 1,
                    base_mip_level: level - 1,
                })
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .build();
            let barriers = [barrier];

            command_pool.transition_image_layout(
                &barriers,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
            );

            let blit = vk::ImageBlit::builder()
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: mip_width,
                        y: mip_height,
                        z: 1,
                    },
                ])
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level - 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: next_mip_width,
                        y: next_mip_height,
                        z: 1,
                    },
                ])
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .build();
            let blits = [blit];

            command_pool.execute_command_once(
                self.context.graphics_queue(),
                |command_buffer| unsafe {
                    self.context
                        .logical_device()
                        .logical_device()
                        .cmd_blit_image(
                            command_buffer,
                            self.image(),
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            self.image(),
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &blits,
                            vk::Filter::LINEAR,
                        )
                },
            );

            let barrier = vk::ImageMemoryBarrier::builder()
                .image(self.image())
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_array_layer: 0,
                    layer_count: 1,
                    level_count: 1,
                    base_mip_level: level - 1,
                })
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .build();
            let barriers = [barrier];

            command_pool.transition_image_layout(
                &barriers,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
            );

            mip_width = next_mip_width;
            mip_height = next_mip_height;
        }

        let barrier = vk::ImageMemoryBarrier::builder()
            .image(self.image())
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_array_layer: 0,
                layer_count: 1,
                level_count: 1,
                base_mip_level: texture_description.mip_levels - 1,
            })
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .build();
        let barriers = [barrier];

        command_pool.transition_image_layout(
            &barriers,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        );
    }

    pub fn image(&self) -> vk::Image {
        self.image
    }

    pub fn allocation(&self) -> &vk_mem::Allocation {
        &self.allocation
    }

    pub fn allocation_info(&self) -> &vk_mem::AllocationInfo {
        &self.allocation_info
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        self.context
            .allocator()
            .destroy_image(self.image, &self.allocation)
            .expect("Failed to destroy image!");
    }
}

pub struct CubemapFaces {
    pub right: String,
    pub left: String,
    pub top: String,
    pub bottom: String,
    pub front: String,
    pub back: String,
}

impl CubemapFaces {
    fn ordered_faces(&self) -> impl Iterator<Item = String> {
        iter::once(self.right.to_string())
            .chain(iter::once(self.left.to_string()))
            .chain(iter::once(self.top.to_string()))
            .chain(iter::once(self.bottom.to_string()))
            .chain(iter::once(self.back.to_string()))
            .chain(iter::once(self.front.to_string()))
    }
}

pub struct Cubemap {
    pub texture: Texture,
    pub view: ImageView,
    pub sampler: Sampler,
}

impl Cubemap {
    pub fn new(
        context: Arc<VulkanContext>,
        command_pool: &CommandPool,
        faces: &CubemapFaces,
    ) -> Self {
        let face_descriptions = faces
            .ordered_faces()
            .map(|face| TextureDescription::from_file(&face))
            .collect::<Vec<_>>();

        // TODO: Calculate miplevels and dimension
        let dimension = face_descriptions[0].width;
        let format = face_descriptions[0].format;
        let cubemap_description = TextureDescription {
            width: dimension,
            height: dimension,
            pixels: Vec::new(),
            format,
            mip_levels: 1,
        };

        let texture = Self::create_texture(context.clone(), &cubemap_description);

        Self::upload_texture_data(
            context.clone(),
            &command_pool,
            &texture,
            &face_descriptions,
            &cubemap_description,
        );

        let view = Self::create_image_view(context.clone(), &texture, &cubemap_description);

        let sampler = Self::create_sampler(context, &cubemap_description);

        Self {
            texture,
            view,
            sampler,
        }
    }

    pub fn upload_texture_data(
        context: Arc<VulkanContext>,
        command_pool: &CommandPool,
        texture: &Texture,
        face_descriptions: &[TextureDescription],
        cubemap_description: &TextureDescription,
    ) {
        let mut pixels: Vec<u8> = Vec::new();
        face_descriptions.iter().for_each(|description| {
            pixels.extend(&description.pixels);
        });

        let buffer = Buffer::new_mapped_basic(
            context.clone(),
            texture.allocation_info().get_size() as _,
            vk::BufferUsageFlags::TRANSFER_SRC,
            vk_mem::MemoryUsage::CpuToGpu,
        );
        buffer.upload_to_buffer(&pixels, 0, std::mem::align_of::<u8>() as _);

        let barrier = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(texture.image())
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 6,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .build();
        let barriers = [barrier];

        command_pool.transition_image_layout(
            &barriers,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
        );

        let mut offset = 0;
        let regions = face_descriptions
            .iter()
            .enumerate()
            .map(|(face_index, face)| {
                let region = vk::BufferImageCopy::builder()
                    .buffer_offset(offset as _)
                    .buffer_row_length(0)
                    .buffer_image_height(0)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: face_index as _,
                        layer_count: 1,
                    })
                    .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
                    .image_extent(vk::Extent3D {
                        width: cubemap_description.width,
                        height: cubemap_description.height,
                        depth: 1,
                    })
                    .build();
                offset += face.pixels.len() * std::mem::size_of::<u8>();
                region
            })
            .collect::<Vec<_>>();

        command_pool.copy_buffer_to_image(
            context.graphics_queue(),
            buffer.buffer(),
            texture.image(),
            &regions,
        );

        let barrier = vk::ImageMemoryBarrier::builder()
            .image(texture.image())
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_array_layer: 0,
                layer_count: 6,
                level_count: 1,
                base_mip_level: cubemap_description.mip_levels - 1,
            })
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .build();
        let barriers = [barrier];

        command_pool.transition_image_layout(
            &barriers,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        );
    }

    fn create_texture(context: Arc<VulkanContext>, description: &TextureDescription) -> Texture {
        let image_create_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D {
                width: description.width,
                height: description.height,
                depth: 1,
            })
            .mip_levels(description.mip_levels)
            .array_layers(6)
            .format(description.format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(
                vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::SAMPLED,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .samples(vk::SampleCountFlags::TYPE_1)
            .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE)
            .build();

        let allocation_create_info = vk_mem::AllocationCreateInfo {
            usage: vk_mem::MemoryUsage::GpuOnly,
            ..Default::default()
        };

        Texture::new(context, &allocation_create_info, &image_create_info)
    }

    fn create_image_view(
        context: Arc<VulkanContext>,
        texture: &Texture,
        description: &TextureDescription,
    ) -> ImageView {
        let create_info = vk::ImageViewCreateInfo::builder()
            .image(texture.image())
            .view_type(vk::ImageViewType::CUBE)
            .format(description.format)
            .components(vk::ComponentMapping {
                r: vk::ComponentSwizzle::IDENTITY,
                g: vk::ComponentSwizzle::IDENTITY,
                b: vk::ComponentSwizzle::IDENTITY,
                a: vk::ComponentSwizzle::IDENTITY,
            })
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: description.mip_levels,
                base_array_layer: 0,
                layer_count: 6,
            })
            .build();
        ImageView::new(context, create_info)
    }

    fn create_sampler(context: Arc<VulkanContext>, description: &TextureDescription) -> Sampler {
        let sampler_info = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .compare_op(vk::CompareOp::ALWAYS)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .mip_lod_bias(0.0)
            .min_lod(0.0)
            .max_lod(description.mip_levels as _)
            .build();
        Sampler::new(context, sampler_info)
    }
}
