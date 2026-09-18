//! `fdb seq advance`: drive an open sequence forward by factoring its last composite locally with
//! gmp-ecm, reporting each factor to the service and letting the service extend the sequence.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Ctx;
use crate::client::{Error, Result};

/// The GMP-ECM ladder: (digit level, B1, expected curves with the default parametrization).
pub const LADDER: &[(u8, u64, u32)] = &[
    (15, 2_000, 25),
    (20, 11_000, 74),
    (25, 50_000, 221),
    (30, 250_000, 453),
    (35, 1_000_000, 984),
    (40, 3_000_000, 2541),
    (45, 11_000_000, 4949),
    (50, 43_000_000, 8266),
    (55, 110_000_000, 20158),
    (60, 260_000_000, 47173),
    (65, 850_000_000, 77666),
    (70, 2_250_000_000, 144000),
    (75, 6_000_000_000, 300000),
    (80, 20_000_000_000, 900000),
];

pub fn level_index(digits: u8) -> Option<usize> {
    LADDER.iter().position(|(d, _, _)| *d == digits)
}

/// `--from` / `--to`: one of the ladder's digit levels.
pub fn parse_level(s: &str) -> std::result::Result<u8, String> {
    let d: u8 = s.trim().parse().map_err(|_| format!("'{s}' is not a level"))?;
    if level_index(d).is_some() {
        Ok(d)
    } else {
        let all: Vec<String> = LADDER.iter().map(|(d, _, _)| d.to_string()).collect();
        Err(format!("level must be one of {} (the digit size of the factors searched for)", all.join(", ")))
    }
}

pub struct Opts {
    pub start: u64,
    pub kind: u8,
    pub kind_name: String,
    pub threads: usize,
    pub from: u8,
    pub to: u8,
    /// Stop after advancing this many terms (0 = no limit).
    pub terms: u64,
    /// Stop when the composite is larger than this (0 = no limit).
    pub max_digits: u64,
    pub ecm: String,
    pub heartbeat: Duration,
    pub submit: bool,
}

/// A composite leaf of the last term: what to run ECM on and how to address it when reporting.
struct Leaf {
    status: String,
    fid: Option<u64>,
    value: String,
}

impl Leaf {
    fn target(&self) -> Value {
        match self.fid {
            Some(f) => json!({ "id": f }),
            None => json!({ "expr": self.value }),
        }
    }
}

struct Found {
    term_index: u64,
    composite_digits: usize,
    factor: String,
    level: u8,
    b1: u64,
    seconds: f64,
    reported: Option<String>,
}

