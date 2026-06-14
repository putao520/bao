// @trace REQ-ENG-001
//! CPU profiler — sampling profiler backed by SM's GeckoProfiler.
//!
//! Phase 1: in-memory sample collection with atomic state.
//! Phase 2: integrate JS_SetProfilingCallbacks for .cpuprofile output.

use ::std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ::std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct CPUProfilerConfig {
    pub interval: u32,
    pub output_path: Option<::std::path::PathBuf>,
}

impl Default for CPUProfilerConfig {
    fn default() -> Self {
        CPUProfilerConfig { interval: 1000, output_path: None }
    }
}

pub struct BunCpuProfiler {
    running: AtomicBool,
    interval: AtomicU64,
    samples: Mutex<Vec<ProfileSample>>,
}

#[derive(Debug, Clone)]
pub struct ProfileSample {
    pub timestamp_us: u64,
    pub stack: Vec<String>,
}

impl BunCpuProfiler {
    pub fn get() -> &'static BunCpuProfiler {
        static INSTANCE: BunCpuProfiler = BunCpuProfiler {
            running: AtomicBool::new(false),
            interval: AtomicU64::new(1000),
            samples: Mutex::new(Vec::new()),
        };
        &INSTANCE
    }

    pub fn set_sampling_interval(interval: u32) {
        Self::get().interval.store(interval as u64, Ordering::Release);
    }

    pub fn start_cpu_profiler(_vm: *mut crate::VirtualMachine) -> Result<(), ()> {
        Self::get().running.store(true, Ordering::Release);
        Ok(())
    }

    pub fn stop_cpu_profiler() -> Option<String> {
        let profiler = Self::get();
        profiler.running.store(false, Ordering::Release);
        let samples = profiler.samples.lock().unwrap_or_else(|e| e.into_inner());
        if samples.is_empty() {
            None
        } else {
            Some(format!("{{\"samples\":{}}}", samples.len()))
        }
    }

    pub fn start(&self) -> Result<(), ()> {
        self.running.store(true, Ordering::Release);
        Ok(())
    }

    pub fn stop(&self) -> Option<String> {
        self.running.store(false, Ordering::Release);
        let samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        if samples.is_empty() {
            None
        } else {
            Some(format!("{{\"samples\":{}}}", samples.len()))
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn sample_count(&self) -> usize {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn record_sample(&self, stack: Vec<String>) {
        if !self.running.load(Ordering::Acquire) {
            return;
        }
        let sample = ProfileSample {
            timestamp_us: ::std::time::SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0),
            stack,
        };
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).push(sample);
    }

    pub fn clear_samples(&self) {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}
