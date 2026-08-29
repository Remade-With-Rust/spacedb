//! W1.2 measurement: what per-row zstd inside the seal boundary buys and costs.
//!
//! Three instruments, in evidence order (codec-measurement discipline):
//!
//! 1. **Size ratio** — deterministic byte counts of engine-side sealed rows,
//!    compression on vs off, per payload class. The counter is primary.
//! 2. **Floor sweep** — the payload size where compression starts winning on
//!    realistic structured content; sets `DEFAULT_MIN_LEN` from data.
//! 3. **Speed cost** — ABBA put/get throughput, on vs off, leading arm
//!    alternated, N pairs, null arm printed. The clock is confirmatory.
//!
//! Run pinned to one core at High priority (see the harness invocation).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use spacedb_store::{
    Collection, Compression, Durability, KeyProvider, KvEngine, MemEngine, Readable,
    StaticKeyProvider, WriteTx,
};

fn provider() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new([0x42; 32]))
}

/// Deterministic pseudo-random bytes — the incompressible class.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A structured row of roughly `target` bytes: JSON-shaped, field names
/// repeating, values varying — the shape of real vault/business rows.
fn structured_row(target: usize, i: usize) -> String {
    let mut s = format!(
        "{{\"id\":\"{i:08x}\",\"kind\":\"record\",\"site\":\"https://service-{}.example.com/login\",\"user\":\"user{}@example.com\",\"created_at\":17{:08},\"tags\":[\"personal\",\"imported\"],\"notes\":\"",
        i % 17, i, i * 977
    );
    let filler = "meeting notes: follow up on the quarterly sync; ";
    while s.len() < target.saturating_sub(2) {
        s.push_str(filler);
    }
    s.truncate(target.saturating_sub(2));
    s.push_str("\"}");
    s
}

fn median(xs: &mut [f64]) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn z_score(wins: usize, n: usize) -> f64 {
    (wins as f64 - n as f64 / 2.0) / (0.5 * (n as f64).sqrt())
}

/// Total engine-side sealed bytes for `rows` written through `compression`.
fn sealed_bytes(rows: &[Vec<u8>], compression: Compression) -> usize {
    let engine = MemEngine::new();
    let col: Collection<u64, Vec<u8>> =
        Collection::open_or_create_with(&engine, provider(), "c", 1, compression).unwrap();
    let mut w = engine.begin_write(Durability::Immediate).unwrap();
    for (i, row) in rows.iter().enumerate() {
        col.put(&mut w, &(i as u64), row).unwrap();
    }
    w.commit().unwrap();
    let r = engine.begin_read().unwrap();
    let raw = r
        .range_raw("c", &spacedb_store::KeyEncode::encode(&0u64), &spacedb_store::KeyEncode::encode(&u64::MAX))
        .unwrap();
    assert_eq!(raw.len(), rows.len(), "row count parity");
    raw.iter().map(|(_, v)| v.len()).sum()
}