fn secs_h(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{}m {:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{}h {:02}m", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    }
}

fn end_text(end: &Value) -> String {
    match end.get("kind").and_then(Value::as_str).unwrap_or("open") {
        "open" => "open".into(),
        "merge" => format!("merges into {} at index {}", end["base"], end["at"]),
        "cycle" => format!("enters a cycle of length {}", end["len"]),
        "terminus" => "terminates".into(),
        other => other.into(),
    }
}

pub fn run(ctx: &Ctx, o: &Opts) -> Result<()> {
    if Command::new(&o.ecm).arg("-h").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        return Err(Error::Usage(format!("cannot run gmp-ecm as '{}': install gmp-ecm or pass --ecm PATH", o.ecm)));
    }
    let from_idx = level_index(o.from).expect("validated");
    let to_idx = level_index(o.to).expect("validated");
    if from_idx > to_idx {
        return Err(Error::Usage(format!("--from t{} is above --to t{}", o.from, o.to)));
    }
    let say = |msg: String| ctx.status(msg);
    let t_all = Instant::now();
    let mut level = from_idx;
    let mut first_index: Option<u64> = None;
    let mut last_index: Option<u64> = None;
    let mut start_length = 0u64;
    let mut length: u64;
    let mut found: Vec<Found> = Vec::new();
    let mut stalled_extends = 0u32;
    let stop: String;

    loop {
        // The view advances the frontier as far as the service can factor, then shows the last term.
        let view = ctx.client.call_long("sequence_view", json!({ "start": o.start, "type": o.kind, "part": "last" }))?;
        let status = &view["status"];
        length = status["length"].as_u64().unwrap_or(0);
        let end = &status["end"];
        if first_index.is_none() {
            start_length = length;
            say(format!("sequence {} ({}): length {}, {}", o.start, o.kind_name, length, end_text(end)));
        }
        if end.get("kind").and_then(Value::as_str) != Some("open") {
            stop = format!("the sequence is no longer open: it {}", end_text(end));
            break;
        }
        let Some(term) = view["get"]["terms"].as_array().and_then(|t| t.last()).cloned() else {
            stop = "the service returned no terms".into();
            break;
        };
        let index = term["index"].as_u64().unwrap_or(0);
        if first_index.is_none() {
            first_index = Some(index);
        }
        if let Some(prev) = last_index {
            if index != prev {
                level = from_idx; // a new term: start the ladder again
            }
        }
        last_index = Some(index);
        let advanced = index.saturating_sub(first_index.unwrap_or(index));
        if o.terms > 0 && advanced >= o.terms {
            stop = format!("reached the --terms limit ({advanced} term(s) advanced)");
            break;
        }

        // The composite to work on: the largest unfactored leaf of the last term.
        let leaves = composite_leaves(ctx, term["factors"].as_array().map(Vec::as_slice).unwrap_or(&[]), 0)?;
        let Some(leaf) = leaves.into_iter().max_by_key(|l| l.value.len()) else {
            // Fully factored yet still open: the service should be able to extend it.
            let ext = ctx.client.call_long("extend_sequence", json!({ "start": o.start, "type": o.kind, "steps": 1000 }))?;
            let new_len = ext["length"].as_u64().unwrap_or(0);
            if new_len > length {
                stalled_extends = 0;
                continue;
            }
            stalled_extends += 1;
            if stalled_extends >= 2 {
                stop = format!("term {index} is fully factored but the service does not extend the sequence");
                break;
            }
            continue;
        };
        if leaf.status == "U" {
            settle_untested(ctx, &leaf, &say)?;
            continue;
        }
        let digits = leaf.value.len();
        if o.max_digits > 0 && digits as u64 > o.max_digits {
            stop = format!("term {index}: the composite has {digits} digits, above --max-digits {}", o.max_digits);
            break;
        }
        if level > to_idx {
            stop = format!(
                "term {index}: no factor of the {digits}-digit composite up to t{} (B1={}); raise --to, or use another method",
                o.to, LADDER[to_idx].1
            );
            break;
        }
        let (lvl, level_b1, curves) = LADDER[level];
        let procs = o.threads.min(curves as usize).max(1);
        say(format!("term {index}: C{digits}  t{lvl}"));
        let t0 = Instant::now();
        let hit = level_search(&leaf.value, level_b1, curves, procs, &o.ecm, o.heartbeat, &say)?;
        let secs = t0.elapsed();
        match hit {
            None => {
                say(format!("  no factor at t{lvl} ({})", secs_h(secs)));
                level += 1;
            }
            Some((factor, b1)) => {
                say(format!("  factor found: {factor} ({} digits) after {}", factor.len(), secs_h(secs)));
                let mut rec = Found {
                    term_index: index,
                    composite_digits: digits,
                    factor: factor.clone(),
                    level: lvl,
                    b1,
                    seconds: secs.as_secs_f64(),
                    reported: None,
                };
                if o.submit {
                    let r = ctx.client.call_long("report_factors", json!({ "target": leaf.target(), "factors": [factor] }))?;
                    let st = r["status"].as_str().unwrap_or("?").to_string();
                    say(format!("  reported to the service: composite is now {st}"));
                    rec.reported = Some(st);
                    found.push(rec);
                } else {
                    found.push(rec);
                    stop = "factor found but not submitted (--no-submit)".into();
                    break;
                }
            }
        }
    }

    let advanced = last_index.unwrap_or(0).saturating_sub(first_index.unwrap_or(0));
    if ctx.json {
        let factors: Vec<Value> = found
            .iter()
            .map(|f| {
                json!({
                    "term_index": f.term_index, "composite_digits": f.composite_digits, "factor": f.factor,
                    "factor_digits": f.factor.len(), "level": f.level, "b1": f.b1, "seconds": f.seconds,
                    "reported_status": f.reported,
                })
            })
            .collect();
        ctx.print_json(&json!({
            "start": o.start, "type": o.kind, "length_before": start_length, "length_after": length,
            "terms_advanced": advanced, "factors": factors, "stopped": stop, "seconds": t_all.elapsed().as_secs_f64(),
        }));
    } else {
        println!(
            "done: advanced {advanced} term(s) (length {start_length} → {length}), {} factor(s) found, {}",
            found.len(),
            secs_h(t_all.elapsed())
        );
        println!("stopped: {stop}");
    }
    if advanced == 0 && found.is_empty() { Err(Error::Failed(stop)) } else { Ok(()) }
}

