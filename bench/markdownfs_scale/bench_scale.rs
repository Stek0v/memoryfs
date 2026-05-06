#!/usr/bin/env -S cargo +nightly -Zscript
//! MemoryFS scale benchmark — measures put/get/list/commit on 100k files.
//!
//! Run: `cargo test -p memoryfs-core --test bench_scale --release -- --nocapture`
//!
//! Outputs p50/p95/p99 latencies and total throughput.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use memoryfs_core::commit::CommitGraph;
use memoryfs_core::storage::{InodeIndex, ObjectStore};

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64) * pct / 100.0).ceil() as usize;
    sorted[idx.saturating_sub(1).min(sorted.len() - 1)]
}

fn report(label: &str, latencies: &mut [Duration]) {
    latencies.sort();
    let total: Duration = latencies.iter().sum();
    let count = latencies.len();
    let p50 = percentile(latencies, 50.0);
    let p95 = percentile(latencies, 95.0);
    let p99 = percentile(latencies, 99.0);
    let ops_per_sec = if total.as_secs_f64() > 0.0 {
        count as f64 / total.as_secs_f64()
    } else {
        0.0
    };
    println!(
        "{label:20} | n={count:>6} | total={:.2}s | p50={:.3}ms | p95={:.3}ms | p99={:.3}ms | {:.0} ops/s",
        total.as_secs_f64(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0,
        ops_per_sec,
    );
}

const FILE_COUNT: usize = 100_000;

#[test]
fn bench_100k_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(dir.path().join("objects")).unwrap();
    let mut index = InodeIndex::new();

    println!("\n=== MemoryFS Scale Benchmark ({FILE_COUNT} files) ===\n");

    // ── PUT ──
    let mut put_latencies = Vec::with_capacity(FILE_COUNT);
    let overall_start = Instant::now();

    for i in 0..FILE_COUNT {
        let content = format!(
            "---\nid: mem_{i:06}\ntitle: Memory {i}\nschema_version: memoryfs/v1\n---\n\
             This is memory number {i}. It contains information about topic {topic}.\n\
             Additional context: {ctx}\n",
            topic = i % 100,
            ctx = "x".repeat(200 + (i % 300)),
        );

        let start = Instant::now();
        let hash = store.put(content.as_bytes()).unwrap();
        let elapsed = start.elapsed();
        put_latencies.push(elapsed);

        index.set(format!("memories/mem_{i:06}.md"), hash);
    }

    report("put", &mut put_latencies);

    // ── GET (random sample) ──
    let sample_size = FILE_COUNT.min(10_000);
    let mut get_latencies = Vec::with_capacity(sample_size);
    let paths: Vec<String> = (0..sample_size)
        .map(|i| format!("memories/mem_{:06}.md", i * (FILE_COUNT / sample_size)))
        .collect();

    for path in &paths {
        let hash = index.get(path).unwrap();
        let start = Instant::now();
        let _data = store.get(hash).unwrap();
        let elapsed = start.elapsed();
        get_latencies.push(elapsed);
    }

    report("get", &mut get_latencies);

    // ── VERIFY (sample) ──
    let verify_sample = 1_000;
    let mut verify_latencies = Vec::with_capacity(verify_sample);

    for i in 0..verify_sample {
        let path = format!("memories/mem_{:06}.md", i * (FILE_COUNT / verify_sample));
        let hash = index.get(&path).unwrap();
        let start = Instant::now();
        store.verify(hash).unwrap();
        let elapsed = start.elapsed();
        verify_latencies.push(elapsed);
    }

    report("verify", &mut verify_latencies);

    // ── LIST ──
    let mut list_latencies = Vec::with_capacity(10);

    for _ in 0..10 {
        let start = Instant::now();
        let count = index.iter().count();
        let elapsed = start.elapsed();
        assert_eq!(count, FILE_COUNT);
        list_latencies.push(elapsed);
    }

    report("list (full scan)", &mut list_latencies);

    // ── LIST with prefix ──
    let mut prefix_latencies = Vec::with_capacity(100);

    for i in 0..100 {
        let prefix = format!("memories/mem_{:03}", i);
        let start = Instant::now();
        let _count = index
            .iter()
            .filter(|(p, _)| p.starts_with(&prefix))
            .count();
        let elapsed = start.elapsed();
        prefix_latencies.push(elapsed);
    }

    report("list (prefix)", &mut prefix_latencies);

    // ── COMMIT ──
    let mut graph = CommitGraph::new();
    let snap: BTreeMap<String, String> = index
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
        .collect();

    let start = Instant::now();
    let commit = graph
        .commit("user:bench", "bulk load 100k files", snap, None)
        .unwrap();
    let commit_elapsed = start.elapsed();
    let commit_hash = commit.hash.clone();

    println!(
        "{:20} | n={:>6} | total={:.2}s",
        "commit (100k snap)",
        1,
        commit_elapsed.as_secs_f64()
    );

    // ── LOG ──
    let mut log_latencies = Vec::with_capacity(10);
    for _ in 0..10 {
        let start = Instant::now();
        let _log = graph.log(Some(100));
        let elapsed = start.elapsed();
        log_latencies.push(elapsed);
    }

    report("log", &mut log_latencies);

    // ── DIFF (one commit vs empty) ──
    let mut diff_latencies = Vec::with_capacity(10);
    for _ in 0..10 {
        let start = Instant::now();
        let _diff = graph.diff(None, &commit_hash);
        let elapsed = start.elapsed();
        diff_latencies.push(elapsed);
    }

    report("diff (100k changes)", &mut diff_latencies);

    // ── SERIALIZATION ──
    let start = Instant::now();
    let json = graph.to_json().unwrap();
    let ser_elapsed = start.elapsed();

    let start = Instant::now();
    let _restored = CommitGraph::from_json(&json).unwrap();
    let deser_elapsed = start.elapsed();

    println!(
        "{:20} | json_size={:.1}MB | serialize={:.2}s | deserialize={:.2}s",
        "graph serde",
        json.len() as f64 / 1_048_576.0,
        ser_elapsed.as_secs_f64(),
        deser_elapsed.as_secs_f64(),
    );

    let total_elapsed = overall_start.elapsed();
    println!(
        "\n=== Total: {:.2}s | Files: {FILE_COUNT} ===\n",
        total_elapsed.as_secs_f64()
    );

    // ── ASSERTIONS (SLO from 04-tasks-dod.md) ──
    let p95_get = percentile(&{
        let mut v = get_latencies.clone();
        v.sort();
        v
    }, 95.0);
    assert!(
        p95_get < Duration::from_millis(50),
        "p95 get latency {:.3}ms exceeds 50ms SLO",
        p95_get.as_secs_f64() * 1000.0
    );

    let p95_put = percentile(&{
        let mut v = put_latencies.clone();
        v.sort();
        v
    }, 95.0);
    assert!(
        p95_put < Duration::from_millis(50),
        "p95 put latency {:.3}ms exceeds 50ms SLO",
        p95_put.as_secs_f64() * 1000.0
    );
}
