//! `fdb snfs`: SNFS polynomials (`snfs_poly`) as a list or as the input file of an NFS tool.
//!
//! The RPC sends each polynomial pair with its skew, difficulty, scaled difficulty, special-q side
//! and Murphy E only; the sieving parameters and the file formats are derived here. The tables and
//! the formats mirror the core's reference (fdb-snfs: params.rs, render.rs) and the website's
//! lib/snfs.php - keep the three in step.
//!
//! Parameters follow yafu: the scaled difficulty maps to a GNFS size of equal effort
//! (0.56·d + 30), which indexes yafu's SNFS parameter table (rlim, alim, start-q, q-range
//! interpolated, the rest from the nearer row); norms 20 or more orders of magnitude apart shift
//! the limits and large-prime bounds to the sieved side.

use std::fmt::Write;

use serde_json::Value;

use crate::render;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Ggnfs,
    Msieve,
    Cado,
}

/// GNFS digits of equal effort for an SNFS difficulty (truncated).
pub fn gnfs_size(difficulty: f64) -> u32 {
    (0.56 * difficulty + 30.0).max(0.0) as u32
}

/// digits, rlim, alim, lpbr, lpba, mfbr, mfba, rlambda, alambda, siever, start-q, q-range
const TABLE: [[f64; 12]; 23] = [
    [75.0, 300000.0, 300000.0, 23.0, 23.0, 46.0, 46.0, 2.0, 2.0, 11.0, 150000.0, 1000.0],
    [80.0, 600000.0, 600000.0, 24.0, 24.0, 48.0, 48.0, 2.1, 2.1, 11.0, 300000.0, 1000.0],
    [85.0, 900000.0, 900000.0, 24.0, 24.0, 48.0, 48.0, 2.1, 2.1, 11.0, 450000.0, 1000.0],
    [90.0, 1200000.0, 1200000.0, 25.0, 25.0, 50.0, 50.0, 2.3, 2.3, 11.0, 600000.0, 1000.0],
    [95.0, 1500000.0, 1500000.0, 25.0, 25.0, 50.0, 50.0, 2.5, 2.5, 12.0, 750000.0, 2000.0],
    [100.0, 1800000.0, 1800000.0, 26.0, 26.0, 52.0, 52.0, 2.5, 2.5, 12.0, 900000.0, 2000.0],
    [105.0, 2500000.0, 2500000.0, 26.0, 26.0, 52.0, 52.0, 2.5, 2.5, 12.0, 1250000.0, 2000.0],
    [110.0, 3200000.0, 3200000.0, 26.0, 26.0, 52.0, 52.0, 2.5, 2.5, 13.0, 1600000.0, 4000.0],
    [115.0, 4500000.0, 4500000.0, 27.0, 27.0, 54.0, 54.0, 2.5, 2.5, 13.0, 2250000.0, 4000.0],
    [120.0, 5500000.0, 5500000.0, 27.0, 27.0, 54.0, 54.0, 2.5, 2.5, 13.0, 2750000.0, 4000.0],
    [125.0, 7000000.0, 7000000.0, 27.0, 27.0, 54.0, 54.0, 2.5, 2.5, 13.0, 3500000.0, 4000.0],
    [130.0, 9000000.0, 9000000.0, 28.0, 28.0, 56.0, 56.0, 2.5, 2.5, 13.0, 4500000.0, 8000.0],
    [135.0, 11500000.0, 11500000.0, 28.0, 28.0, 56.0, 56.0, 2.6, 2.6, 14.0, 5750000.0, 8000.0],
    [140.0, 14000000.0, 14000000.0, 28.0, 28.0, 56.0, 56.0, 2.6, 2.6, 14.0, 7000000.0, 8000.0],
    [145.0, 19000000.0, 19000000.0, 28.0, 28.0, 56.0, 56.0, 2.6, 2.6, 14.0, 9500000.0, 8000.0],
    [150.0, 25000000.0, 25000000.0, 29.0, 29.0, 58.0, 58.0, 2.6, 2.6, 14.0, 12500000.0, 16000.0],
    [155.0, 32000000.0, 32000000.0, 29.0, 29.0, 58.0, 58.0, 2.6, 2.6, 14.0, 16000000.0, 16000.0],
    [160.0, 40000000.0, 40000000.0, 30.0, 30.0, 60.0, 60.0, 2.6, 2.6, 14.0, 20000000.0, 16000.0],
    [165.0, 49000000.0, 49000000.0, 30.0, 30.0, 60.0, 60.0, 2.6, 2.6, 14.0, 24500000.0, 16000.0],
    [170.0, 59000000.0, 59000000.0, 31.0, 31.0, 62.0, 62.0, 2.6, 2.6, 14.0, 29500000.0, 32000.0],
    [175.0, 70000000.0, 70000000.0, 31.0, 31.0, 62.0, 62.0, 2.6, 2.6, 15.0, 35000000.0, 32000.0],
    [180.0, 82000000.0, 82000000.0, 31.0, 31.0, 62.0, 62.0, 2.6, 2.6, 15.0, 41000000.0, 32000.0],
    [185.0, 100000000.0, 100000000.0, 32.0, 32.0, 64.0, 64.0, 2.6, 2.6, 16.0, 50000000.0, 32000.0],
];

