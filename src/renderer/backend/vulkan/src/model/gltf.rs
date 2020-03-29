use crate::{
    core::VulkanContext,
    model::ModelBuffers,
    render::Renderer,
    resource::{Buffer, CommandPool, ImageView, Sampler, Texture, TextureDescription},
};
use ash::vk;
use gltf::animation::{util::ReadOutputs, Interpolation};
use nalgebra::{Matrix4, Quaternion, UnitQuaternion};
use nalgebra_glm as glm;
use petgraph::{
    graph::{Graph, NodeIndex},
    prelude::*,
    visit::Dfs,
};
use std::sync::Arc;

#[derive(Debug)]
pub enum TransformationSet {
    Translations(Vec<glm::Vec3>),
    Rotations(Vec<glm::Vec4>),
    Scales(Vec<glm::Vec3>),
    MorphTargetWeights(Vec<f32>),
}

#[derive(Debug, Default)]
pub struct Transform {
    translation: Option<glm::Vec3>,
    rotation: Option<glm::Quat>,
    scale: Option<glm::Vec3>,
}

impl Transform {
    pub fn matrix(&self) -> glm::Mat4 {
        let mut matrix = glm::Mat4::identity();

        if let Some(translation) = self.translation {
            matrix *= Matrix4::new_translation(&translation);
        }

        if let Some(rotation) = self.rotation {
            matrix *= Matrix4::from(UnitQuaternion::from_quaternion(rotation));
        }

        if let Some(scale) = self.scale {
            matrix *= Matrix4::new_nonuniform_scaling(&scale);
        }

        matrix
    }
}

pub struct MorphTargets {
    positions: Vec<glm::Vec3>,
    normals: Vec<glm::Vec3>,
    tangents: Vec<glm::Vec3>,
}

pub type NodeGraph = Graph<Node, ()>;

pub struct Node {
    pub animation_transform: Transform,
    pub local_transform: glm::Mat4,
    pub mesh: Option<Mesh>,
    pub index: usize,
}

pub struct Scene {
    pub node_graphs: Vec<NodeGraph>,
}

pub struct Mesh {
    pub primitives: Vec<Primitive>,
    pub mesh_id: usize,
    pub weights: Vec<f32>,
}

pub struct Primitive {
    pub number_of_indices: u32,
    pub first_index: u32,
    pub material_index: Option<usize>,
}

// TODO: Properly decouple the animation state from the asset as a component to make it reusable.
pub struct Animation {
    pub time: f32,
    channels: Vec<Channel>,
    max_animation_time: f32,
}

pub struct Channel {
    node_index: usize,
    inputs: Vec<f32>,
    transformations: TransformationSet,
    _interpolation: Interpolation,
    previous_key: usize,
    previous_time: f32,
}

pub struct GltfAsset {
    pub gltf: gltf::Document,
    pub textures: Vec<GltfTextureData>,
    pub scenes: Vec<Scene>,
    pub number_of_meshes: usize,
    pub buffers: ModelBuffers,
    pub animations: Vec<Animation>,
}

impl GltfAsset {
    pub fn new(renderer: &Renderer, asset_name: &str) -> GltfAsset {
        let (gltf, buffers, asset_textures) =
            gltf::import(&asset_name).expect("Couldn't import file!");

        let textures = asset_textures
            .iter()
            .map(|properties| GltfTextureData::new(&renderer, properties))
            .collect::<Vec<_>>();

        let animations = Self::prepare_animations(&gltf, &buffers);

        let (mut scenes, vertices, indices) = Self::prepare_scenes(&gltf, &buffers, &renderer);
        Self::update_ubo_indices(&mut scenes);

        let number_of_meshes = gltf.nodes().filter(|node| node.mesh().is_some()).count();

        let buffers =
            ModelBuffers::new(&renderer.transient_command_pool, &vertices, Some(&indices));
        GltfAsset {
            gltf,
            textures,
            scenes,
            number_of_meshes,
            buffers,
            animations,
        }
    }

