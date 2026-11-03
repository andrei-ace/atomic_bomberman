use serde::Serialize;

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Resources {
    pub cpu_threads: usize,
    pub available_memory_bytes: u64,
    pub gpu_budget_bytes: Option<u64>,
    pub max_workers: usize,
}

impl Resources {
    pub fn detect() -> Self {
        let cpu_threads = std::thread::available_parallelism().map_or(1, usize::from);
        let available_memory_bytes = available_memory().unwrap_or(2 * GIB);
        let gpu_budget_bytes = gpu_budget();
        Self::from_capacity(cpu_threads, available_memory_bytes, gpu_budget_bytes)
    }
    fn from_capacity(
        cpu_threads: usize,
        available_memory_bytes: u64,
        gpu_budget_bytes: Option<u64>,
    ) -> Self {
        // Leave CPU capacity for the UI/OS and RAM for the original cache and
        // temporary PNG/canvas buffers. Each helper keeps its own model/tiles.
        let cpu_limit = (cpu_threads.saturating_sub(2) / 3).clamp(1, 4);
        let reserve = (available_memory_bytes / 4).clamp(512 * 1024 * 1024, 2 * GIB);
        let ram_limit = (available_memory_bytes.saturating_sub(reserve) / GIB).max(1) as usize;
        // Unreported GPU memory is handled conservatively; an inference failure
        // tears down the pool before one bounded single-worker retry.
        let gpu_limit = gpu_budget_bytes.map_or(2, |bytes| {
            ((bytes / 4 * 3) / (512 * 1024 * 1024)).max(1) as usize
        });
        let max_workers = cpu_limit.min(ram_limit).min(gpu_limit).clamp(1, 4);
        Self {
            cpu_threads,
            available_memory_bytes,
            gpu_budget_bytes,
            max_workers,
        }
    }
}

pub(super) fn available_memory() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let available = system.available_memory();
    (available > 0).then_some(available)
}

#[cfg(target_os = "macos")]
fn gpu_budget() -> Option<u64> {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut Object;
    }
    objc::rc::autoreleasepool(|| unsafe {
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            return None;
        }
        let recommended: u64 = msg_send![device, recommendedMaxWorkingSetSize];
        let allocated: u64 = msg_send![device, currentAllocatedSize];
        let _: () = msg_send![device, release];
        (recommended > 0).then_some(recommended.saturating_sub(allocated))
    })
}
#[cfg(not(target_os = "macos"))]
fn gpu_budget() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workers_respect_cpu_ram_and_gpu_budgets() {
        assert_eq!(
            Resources::from_capacity(14, 24 * GIB, Some(32 * GIB)).max_workers,
            4
        );
        assert_eq!(
            Resources::from_capacity(4, 24 * GIB, Some(32 * GIB)).max_workers,
            1
        );
        assert_eq!(
            Resources::from_capacity(14, 1500 * 1024 * 1024, Some(32 * GIB)).max_workers,
            1
        );
        assert_eq!(
            Resources::from_capacity(14, 24 * GIB, Some(700 * 1024 * 1024)).max_workers,
            1
        );
        assert_eq!(Resources::from_capacity(32, 24 * GIB, None).max_workers, 2);
    }
}