/// The unfactored leaves (C / U) under a factor list, expanding partially factored (CF / X)
/// factors through `get_factors`, with each value as a decimal string.
fn composite_leaves(ctx: &Ctx, factors: &[Value], depth: u32) -> Result<Vec<Leaf>> {
    let mut out = Vec::new();
    for f in factors {
        let status = f["status"].as_str().unwrap_or("").to_string();
        let fid = f["fid"].as_u64();
        let base = f["base"].as_str().unwrap_or("").to_string();
        match status.as_str() {
            "C" | "U" => out.push(Leaf { status, fid, value: decimal_of(ctx, fid, &base)? }),
            "CF" | "X" if depth < 8 => {
                let target = match fid {
                    Some(id) => json!({ "id": id }),
                    None => json!({ "expr": base }),
                };
                let sub = ctx.client.call("get_factors", json!({ "target": target }))?;
                out.extend(composite_leaves(ctx, sub["factors"].as_array().map(Vec::as_slice).unwrap_or(&[]), depth + 1)?);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A factor's decimal value: the base when it already is one, else fetched from the service.
fn decimal_of(ctx: &Ctx, fid: Option<u64>, base: &str) -> Result<String> {
    if !base.is_empty() && base.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(base.to_string());
    }
    let target = match fid {
        Some(id) => json!({ "id": id }),
        None => json!({ "expr": base }),
    };
    let v = ctx.client.call("get_number", json!({ "target": target, "decimal": true, "detail": 0 }))?;
    v["decimal"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Failed(format!("the composite {base} is too large for the service to return in decimal")))
}

/// An untested (U) leaf must be settled (PRP or C) before ECM makes sense: ask the service to test
/// it, waiting for a queued test to finish.
fn settle_untested(ctx: &Ctx, leaf: &Leaf, say: &dyn Fn(String)) -> Result<()> {
    say(format!("  {}-digit factor is untested: asking the service for a PRP test", leaf.value.len()));
    let r = ctx.client.call_long("prp_test", json!({ "target": leaf.target() }))?;
    if r["busy"].as_bool().unwrap_or(false) {
        return Err(Error::Failed("the service's test queue is full; try again later".into()));
    }
    if r["queued"].as_bool().unwrap_or(false) {
        let t0 = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(15));
            let s = ctx.client.call("proof_state", json!({ "target": leaf.target() }))?;
            let st = s["status"].as_str().unwrap_or("U");
            if st != "U" {
                say(format!("  PRP test done: {st}"));
                return Ok(());
            }
            if t0.elapsed() > Duration::from_secs(4 * 3600) {
                return Err(Error::Failed("gave up waiting for the queued PRP test".into()));
            }
        }
    }
    say(format!("  PRP test done: {}", r["status"].as_str().unwrap_or("?")));
    Ok(())
}

/// The factor a gmp-ecm output line announces, if it is that line.
pub fn parse_factor_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("********** Factor found in step")?;
    let (_, f) = rest.split_once(": ")?;
    let f = f.trim();
    if !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) { Some(f.to_string()) } else { None }
}

/// What a gmp-ecm output line means for the number `value` being worked on.
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    /// A proper factor.
    Factor(String),
    /// gmp-ecm "found" the whole input number: every prime factor's group order was smooth at
    /// once, i.e. B1/B2 are far too large for the factors dial B1 back.
    InputFound,
}

pub fn classify_line(line: &str, value: &str) -> Option<Signal> {
    if line.starts_with("Found input number") {
        return Some(Signal::InputFound);
    }
    let f = parse_factor_line(line)?;
    if f == value || f == "1" { Some(Signal::InputFound) } else { Some(Signal::Factor(f)) }
}

/// The outcome of one gmp-ecm run.
#[derive(Debug, PartialEq, Eq)]
enum Hit {
    Factor(String),
    InputFound,
    Nothing,
}

/// Most dial-back / dial-up rounds spent on one ladder level before giving it up.
const MAX_DIAL_ROUNDS: u32 = 10;

/// One ladder level on `value`: `curves` curves at `b1`, `procs` processes at a time. When gmp-ecm
/// keeps finding the whole number, B1 is dialed back tenfold and the level is rerun; when that
/// smaller B1 then finds nothing, B1 is raised again but kept below the smallest B1 known to
/// find the whole number, until a proper factor appears or the rounds run out. Returns the factor
/// and the B1 that found it; `Ok(None)` when the level is exhausted without a split.
fn level_search(
    value: &str,
    level_b1: u64,
    curves: u32,
    procs: usize,
    bin: &str,
    heartbeat: Duration,
    say: &dyn Fn(String),
) -> Result<Option<(String, u64)>> {
    let per = curves.div_ceil(procs as u32);
    let mut b1 = level_b1;
    let mut too_large: Option<u64> = None; // the smallest B1 seen to find the whole number
    let mut rounds = 0u32;
    loop {
        say(format!("  B1={b1}, {curves} curves ({procs} × {per})"));
        match ecm_run(value, b1, per, procs, bin, heartbeat, say)? {
            Hit::Factor(f) => return Ok(Some((f, b1))),
            Hit::InputFound => {
                rounds += 1;
                too_large = Some(too_large.map_or(b1, |t| t.min(b1)));
                let next = b1 / 10;
                if rounds > MAX_DIAL_ROUNDS || next < 10 {
                    say(format!("  gmp-ecm keeps finding the whole number even at B1={b1}; giving this level up"));
                    return Ok(None);
                }
                say(format!("  gmp-ecm found the whole number at B1={b1} (B1 too large for its factors): dialing back to B1={next}"));
                b1 = next;
            }
            Hit::Nothing => {
                let Some(limit) = too_large else { return Ok(None) }; // a clean level: nothing here
                rounds += 1;
                let next = (b1 * 3).min(limit / 2);
                if rounds > MAX_DIAL_ROUNDS || next <= b1 {
                    say(format!("  no split between B1={b1} and B1={limit}; giving this level up"));
                    return Ok(None);
                }
                say(format!("  no factor at B1={b1}: trying B1={next} (below the B1={limit} that found the whole number)"));
                b1 = next;
            }
        }
    }
}

