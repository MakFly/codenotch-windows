//! The computer's resource use, measured the way Task Manager's Performance tab shows it:
//! CPU busy share since the previous sample, physical memory in use, and the busiest GPU
//! engine when the machine exposes GPU counters.

pub struct Sampler {
    last_cpu: CpuTimes,
    gpu: Option<gpu::Query>,
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    /// Percent of the whole machine, like Task Manager's CPU graph.
    pub cpu: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    /// `None` when there is no GPU, or Windows publishes no counters for it.
    pub gpu: Option<f32>,
}

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    idle: u64,
    total: u64,
}

impl Sampler {
    pub fn new() -> Self {
        Self { last_cpu: cpu_times(), gpu: gpu::Query::open() }
    }

    pub fn sample(&mut self) -> Usage {
        let cpu = cpu_times();
        let total = cpu.total.saturating_sub(self.last_cpu.total);
        let idle = cpu.idle.saturating_sub(self.last_cpu.idle);
        self.last_cpu = cpu;
        let (memory_used, memory_total) = memory();
        Usage {
            cpu: if total == 0 { 0. } else { (total - idle.min(total)) as f32 / total as f32 * 100. },
            memory_used,
            memory_total,
            gpu: self.gpu.as_mut().and_then(|q| q.sample()),
        }
    }
}

/// Idle and idle-plus-busy time of all cores, in 100 ns units (kernel time includes idle).
#[cfg(windows)]
fn cpu_times() -> CpuTimes {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetSystemTimes};
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut idle, mut kernel, mut user) = (zero(), zero(), zero());
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    unsafe {
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
            return CpuTimes::default();
        }
    }
    CpuTimes { idle: ticks(idle), total: ticks(kernel) + ticks(user) }
}

/// Physical memory in use and installed, in bytes.
#[cfg(windows)]
fn memory() -> (u64, u64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return (0, 0);
        }
        (status.ullTotalPhys.saturating_sub(status.ullAvailPhys), status.ullTotalPhys)
    }
}

#[cfg(not(windows))]
fn cpu_times() -> CpuTimes {
    CpuTimes::default()
}

#[cfg(not(windows))]
fn memory() -> (u64, u64) {
    (0, 0)
}

#[cfg(windows)]
mod gpu {
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    };

    /// The "GPU Engine" counters Task Manager reads, one instance per process and engine.
    pub struct Query {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
    }

    impl Query {
        pub fn open() -> Option<Self> {
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
            unsafe {
                let mut query = std::ptr::null_mut();
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return None;
                }
                let mut counter = std::ptr::null_mut();
                if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return None;
                }
                // A rate counter: the first collection only sets the baseline.
                PdhCollectQueryData(query);
                Some(Self { query, counter })
            }
        }

        /// Like Task Manager: per engine, the sum over processes; then the busiest engine.
        pub fn sample(&mut self) -> Option<f32> {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let (mut size, mut count) = (0u32, 0u32);
                PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
                if size == 0 {
                    return None;
                }
                // u64 words keep the items aligned; the names live in the same buffer.
                let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, items) != 0 {
                    return None;
                }
                let mut engines: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
                for item in std::slice::from_raw_parts(items, count as usize) {
                    if item.FmtValue.CStatus != 0 {
                        continue;
                    }
                    let name = wide(item.szName);
                    // "pid_1234_luid_0x0_0xD1B5_phys_0_eng_0_engtype_3D": the engine is from luid on.
                    let engine = name.find("luid_").map_or(name.as_str(), |i| &name[i..]).to_owned();
                    *engines.entry(engine).or_default() += item.FmtValue.Anonymous.doubleValue;
                }
                // No engine at all: no GPU counters to show.
                let busiest = engines.into_values().reduce(f64::max)?;
                Some(busiest.clamp(0., 100.) as f32)
            }
        }
    }

    impl Drop for Query {
        fn drop(&mut self) {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    unsafe fn wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut len = 0;
        while unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

#[cfg(not(windows))]
mod gpu {
    pub struct Query;

    impl Query {
        pub fn open() -> Option<Self> {
            None
        }

        pub fn sample(&mut self) -> Option<f32> {
            None
        }
    }
}

/// "CPU 12 % · RAM 18,4/32 Go · GPU 7 %", in the interface language.
pub fn label(usage: Usage, french: bool) -> String {
    // Before the first sample, two seconds after the window opens.
    if usage.memory_total == 0 {
        return "…".into();
    }
    let gb = |bytes: u64| bytes as f64 / (1024. * 1024. * 1024.);
    let (used, total) = (gb(usage.memory_used), gb(usage.memory_total));
    let percent = |v: f32| format!("{v:.0}");
    let mut parts = vec![
        format!("CPU {}{}%", percent(usage.cpu), if french { " " } else { "" }),
        format!("RAM {used:.1}/{total:.0} {}", if french { "Go" } else { "GB" }),
    ];
    if let Some(gpu) = usage.gpu {
        parts.push(format!("GPU {}{}%", percent(gpu), if french { " " } else { "" }));
    }
    let text = parts.join(" · ");
    if french { text.replace('.', ",") } else { text }
}