pub struct JobParams {
    pub rlim: u32,
    pub alim: u32,
    pub lpbr: u32,
    pub lpba: u32,
    pub mfbr: u32,
    pub mfba: u32,
    pub rlambda: f64,
    pub alambda: f64,
    pub siever: u32,
    pub qstart: u32,
    pub qrange: u32,
}

/// The job parameters for a polynomial of the given difficulty, scaled difficulty and side.
pub fn job_params(difficulty: f64, scaled: f64, rational: bool) -> JobParams {
    let scaled = scaled.max(difficulty);
    let d = gnfs_size(scaled) as f64;
    let last = TABLE.len() - 1;
    // the rows around d and the weight of the lower one; a single row outside the table
    let (mut lo, mut hi, mut w) = if d <= TABLE[0][0] { (0, 0, 1.0) } else { (last, last, 1.0) };
    for i in 0..last {
        if d > TABLE[i][0] && d <= TABLE[i + 1][0] {
            (lo, hi, w) = (i, i + 1, (TABLE[i + 1][0] - d) / (TABLE[i + 1][0] - TABLE[i][0]));
            break;
        }
    }
    let near = if w > 0.5 { lo } else { hi };
    let interp = |col: usize| -> u32 { (TABLE[hi][col] - (w * (TABLE[hi][col] - TABLE[lo][col])).floor()) as u32 };
    let mut p = JobParams {
        rlim: interp(1),
        alim: interp(2),
        lpbr: TABLE[near][3] as u32,
        lpba: TABLE[near][4] as u32,
        mfbr: TABLE[near][5] as u32,
        mfba: TABLE[near][6] as u32,
        rlambda: TABLE[near][7],
        alambda: TABLE[near][8],
        siever: TABLE[near][9] as u32,
        qstart: interp(10),
        qrange: interp(11),
    };
    // unbalanced norms: one digit of scaled difficulty per five orders of magnitude
    let oom = scaled - difficulty;
    let shift = |lim: &mut u32, other: &mut u32, lpb: &mut u32, mfb: &mut u32, lambda: &mut f64| {
        if oom >= 4.0 {
            *lpb += 1;
            *mfb += 2;
            let pct = oom * 0.05;
            *other = other.saturating_sub((pct * *other as f64 / 5.0) as u32);
            *lim += (pct * *lim as f64) as u32;
        }
        if oom >= 5.0 {
            *lpb += 1;
            *mfb += 2;
        }
        if oom >= 6.0 {
            *mfb = (*lpb as f64 * 2.9) as u32;
            *lambda = 3.6;
        }
    };
    if rational {
        shift(&mut p.rlim, &mut p.alim, &mut p.lpbr, &mut p.mfbr, &mut p.rlambda);
    } else {
        shift(&mut p.alim, &mut p.rlim, &mut p.lpba, &mut p.mfba, &mut p.alambda);
    }
    p
}

