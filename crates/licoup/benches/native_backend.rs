//! Deterministic synthetic native-backend performance smoke.
//!
//! Cases cover the three backend workloads this migration makes intentional:
//! bounded SQLite state, synthetic payload parsing, and bounded scheduler
//! fanout. All fixtures are in-memory synthetic values; no filesystem path,
//! network, client state, credential, or live runtime data is used. Evidence
//! is structural: the managed runner records completed cases and ceilings
//! instead of workstation-specific latency.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use rusqlite::{Connection, params};
use serde::Deserialize;
use tokio::sync::mpsc;

const SYNTHETIC_ROWS: usize = 256;
const SYNTHETIC_JOBS: usize = 256;
const SCHEDULER_WORKERS: usize = 4;
const SCHEDULER_CAPACITY: usize = 64;

#[derive(Deserialize)]
struct SyntheticMessage {
    id: String,
    role: String,
    content: String,
    at: i64,
}

fn synthetic_payloads(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            format!(
                r#"{{"id":"syn-{index:04}","role":"user","content":"synthetic message body {index}","at":1780000000}}"#
            )
        })
        .collect()
}

fn database_synthetic_roundtrip(c: &mut Criterion) {
    c.bench_function("database/synthetic-roundtrip", |b| {
        b.iter(|| {
            let connection = Connection::open_in_memory().expect("in-memory synthetic database");
            connection
                .execute_batch(
                    "CREATE TABLE messages (id INTEGER PRIMARY KEY, kind TEXT, body TEXT, at INTEGER);",
                )
                .expect("create synthetic table");
            for index in 0..SYNTHETIC_ROWS {
                connection
                    .execute(
                        "INSERT INTO messages (kind, body, at) VALUES (?1, ?2, ?3)",
                        params!["syn", format!("body {index}"), 1_780_000_000 + index as i64],
                    )
                    .expect("insert synthetic row");
            }
            let mut statement = connection
                .prepare("SELECT id, body FROM messages ORDER BY id")
                .expect("prepare synthetic query");
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                .expect("run synthetic query");
            let mut total = 0_i64;
            for row in rows {
                total += row.expect("synthetic row").0;
            }
            black_box(total)
        });
    });
}

fn parser_synthetic_json(c: &mut Criterion) {
    let payloads = synthetic_payloads(SYNTHETIC_ROWS);
    c.bench_function("parser/synthetic-json", |b| {
        b.iter(|| {
            let mut total = 0_usize;
            for payload in &payloads {
                let message: SyntheticMessage =
                    serde_json::from_str(payload).expect("parse synthetic payload");
                total += message.id.len() + message.role.len() + message.content.len();
                total += message.at as usize;
            }
            black_box(total)
        });
    });
}

fn scheduler_bounded_fanout(c: &mut Criterion) {
    c.bench_function("scheduler/bounded-fanout", |b| {
        b.iter(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("synthetic runtime");
            let completed = runtime.block_on(async {
                let (results_sender, mut results_receiver) =
                    mpsc::channel::<u64>(SCHEDULER_CAPACITY);
                let mut worker_senders = Vec::with_capacity(SCHEDULER_WORKERS);
                let mut workers = Vec::with_capacity(SCHEDULER_WORKERS);
                for _ in 0..SCHEDULER_WORKERS {
                    let (worker_sender, mut worker_receiver) =
                        mpsc::channel::<u64>(SCHEDULER_CAPACITY / SCHEDULER_WORKERS);
                    worker_senders.push(worker_sender);
                    let results_sender = results_sender.clone();
                    workers.push(tokio::spawn(async move {
                        let mut sum = 0_u64;
                        while let Some(job) = worker_receiver.recv().await {
                            sum += job;
                            results_sender.send(1_u64).await.expect("ack synthetic job");
                        }
                        sum
                    }));
                }
                drop(results_sender);
                let dispatcher = tokio::spawn(async move {
                    for index in 0..SYNTHETIC_JOBS as u64 {
                        worker_senders[index as usize % SCHEDULER_WORKERS]
                            .send(index)
                            .await
                            .expect("enqueue synthetic job");
                    }
                    drop(worker_senders);
                });
                let mut completed = 0_u64;
                while let Some(ack) = results_receiver.recv().await {
                    completed += ack;
                }
                dispatcher.await.expect("join synthetic dispatcher");
                for worker in workers {
                    worker.await.expect("join synthetic worker");
                }
                completed
            });
            black_box(completed)
        });
    });
}

criterion_group!(
    native_backend,
    database_synthetic_roundtrip,
    parser_synthetic_json,
    scheduler_bounded_fanout
);
criterion_main!(native_backend);