    fn determine_transform(node: &gltf::Node) -> glm::Mat4 {
        let transform: Vec<f32> = node
            .transform()
            .matrix()
            .iter()
            .flat_map(|array| array.iter())
            .cloned()
            .collect();
        glm::make_mat4(&transform.as_slice())
    }

    fn prepare_scenes(
        gltf: &gltf::Document,
        buffers: &[gltf::buffer::Data],
        renderer: &Renderer,
    ) -> (Vec<Scene>, Vec<f32>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut scenes: Vec<Scene> = Vec::new();
        for scene in gltf.scenes() {
            let mut node_graphs: Vec<NodeGraph> = Vec::new();
            for node in scene.nodes() {
                let mut node_graph = NodeGraph::new();
                Self::visit_children(
                    &node,
                    &buffers,
                    &mut node_graph,
                    NodeIndex::new(0_usize),
                    &renderer,
                    &mut vertices,
                    &mut indices,
                );
                node_graphs.push(node_graph);
            }
            scenes.push(Scene { node_graphs });
        }
        (scenes, vertices, indices)
    }

    fn visit_children(
        node: &gltf::Node,
        buffers: &[gltf::buffer::Data],
        node_graph: &mut NodeGraph,
        parent_index: NodeIndex,
        renderer: &Renderer,
        vertices: &mut Vec<f32>,
        indices: &mut Vec<u32>,
    ) {
        let mesh = Self::load_mesh(node, buffers, vertices, indices);
        let node_info = Node {
            animation_transform: Transform::default(),
            local_transform: Self::determine_transform(node),
            mesh,
            index: node.index(),
        };

        let node_index = node_graph.add_node(node_info);
        if parent_index != node_index {
            node_graph.add_edge(parent_index, node_index, ());
        }

        for child in node.children() {
            Self::visit_children(
                &child, buffers, node_graph, node_index, renderer, vertices, indices,
            );
        }
    }