/// The CADO-NFS `parameters/factor/params.c<size>` file nearest to a GNFS-equivalent size.
pub fn cado_size(gnfs: u32) -> u32 {
    const SIZES: [u32; 42] = [
        30, 60, 65, 70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120, 125, 130, 135, 140, 143, 145, 148, 150, 153, 155, 158, 160,
        163, 165, 170, 175, 180, 185, 190, 195, 200, 210, 220, 230, 240, 270, 310, 320,
    ];
    let mut best = SIZES[0];
    for s in SIZES {
        if s.abs_diff(gnfs) < best.abs_diff(gnfs) {
            best = s;
        }
    }
    best
}

/// A skew with four decimals, or in exponent form when that would print as zero.
pub fn skew_str(skew: f64) -> String {
    if skew >= 0.001 {
        format!("{skew:.4}")
    } else {
        format!("{skew:.3e}")
    }
}

fn f(p: &Value, k: &str) -> f64 {
    p.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}
fn st<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("")
}
fn coeffs(p: &Value) -> Vec<&str> {
    p.get("c").and_then(Value::as_array).map(|a| a.iter().map(|c| c.as_str().unwrap_or("0")).collect()).unwrap_or_default()
}

/// One polynomial of an `snfs_poly` result as the input file of an NFS tool.
pub fn render_file(n: &str, p: &Value, format: FileFormat) -> String {
    let c = coeffs(p);
    let (y1, y0, form) = (st(p, "y1"), st(p, "y0"), st(p, "form"));
    let difficulty = f(p, "difficulty");
    let scaled = f(p, "scaled").max(difficulty);
    let rational = st(p, "side") == "r";
    let side = if rational { "rational" } else { "algebraic" };
    let skew = skew_str(f(p, "skew"));
    let mut s = String::new();
    match format {
        FileFormat::Msieve => {
            let _ = write!(s, "N {n}\nSKEW {skew}\nR0 {y0}\nR1 {y1}\n");
            for (i, ci) in c.iter().enumerate() {
                let _ = writeln!(s, "A{i} {ci}");
            }
        }
        FileFormat::Cado => {
            let _ = write!(s, "n: {n}\nskew: {skew}\n");
            for (i, ci) in c.iter().enumerate() {
                let _ = writeln!(s, "c{i}: {ci}");
            }
            let _ = write!(s, "Y0: {y0}\nY1: {y1}\n");
            let _ = writeln!(s, "# {form}, difficulty: {difficulty:.2}");
            let _ = writeln!(
                s,
                "# parameters: parameters/factor/params.c{}, tasks.sieve.sqside = {} ({side} side)",
                cado_size(gnfs_size(scaled)),
                if rational { 0 } else { 1 }
            );
        }
        FileFormat::Ggnfs => {
            let jp = job_params(difficulty, scaled, rational);
            let _ = writeln!(s, "n: {n}");
            let _ = writeln!(s, "# {form}, difficulty: {difficulty:.2}, scaled difficulty: {scaled:.2}");
            let _ = writeln!(
                s,
                "# special-q on the {side} side, siever gnfs-lasieve4I{}e, start at q = {} in ranges of {}",
                jp.siever, jp.qstart, jp.qrange
            );
            let _ = write!(s, "type: snfs\nsize: {}\nskew: {skew}\n", difficulty as u32);
            for (i, ci) in c.iter().enumerate().rev() {
                if *ci != "0" {
                    let _ = writeln!(s, "c{i}: {ci}");
                }
            }
            let _ = write!(s, "Y1: {y1}\nY0: {y0}\n");
            let _ = write!(s, "rlim: {}\nalim: {}\n", jp.rlim, jp.alim);
            let _ = write!(s, "lpbr: {}\nlpba: {}\n", jp.lpbr, jp.lpba);
            let _ = write!(s, "mfbr: {}\nmfba: {}\n", jp.mfbr, jp.mfba);
            let _ = write!(s, "rlambda: {:.1}\nalambda: {:.1}\n", jp.rlambda, jp.alambda);
        }
    }
    s
}