/// Run `procs` gmp-ecm processes of `per` curves each at `b1` on `value`; the first signal (a
/// proper factor, or the whole number) stops the others.
fn ecm_run(value: &str, b1: u64, per: u32, procs: usize, bin: &str, heartbeat: Duration, say: &dyn Fn(String)) -> Result<Hit> {
    let total = per as u64 * procs as u64;
    let started = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<Signal>();
    let mut children: Vec<Child> = Vec::with_capacity(procs);
    for _ in 0..procs {
        let mut child = Command::new(bin)
            .arg("-c")
            .arg(per.to_string())
            .arg(b1.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::Other(anyhow::anyhow!("cannot start {bin}: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(value.as_bytes());
            let _ = stdin.write_all(b"\n");
        }
        if let Some(stdout) = child.stdout.take() {
            let tx = tx.clone();
            let started = started.clone();
            let value = value.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                    if line.starts_with("Using B1=") {
                        started.fetch_add(1, Ordering::Relaxed);
                    } else if let Some(sig) = classify_line(&line, &value) {
                        let _ = tx.send(sig);
                    }
                }
            });
        }
        children.push(child);
    }
    drop(tx);
    let t0 = Instant::now();
    let mut last_beat = Instant::now();
    let to_hit = |sig: Signal| match sig {
        Signal::Factor(f) => Hit::Factor(f),
        Signal::InputFound => Hit::InputFound,
    };
    let outcome = loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(sig) => break to_hit(sig),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Hit::Nothing, // every process finished its output
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if children.iter_mut().all(|c| matches!(c.try_wait(), Ok(Some(_)))) {
            break rx.try_iter().next().map_or(Hit::Nothing, to_hit);
        }
        if heartbeat.as_secs() > 0 && last_beat.elapsed() >= heartbeat {
            last_beat = Instant::now();
            say(format!("  … {}/{} curves started, {}", started.load(Ordering::Relaxed).min(total), total, secs_h(t0.elapsed())));
        }
    };
    for c in &mut children {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_line() {
        assert_eq!(parse_factor_line("********** Factor found in step 1: 1000000007"), Some("1000000007".into()));
        assert_eq!(parse_factor_line("********** Factor found in step 2: 123"), Some("123".into()));
        assert_eq!(parse_factor_line("Found prime factor of 10 digits: 1000000007"), None);
        assert_eq!(parse_factor_line("Using B1=11000, B2=1873422"), None);
    }

    #[test]
    fn line_classes() {
        let n = "1000036000099";
        assert_eq!(classify_line("********** Factor found in step 1: 1000003", n), Some(Signal::Factor("1000003".into())));
        assert_eq!(classify_line("********** Factor found in step 2: 1000036000099", n), Some(Signal::InputFound));
        assert_eq!(classify_line("Found input number N", n), Some(Signal::InputFound));
        assert_eq!(classify_line("Step 1 took 5ms", n), None);
    }

    #[test]
    fn dial_back_on_whole_number() {
        if Command::new("ecm").arg("-h").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
            return;
        }
        let log = std::sync::Mutex::new(Vec::new());
        let say = |m: String| log.lock().unwrap().push(m);
        let r = level_search("1000036000099", 1_000_000, 8, 4, "ecm", Duration::ZERO, &say).unwrap();
        let log = log.lock().unwrap();
        assert!(log.iter().any(|m| m.contains("dialing back")), "no dial-back happened: {log:?}");
        if let Some((f, b1)) = r {
            assert!(f == "1000003" || f == "1000033", "improper factor {f}");
            assert!(b1 < 1_000_000);
        }
    }

    #[test]
    fn ladder_levels() {
        assert_eq!(level_index(20), Some(1));
        assert!(parse_level("40").is_ok());
        assert!(parse_level("42").is_err());
        assert!(LADDER.windows(2).all(|w| w[0].1 < w[1].1 && w[0].2 < w[1].2));
    }
}