fn main() {
    println!("method: sizes are deterministic byte counts (counter-primary); speed is in-process ABBA, leading arm alternated, N={N_PAIRS} pairs, per-rep >= {TARGET_REP_SECS}s, wall clock in a process pinned to one core at High priority; null arm printed last; speed ratios are off/on (>1 = compression path faster, <1 = compression costs).");

    // ── 1. Size ratio per payload class ─────────────────────────────────────
    println!("\n== size (engine-side sealed bytes, 512 rows/class) ==");
    for (label, rows) in [
        ("password-ish 300 B", (0..512).map(|i| structured_row(300, i).into_bytes()).collect::<Vec<_>>()),
        ("contact-ish 600 B", (0..512).map(|i| structured_row(600, i).into_bytes()).collect()),
        ("record-ish 1.5 KiB", (0..512).map(|i| structured_row(1536, i).into_bytes()).collect()),
        ("doc-ish 8 KiB", (0..512).map(|i| structured_row(8192, i).into_bytes()).collect()),
        ("incompressible 2 KiB", (0..512).map(|i| noise(2048, i as u64)).collect()),
    ] {
        let on = sealed_bytes(&rows, Compression::default());
        let off = sealed_bytes(&rows, Compression::Off);
        println!(
            "{label:22} on: {on:>9} B   off: {off:>9} B   on/off = {:.3}  ({:.1}% saved)",
            on as f64 / off as f64,
            (1.0 - on as f64 / off as f64) * 100.0
        );
    }

    // ── 2. Floor sweep: where does compression start paying? ───────────────
    println!("\n== floor sweep (structured rows, per-row sealed bytes on vs off) ==");
    for target in [32usize, 48, 64, 96, 128, 192, 256, 512, 1024] {
        let rows: Vec<Vec<u8>> = (0..256).map(|i| structured_row(target, i).into_bytes()).collect();
        let on = sealed_bytes(
            &rows,
            Compression::On { level: 3, min_len: 0 }, // floor disabled: measure the raw tradeoff
        );
        let off = sealed_bytes(&rows, Compression::Off);
        println!(
            "{target:>5} B rows: on/off = {:.3}  ({:+.1}%)",
            on as f64 / off as f64,
            (on as f64 / off as f64 - 1.0) * 100.0
        );
    }

    // ── 3. Speed cost: put and get throughput, on vs off ───────────────────
    // SEAL_AB_SIZE_ONLY=1 skips the timing arms: the byte counts above are
    // load-immune, but a clock number taken on a busy box is not a number.
    if std::env::var_os("SEAL_AB_SIZE_ONLY").is_some() {
        println!("\n== speed: SKIPPED (SEAL_AB_SIZE_ONLY set — run on a quiet box) ==");
        return;
    }
    println!("\n== speed (1.5 KiB structured rows, MemEngine — codec+crypto cost isolated) ==");
    let rows: Vec<Vec<u8>> = (0..256).map(|i| structured_row(1536, i).into_bytes()).collect();
    let engine = MemEngine::new();
    let on: Collection<u64, Vec<u8>> =
        Collection::open_or_create_with(&engine, provider(), "on", 1, Compression::default()).unwrap();
    let off: Collection<u64, Vec<u8>> =
        Collection::open_or_create_with(&engine, provider(), "off", 1, Compression::Off).unwrap();

    // Correctness once: both round-trip identical values.
    {
        let mut w = engine.begin_write(Durability::Immediate).unwrap();
        on.put(&mut w, &0, &rows[0]).unwrap();
        off.put(&mut w, &0, &rows[0]).unwrap();
        w.commit().unwrap();
        let r = engine.begin_read().unwrap();
        assert_eq!(on.get(&r, &0).unwrap().unwrap(), rows[0]);
        assert_eq!(off.get(&r, &0).unwrap().unwrap(), rows[0]);
    }

    bench(
        "put 256 rows",
        &mut || {
            let mut w = engine.begin_write(Durability::Eventual).unwrap();
            for (i, row) in rows.iter().enumerate() {
                on.put(&mut w, &(i as u64), black_box(row)).unwrap();
            }
            w.commit().unwrap();
        },
        &mut || {
            let mut w = engine.begin_write(Durability::Eventual).unwrap();
            for (i, row) in rows.iter().enumerate() {
                off.put(&mut w, &(i as u64), black_box(row)).unwrap();
            }
            w.commit().unwrap();
        },
    );

    bench(
        "get 256 rows",
        &mut || {
            let r = engine.begin_read().unwrap();
            for i in 0..256u64 {
                black_box(on.get(&r, &i).unwrap());
            }
        },
        &mut || {
            let r = engine.begin_read().unwrap();
            for i in 0..256u64 {
                black_box(off.get(&r, &i).unwrap());
            }
        },
    );

    bench(
        "null (off vs off)",
        &mut || {
            let r = engine.begin_read().unwrap();
            for i in 0..256u64 {
                black_box(off.get(&r, &i).unwrap());
            }
        },
        &mut || {
            let r = engine.begin_read().unwrap();
            for i in 0..256u64 {
                black_box(off.get(&r, &i).unwrap());
            }
        },
    );
}

const N_PAIRS: usize = 24;
const TARGET_REP_SECS: f64 = 0.08;

/// ABBA pair loop: leading arm alternates; identical inner iters both arms.
fn bench(label: &str, arm_on: &mut dyn FnMut(), arm_off: &mut dyn FnMut()) {
    let once = |f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        t.elapsed().as_secs_f64()
    };
    let w_on = once(arm_on);
    let w_off = once(arm_off);
    let iters = (TARGET_REP_SECS / w_on.max(w_off)).ceil().max(1.0) as usize;
    let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(N_PAIRS);
    for round in 0..N_PAIRS {
        let time = |f: &mut dyn FnMut()| {
            let t = Instant::now();
            for _ in 0..iters {
                f();
            }
            t.elapsed().as_secs_f64() / iters as f64
        };
        let (t_on, t_off) = if round % 2 == 0 {
            let a = time(arm_on);
            let b = time(arm_off);
            (a, b)
        } else {
            let b = time(arm_off);
            let a = time(arm_on);
            (a, b)
        };
        pairs.push((t_on, t_off));
    }
    let mut ratios: Vec<f64> = pairs.iter().map(|(on_t, off_t)| off_t / on_t).collect();
    let wins_on = pairs.iter().filter(|(on_t, off_t)| on_t < off_t).count();
    let med = median(&mut ratios);
    let rows_per_s = |t: f64| 256.0 / t;
    let best_on = pairs.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let best_off = pairs.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    println!(
        "{label}: median off/on = {med:.3}x | on wins {wins_on}/{N_PAIRS} (z={:+.2}) | best-of-N: on {:.0} rows/s, off {:.0} rows/s (iters={iters})",
        z_score(wins_on, N_PAIRS),
        rows_per_s(best_on),
        rows_per_s(best_off),
    );
}