/// A long decimal as head…tail<digits>.
fn short(v: &str, max: usize) -> String {
    let digits = v.trim_start_matches('-');
    if digits.len() <= max {
        return v.to_string();
    }
    format!("{}{}…{}<{}>", if v.starts_with('-') { "-" } else { "" }, &digits[..8], &digits[digits.len() - 4..], digits.len())
}

/// `f(x)` on one line: "4x^5 - 1".
pub fn poly_text(c: &[&str]) -> String {
    let mut s = String::new();
    for (i, ci) in c.iter().enumerate().rev() {
        if *ci == "0" {
            continue;
        }
        let neg = ci.starts_with('-');
        let abs = ci.trim_start_matches('-');
        if !s.is_empty() || neg {
            s.push_str(if neg { " - " } else { " + " });
        }
        if abs != "1" || i == 0 {
            s.push_str(&short(abs, 24));
        }
        match i {
            0 => {}
            1 => s.push('x'),
            _ => {
                let _ = write!(s, "x^{i}");
            }
        }
    }
    s.trim_start().to_string()
}

/// The polynomials of an `snfs_poly` result as a table, best first.
/// What to tell the reader when the polynomials are for a number nobody needs to sieve: its
/// state (the server answers for any number) and whether the polynomials lose to GNFS. One note
/// per line, without a prefix; empty for an open composite with a worthwhile polynomial.
pub fn notes(v: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    match st(v, "status") {
        "P" => out.push("this number is a proven prime - there is nothing to factor"),
        "PRP" => out.push("this number is a probable prime - there is most likely nothing to factor"),
        "CF" => out.push("this number already has known factors - sieve its unfactored cofactor (the polynomial is valid for every divisor)"),
        "FF" => out.push("this number is already fully factored"),
        "U" => out.push("this number is untested - it may be prime"),
        _ => {}
    }
    if v.get("weak").and_then(Value::as_bool).unwrap_or(false) {
        out.push("none of these beats GNFS (or QS/ECM for a small number) on the number itself");
    }
    out
}