    fn load_mesh(
        node: &gltf::Node,
        buffers: &[gltf::buffer::Data],
        vertices: &mut Vec<f32>,
        indices: &mut Vec<u32>,
    ) -> Option<Mesh> {
        if let Some(mesh) = node.mesh() {
            let mut all_mesh_primitives = Vec::new();
            for primitive in mesh.primitives() {
                // Position (3), Normal (3), TexCoords_0 (2)
                let stride = 8 * std::mem::size_of::<f32>();
                let vertex_list_size = vertices.len() * std::mem::size_of::<u32>();
                let vertex_count = (vertex_list_size / stride) as u32;

                // Start reading primitive data
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                let positions = reader
                    .read_positions()
                    .expect("Failed to read any vertex positions from the model. Vertex positions are required.")
                    .map(glm::Vec3::from)
                    .collect::<Vec<_>>();

                let normals = reader
                    .read_normals()
                    .map_or(vec![glm::vec3(0.0, 0.0, 0.0); positions.len()], |normals| {
                        normals.map(glm::Vec3::from).collect::<Vec<_>>()
                    });

                let convert_coords =
                    |coords: gltf::mesh::util::ReadTexCoords<'_>| -> Vec<glm::Vec2> {
                        coords.into_f32().map(glm::Vec2::from).collect::<Vec<_>>()
                    };
                let tex_coords_0 = reader
                    .read_tex_coords(0)
                    .map_or(vec![glm::vec2(0.0, 0.0); positions.len()], convert_coords);

                // TODO: Add checks to see if normals and tex_coords are even available
                for ((position, normal), tex_coord_0) in positions
                    .iter()
                    .zip(normals.iter())
                    .zip(tex_coords_0.iter())
                {
                    vertices.extend_from_slice(position.as_slice());
                    vertices.extend_from_slice(normal.as_slice());
                    vertices.extend_from_slice(tex_coord_0.as_slice());
                }

                let first_index = indices.len() as u32;

                let primitive_indices = reader
                    .read_indices()
                    .map(|read_indices| {
                        read_indices
                            .into_u32()
                            .map(|x| x + vertex_count)
                            .collect::<Vec<_>>()
                    })
                    .expect("Failed to read indices!");
                indices.extend_from_slice(&primitive_indices);

                let number_of_indices = primitive_indices.len() as u32;

                let morph_targets = reader
                    .read_morph_targets()
                    .map(
                        |(position_displacements, normal_displacements, tangent_displacements)| {
                            let positions = if let Some(displacements) = position_displacements {
                                displacements.map(glm::Vec3::from).collect::<Vec<_>>()
                            } else {
                                Vec::new()
                            };

                            let normals = if let Some(displacements) = normal_displacements {
                                displacements.map(glm::Vec3::from).collect::<Vec<_>>()
                            } else {
                                Vec::new()
                            };

                            let tangents = if let Some(displacements) = tangent_displacements {
                                displacements.map(glm::Vec3::from).collect::<Vec<_>>()
                            } else {
                                Vec::new()
                            };

                            MorphTargets {
                                positions,
                                normals,
                                tangents,
                            }
                        },
                    )
                    .collect::<Vec<_>>();

                all_mesh_primitives.push(Primitive {
                    first_index,
                    number_of_indices,
                    material_index: primitive.material().index(),
                });
            }

            let weights = if let Some(weights) = mesh.weights() {
                weights.to_vec()
            } else {
                Vec::new()
            };

            Some(Mesh {
                weights,
                primitives: all_mesh_primitives,
                mesh_id: 0,
            })
        } else {
            None
        }
    }

    fn update_ubo_indices(scenes: &mut Vec<Scene>) {
        let mut indices = Vec::new();
        for (scene_index, scene) in scenes.iter().enumerate() {
            for (graph_index, graph) in scene.node_graphs.iter().enumerate() {
                let mut dfs = Dfs::new(&graph, NodeIndex::new(0));
                while let Some(node_index) = dfs.next(&graph) {
                    if graph[node_index].mesh.is_some() {
                        indices.push((scene_index, graph_index, node_index));
                    }
                }
            }
        }

        for (mesh_id, (scene_index, graph_index, node_index)) in indices.into_iter().enumerate() {
            scenes[scene_index].node_graphs[graph_index][node_index]
                .mesh
                .as_mut()
                .expect("Failed to get mesh!")
                .mesh_id = mesh_id;
        }
    }

    // TODO: Write this method for vec3's and vec4's
    // fn interpolate(interpolation: Interpolation) {
    //     match interpolation {
    //         Interpolation::Linear => {}
    //         Interpolation::Step => {}
    //         Interpolation::CatmullRomSpline => {}
    //         Interpolation::CubicSpline => {}
    //     }
    // }

    fn prepare_animations(gltf: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<Animation> {
        // TODO: load names if present as well
        let mut animations = Vec::new();
        for animation in gltf.animations() {
            let mut channels = Vec::new();
            for channel in animation.channels() {
                let sampler = channel.sampler();
                let _interpolation = sampler.interpolation();
                let node_index = channel.target().node().index();
                let reader = channel.reader(|buffer| Some(&buffers[buffer.index()]));
                let inputs = reader.read_inputs().unwrap().collect::<Vec<_>>();
                let outputs = reader.read_outputs().unwrap();
                let transformations: TransformationSet;
                match outputs {
                    ReadOutputs::Translations(translations) => {
                        let translations = translations.map(glm::Vec3::from).collect::<Vec<_>>();
                        transformations = TransformationSet::Translations(translations);
                    }
                    ReadOutputs::Rotations(rotations) => {
                        let rotations = rotations
                            .into_f32()
                            .map(glm::Vec4::from)
                            .collect::<Vec<_>>();
                        transformations = TransformationSet::Rotations(rotations);
                    }
                    ReadOutputs::Scales(scales) => {
                        let scales = scales.map(glm::Vec3::from).collect::<Vec<_>>();
                        transformations = TransformationSet::Scales(scales);
                    }
                    ReadOutputs::MorphTargetWeights(weights) => {
                        let morph_target_weights = weights.into_f32().collect::<Vec<_>>();
                        transformations =
                            TransformationSet::MorphTargetWeights(morph_target_weights);
                    }
                }
                channels.push(Channel {
                    node_index,
                    inputs,
                    transformations,
                    _interpolation,
                    previous_key: 0,
                    previous_time: 0.0,
                });
            }

            let max_animation_time = channels
                .iter()
                .flat_map(|channel| channel.inputs.iter().copied())
                .fold(0.0, f32::max);

            animations.push(Animation {
                channels,
                time: 0.0,
                max_animation_time,
            });
        }
        animations
    }

    pub fn animate(&mut self) {
        // TODO: Allow for specifying a specific animation by name
        for animation in self.animations.iter_mut() {
            if animation.time > animation.max_animation_time {
                animation.time = 0.0;
            }
            if animation.time < 0.0 {
                animation.time = animation.max_animation_time;
            }
            for channel in animation.channels.iter_mut() {
                for scene in self.scenes.iter_mut() {
                    for graph in scene.node_graphs.iter_mut() {
                        for node_index in graph.node_indices() {
                            if graph[node_index].index == channel.node_index {
                                let max = *channel.inputs.last().unwrap();
                                let mut time = animation.time % max;
                                let first_input = channel.inputs.first().unwrap();
                                if time.lt(first_input) {
                                    time = *first_input;
                                }

                                if channel.previous_time > time {
                                    channel.previous_key = 0;
                                }
                                channel.previous_time = time;

                                let mut next_key: usize = 0;
                                for index in channel.previous_key..channel.inputs.len() {
                                    let index = index as usize;
                                    if time <= channel.inputs[index] {
                                        next_key =
                                            nalgebra::clamp(index, 1, channel.inputs.len() - 1);
                                        break;
                                    }
                                }
                                channel.previous_key = nalgebra::clamp(next_key - 1, 0, next_key);

                                let key_delta =
                                    channel.inputs[next_key] - channel.inputs[channel.previous_key];
                                let normalized_time =
                                    (time - channel.inputs[channel.previous_key]) / key_delta;

                                // TODO: Interpolate with other methods
                                // Only Linear interpolation is used for now
                                match &channel.transformations {
                                    TransformationSet::Translations(translations) => {
                                        let start = translations[channel.previous_key];
                                        let end = translations[next_key];
                                        let translation = start.lerp(&end, normalized_time);
                                        let translation_vec =
                                            glm::make_vec3(translation.as_slice());
                                        graph[node_index].animation_transform.translation =
                                            Some(translation_vec);
                                    }
                                    TransformationSet::Rotations(rotations) => {
                                        let start = rotations[channel.previous_key];
                                        let end = rotations[next_key];
                                        let start_quat =
                                            Quaternion::new(start[3], start[0], start[1], start[2]);
                                        let end_quat =
                                            Quaternion::new(end[3], end[0], end[1], end[2]);
                                        let rotation_quat =
                                            start_quat.lerp(&end_quat, normalized_time);
                                        graph[node_index].animation_transform.rotation =
                                            Some(rotation_quat);
                                    }
                                    TransformationSet::Scales(scales) => {
                                        let start = scales[channel.previous_key];
                                        let end = scales[next_key];
                                        let scale = start.lerp(&end, normalized_time);
                                        let scale_vec = glm::make_vec3(scale.as_slice());
                                        graph[node_index].animation_transform.scale =
                                            Some(scale_vec);
                                    }
                                    TransformationSet::MorphTargetWeights(weights) => {
                                        let start = weights[channel.previous_key];
                                        let end = weights[next_key];
                                        let weight = glm::lerp_scalar(start, end, normalized_time);
                                        // TODO: Assign the interpolated weight
                                    }
                                }

                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn path_between_nodes(
        starting_node_index: NodeIndex,
        node_index: NodeIndex,
        graph: &NodeGraph,
    ) -> Vec<NodeIndex> {
        let mut indices = Vec::new();
        let mut dfs = Dfs::new(&graph, starting_node_index);
        while let Some(current_node_index) = dfs.next(&graph) {
            let mut incoming_walker = graph
                .neighbors_directed(current_node_index, Incoming)
                .detach();
            let mut outgoing_walker = graph
                .neighbors_directed(current_node_index, Outgoing)
                .detach();

            if let Some(parent) = incoming_walker.next_node(&graph) {
                while let Some(last_index) = indices.last() {
                    if *last_index == parent {
                        break;
                    }
                    // Discard indices for transforms that are no longer needed
                    indices.pop();
                }
            }

            indices.push(current_node_index);

            if node_index == current_node_index {
                break;
            }

            // If the node has no children, don't store the index
            if outgoing_walker.next(&graph).is_none() {
                indices.pop();
            }
        }
        indices
    }

    pub fn calculate_global_transform(node_index: NodeIndex, graph: &NodeGraph) -> glm::Mat4 {
        let indices = Self::path_between_nodes(NodeIndex::new(0), node_index, graph);
        indices
            .iter()
            .fold(glm::Mat4::identity(), |transform, index| {
                transform
                    * graph[*index].local_transform
                    * graph[*index].animation_transform.matrix()
            })
    }

    pub fn walk<F>(&self, action: F)
    where
        F: Fn(NodeIndex, &NodeGraph),
    {
        for scene in self.scenes.iter() {
            for graph in scene.node_graphs.iter() {
                let mut dfs = Dfs::new(&graph, NodeIndex::new(0));
                while let Some(node_index) = dfs.next(&graph) {
                    action(node_index, &graph);
                }
            }
        }
    }

    pub fn create_vertex_attributes() -> [vk::VertexInputAttributeDescription; 3] {
        let position_description = vk::VertexInputAttributeDescription::builder()
            .binding(0)
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0)
            .build();

        let normal_description = vk::VertexInputAttributeDescription::builder()
            .binding(0)
            .location(1)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset((3 * std::mem::size_of::<f32>()) as _)
            .build();

        let tex_coord_description = vk::VertexInputAttributeDescription::builder()
            .binding(0)
            .location(2)
            .format(vk::Format::R32G32_SFLOAT)
            .offset((6 * std::mem::size_of::<f32>()) as _)
            .build();

        [
            position_description,
            normal_description,
            tex_coord_description,
        ]
    }

    pub fn create_vertex_input_descriptions() -> [vk::VertexInputBindingDescription; 1] {
        let vertex_input_binding_description = vk::VertexInputBindingDescription::builder()
            .binding(0)
            .stride((8 * std::mem::size_of::<f32>()) as _)
            .input_rate(vk::VertexInputRate::VERTEX)
            .build();
        [vertex_input_binding_description]
    }
}

pub struct GltfTextureData {
    pub texture: Texture,
    pub view: ImageView,
    pub sampler: Sampler,
}

impl GltfTextureData {
    pub fn new(renderer: &Renderer, image_data: &gltf::image::Data) -> Self {
        let description = TextureDescription::from_gltf(&image_data);

        let texture = Self::create_texture(renderer.context.clone(), &description);

        Self::upload_texture_data(
            renderer.context.clone(),
            &renderer.command_pool,
            &texture,
            &description,
        );

        let view = Self::create_image_view(renderer.context.clone(), &texture, &description);

        let sampler = Self::create_sampler(renderer.context.clone(), description.mip_levels);

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
            context.clone(),
            texture.allocation_info().get_size() as _,
            vk::BufferUsageFlags::TRANSFER_SRC,
            vk_mem::MemoryUsage::CpuToGpu,
        );
        buffer.upload_to_buffer(&description.pixels, 0, std::mem::align_of::<u8>() as _);

        let barrier = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(texture.image())
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
            context.graphics_queue(),
            buffer.buffer(),
            texture.image(),
            &regions,
        );

        texture.generate_mipmaps(&command_pool, &description);
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
            .array_layers(1)
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
            .flags(vk::ImageCreateFlags::empty())
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
            .view_type(vk::ImageViewType::TYPE_2D)
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
                layer_count: 1,
            })
            .build();
        ImageView::new(context, create_info)
    }

    fn create_sampler(context: Arc<VulkanContext>, mip_levels: u32) -> Sampler {
        let sampler_info = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .compare_op(vk::CompareOp::ALWAYS)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .mip_lod_bias(0.0)
            .min_lod(0.0)
            .max_lod(mip_levels as _)
            .build();
        Sampler::new(context, sampler_info)
    }
}
