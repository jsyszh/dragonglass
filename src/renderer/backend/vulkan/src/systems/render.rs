use crate::{
    model::{
        gltf::GltfAsset,
        pbr_asset::{DynamicUniformBufferObject, UniformBufferObject},
    },
    pipelines::GltfPipeline,
    render::Renderer,
    sync::{SynchronizationSet, SynchronizationSetConstants},
};
use ash::vk;
use dragonglass_core::{
    camera::CameraViewMatrix,
    components::{AssetName, Transform},
};
use legion::prelude::*;
use nalgebra_glm as glm;
use petgraph::{graph::NodeIndex, visit::Dfs};

pub fn prepare_renderer_system() -> Box<dyn Schedulable> {
    SystemBuilder::new("prepare_renderer")
        .write_resource::<Renderer>()
        .with_query(<Read<AssetName>>::query())
        .build(|_, mut world, mut renderer, query| {
            let asset_names = query
                .iter(&mut world)
                .map(|asset_name| asset_name.0.to_string())
                .collect::<Vec<_>>();
            let pipeline_gltf = GltfPipeline::new(&mut renderer, &asset_names);
            renderer.pipeline_gltf = Some(pipeline_gltf);
        })
}

pub fn render_system() -> Box<dyn Runnable> {
    SystemBuilder::new("render")
        .write_resource::<Renderer>()
        .read_resource::<CameraViewMatrix>()
        .with_query(<Read<Transform>>::query())
        .build_thread_local(move |_, mut world, (renderer, camera_view_matrix), query| {
            let context = renderer.context.clone();

            let current_frame_synchronization = renderer
                .synchronization_set
                .current_frame_synchronization(renderer.current_frame);

            context
                .logical_device()
                .wait_for_fence(&current_frame_synchronization);

            // Acquire the next image from the swapchain
            let image_index_result = renderer.vulkan_swapchain.swapchain.acquire_next_image(
                current_frame_synchronization.image_available(),
                vk::Fence::null(),
            );

            let image_index = match image_index_result {
                Ok((image_index, _)) => image_index,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    // TODO: Recreate the swapchain
                    return;
                }
                Err(error) => panic!("Error while acquiring next image. Cause: {}", error),
            };
            let image_indices = [image_index];

            context
                .logical_device()
                .reset_fence(&current_frame_synchronization);

            // Update UBOS

            let projection = glm::perspective_zo(
                renderer
                    .vulkan_swapchain
                    .swapchain
                    .properties()
                    .aspect_ratio(),
                90_f32.to_radians(),
                0.1_f32,
                1000_f32,
            );

            for transform in query.iter(&mut world) {
                // TODO: Keep track of the global transform using the gltf document
                // and render meshes at the correct transform
                // TODO: Go through all assets
                let asset_transform = transform.translate * transform.rotate * transform.scale;
                let asset_index = 0;
                let pbr_asset = &renderer.pipeline_gltf.as_ref().unwrap().assets[asset_index];

                let ubo = UniformBufferObject {
                    view: camera_view_matrix.0,
                    projection,
                };
                let ubos = [ubo];
                let buffer = &pbr_asset.uniform_buffer;
                buffer.upload_to_buffer(&ubos, 0, std::mem::align_of::<UniformBufferObject>() as _);

                let full_dynamic_ubo_size =
                    (pbr_asset.asset.number_of_meshes as u64 * pbr_asset.dynamic_alignment) as u64;

                for scene in pbr_asset.asset.scenes.iter() {
                    for graph in scene.node_graphs.iter() {
                        let mut dfs = Dfs::new(&graph, NodeIndex::new(0));
                        while let Some(node_index) = dfs.next(&graph) {
                            let global_transform =
                                GltfAsset::calculate_global_transform(node_index, graph);
                            if let Some(mesh) = graph[node_index].mesh.as_ref() {
                                let dynamic_ubo = DynamicUniformBufferObject {
                                    model: asset_transform * global_transform,
                                };
                                let ubos = [dynamic_ubo];
                                let buffer = &pbr_asset.dynamic_uniform_buffer;
                                let offset =
                                    (pbr_asset.dynamic_alignment * mesh.mesh_id as u64) as usize;

                                buffer.upload_to_buffer(&ubos, offset, pbr_asset.dynamic_alignment);
                                buffer
                                    .flush(0, full_dynamic_ubo_size as _)
                                    .expect("Failed to flush buffer!");
                            }
                        }
                    }
                }
            }

            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            renderer.command_pool.submit_command_buffer(
                image_index as usize,
                renderer.graphics_queue,
                &wait_stages,
                &current_frame_synchronization,
            );

            let swapchain_presentation_result =
                renderer.vulkan_swapchain.swapchain.present_rendered_image(
                    &current_frame_synchronization,
                    &image_indices,
                    renderer.present_queue,
                );

            match swapchain_presentation_result {
                Ok(is_suboptimal) if is_suboptimal => {
                    // TODO: Recreate the swapchain
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    // TODO: Recreate the swapchain
                }
                Err(error) => panic!("Failed to present queue. Cause: {}", error),
                _ => {}
            }

            // TODO: Recreate the swapchain if resize was requested

            renderer.current_frame +=
                (1 + renderer.current_frame) % SynchronizationSet::MAX_FRAMES_IN_FLIGHT as usize;
        })
}