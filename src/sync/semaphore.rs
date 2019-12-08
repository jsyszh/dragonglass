use crate::core::Instance;
use ash::{version::DeviceV1_0, vk};
use std::sync::Arc;

use snafu::{ResultExt, Snafu};

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(crate)")]
pub enum Error {
    #[snafu(display("Failed to create fence: {}", source))]
    SemaphoreCreation { source: vk::Result },
}

pub struct Semaphore {
    semaphore: vk::Semaphore,
    instance: Arc<Instance>,
}

impl Semaphore {
    pub fn new(instance: Arc<Instance>) -> Result<Self> {
        let semaphore_info = vk::SemaphoreCreateInfo::builder().build();
        let semaphore = unsafe {
            instance
                .logical_device()
                .logical_device()
                .create_semaphore(&semaphore_info, None)
                .context(SemaphoreCreation)?
        };
        Ok(Semaphore {
            semaphore,
            instance,
        })
    }

    pub fn semaphore(&self) -> vk::Semaphore {
        self.semaphore
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe {
            self.instance
                .logical_device()
                .logical_device()
                .destroy_semaphore(self.semaphore, None)
        }
    }
}
