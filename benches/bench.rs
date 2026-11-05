// Benchmark the cnf translation end to end: parse and translate a cnf
// document, reporting throughput. A port of the Python and Go
// benchmark.py - a simple assessment, not a comparison.
//
// Usage: cargo bench [bench]
//
// The default document is tmp/benchmark.cnf, overridable by the
// CNF_BENCH_FILE environment variable, just as the sibling
// implementations do.

use std::env;
use std::fs;
use std::time::Instant;

const RUNS: usize = 5;

fn doc_path() -> String {
    if let Some(p) = env::var_os("CNF_BENCH_FILE") {
        return p.to_string_lossy().into_owned();
    }
    format!("{}/tmp/benchmark.cnf", env!("CARGO_MANIFEST_DIR"))
}

fn main() {
    let path = doc_path();
    let data = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    // Correctness first: one pass must succeed and hold one json value.
    let out = cnf_rs::to_json(&data, None).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let out_len = out.len();

    // Warmup, then timed runs - enough iterations for a stable number.
    let mut times = vec![];
    for _ in 0..RUNS {
        let start = Instant::now();
        let result = cnf_rs::to_json(&data, None).expect("unexpected error");
        assert_eq!(result.len(), out_len);
        times.push(start.elapsed());
    }

    let mb = data.len() as f64 / (1024.0 * 1024.0);
    times.sort();
    println!("File:     {path}  ({mb:.1} MB, {} bytes)", data.len());
    println!("JSON out: {out_len} bytes");
    println!("Runs:     {RUNS}");
    for (i, t) in times.iter().enumerate() {
        let ms = t.as_secs_f64() * 1000.0;
        println!("   Run {}: {ms:<10.1} {:6.2} MB/s", i + 1, mb / t.as_secs_f64());
    }
    let best = times[0];
    let ms = best.as_secs_f64() * 1000.0;
    println!("Best:     {ms:<10.1} {:6.2} MB/s", mb / best.as_secs_f64());
}
