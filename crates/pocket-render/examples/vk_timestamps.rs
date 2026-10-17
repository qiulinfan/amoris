//! A raw Vulkan reproduction of the timestamp readings of docs/bench/gpu-matrix.md 3 a, without
//! wgpu: through ash, the command sequence wgpu-hal 30 records for a timestamp pair resolved and
//! read back in the command buffer that sampled it, and variants of it. Each round takes a fresh
//! query pool and records, in one primary command buffer:
//!
//! ```text
//! vkCmdResetQueryPool(pool, 0, 2)
//! vkCmdWriteTimestamp(BOTTOM_OF_PIPE, pool, 0)
//! vkCmdWriteTimestamp(BOTTOM_OF_PIPE, pool, 1)
//! vkCmdCopyQueryPoolResults(pool, 0, 2, resolve, 0, 8, TYPE_64 | WAIT)
//! vkCmdPipelineBarrier(TRANSFER -> TRANSFER, resolve: TRANSFER_WRITE -> TRANSFER_READ)
//! vkCmdCopyBuffer(resolve -> readback, 16 bytes)
//! vkCmdPipelineBarrier(TRANSFER -> HOST, readback: TRANSFER_WRITE -> HOST_READ)
//! ```
//!
//! then submits it with a fence, waits for the fence and reads the readback buffer (host-visible,
//! coherent). The resolve buffer is device-local and kept from round to round, as a suballocator
//! would hand the same memory out again, so a copy that misses the query results shows as the
//! previous round's pair; the readback buffer is filled with a sentinel by the host before each
//! submission, so a copy that misses the host read shows as the sentinel.
//!
//! Modes (`--modes`, comma-separated; by default every mode but the two controls, in the order
//! same, deferred, barrier, split, global, global-transfer, buffer-all, buffer-compute, direct,
//! host, fill, buffer-src-all, buffer-src-compute):
//! - `same`: the sequence above (wgpu's immediate resolve).
//! - `deferred`: the reset and the two timestamps in one submission, waited for; the copies in a
//!   second (the engine's resolve on AMD's Vulkan since 2026-10-15).
//! - `barrier`: `same` with a global barrier (ALL_COMMANDS, MEMORY_WRITE -> TRANSFER,
//!   TRANSFER_READ | TRANSFER_WRITE) between the timestamps and the query copy, which the
//!   specification does not require.
//! - `split`: `same` with the barrier and the buffer copy in a second command buffer of the same
//!   submission.
//! - `global`, `global-transfer`: `same` with the barrier between the copies replaced by a global
//!   memory barrier, ALL_COMMANDS to ALL_COMMANDS (MEMORY_WRITE -> MEMORY_READ | MEMORY_WRITE), or
//!   with wgpu's stages and accesses (TRANSFER, TRANSFER_WRITE -> TRANSFER, TRANSFER_READ).
//! - `buffer-all`, `buffer-compute`: `same` with the barrier between the copies given ALL_COMMANDS
//!   as both of its stages, or COMPUTE_SHADER and SHADER_WRITE added to its source.
//! - `buffer-src-all`, `buffer-src-compute`: only the barrier's source stage changed, to
//!   ALL_COMMANDS or to TRANSFER | COMPUTE_SHADER (accesses and destination as in `same`).
//! - `direct`: `same`, copying the query results straight into the readback buffer.
//! - `host`: the reset and the timestamps submitted and waited for, then
//!   vkGetQueryPoolResults(TYPE_64 | WAIT) on the host.
//! - `fill`: vkCmdFillBuffer (a value per round) in place of the query copy, then `same`'s barrier
//!   and buffer copy.
//! - `unreset` and `nobarrier`, the validation layer's positive controls (not run by default):
//!   `same` without the query reset (a core error), and without the barrier between the two copies
//!   (a synchronization hazard).
//!
//! After each round's fence the host reads the two queries itself (vkGetQueryPoolResults,
//! TYPE_64 | WAIT; `fill`'s reference is its value). A reading is wrong when it differs from that
//! reference or is the sentinel; a wrong reading is counted as zero, as the previous round's pair
//! (what the resolve buffer held before the submission: the last host read of a round that wrote
//! it), or as other. The host reads themselves are checked to be nonzero and increasing. `unreset`
//! is not checked. `--adapter NAME` picks a device by a case-insensitive part of its name;
//! `--rounds N` (default 200). Run it under the Khronos validation layer with synchronization
//! validation (tools/vk_validate.py) to check the sequence (docs/bench/gpu-matrix.md 8).

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use ash::vk;

    const SENTINEL: u64 = 0xBBBB_BBBB_BBBB_BBBB;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Mode {
        Same,
        Deferred,
        Barrier,
        Direct,
        Host,
        Split,
        Global,
        GlobalTransfer,
        BufferAll,
        BufferCompute,
        BufferSrcAll,
        BufferSrcCompute,
        Fill,
        Unreset,
        NoBarrier,
    }

    /// Every mode but the validation layer's two controls.
    const DEFAULT_MODES: &str = "same,deferred,barrier,split,global,global-transfer,buffer-all,\
                                 buffer-compute,direct,host,fill,buffer-src-all,buffer-src-compute";

    #[derive(Clone, Copy)]
    enum Source {
        Query(vk::QueryPool),
        Fill(u32),
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Between {
        /// The buffer barrier wgpu records (TRANSFER, TRANSFER_WRITE -> TRANSFER, TRANSFER_READ).
        Buffer,
        /// A global barrier (ALL_COMMANDS, MEMORY_WRITE -> ALL_COMMANDS, MEMORY_READ | WRITE).
        Global,
        /// A global barrier with wgpu's stages and accesses (TRANSFER, TRANSFER_WRITE ->
        /// TRANSFER, TRANSFER_READ).
        GlobalTransfer,
        /// The buffer barrier with ALL_COMMANDS for both stages.
        BufferAll,
        /// The buffer barrier with COMPUTE_SHADER and SHADER_WRITE added to its source.
        BufferCompute,
        /// The buffer barrier with ALL_COMMANDS as its source stage only.
        BufferSrcAll,
        /// The buffer barrier with COMPUTE_SHADER added to its source stage only.
        BufferSrcCompute,
        /// The buffer barrier, at the start of a second command buffer of the same submission.
        Split,
        /// No second copy: the first writes the readback buffer.
        Direct,
        /// No barrier (the synchronization control).
        Nothing,
    }

    impl Mode {
        fn parse(s: &str) -> Mode {
            match s {
                "same" => Mode::Same,
                "deferred" => Mode::Deferred,
                "barrier" => Mode::Barrier,
                "direct" => Mode::Direct,
                "host" => Mode::Host,
                "split" => Mode::Split,
                "global" => Mode::Global,
                "global-transfer" => Mode::GlobalTransfer,
                "buffer-all" => Mode::BufferAll,
                "buffer-compute" => Mode::BufferCompute,
                "buffer-src-all" => Mode::BufferSrcAll,
                "buffer-src-compute" => Mode::BufferSrcCompute,
                "fill" => Mode::Fill,
                "unreset" => Mode::Unreset,
                "nobarrier" => Mode::NoBarrier,
                _ => panic!("unknown mode {s}: {DEFAULT_MODES},unreset,nobarrier"),
            }
        }
    }

    struct Buffer {
        raw: vk::Buffer,
        memory: vk::DeviceMemory,
        mapped: *mut u64,
    }

    struct Context {
        _entry: ash::Entry,
        instance: ash::Instance,
        device: ash::Device,
        queue: vk::Queue,
        pool: vk::CommandPool,
        cb: [vk::CommandBuffer; 2],
        fence: vk::Fence,
        resolve: Buffer,
        readback: Buffer,
    }

    fn memory_type(
        props: &vk::PhysicalDeviceMemoryProperties,
        bits: u32,
        want: vk::MemoryPropertyFlags,
        avoid: vk::MemoryPropertyFlags,
    ) -> u32 {
        let types = props.memory_types_as_slice();
        let ok = |i: usize, avoid: vk::MemoryPropertyFlags| {
            bits & (1 << i) != 0
                && types[i].property_flags.contains(want)
                && !types[i].property_flags.intersects(avoid)
        };
        (0..types.len())
            .find(|&i| ok(i, avoid))
            .or_else(|| (0..types.len()).find(|&i| ok(i, vk::MemoryPropertyFlags::empty())))
            .expect("a memory type") as u32
    }

    impl Context {
        unsafe fn new(adapter: Option<&str>) -> Context {
            unsafe {
                let entry = ash::Entry::load().expect("the Vulkan loader");
                let app = vk::ApplicationInfo::default()
                    .application_name(c"vk_timestamps")
                    .api_version(vk::API_VERSION_1_2);
                let instance = entry
                    .create_instance(
                        &vk::InstanceCreateInfo::default().application_info(&app),
                        None,
                    )
                    .expect("an instance");
                let devices = instance.enumerate_physical_devices().expect("devices");
                let named: Vec<(vk::PhysicalDevice, String)> = devices
                    .iter()
                    .map(|&pd| {
                        let p = instance.get_physical_device_properties(pd);
                        (
                            pd,
                            p.device_name_as_c_str()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                        )
                    })
                    .collect();
                let (pd, name) = named
                    .iter()
                    .find(|(_, n)| {
                        adapter.is_none_or(|a| n.to_lowercase().contains(&a.to_lowercase()))
                    })
                    .unwrap_or_else(|| panic!("no device matches {adapter:?}; have {named:?}"))
                    .clone();
                let mut driver = vk::PhysicalDeviceDriverProperties::default();
                let (period, api) = {
                    let mut props2 =
                        vk::PhysicalDeviceProperties2::default().push_next(&mut driver);
                    instance.get_physical_device_properties2(pd, &mut props2);
                    (
                        props2.properties.limits.timestamp_period,
                        props2.properties.api_version,
                    )
                };
                println!(
                    "device {name}, driver {} {}, API {}.{}.{}, timestamp period {period} ns",
                    driver.driver_name_as_c_str().unwrap().to_string_lossy(),
                    driver.driver_info_as_c_str().unwrap().to_string_lossy(),
                    vk::api_version_major(api),
                    vk::api_version_minor(api),
                    vk::api_version_patch(api),
                );
                // wgpu takes the first family with graphics and compute.
                let families = instance.get_physical_device_queue_family_properties(pd);
                let family = families
                    .iter()
                    .position(|f| {
                        f.queue_flags
                            .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                    })
                    .expect("a graphics queue") as u32;
                println!(
                    "queue family {family}, timestamp valid bits {}",
                    families[family as usize].timestamp_valid_bits
                );
                let priorities = [1.0f32];
                let queue_info = vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&priorities);
                let device = instance
                    .create_device(
                        pd,
                        &vk::DeviceCreateInfo::default()
                            .queue_create_infos(std::slice::from_ref(&queue_info)),
                        None,
                    )
                    .expect("a device");
                let queue = device.get_device_queue(family, 0);
                let pool = device
                    .create_command_pool(
                        &vk::CommandPoolCreateInfo::default()
                            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                            .queue_family_index(family),
                        None,
                    )
                    .unwrap();
                let cbs = device
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(2),
                    )
                    .unwrap();
                let fence = device
                    .create_fence(&vk::FenceCreateInfo::default(), None)
                    .unwrap();
                let mem = instance.get_physical_device_memory_properties(pd);
                let usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
                let make = |want, avoid, map: bool| {
                    let raw = device
                        .create_buffer(&vk::BufferCreateInfo::default().size(16).usage(usage), None)
                        .unwrap();
                    let req = device.get_buffer_memory_requirements(raw);
                    let index = memory_type(&mem, req.memory_type_bits, want, avoid);
                    println!(
                        "buffer memory type {index}: {:?}",
                        mem.memory_types_as_slice()[index as usize].property_flags
                    );
                    let memory = device
                        .allocate_memory(
                            &vk::MemoryAllocateInfo::default()
                                .allocation_size(req.size)
                                .memory_type_index(index),
                            None,
                        )
                        .unwrap();
                    device.bind_buffer_memory(raw, memory, 0).unwrap();
                    let mapped = if map {
                        device
                            .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                            .unwrap()
                            .cast::<u64>()
                    } else {
                        std::ptr::null_mut()
                    };
                    Buffer {
                        raw,
                        memory,
                        mapped,
                    }
                };
                let resolve = make(
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    vk::MemoryPropertyFlags::HOST_VISIBLE,
                    false,
                );
                let readback = make(
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    true,
                );
                Context {
                    _entry: entry,
                    instance,
                    device,
                    queue,
                    pool,
                    cb: [cbs[0], cbs[1]],
                    fence,
                    resolve,
                    readback,
                }
            }
        }

        unsafe fn barrier(
            &self,
            cb: vk::CommandBuffer,
            buffer: vk::Buffer,
            (src_stage, src_access): (vk::PipelineStageFlags, vk::AccessFlags),
            (dst_stage, dst_access): (vk::PipelineStageFlags, vk::AccessFlags),
        ) {
            let b = vk::BufferMemoryBarrier::default()
                .src_access_mask(src_access)
                .dst_access_mask(dst_access)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(buffer)
                .size(vk::WHOLE_SIZE);
            unsafe {
                self.device.cmd_pipeline_barrier(
                    cb,
                    src_stage,
                    dst_stage,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[b],
                    &[],
                )
            };
        }

        unsafe fn submit(&self, cb: vk::CommandBuffer) {
            unsafe {
                let d = &self.device;
                d.end_command_buffer(cb).unwrap();
                let info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cb));
                d.queue_submit(self.queue, &[info], self.fence).unwrap();
                d.wait_for_fences(&[self.fence], true, u64::MAX).unwrap();
                d.reset_fences(&[self.fence]).unwrap();
            }
        }

        unsafe fn begin(&self, cb: vk::CommandBuffer) {
            unsafe {
                self.device
                    .reset_command_buffer(cb, vk::CommandBufferResetFlags::empty())
                    .unwrap();
                self.device
                    .begin_command_buffer(
                        cb,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                    .unwrap();
            }
        }

        /// The copies: query results to the resolve buffer (or the readback buffer), then to the
        /// readback buffer, then a barrier to the host. `between` separates the two copies;
        /// [`Between::Split`] ends `cb` after the first and records the rest into `self.cb[1]`,
        /// returning it.
        unsafe fn copies(
            &self,
            cb: vk::CommandBuffer,
            source: Source,
            between: Between,
        ) -> Option<vk::CommandBuffer> {
            use vk::{AccessFlags as A, PipelineStageFlags as S};
            let d = &self.device;
            let flags = vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT;
            let mut cb = cb;
            let mut second = None;
            unsafe {
                let target = match between {
                    Between::Direct => self.readback.raw,
                    _ => self.resolve.raw,
                };
                match source {
                    Source::Query(q) => {
                        d.cmd_copy_query_pool_results(cb, q, 0, 2, target, 0, 8, flags)
                    }
                    Source::Fill(value) => d.cmd_fill_buffer(cb, target, 0, 16, value),
                }
                match between {
                    Between::Direct | Between::Nothing => {}
                    Between::Buffer => self.barrier(
                        cb,
                        self.resolve.raw,
                        (S::TRANSFER, A::TRANSFER_WRITE),
                        (S::TRANSFER, A::TRANSFER_READ),
                    ),
                    Between::Global | Between::GlobalTransfer => {
                        let (src, dst, from, to) = if between == Between::Global {
                            (
                                S::ALL_COMMANDS,
                                S::ALL_COMMANDS,
                                A::MEMORY_WRITE,
                                A::MEMORY_READ | A::MEMORY_WRITE,
                            )
                        } else {
                            (
                                S::TRANSFER,
                                S::TRANSFER,
                                A::TRANSFER_WRITE,
                                A::TRANSFER_READ,
                            )
                        };
                        let g = vk::MemoryBarrier::default()
                            .src_access_mask(from)
                            .dst_access_mask(to);
                        d.cmd_pipeline_barrier(
                            cb,
                            src,
                            dst,
                            vk::DependencyFlags::empty(),
                            &[g],
                            &[],
                            &[],
                        );
                    }
                    Between::BufferAll => self.barrier(
                        cb,
                        self.resolve.raw,
                        (S::ALL_COMMANDS, A::TRANSFER_WRITE),
                        (S::ALL_COMMANDS, A::TRANSFER_READ),
                    ),
                    Between::BufferCompute => self.barrier(
                        cb,
                        self.resolve.raw,
                        (
                            S::TRANSFER | S::COMPUTE_SHADER,
                            A::TRANSFER_WRITE | A::SHADER_WRITE,
                        ),
                        (S::TRANSFER, A::TRANSFER_READ),
                    ),
                    Between::BufferSrcAll => self.barrier(
                        cb,
                        self.resolve.raw,
                        (S::ALL_COMMANDS, A::TRANSFER_WRITE),
                        (S::TRANSFER, A::TRANSFER_READ),
                    ),
                    Between::BufferSrcCompute => self.barrier(
                        cb,
                        self.resolve.raw,
                        (S::TRANSFER | S::COMPUTE_SHADER, A::TRANSFER_WRITE),
                        (S::TRANSFER, A::TRANSFER_READ),
                    ),
                    Between::Split => {
                        d.end_command_buffer(cb).unwrap();
                        cb = self.cb[1];
                        self.begin(cb);
                        second = Some(cb);
                        self.barrier(
                            cb,
                            self.resolve.raw,
                            (S::TRANSFER, A::TRANSFER_WRITE),
                            (S::TRANSFER, A::TRANSFER_READ),
                        );
                    }
                }
                if between != Between::Direct {
                    let region = vk::BufferCopy {
                        src_offset: 0,
                        dst_offset: 0,
                        size: 16,
                    };
                    d.cmd_copy_buffer(cb, self.resolve.raw, self.readback.raw, &[region]);
                }
                self.barrier(
                    cb,
                    self.readback.raw,
                    (S::TRANSFER, A::TRANSFER_WRITE),
                    (S::HOST, A::HOST_READ),
                );
            }
            second
        }

        /// Submits `first` (already ended) and `second` (still open) in one vkQueueSubmit.
        unsafe fn submit_two(&self, first: vk::CommandBuffer, second: vk::CommandBuffer) {
            unsafe {
                let d = &self.device;
                d.end_command_buffer(second).unwrap();
                let cbs = [first, second];
                let info = vk::SubmitInfo::default().command_buffers(&cbs);
                d.queue_submit(self.queue, &[info], self.fence).unwrap();
                d.wait_for_fences(&[self.fence], true, u64::MAX).unwrap();
                d.reset_fences(&[self.fence]).unwrap();
            }
        }

        /// Records the copies of `source` and submits them.
        unsafe fn copied(&self, cb: vk::CommandBuffer, source: Source, between: Between) {
            unsafe {
                match self.copies(cb, source, between) {
                    Some(second) => self.submit_two(cb, second),
                    None => self.submit(cb),
                }
            }
        }

        /// One round of `mode`; `fill` is the value [`Mode::Fill`] writes. Returns the reading
        /// and its reference: the host's own read of the queries after the fence, or the value
        /// [`Mode::Fill`] wrote (none for [`Mode::Unreset`]).
        unsafe fn round(&self, mode: Mode, fill: u32) -> ((u64, u64), Option<(u64, u64)>) {
            use vk::{AccessFlags as A, PipelineStageFlags as S};
            unsafe {
                let d = &self.device;
                let q = d
                    .create_query_pool(
                        &vk::QueryPoolCreateInfo::default()
                            .query_type(vk::QueryType::TIMESTAMP)
                            .query_count(2),
                        None,
                    )
                    .unwrap();
                std::ptr::write_volatile(self.readback.mapped, SENTINEL);
                std::ptr::write_volatile(self.readback.mapped.add(1), SENTINEL);
                let cb = self.cb[0];
                self.begin(cb);
                if mode != Mode::Fill {
                    if mode != Mode::Unreset {
                        d.cmd_reset_query_pool(cb, q, 0, 2);
                    }
                    d.cmd_write_timestamp(cb, S::BOTTOM_OF_PIPE, q, 0);
                    d.cmd_write_timestamp(cb, S::BOTTOM_OF_PIPE, q, 1);
                }
                let query = Source::Query(q);
                let mut host = None;
                match mode {
                    Mode::Same | Mode::Unreset => self.copied(cb, query, Between::Buffer),
                    Mode::NoBarrier => self.copied(cb, query, Between::Nothing),
                    Mode::Direct => self.copied(cb, query, Between::Direct),
                    Mode::Global => self.copied(cb, query, Between::Global),
                    Mode::GlobalTransfer => self.copied(cb, query, Between::GlobalTransfer),
                    Mode::BufferAll => self.copied(cb, query, Between::BufferAll),
                    Mode::BufferCompute => self.copied(cb, query, Between::BufferCompute),
                    Mode::BufferSrcAll => self.copied(cb, query, Between::BufferSrcAll),
                    Mode::BufferSrcCompute => self.copied(cb, query, Between::BufferSrcCompute),
                    Mode::Split => self.copied(cb, query, Between::Split),
                    Mode::Fill => self.copied(cb, Source::Fill(fill), Between::Buffer),
                    Mode::Barrier => {
                        let g = vk::MemoryBarrier::default()
                            .src_access_mask(A::MEMORY_WRITE)
                            .dst_access_mask(A::TRANSFER_READ | A::TRANSFER_WRITE);
                        d.cmd_pipeline_barrier(
                            cb,
                            S::ALL_COMMANDS,
                            S::TRANSFER,
                            vk::DependencyFlags::empty(),
                            &[g],
                            &[],
                            &[],
                        );
                        self.copied(cb, query, Between::Buffer);
                    }
                    Mode::Deferred => {
                        self.submit(cb);
                        let cb = self.cb[1];
                        self.begin(cb);
                        self.copied(cb, query, Between::Buffer);
                    }
                    Mode::Host => {
                        self.submit(cb);
                        host = Some(self.host_read(q));
                    }
                }
                let reference = match mode {
                    Mode::Fill => {
                        let v = (u64::from(fill) << 32) | u64::from(fill);
                        Some((v, v))
                    }
                    Mode::Unreset => None,
                    _ => Some(self.host_read(q)),
                };
                d.destroy_query_pool(q, None);
                let reading = host.unwrap_or_else(|| {
                    (
                        std::ptr::read_volatile(self.readback.mapped),
                        std::ptr::read_volatile(self.readback.mapped.add(1)),
                    )
                });
                (reading, reference)
            }
        }

        /// The two queries read by the host (TYPE_64 | WAIT), after the fence.
        unsafe fn host_read(&self, q: vk::QueryPool) -> (u64, u64) {
            let mut data = [0u64; 2];
            unsafe {
                self.device
                    .get_query_pool_results(
                        q,
                        0,
                        &mut data,
                        vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                    )
                    .unwrap();
            }
            (data[0], data[1])
        }
    }

    impl Drop for Context {
        fn drop(&mut self) {
            unsafe {
                let d = &self.device;
                d.device_wait_idle().unwrap();
                for b in [&self.resolve, &self.readback] {
                    d.destroy_buffer(b.raw, None);
                    d.free_memory(b.memory, None);
                }
                d.destroy_fence(self.fence, None);
                d.destroy_command_pool(self.pool, None);
                d.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let value = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        let adapter = value("--adapter");
        let rounds: usize = value("--rounds").map_or(200, |r| r.parse().unwrap());
        let modes: Vec<Mode> = value("--modes")
            .unwrap_or_else(|| DEFAULT_MODES.into())
            .split(',')
            .map(Mode::parse)
            .collect();
        let ctx = unsafe { Context::new(adapter.as_deref()) };
        // What the resolve buffer holds before a round: the reference of the last round that
        // wrote it (its query copy has landed by the fence even when the buffer copy missed it).
        let mut resolve_holds: Option<(u64, u64)> = None;
        let mut last_end = 0u64;
        for mode in modes {
            let (mut zero, mut previous, mut other, mut sentinel) = (0, 0, 0, 0);
            let (mut unchecked, mut bad_reference) = (0, 0);
            let mut shown = 0;
            for round in 0..rounds {
                // Fill: a value per round, never zero.
                let fill = round as u32 + 1;
                let (reading, reference) = unsafe { ctx.round(mode, fill) };
                let (a, b) = reading;
                let kind = match reference {
                    _ if a == SENTINEL || b == SENTINEL => {
                        sentinel += 1;
                        "sentinel"
                    }
                    None => {
                        unchecked += 1;
                        ""
                    }
                    Some(r) if r == reading => "",
                    Some(_) if a == 0 || b == 0 => {
                        zero += 1;
                        "zero"
                    }
                    Some(_) if Some(reading) == resolve_holds => {
                        previous += 1;
                        "previous pair"
                    }
                    Some(_) => {
                        other += 1;
                        "other"
                    }
                };
                if !kind.is_empty() && shown < 5 {
                    shown += 1;
                    println!(
                        "  {mode:?} round {round}: {kind} ({a}, {b}), host read {reference:?}, \
                         resolve buffer held {resolve_holds:?}"
                    );
                }
                if mode != Mode::Fill
                    && let Some((ra, rb)) = reference
                {
                    if ra == 0 || rb < ra || ra <= last_end {
                        bad_reference += 1;
                        println!(
                            "  {mode:?} round {round}: host read ({ra}, {rb}) is zero or out of \
                             order (previous end {last_end})"
                        );
                    }
                    last_end = last_end.max(rb);
                }
                if !matches!(mode, Mode::Direct | Mode::Host) {
                    resolve_holds = reference;
                }
            }
            let wrong = zero + previous + other + sentinel;
            println!(
                "mode {mode:?}: {wrong} of {rounds} wrong ({zero} zero, {previous} previous pair, \
                 {other} other, {sentinel} sentinel); {unchecked} unchecked, {bad_reference} host \
                 reads out of order"
            );
        }
    }
}