pub fn list(o: &mut String, v: &Value) {
    let polys = v.get("polys").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    if polys.is_empty() {
        o.push_str("no SNFS polynomial (no known special form)\n");
        return;
    }
    let n = st(v, "n");
    let best = &polys[0];
    for note in notes(v) {
        let _ = writeln!(o, "note     {note}");
    }
    let _ = writeln!(o, "number   {} ({} digits)", short(n, 40), n.len());
    let _ = writeln!(o, "form     {}", st(best, "form"));
    let _ = writeln!(
        o,
        "effort   about GNFS on {} digits (SNFS difficulty {:.1}, scaled {:.1})",
        gnfs_size(f(best, "scaled").max(f(best, "difficulty"))),
        f(best, "difficulty"),
        f(best, "scaled")
    );
    let rows: Vec<Vec<String>> = polys
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let c = coeffs(p);
            let root = if st(p, "y1") == "1" {
                format!("m = {}", short(st(p, "y0").trim_start_matches('-'), 28))
            } else {
                format!("Y1 = {}  Y0 = {}", short(st(p, "y1"), 20), short(st(p, "y0"), 20))
            };
            vec![
                i.to_string(),
                (c.len().saturating_sub(1)).to_string(),
                format!("{:.1}", f(p, "difficulty")),
                if st(p, "side") == "r" { "rational".into() } else { "algebraic".into() },
                format!("{:.2e}", f(p, "e")),
                poly_text(&c),
                root,
            ]
        })
        .collect();
    render::table(o, &["#", "deg", "difficulty", "sieve side", "Murphy E", "f(x)", "root"], &rows, 0);
    o.push_str("best first (for a sieve of this job's size); --format ggnfs|msieve|cado prints polynomial --poly as a file\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "c": ["1", "0", "-1", "0", "1"], "y1": "1", "y0": "-100000000000000000000000000000000000",
            "skew": 1.0, "difficulty": 140.0, "scaled": 142.9, "side": "r", "e": 4.637e-9, "form": "10^140-10^70+1"
        })
    }

    #[test]
    fn files_match_the_reference() {
        // the same polynomial rendered by the core's reference (fdb-snfs render.rs, snfsgen)
        let p = sample();
        assert_eq!(
            render_file("N", &p, FileFormat::Ggnfs),
            "n: N\n# 10^140-10^70+1, difficulty: 140.00, scaled difficulty: 142.90\n\
             # special-q on the rational side, siever gnfs-lasieve4I13e, start at q = 1600000 in ranges of 4000\n\
             type: snfs\nsize: 140\nskew: 1.0000\nc4: 1\nc2: -1\nc0: 1\nY1: 1\nY0: -100000000000000000000000000000000000\n\
             rlim: 3200000\nalim: 3200000\nlpbr: 26\nlpba: 26\nmfbr: 52\nmfba: 52\nrlambda: 2.5\nalambda: 2.5\n"
        );
        assert_eq!(
            render_file("N", &p, FileFormat::Msieve),
            "N N\nSKEW 1.0000\nR0 -100000000000000000000000000000000000\nR1 1\nA0 1\nA1 0\nA2 -1\nA3 0\nA4 1\n"
        );
        assert_eq!(
            render_file("N", &p, FileFormat::Cado),
            "n: N\nskew: 1.0000\nc0: 1\nc1: 0\nc2: -1\nc3: 0\nc4: 1\nY0: -100000000000000000000000000000000000\nY1: 1\n\
             # 10^140-10^70+1, difficulty: 140.00\n# parameters: parameters/factor/params.c110, tasks.sieve.sqside = 0 (rational side)\n"
        );
    }

    #[test]
    fn parameters() {
        assert_eq!(gnfs_size(200.0), 142);
        let p = job_params(129.0, 129.0, false); // gnfs 102: between the rows 100 and 105
        assert_eq!((p.rlim, p.lpbr, p.siever, p.qstart), (2080000, 26, 12, 1040000));
        assert_eq!(job_params(60.0, 60.0, false).rlim, 300000);
        assert_eq!(job_params(320.0, 320.0, false).rlim, 100000000);
        // norms 22.5 orders of magnitude apart: the sieved (rational) side gets the larger bounds
        let (a, b) = (job_params(200.0, 204.5, true), job_params(204.5, 204.5, true));
        assert_eq!((a.lpbr, a.mfbr, a.lpba), (b.lpbr + 1, b.mfbr + 2, b.lpba));
        assert!(a.rlim > b.rlim && a.alim < b.alim);
        assert_eq!((cado_size(142), cado_size(107), cado_size(300)), (143, 105, 310));
        assert_eq!((skew_str(0.5400), skew_str(0.00012344)), ("0.5400".to_string(), "1.234e-4".to_string()));
    }

    #[test]
    fn polynomial_text() {
        assert_eq!(poly_text(&["-1", "0", "0", "0", "0", "4"]), "4x^5 - 1");
        assert_eq!(poly_text(&["1", "-1", "1", "-1", "1"]), "x^4 - x^3 + x^2 - x + 1");
        assert_eq!(poly_text(&["5", "1"]), "x + 5");
        assert_eq!(poly_text(&["-7", "0", "-3"]), "- 3x^2 - 7");
    }
}
