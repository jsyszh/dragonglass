use crate::core::Instance;
use ash::{version::DeviceV1_0, vk};
use std::{ffi::CStr, sync::Arc};

// TODO: Add snafu errors

pub struct Shader {
    instance: Arc<Instance>,
    module: vk::ShaderModule,
    state_info: vk::PipelineShaderStageCreateInfo,
}

impl Shader {
    // TODO: Refactor this to have less parameters
    pub fn from_file(
        instance: Arc<Instance>,
        path: &str,
        flags: vk::ShaderStageFlags,
        entry_point_name: &CStr,
    ) -> Self {
        let mut shader_file = std::fs::File::open(path).expect("Failed to find shader file path");
        let shader_source = ash::util::read_spv(&mut shader_file)
            .expect("Failed to read SPIR-V shader source from bytes");
        let shader_create_info = vk::ShaderModuleCreateInfo::builder()
            .code(&shader_source)
            .build();
        let module = unsafe {
            instance
                .logical_device()
                .logical_device()
                .create_shader_module(&shader_create_info, None)
                .expect("Failed to create shader module")
        };
        let state_info = Self::create_state_info(module, flags, entry_point_name);
        Shader {
            module,
            instance,
            state_info,
        }
    }

    pub fn state_info(&self) -> vk::PipelineShaderStageCreateInfo {
        self.state_info
    }

    fn create_state_info(
        module: vk::ShaderModule,
        flags: vk::ShaderStageFlags,
        entry_point_name: &CStr,
    ) -> vk::PipelineShaderStageCreateInfo {
        vk::PipelineShaderStageCreateInfo::builder()
            .stage(flags)
            .module(module)
            .name(entry_point_name)
            .build()
    }
}

impl Drop for Shader {
    fn drop(&mut self) {
        unsafe {
            self.instance
                .logical_device()
                .logical_device()
                .destroy_shader_module(self.module, None);
        }
    }
}
