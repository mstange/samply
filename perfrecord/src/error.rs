use crate::kernel_error::KernelError;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum SamplingError {
    #[error("Fatal error encountered during sampling: {0}, {1}")]
    Fatal(&'static str, KernelError),

    #[error("Ignorable error encountered during sampling: {0}, {1}")]
    Ignorable(&'static str, KernelError),

    #[error("The target thread has probably been terminated. {0}, {1}")]
    ThreadTerminated(&'static str, KernelError),

    #[error("The target process has probably been terminated. {0}, {1}")]
    ProcessTerminated(&'static str, KernelError),

    #[error("Could not obtain root task.")]
    CouldNotObtainRootTask,
}
