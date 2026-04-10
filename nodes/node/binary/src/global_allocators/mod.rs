// Replace the current global allocator with the DHAT heap profiler,
// regardless of what it is.
#[cfg(feature = "dhat-heap")]
pub mod dhat_heap;

// Replace the global allocator with jemalloc.
// If `dhat-heap` is enabled, this must not be applied.
#[cfg(all(
    feature = "jemalloc",
    not(feature = "dhat-heap"),
    not(target_env = "msvc"),
    // jemalloc supports ARM but users must change page size to 4KB.
    // We disable jemalloc for ARM until we verify that 4KB page size is okay.
    not(any(target_arch = "arm", target_arch = "aarch64"))
))]
pub mod jemalloc;
