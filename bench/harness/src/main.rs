//! bench-harness — reproducible performance baseline driver (issue #19 Phase A).
//!
//! One bench per process (deadline isolation from the test fleet, see
//! bench/METHODOLOGY.md §6). Emits exactly one schema-conformant JSON document
//! to stdout; non-zero exit + `error` field on any failure (fail-closed).
//!
//! Usage: bench-harness <bench> [--key value ...]
//! Benches: runtime-create-drop | realm-create-drop | page-churn |
//!          fetch-small-payload | rss-sample | soak

mod common;
mod fetch_bench;
mod page_bench;
mod realm_bench;
mod rss_bench;
mod runtime_bench;
mod soak_bench;
mod stencil_bench;

use std::collections::HashMap;

fn usage() -> ! {
    eprintln!("usage: bench-harness <runtime-create-drop|realm-create-drop|page-churn|fetch-small-payload|rss-sample|stencil-cost|soak> [--key value ...]");
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
    }
    let bench = argv[0].clone();
    let mut map = HashMap::new();
    let mut i = 1;
    while i < argv.len() {
        let key = argv[i].strip_prefix("--").unwrap_or(&argv[i]).to_string();
        let value = argv.get(i + 1).cloned().unwrap_or_else(|| "true".into());
        map.insert(key, value);
        i += 2;
    }
    // --out <path>: write the result JSON to a dedicated file. Servo and
    // engine layers log to stdout (libEGL, wiring warnings), so a file is the
    // only pollution-free channel for the machine-readable document.
    let out_path = map.remove("out");
    let params = common::Params::new(map);

    let outcome = match bench.as_str() {
        "runtime-create-drop" => runtime_bench::run(&params),
        "realm-create-drop" => realm_bench::run(&params),
        "page-churn" => page_bench::run(&params),
        "fetch-small-payload" => fetch_bench::run(&params),
        "rss-sample" => rss_bench::run(&params),
        // #26 SM-EVOLUTION judgment bench (not in the default suite — on demand).
        "stencil-cost" => stencil_bench::run(&params),
        // soak streams a per-cycle series to a sidecar derived from --out.
        "soak" => soak_bench::run(&params, out_path.as_deref()),
        _ => usage(),
    };

    match outcome {
        Ok(builder) => {
            let result = builder.finish_ok();
            if result.metrics.is_empty() {
                let result = builder_with_error(&bench, "bench produced zero metrics");
                common::emit(&result, out_path.as_deref());
                std::process::exit(1);
            }
            common::emit(&result, out_path.as_deref());
        }
        Err(err) => {
            let result = builder_with_error(&bench, &err);
            common::emit(&result, out_path.as_deref());
            std::process::exit(1);
        }
    }
}

fn builder_with_error(bench: &str, err: &str) -> common::BenchResult {
    let mut builder = common::ResultBuilder::new(bench);
    builder.note("bench failed — fail-closed; this document carries the error, not a partial baseline");
    builder.finish_err(err.to_string())
}
