//! Shared bench infrastructure: environment snapshot (issue #19 Phase A six
//! elements), statistics, machine-readable result emission
//! (`bench/schema/bench-result.schema.json`), and /proc probes.
//!
//! Fail-closed contract (bench/METHODOLOGY.md §6): any verification failure
//! returns Err → the driver prints a result document carrying `error` and the
//! process exits non-zero. No partial-success result may masquerade as a
//! complete baseline.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

// ─── Parameters ─────────────────────────────────────────────────────────────

pub struct Params {
    map: HashMap<String, String>,
}

impl Params {
    pub fn new(map: HashMap<String, String>) -> Self {
        Params { map }
    }
    pub fn str_of(&self, key: &str, default: &str) -> String {
        self.map.get(key).cloned().unwrap_or_else(|| default.to_string())
    }
    pub fn usize_of(&self, key: &str, default: usize) -> usize {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    pub fn u64_of(&self, key: &str, default: u64) -> u64 {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    pub fn f64_of(&self, key: &str, default: f64) -> f64 {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

// ─── Environment (six elements, auto-captured) ──────────────────────────────

#[derive(Serialize)]
pub struct Environment {
    pub host: String,
    pub kernel: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub mem_total_kib: u64,
    pub loadavg: [f64; 3],
    pub git_commit: String,
    pub git_dirty: bool,
    pub rustc: String,
    pub cargo_profile: String,
    pub display: Option<String>,
    pub date_utc: String,
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// ISO 8601 UTC timestamp without external crates (civil-from-days).
pub fn iso_utc_now() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn capture_environment() -> Environment {
    let cpu_model = read_trim("/proc/cpuinfo")
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        })
        .unwrap_or_else(|| "unknown".into());
    let kernel = read_trim("/proc/version").unwrap_or_else(|| "unknown".into());
    let mem_total_kib = read_trim("/proc/meminfo")
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
        })
        .unwrap_or(0);
    let loadavg = read_trim("/proc/loadavg")
        .map(|s| {
            let mut out = [0.0f64; 3];
            for (i, v) in s.split_whitespace().take(3).enumerate() {
                out[i] = v.parse().unwrap_or(0.0);
            }
            out
        })
        .unwrap_or([0.0; 3]);
    Environment {
        host: read_trim("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".into()),
        kernel,
        cpu_model,
        cpu_cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        mem_total_kib,
        loadavg,
        git_commit: env_or("BAO_BENCH_GIT_COMMIT", "unknown"),
        git_dirty: env_or("BAO_BENCH_GIT_DIRTY", "unknown") == "1",
        rustc: env_or("BAO_BENCH_RUSTC", "unknown"),
        cargo_profile: env_or("BAO_BENCH_PROFILE", "unknown"),
        display: std::env::var("DISPLAY").ok(),
        date_utc: iso_utc_now(),
    }
}

// ─── /proc self probes (RSS / fd / threads) ─────────────────────────────────

pub fn proc_status_field(field: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with(field))?.split_whitespace().nth(1)?.parse().ok()
}

pub fn vm_rss_kib() -> Option<u64> {
    proc_status_field("VmRSS:")
}
pub fn vm_hwm_kib() -> Option<u64> {
    proc_status_field("VmHWM:")
}
pub fn thread_count() -> Option<u64> {
    proc_status_field("Threads:")
}
pub fn fd_count() -> Option<u64> {
    std::fs::read_dir("/proc/self/fd").map(|d| d.count() as u64).ok()
}

// ─── Statistics (nearest-rank percentiles) ──────────────────────────────────

pub struct Stats {
    pub n: usize,
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub cv: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[idx.clamp(1, sorted.len()) - 1]
}

pub fn stats(samples: &[f64]) -> Stats {
    assert!(!samples.is_empty(), "stats over empty sample set");
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let var = samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f64>() / n as f64;
    Stats {
        n,
        min: sorted[0],
        p50: percentile(&sorted, 50.0),
        p90: percentile(&sorted, 90.0),
        p95: percentile(&sorted, 95.0),
        p99: percentile(&sorted, 99.0),
        max: sorted[n - 1],
        mean,
        cv: var.sqrt() / mean,
    }
}

// ─── Result document (schema-conformant) ────────────────────────────────────

