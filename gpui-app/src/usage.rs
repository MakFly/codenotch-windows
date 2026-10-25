//! CodeNotch's own resource use, measured the way Task Manager shows it: CPU share across
//! all cores since the previous sample, and the private working set ("Memory" column).
use std::time::Instant;

pub struct Sampler {
    last_cpu: u64,
    last_at: Instant,
    cores: f32,
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    /// Percent of the whole machine, like Task Manager's CPU column.
    pub cpu: f32,
    pub memory: u64,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            last_cpu: cpu_time(),
            last_at: Instant::now(),
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()) as f32,
        }
    }

    pub fn sample(&mut self) -> Usage {
        let (cpu, now) = (cpu_time(), Instant::now());
        let busy = cpu.saturating_sub(self.last_cpu) as f32 / 10_000_000.;
        let elapsed = now.duration_since(self.last_at).as_secs_f32().max(0.001);
        self.last_cpu = cpu;
        self.last_at = now;
        Usage {
            cpu: (busy / elapsed / self.cores * 100.).clamp(0., 100.),
            memory: private_memory(),
        }
    }
}

/// Kernel plus user time of this process, in 100 ns units.
#[cfg(windows)]
fn cpu_time() -> u64 {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero(), zero(), zero(), zero());
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) == 0 {
            return 0;
        }
    }
    ticks(kernel) + ticks(user)
}

/// Private working set: the memory only this process uses, as Task Manager reports it.
#[cfg(windows)]
fn private_memory() -> u64 {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2},
        Threading::GetCurrentProcess,
    };
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS_EX2 = std::mem::zeroed();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        counters.cb = size;
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            size,
        ) == 0
        {
            return 0;
        }
        counters.PrivateWorkingSetSize as u64
    }
}

#[cfg(not(windows))]
fn cpu_time() -> u64 {
    0
}

#[cfg(not(windows))]
fn private_memory() -> u64 {
    0
}

/// "CPU 0,4 % · RAM 48 Mo", in the interface language.
pub fn label(usage: Usage, french: bool) -> String {
    let cpu = format!("{:.1}", usage.cpu);
    let mb = usage.memory as f64 / (1024. * 1024.);
    let ram = if mb >= 100. { format!("{mb:.0}") } else { format!("{mb:.1}") };
    if french {
        format!("CPU {} % · RAM {} Mo", cpu.replace('.', ","), ram.replace('.', ","))
    } else {
        format!("CPU {cpu}% · RAM {ram} MB")
    }
}