#[derive(Serialize)]
pub struct Metric {
    pub name: String,
    pub unit: &'static str,
    pub kind: &'static str,
    pub higher_is_better: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'static str>,
    pub n: usize,
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub cv: f64,
    pub unstable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<f64>>,
}

/// cv gate (METHODOLOGY.md §5): per-call latency distributions are tail-heavy
/// by nature (SM GC pauses are honest latency, not noise), so they use a wider
/// structural-tail threshold; everything else uses the run-median 5% gate.
fn cv_gate(kind: &str, cv: f64) -> bool {
    if kind == "latency" { cv > 0.25 } else { cv > 0.05 }
}

impl Metric {
    pub fn from_samples(
        name: &str,
        unit: &'static str,
        kind: &'static str,
        higher_is_better: bool,
        phase: Option<&'static str>,
        samples: &[f64],
    ) -> Metric {
        let s = stats(samples);
        Metric {
            name: name.to_string(),
            unit,
            kind,
            higher_is_better,
            phase,
            n: s.n,
            min: s.min,
            p50: s.p50,
            p90: s.p90,
            p95: s.p95,
            p99: s.p99,
            max: s.max,
            mean: s.mean,
            cv: s.cv,
            unstable: cv_gate(kind, s.cv),
            samples: if samples.len() <= 2000 { Some(samples.to_vec()) } else { None },
        }
    }

    /// Single-sample metric (cold observations, init costs).
    pub fn single(
        name: &str,
        unit: &'static str,
        kind: &'static str,
        higher_is_better: bool,
        phase: Option<&'static str>,
        value: f64,
    ) -> Metric {
        Metric {
            name: name.to_string(),
            unit,
            kind,
            higher_is_better,
            phase,
            n: 1,
            min: value,
            p50: value,
            p90: value,
            p95: value,
            p99: value,
            max: value,
            mean: value,
            cv: 0.0,
            unstable: false,
            samples: Some(vec![value]),
        }
    }
}

#[derive(Serialize)]
pub struct BenchResult {
    pub schema_version: u32,
    pub benchmark: String,
    pub date_utc: String,
    pub environment: Environment,
    pub parameters: HashMap<String, serde_json::Value>,
    pub metrics: Vec<Metric>,
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct ResultBuilder {
    pub benchmark: String,
    pub parameters: HashMap<String, serde_json::Value>,
    pub metrics: Vec<Metric>,
    pub notes: Vec<String>,
}

impl ResultBuilder {
    pub fn new(benchmark: &str) -> Self {
        ResultBuilder {
            benchmark: benchmark.to_string(),
            parameters: HashMap::new(),
            metrics: Vec::new(),
            notes: Vec::new(),
        }
    }
    pub fn param(&mut self, k: &str, v: serde_json::Value) {
        self.parameters.insert(k.to_string(), v);
    }
    pub fn metric(&mut self, m: Metric) {
        self.metrics.push(m);
    }
    pub fn note(&mut self, n: impl Into<String>) {
        self.notes.push(n.into());
    }
    pub fn finish_ok(self) -> BenchResult {
        BenchResult {
            schema_version: 1,
            benchmark: self.benchmark,
            date_utc: iso_utc_now(),
            environment: capture_environment(),
            parameters: self.parameters,
            metrics: self.metrics,
            notes: self.notes,
            error: None,
        }
    }
    pub fn finish_err(self, err: String) -> BenchResult {
        BenchResult {
            schema_version: 1,
            benchmark: self.benchmark,
            date_utc: iso_utc_now(),
            environment: capture_environment(),
            parameters: self.parameters,
            metrics: self.metrics,
            notes: self.notes,
            error: Some(err),
        }
    }
}

/// Emit the result document. With `out_path`, write the JSON to a dedicated
/// file (stdout carries servo/engine log noise — libEGL, wiring warnings — and
/// must not be the machine-readable channel); otherwise fall back to stdout.
pub fn emit(result: &BenchResult, out_path: Option<&str>) {
    let json = serde_json::to_string_pretty(result).expect("serialize result");
    match out_path {
        Some(path) => {
            std::fs::write(path, &json).unwrap_or_else(|e| {
                eprintln!("FATAL: cannot write result to {path}: {e}");
                std::process::exit(1);
            });
            eprintln!("[bench-harness] result written to {path}");
        }
        None => println!("{json}"),
    }
}
