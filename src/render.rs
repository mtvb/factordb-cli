//! Human-readable rendering of RPC results: one formatter per method, falling back to pretty JSON
//! for anything unknown. The JSON shapes are those produced by fdb-rpc (wire.rs / main.rs).

use std::fmt::Write;

use serde_json::Value;

/// Ids at or below this are literal values, not stored rows (fdb_core::ids::STARTDB).
const STARTDB: u64 = 1_000_000_000_000_000_000;

pub fn render(method: &str, v: &Value) -> String {
    let mut o = String::new();
    match method {
        "get_id" => r_get_id(&mut o, v),
        "get_number" => r_number(&mut o, v),
        "nearest_prime" => r_nearest_prime(&mut o, v),
        "get_factors" => r_factors(&mut o, v),
        "factor_of" => r_factor_of(&mut o, v),
        "primality" => primality_kv(v).write(&mut o, 0),
        "algebraic_factors" => r_algebraic(&mut o, v, 0),
        "get_family" => r_family(&mut o, v),
        "report_factors" => r_report(&mut o, v),
        "prove" => r_prove(&mut o, v),
        "proof_progress" => r_proof_progress(&mut o, v),
        "proof_state" => r_proof_state(&mut o, v),
        "proof_list" => r_proof_list(&mut o, v),
        "prp_test" => r_prp_test(&mut o, v),
        "prp_test_info" => r_prp_test_info(&mut o, v),
        "get_certificate" => r_certificate_meta(&mut o, v),
        "upload_certificate" => r_upload_cert(&mut o, v),
        "cert_list" => r_cert_list(&mut o, v),
        "cert_chain" => r_cert_chain(&mut o, v),
        "cert_stats" => cert_stats_kv(v).write(&mut o, 0),
        "get_sequence" => r_seq_get(&mut o, v),
        "sequence_sizes" => r_seq_sizes(&mut o, v),
        "sequence_status" => r_seq_status(&mut o, v, 0),
        "sequence_view" => r_seq_view(&mut o, v),
        "extend_sequence" => r_seq_extend(&mut o, v),
        "list_sequences" => r_seq_list(&mut o, v),
        "sequence_of" => r_seq_of(&mut o, v, 0),
        "stats" => stats_kv(v).write(&mut o, 0),
        "smallest" => r_smallest(&mut o, v, 0),
        "comb_progress" => r_comb_progress(&mut o, v, 0),
        "status" => r_status(&mut o, v),
        "digit_distribution" => r_digit_dist(&mut o, v),
        "factor_tables" => r_factor_tables(&mut o, v),
        "list_by_type" => r_list_by_type(&mut o, v),
        "ecm_list" => r_ecm_list(&mut o, v),
        "ecm_group_order" => r_group_order(&mut o, v),
        "download" => r_download(&mut o, v),
        "login" | "register" | "regenerate_token" => r_session(&mut o, v),
        "whoami" => r_whoami(&mut o, v),
        "logout" => o.push_str("logged out\n"),
        "quota_status" => r_quota(&mut o, v),
        "health" => writeln!(o, "{}", if s(v, "status") == "ok" { "ok" } else { "unhealthy" }).unwrap(),
        _ => {
            o.push_str(&pretty(v));
            o.push('\n');
        }
    }
    if !o.is_empty() && !o.ends_with('\n') {
        o.push('\n');
    }
    o
}

// ---- generic helpers ---------------------------------------------------------------------------

pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn u(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(t)) => t.parse().unwrap_or(0),
        _ => 0,
    }
}
fn i(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(t)) => t.parse().unwrap_or(0),
        _ => 0,
    }
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn has(v: &Value, k: &str) -> bool {
    v.get(k).is_some_and(|x| !x.is_null())
}
fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Compact "N ago" for a duration in seconds.
fn ago(secs: u64) -> String {
    if secs < 90 {
        format!("{secs}s ago")
    } else if secs < 5400 {
        format!("{} min ago", (secs + 30) / 60)
    } else if secs < 172800 {
        format!("{} h ago", (secs + 1800) / 3600)
    } else {
        format!("{} d ago", (secs + 43200) / 86400)
    }
}
/// Any scalar as display text ("-" for null/absent, yes/no for booleans).
fn n(v: &Value, k: &str) -> String {
    match v.get(k) {
        None | Some(Value::Null) => "-".into(),
        Some(Value::Bool(x)) => yn(*x).into(),
        Some(Value::String(t)) => t.clone(),
        Some(Value::Number(x)) => x.to_string(),
        Some(other) => other.to_string(),
    }
}
fn yn(x: bool) -> &'static str {
    if x { "yes" } else { "no" }
}

/// `1234567` to `1,234,567`.
pub fn commas(x: u64) -> String {
    let digits = x.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
fn commas_i(x: i64) -> String {
    if x < 0 { format!("-{}", commas(x.unsigned_abs())) } else { commas(x as u64) }
}
fn bytes_h(x: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut f = x as f64;
    let mut idx = 0;
    while f >= 1024.0 && idx < UNITS.len() - 1 {
        f /= 1024.0;
        idx += 1;
    }
    if idx == 0 { format!("{x} B") } else { format!("{f:.1} {} ({} bytes)", UNITS[idx], commas(x)) }
}
fn ms_h(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{}h {}m", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}
fn secs_h(secs: u64) -> String {
    ms_h(secs * 1000)
}
/// A Unix timestamp as YYYY-MM-DD HH:MM UTC (civil-from-days, no calendar dependency).
fn date_utc(ts: i64) -> String {
    if ts <= 0 {
        return "-".into();
    }
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/// A status code with its meaning. ff is the number's fully_factored flag when the reply
/// carries one: a literal (> 10^18) always comes back as "C", so "no factor known" is only claimed
/// when the reply says the number is not fully factored.
fn status_text(st: &str, ff: Option<bool>) -> String {
    match st {
        "P" => "P (prime)".into(),
        "PRP" => "PRP (probable prime)".into(),
        "FF" => "FF (fully factored)".into(),
        "CF" => "CF (composite, partially factored)".into(),
        "U" => "U (untested)".into(),
        "X" => "X (unresolved)".into(),
        "C" => match ff {
            Some(true) => "C (composite, fully factored)".into(),
            Some(false) => "C (composite, no factor known)".into(),
            None => "C (composite)".into(),
        },
        other => other.into(),
    }
}

/// The display label of an IdWire: #fid, the literal value, or the expression.
fn id_label(id: &Value) -> String {
    match s(id, "kind") {
        "stored" => format!("#{}", u(id, "fid")),
        "literal" => s(id, "text").to_string(),
        "expr" => format!("expr {}", s(id, "text")),
        _ => id.to_string(),
    }
}
/// An id field as a label: #fid for a stored row, the plain value for a literal, - for none.
fn fid_label(v: &Value, k: &str) -> String {
    let f = u(v, k);
    if f == 0 {
        "-".into()
    } else if f <= STARTDB {
        f.to_string()
    } else {
        format!("#{f}")
    }
}
/// A sequence base/start: a value up to 10^18, else the stored id it really is.
fn seq_base(n: u64) -> String {
    if n > STARTDB { format!("#{n}") } else { commas(n) }
}

fn end_str(e: &Value) -> String {
    match s(e, "kind") {
        "open" => "open".into(),
        "merge" => {
            let base = u(e, "base");
            if base == 1 {
                "terminates (reaches 1)".into()
            } else {
                format!("merges into {} at index {}", seq_base(base), u(e, "at"))
            }
        }
        "cycle" => format!("cycle of length {}", u(e, "len")),
        "terminus" => "terminates".into(),
        other if other.is_empty() => "-".into(),
        other => other.into(),
    }
}

fn proof_type_name(t: i64) -> &'static str {
    match t {
        1 => "N-1",
        2 => "N+1",
        3 => "combined",
        _ => "?",
    }
}
fn ecm_type_name(t: i64) -> &'static str {
    match t {
        1 => "ECM (Montgomery)",
        2 => "P-1",
        3 => "P+1",
        4 => "ECM (Edwards)",
        _ => "?",
    }
}
fn cert_state(processed: i64) -> &'static str {
    match processed {
        0 => "pending",
        1 => "verified",
        2 => "processing",
        _ => "?",
    }
}
pub fn perm_desc(perm: u64) -> String {
    let mut bits = Vec::new();
    if perm & 1 != 0 {
        bits.push("admin");
    }
    if perm & 2 != 0 {
        bits.push("resource");
    }
    if perm & 4 != 0 {
        bits.push("limits-bypass");
    }
    if perm & 8 != 0 {
        bits.push("priority");
    }
    if bits.is_empty() { format!("{perm} (none)") } else { format!("{perm} ({})", bits.join(", ")) }
}

/// A number's display for a list cell: the stored formula term if any, else the decimal rendered
/// compactly as "head .. tail<digits>" from the leading (`preview`) and trailing (`tail`) chunks the
/// server sends for a long number (the previews are up to 100 digits each, so slice them short here),
/// else the full value when it is small enough to fit `preview`.
fn number_text(v: &Value) -> String {
    const HEAD: usize = 16;
    const TAIL: usize = 6;
    let term = s(v, "term");
    if !term.is_empty() {
        return term.to_string();
    }
    let p = s(v, "preview");
    let digits = u(v, "digits");
    let tail = s(v, "tail");
    if !tail.is_empty() {
        let head: String = p.chars().take(HEAD).collect();
        let tc: Vec<char> = tail.chars().collect();
        let t: String = tc[tc.len().saturating_sub(TAIL)..].iter().collect();
        return format!("{head}…{t}<{digits}>");
    }
    if digits > p.chars().count() as u64 {
        format!("{}…<{digits}>", p.chars().take(HEAD).collect::<String>())
    } else {
        p.to_string()
    }
}

fn truncate(t: &str, max: usize) -> String {
    if t.chars().count() <= max { t.to_string() } else { format!("{} ", t.chars().take(max).collect::<String>()) }
}

/// Aligned key value lines.
pub struct Kv(Vec<(String, String)>);
impl Kv {
    pub fn new() -> Self {
        Kv(Vec::new())
    }
    pub fn add(&mut self, k: &str, v: impl Into<String>) -> &mut Self {
        self.0.push((k.to_string(), v.into()));
        self
    }
    pub fn add_if(&mut self, cond: bool, k: &str, v: impl Into<String>) -> &mut Self {
        if cond {
            self.add(k, v);
        }
        self
    }
    pub fn write(&self, o: &mut String, indent: usize) {
        let w = self.0.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
        let pad = " ".repeat(indent);
        for (k, v) in &self.0 {
            let mut lines = v.split('\n');
            let first = lines.next().unwrap_or("");
            let _ = writeln!(o, "{pad}{k:<w$}  {first}");
            for extra in lines {
                let _ = writeln!(o, "{pad}{}  {extra}", " ".repeat(w));
            }
        }
    }
}

/// A padded text table. A column whose cells are all numeric is right-aligned.
pub fn table(o: &mut String, headers: &[&str], rows: &[Vec<String>], indent: usize) {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (c, cell) in r.iter().enumerate().take(cols) {
            widths[c] = widths[c].max(cell.chars().count());
        }
    }
    let numeric: Vec<bool> = (0..cols)
        .map(|c| {
            let mut any = false;
            let all = rows.iter().all(|r| {
                let cell = r.get(c).map(String::as_str).unwrap_or("");
                if cell.is_empty() || cell == "-" {
                    return true;
                }
                any = true;
                cell.replace(',', "").parse::<f64>().is_ok()
            });
            any && all
        })
        .collect();
    let pad = " ".repeat(indent);
    let fmt_row = |cells: Vec<&str>| -> String {
        let mut line = pad.clone();
        for (c, cell) in cells.iter().enumerate().take(cols) {
            let w = widths[c];
            let n = cell.chars().count();
            let fill = " ".repeat(w.saturating_sub(n));
            if c > 0 {
                line.push_str("  ");
            }
            if numeric[c] {
                line.push_str(&fill);
                line.push_str(cell);
            } else {
                line.push_str(cell);
                if c + 1 < cols {
                    line.push_str(&fill);
                }
            }
        }
        line.trim_end().to_string()
    };
    let _ = writeln!(o, "{}", fmt_row(headers.to_vec()));
    let _ = writeln!(o, "{pad}{}", widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "));
    for r in rows {
        let _ = writeln!(o, "{}", fmt_row(r.iter().map(String::as_str).collect()));
    }
}

// ---- factors -----------------------------------------------------------------------------------

/// A factor's base for display. A small factor arrives whole; a large one (>1000 digits) arrives as
/// a leading preview (`base`) plus trailing digits (`tail`) and its true `digits` count, so render it
/// as "head…tail<digits>" rather than showing only the truncated leading chunk.
fn factor_base(f: &Value) -> String {
    let base = s(f, "base");
    let tail = s(f, "tail");
    if !tail.is_empty() {
        format!("{base}…{tail}<{}>", u(f, "digits"))
    } else {
        base.to_string()
    }
}

/// One factor as base^exp[status] - the status tag is shown for anything but a proven prime.
fn factor_str(f: &Value) -> String {
    let mut t = factor_base(f);
    let e = u(f, "exponent");
    if e > 1 {
        let _ = write!(t, "^{e}");
    }
    let st = s(f, "status");
    if st != "P" && !st.is_empty() {
        let _ = write!(t, "[{st}]");
    }
    t
}
fn factors_inline(list: &[Value]) -> String {
    list.iter().map(factor_str).collect::<Vec<_>>().join(" * ")
}
fn factor_lines(o: &mut String, f: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    let list = arr(f, "factors");
    if list.is_empty() {
        let _ = writeln!(o, "{pad}(no factors known)");
        return;
    }
    for x in list {
        let mut line = format!("{pad}{}", factor_base(x));
        let e = u(x, "exponent");
        if e > 1 {
            let _ = write!(line, "^{e}");
        }
        let _ = write!(line, "  [{}]", s(x, "status"));
        if has(x, "fid") {
            let _ = write!(line, "  #{}", u(x, "fid"));
        }
        let _ = writeln!(o, "{line}");
    }
}

// ---- numbers & factors -------------------------------------------------------------------------

fn r_get_id(o: &mut String, v: &Value) {
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let mut kv = Kv::new();
    kv.add("id", format!("{} ({})", id_label(&id), s(&id, "kind")));
    kv.add("status", status_text(s(v, "status"), None));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("created", yn(b(v, "created")));
    kv.write(o, 0);
}

fn r_number(o: &mut String, v: &Value) {
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let flag_txt = if b(v, "perfect_power") { "  (perfect power)" } else { "" };
    let status = status_text(s(v, "status"), Some(b(v, "fully_factored")));
    let _ = writeln!(o, "{}  {}  {} digits{}", id_label(&id), status, commas(u(v, "digits")), flag_txt);
    let mut kv = Kv::new();
    let term = s(v, "term");
    kv.add_if(!term.is_empty(), "term", term);
    let p = s(v, "preview");
    let preview = if u(v, "digits") > p.chars().count() as u64 { format!("{p} ") } else { p.to_string() };
    kv.add_if(!p.is_empty(), "preview", preview);
    if has(v, "decimal") {
        kv.add("decimal", s(v, "decimal"));
    } else if b(v, "decimal_omitted") {
        kv.add("decimal", "(omitted: too large to inline)");
    }
    kv.add_if(has(v, "info"), "info", s(v, "info"));
    kv.write(o, 0);
    if let Some(f) = v.get("factors") {
        let _ = writeln!(o, "\nfactors  [{}]", status_text(s(f, "status"), Some(b(f, "fully_factored"))));
        factor_lines(o, f, 2);
    }
    if let Some(p) = v.get("primality") {
        o.push_str("\nprimality\n");
        primality_kv(p).write(o, 2);
    }
    if let Some(a) = v.get("algebraic") {
        o.push_str("\nalgebraic factorization\n");
        r_algebraic(o, a, 2);
    }
    if let Some(sq) = v.get("sequence_of") {
        o.push_str("\nsequence membership\n");
        r_seq_of(o, sq, 2);
    }
}

fn r_factors(o: &mut String, v: &Value) {
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let _ = writeln!(o, "{}  {}", id_label(&id), status_text(s(v, "status"), Some(b(v, "fully_factored"))));
    factor_lines(o, v, 2);
}

fn primality_kv(v: &Value) -> Kv {
    let kind = s(v, "kind");
    let desc = match kind {
        "needs_proof" => "probable prime, no proof yet",
        "cert_pending" => "certificate uploaded, verification pending",
        "direct" => "proven directly",
        "certificate" => "proven by primality certificate",
        "n-1" => "Pocklington N-1 proof",
        "n+1" => "Morrison N+1 proof",
        "combined" => "combined N-1 / N+1 (BLS75) proof",
        "unknown" => "proven, method not recorded",
        "n/a" => "not applicable",
        _ => "",
    };
    let mut kv = Kv::new();
    kv.add("status", status_text(s(v, "status"), None));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("proof", if desc.is_empty() { kind.to_string() } else { format!("{kind} {desc}") });
    kv.add_if(has(v, "base"), "witness base", n(v, "base"));
    kv.add_if(has(v, "cert_size"), "cert size", bytes_h(u(v, "cert_size")));
    kv.add_if(has(v, "cert_digits"), "cert digits", commas(u(v, "cert_digits")));
    kv.add_if(has(v, "cert_type"), "cert type", n(v, "cert_type"));
    kv.add_if(has(v, "cert_processing"), "cert state", if b(v, "cert_processing") { "being verified" } else { "queued" });
    if has(v, "cert_uploader") {
        let name = s(v, "cert_uploader");
        kv.add("cert uploader", if name.is_empty() { "anonymous".to_string() } else { format!("{name} (uid {})", u(v, "cert_user")) });
    }
    kv
}

fn r_algebraic(o: &mut String, v: &Value, indent: usize) {
    let mut kv = Kv::new();
    kv.add("digits", commas(u(v, "digits")));
    kv.add("complete", yn(b(v, "complete")));
    kv.add_if(has(v, "cofactor_digits"), "cofactor digits", commas(u(v, "cofactor_digits")));
    kv.add("parts", format!("{}{}", u(v, "total_parts"), if b(v, "truncated") { " (list truncated)" } else { "" }));
    kv.write(o, indent);
    let parts = arr(v, "parts");
    if !parts.is_empty() {
        let rows: Vec<Vec<String>> =
            parts.iter().map(|p| vec![s(p, "term").to_string(), commas(u(p, "digits")), n(p, "mult"), s(p, "kind").to_string()]).collect();
        table(o, &["term", "digits", "mult", "kind"], &rows, indent);
    }
    for note in arr(v, "notes") {
        let _ = writeln!(o, "{}note: {}", " ".repeat(indent), note.as_str().unwrap_or(""));
    }
}

fn r_family(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "members")
        .iter()
        .map(|m| {
            let x = n(m, "x");
            match m.get("member") {
                Some(mem) if !mem.is_null() => {
                    let id = mem.get("id").cloned().unwrap_or(Value::Null);
                    vec![x, id_label(&id), s(mem, "status").to_string(), commas(u(mem, "digits"))]
                }
                _ => vec![x, "-".into(), "-".into(), "-".into()],
            }
        })
        .collect();
    if rows.is_empty() {
        o.push_str("(no members)\n");
    } else {
        table(o, &["x", "id", "status", "digits"], &rows, 0);
    }
}

fn r_report(o: &mut String, v: &Value) {
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let mut kv = Kv::new();
    kv.add("number", id_label(&id));
    kv.add("status", status_text(s(v, "status"), None));
    let ids: Vec<String> = arr(v, "created_ids")
        .iter()
        .map(|x| match x {
            Value::Number(k) => format!("#{k}"),
            Value::String(t) => format!("#{t}"),
            other => other.to_string(),
        })
        .collect();
    kv.add("created ids", if ids.is_empty() { "none".to_string() } else { ids.join(", ") });
    kv.write(o, 0);
}

// ---- proofs & PRP tests ------------------------------------------------------------------------

fn r_prove(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    if b(v, "busy") {
        kv.add("result", "prover queue is full, try again later");
        kv.add("number", fid_label(v, "fid"));
    } else if b(v, "queued") {
        kv.add("result", "queued for the background prover (poll with `fdb proof-state`)");
        kv.add("number", fid_label(v, "fid"));
        kv.add_if(has(v, "digits"), "digits", commas(u(v, "digits")));
        kv.add_if(has(v, "est_ms"), "estimated cost", ms_h(u(v, "est_ms")));
        kv.add_if(has(v, "queue_len"), "queue length", n(v, "queue_len"));
    } else if b(v, "proved") {
        kv.add("result", "proven prime");
        kv.add("number", fid_label(v, "fid"));
        kv.add_if(has(v, "digits"), "digits", commas(u(v, "digits")));
        kv.add_if(has(v, "method"), "method", proof_type_name(i(v, "method")));
        kv.add_if(has(v, "witness"), "witness", n(v, "witness"));
        kv.add_if(has(v, "promoted"), "promoted to P", yn(b(v, "promoted")));
    } else {
        kv.add("result", "not proven: not a probable prime, or no special-form (N-1 / N+1) proof applies with the known factors");
    }
    kv.write(o, 0);
}

fn completeness(v: &Value) -> String {
    match v.get("completeness") {
        Some(Value::Number(x)) => {
            let f = x.as_f64().unwrap_or(0.0);
            if f <= 1.0 { format!("{:.2}%", f * 100.0) } else { format!("{f}") }
        }
        Some(other) => other.to_string(),
        None => "-".into(),
    }
}

fn r_proof_progress(o: &mut String, v: &Value) {
    if !has(v, "fid") {
        o.push_str("not a stored number - no proof progress to report\n");
        return;
    }
    let mut kv = Kv::new();
    kv.add("number", fid_label(v, "fid"));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("queued", yn(b(v, "queued")));
    kv.add("would queue", yn(b(v, "will_queue")));
    kv.add("estimated cost", ms_h(u(v, "est_ms")));
    kv.add(
        "check budget",
        if b(v, "check_enabled") { format!("{} remaining", ms_h(u(v, "check_remaining_ms"))) } else { "no limit in force".into() },
    );
    for (key, label) in [("nm1", "N-1"), ("np1", "N+1")] {
        if let Some(side) = v.get(key) {
            kv.add(
                label,
                format!(
                    "side {}  completeness {}  provable {}  fully factored {}",
                    fid_label(side, "side_fid"),
                    completeness(side),
                    yn(b(side, "provable")),
                    yn(b(side, "fully"))
                ),
            );
        }
    }
    if let Some(c) = v.get("combined") {
        kv.add("combined", format!("completeness {}  provable {}", completeness(c), yn(b(c, "provable"))));
    }
    kv.write(o, 0);
}

fn r_proof_state(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    kv.add("number", fid_label(v, "fid"));
    kv.add("status", status_text(s(v, "status"), None));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("queued", yn(b(v, "queued")));
    kv.write(o, 0);
}

fn r_proof_list(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "proofs")
        .iter()
        .map(|p| vec![fid_label(p, "fid"), commas(u(p, "digits")), proof_type_name(i(p, "type")).into(), n(p, "base"), number_text(p)])
        .collect();
    if rows.is_empty() {
        o.push_str("(no proofs)\n");
    } else {
        table(o, &["id", "digits", "method", "witness", "number"], &rows, 0);
    }
}

fn r_prp_test(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    if b(v, "busy") {
        kv.add("result", "test queue is full - try again later");
    } else if b(v, "queued") {
        kv.add("result", "queued for a background PRP test (poll with `fdb proof-state`)");
        kv.add_if(has(v, "digits"), "digits", commas(u(v, "digits")));
        kv.add_if(has(v, "est_ms"), "estimated cost", ms_h(u(v, "est_ms")));
    } else if b(v, "tested") && has(v, "is_prp") {
        kv.add("result", if b(v, "is_prp") { "probable prime (U / PRP)" } else { "composite (U / C)" });
    } else if b(v, "tested") {
        kv.add("result", format!("already settled: {}", status_text(s(v, "status"), None)));
    } else {
        kv.add("result", format!("nothing to test: {}", status_text(s(v, "status"), None)));
    }
    kv.add("number", fid_label(v, "fid"));
    kv.add_if(has(v, "status"), "status", status_text(s(v, "status"), None));
    kv.write(o, 0);
}

fn r_prp_test_info(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    kv.add("number", fid_label(v, "fid"));
    kv.add("status", status_text(s(v, "status"), None));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("testable", yn(b(v, "testable")));
    kv.add("queued", yn(b(v, "queued")));
    kv.add("would queue", yn(b(v, "will_queue")));
    kv.add("estimated cost", ms_h(u(v, "est_ms")));
    kv.add(
        "check budget",
        if b(v, "check_enabled") { format!("{} remaining", ms_h(u(v, "check_remaining_ms"))) } else { "no limit in force".into() },
    );
    kv.write(o, 0);
}

// ---- certificates ------------------------------------------------------------------------------

pub fn r_certificate_meta(o: &mut String, v: &Value) {
    if !b(v, "found") {
        o.push_str("no certificate stored\n");
        return;
    }
    let mut kv = Kv::new();
    kv.add("number", fid_label(v, "fid"));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("size", bytes_h(u(v, "size")));
    kv.add_if(b(v, "too_large"), "data", "(too large to return inline)");
    kv.write(o, 0);
}

fn r_upload_cert(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    let outcome = if b(v, "already_prime") {
        "number is already a proven prime - certificate not needed"
    } else if b(v, "stored") {
        "stored; queued for verification"
    } else {
        "not stored"
    };
    kv.add("result", outcome);
    kv.add("number", fid_label(v, "fid"));
    kv.add("digits", commas(u(v, "digits")));
    kv.add("status", status_text(s(v, "status"), None));
    kv.add("plaintext size", bytes_h(u(v, "plaintext_bytes")));
    kv.write(o, 0);
}

fn r_cert_list(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "certs")
        .iter()
        .map(|c| {
            let prog = format!("{} {}", s(c, "program"), s(c, "version")).trim().to_string();
            let user = if s(c, "fullname").is_empty() { s(c, "user").to_string() } else { s(c, "fullname").to_string() };
            vec![
                fid_label(c, "fid"),
                commas(u(c, "digits")),
                commas(u(c, "size")),
                cert_state(i(c, "processed")).into(),
                n(c, "tests"),
                prog,
                user,
                number_text(c),
            ]
        })
        .collect();
    if rows.is_empty() {
        o.push_str("(no certificates)\n");
    } else {
        table(o, &["id", "digits", "bytes", "state", "tests", "program", "user", "number"], &rows, 0);
    }
}

fn r_cert_chain(o: &mut String, v: &Value) {
    let _ = writeln!(o, "certificate chain of #{}", u(v, "fid"));
    let rows: Vec<Vec<String>> = arr(v, "chain")
        .iter()
        .map(|c| {
            let text = if s(c, "term").is_empty() { truncate(s(c, "preview"), 40) } else { truncate(s(c, "term"), 40) };
            vec![n(c, "step"), fid_label(c, "tofid"), commas(u(c, "digits")), n(c, "type"), text]
        })
        .collect();
    if rows.is_empty() {
        o.push_str("(empty chain)\n");
    } else {
        table(o, &["step", "id", "digits", "type", "number"], &rows, 0);
    }
}

fn cert_stats_kv(v: &Value) -> Kv {
    let mut kv = Kv::new();
    kv.add("total", commas(u(v, "total")));
    kv.add("verified", commas(u(v, "verified")));
    kv.add("pending", commas(u(v, "pending")));
    kv.add("processing", commas(u(v, "processing")));
    kv
}

// ---- sequences ---------------------------------------------------------------------------------

fn seq_terms(o: &mut String, terms: &[Value], indent: usize) {
    let pad = " ".repeat(indent);
    for t in terms {
        let value = match t.get("value") {
            Some(Value::String(x)) => x.clone(),
            _ => format!("({} digits, value omitted)", commas(u(t, "digits"))),
        };
        let facs = arr(t, "factors");
        let mut line = format!("{pad}{} .  {value}", u(t, "index"));
        if !facs.is_empty() {
            let _ = write!(line, " = {}", factors_inline(facs));
        }
        if !b(t, "factored") {
            line.push_str("  (not fully factored)");
        }
        let _ = writeln!(o, "{line}");
    }
}

fn r_seq_get(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    kv.add("base", seq_base(u(v, "base")));
    kv.add("base index", n(v, "base_index"));
    kv.add("length", commas(u(v, "length")));
    kv.add("end", end_str(v.get("end").unwrap_or(&Value::Null)));
    kv.write(o, 0);
    let terms = arr(v, "terms");
    if terms.is_empty() {
        o.push_str("(no terms in range)\n");
    } else {
        o.push('\n');
        seq_terms(o, terms, 0);
    }
}

fn r_seq_sizes(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    kv.add("base", seq_base(u(v, "base")));
    kv.add("base index", n(v, "base_index"));
    kv.add("length", commas(u(v, "length")));
    kv.add("end", end_str(v.get("end").unwrap_or(&Value::Null)));
    kv.write(o, 0);
    // Aliquot driver track (parallel to sizes): collapse consecutive equal drivers into index runs -
    // the text analogue of the coloured graph. Shown only when a real driver occurs (aliquot only).
    let drivers: Vec<u64> = arr(v, "drivers").iter().map(|x| x.as_u64().unwrap_or(0)).collect();
    if drivers.iter().any(|&d| d >= 1) {
        o.push_str("drivers by index:\n");
        let mut line = String::new();
        let mut i0 = 0usize;
        for k in 1..=drivers.len() {
            if k == drivers.len() || drivers[k] != drivers[i0] {
                let cell = if i0 == k - 1 {
                    format!("{i0}:{}", driver_word(drivers[i0]))
                } else {
                    format!("{i0}-{}:{}", k - 1, driver_word(drivers[i0]))
                };
                if !line.is_empty() && line.chars().count() + cell.chars().count() + 1 > 100 {
                    let _ = writeln!(o, "  {line}");
                    line.clear();
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&cell);
                i0 = k;
            }
        }
        if !line.is_empty() {
            let _ = writeln!(o, "  {line}");
        }
    }
    let sizes: Vec<String> = arr(v, "sizes").iter().map(|x| x.as_u64().unwrap_or(0).to_string()).collect();
    if !sizes.is_empty() {
        let max = arr(v, "sizes").iter().filter_map(Value::as_u64).max().unwrap_or(0);
        let _ = writeln!(o, "max digits  {max}");
        o.push_str("digits per index:\n");
        let mut line = String::new();
        for (idx, sz) in sizes.iter().enumerate() {
            let cell = format!("{idx}:{sz}");
            if line.chars().count() + cell.chars().count() + 1 > 100 {
                let _ = writeln!(o, "  {line}");
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&cell);
        }
        if !line.is_empty() {
            let _ = writeln!(o, "  {line}");
        }
    }
}

fn r_seq_status(o: &mut String, v: &Value, indent: usize) {
    let mut kv = Kv::new();
    kv.add("base", seq_base(u(v, "base")));
    kv.add("length", commas(u(v, "length")));
    kv.add("end", end_str(v.get("end").unwrap_or(&Value::Null)));
    kv.add("own length", commas(u(v, "own_length")));
    kv.add("own end", end_str(v.get("own_end").unwrap_or(&Value::Null)));
    // `merges_in` counts OTHER sequences that join this one (not where this one goes - that is `end`).
    let joined_by = u(v, "merges_in");
    kv.add(
        "joined by",
        match joined_by {
            0 => "no other sequence merges into this one".to_string(),
            1 => "1 other sequence merges into this one".to_string(),
            n => format!("{} other sequences merge into this one", commas(n)),
        },
    );
    kv.write(o, indent);
    let merges = arr(v, "merges");
    if !merges.is_empty() {
        let rows: Vec<Vec<String>> = merges.iter().map(|m| vec![n(m, "at"), seq_base(u(m, "base")), n(m, "into_at")]).collect();
        let _ = writeln!(o, "{}merge crossings along the walk:", " ".repeat(indent));
        table(o, &["at index", "into sequence", "at its index"], &rows, indent);
    }
}

fn r_seq_view(o: &mut String, v: &Value) {
    if let Some(st) = v.get("status") {
        r_seq_status(o, st, 0);
    }
    // Other sequences the start is a term of (index > 0) its own leg (index 0) is not news.
    if let Some(sq) = v.get("sequence_of") {
        let others: Vec<Value> = arr(sq, "sequences").iter().filter(|m| u(m, "index") > 0).cloned().collect();
        if !others.is_empty() {
            o.push_str("\nthe start is also a term of\n");
            r_seq_of(o, &serde_json::json!({ "sequences": others }), 2);
        }
    }
    match v.get("get") {
        Some(g) if !g.is_null() => {
            let terms = arr(g, "terms");
            let _ = writeln!(o, "\nterms from index {}:", u(v, "from"));
            seq_terms(o, terms, 0);
        }
        _ => o.push_str("\n(no terms)\n"),
    }
}

fn r_seq_extend(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    kv.add("base", seq_base(u(v, "base")));
    kv.add("computed to", commas(u(v, "computed_to")));
    kv.add("length", commas(u(v, "length")));
    kv.add("end", end_str(v.get("end").unwrap_or(&Value::Null)));
    kv.write(o, 0);
}

/// `nearest_prime` result: the neighbour prime's record (same shape as get_number), or a note when
/// there is no previous prime (N ≤ 2).
fn r_nearest_prime(o: &mut String, v: &Value) {
    if !b(v, "found") {
        o.push_str("(no prime below this number)\n");
        return;
    }
    match v.get("number") {
        Some(num) => r_number(o, num),
        None => o.push_str("(no result)\n"),
    }
}

/// Short word for an aliquot driver code (see the core's seq_guide DRIVER_*): the descending
/// downdriver, the growing named drivers, the (very sticky) even-perfect drivers, else a plain guide.
fn driver_word(code: u64) -> &'static str {
    match code {
        1 => "downdriver",
        2..=5 => "driver",
        6 => "perfect",
        _ => "plain",
    }
}

/// The "driver" cell for a sequence overview row: the frontier term's aliquot guide, its kind, and
/// its class (lower = stickier). Empty guide (non-aliquot, or unknown) renders as "-".
fn seq_driver_cell(q: &Value) -> String {
    let guide = s(q, "guide");
    if guide.is_empty() {
        return "-".into();
    }
    let class = i(q, "class");
    match u(q, "driver") {
        0 => format!("{guide} c{class}"), // plain guide, no driver
        code => format!("{} {guide} c{class}", driver_word(code)),
    }
}

fn r_seq_list(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "sequences")
        .iter()
        .map(|q| {
            let start = if has(q, "start") { s(q, "start").to_string() } else { format!("#{}", u(q, "start_id")) };
            let end = q.get("end").cloned().unwrap_or(Value::Null);
            let end_txt = if s(&end, "kind") == "merge" && has(q, "merge_base") {
                format!("merges into {} at index {}", s(q, "merge_base"), u(&end, "at"))
            } else {
                end_str(&end)
            };
            let comp = if has(q, "composite") { format!("{} ({}d)", truncate(s(q, "composite"), 24), u(q, "composite_digits")) } else { "-".into() };
            vec![start, commas(u(q, "digits")), commas(u(q, "length")), end_txt, comp, seq_driver_cell(q)]
        })
        .collect();
    if rows.is_empty() {
        o.push_str("(no sequences)\n");
    } else {
        table(o, &["start", "digits", "length", "end", "last composite", "driver"], &rows, 0);
    }
}

fn r_seq_of(o: &mut String, v: &Value, indent: usize) {
    let rows: Vec<Vec<String>> = arr(v, "sequences")
        .iter()
        .map(|m| {
            let start = if has(m, "start") { s(m, "start").to_string() } else { format!("#{}", u(m, "start_id")) };
            vec![start, n(m, "index"), commas(u(m, "length")), end_str(m.get("end").unwrap_or(&Value::Null))]
        })
        .collect();
    if rows.is_empty() {
        let _ = writeln!(o, "{}(not a term of any known sequence)", " ".repeat(indent));
    } else {
        table(o, &["sequence start", "index", "length", "end"], &rows, indent);
    }
}

// ---- statistics & listings ---------------------------------------------------------------------

fn stats_kv(v: &Value) -> Kv {
    let mut kv = Kv::new();
    kv.add("P (proven prime)", commas(u(v, "p")));
    kv.add("PRP (probable prime)", commas(u(v, "prp")));
    kv.add("C (composite)", commas(u(v, "c")));
    kv.add("CF (partially factored)", commas(u(v, "cf")));
    kv.add("U (untested)", commas(u(v, "u")));
    kv.add("total", commas(u(v, "total")));
    kv.add("data", bytes_h(u(v, "data_bytes")));
    kv.add("indexes", bytes_h(u(v, "index_bytes")));
    let load: Vec<String> = arr(v, "load").iter().map(|x| format!("{:.2}", x.as_f64().unwrap_or(0.0))).collect();
    kv.add_if(!load.is_empty(), "load", load.join(" "));
    kv
}

fn r_smallest(o: &mut String, v: &Value, indent: usize) {
    let mut kv = Kv::new();
    for (key, label) in [("prp", "smallest PRP"), ("c", "smallest C"), ("u", "smallest U")] {
        match v.get(key) {
            Some(x) if !x.is_null() => {
                kv.add(label, format!("{} digits  #{}  {}", commas(u(x, "digits")), u(x, "fid"), number_text(x)));
            }
            _ => {
                kv.add(label, "-");
            }
        }
    }
    kv.write(o, indent);
}

fn r_comb_progress(o: &mut String, v: &Value, indent: usize) {
    let rows: Vec<Vec<String>> = v
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|r| vec![n(r, "level"), s(r, "base").to_string(), commas(u(r, "digits")), s(r, "updated").to_string()])
        .collect();
    if rows.is_empty() {
        let _ = writeln!(o, "{}(no scan progress recorded)", " ".repeat(indent));
    } else {
        table(o, &["level", "base", "digits", "updated"], &rows, indent);
    }
}

fn r_status(o: &mut String, v: &Value) {
    o.push_str("tables\n");
    if let Some(st) = v.get("stats") {
        stats_kv(st).write(o, 2);
    }
    o.push_str("\nsmallest unresolved\n");
    if let Some(sm) = v.get("smallest") {
        r_smallest(o, sm, 2);
    }
    o.push_str("\nsmall-factor scan progress\n");
    match v.get("comb_progress") {
        Some(c) if !c.is_null() => r_comb_progress(o, c, 2),
        _ => o.push_str("  (unavailable)\n"),
    }
    o.push_str("\ncertificates\n");
    match v.get("cert_stats") {
        Some(c) if !c.is_null() => cert_stats_kv(c).write(o, 2),
        _ => o.push_str("  (unavailable)\n"),
    }
    o.push_str("\nspecial-form proofs\n");
    match v.get("proof_stats") {
        Some(p) if !p.is_null() => {
            let _ = writeln!(o, "  total  {}", commas(u(p, "total")));
            let rows: Vec<Vec<String>> =
                arr(p, "types").iter().map(|t| vec![proof_type_name(i(t, "type")).into(), commas(u(t, "count")), commas(u(t, "max_digits"))]).collect();
            if !rows.is_empty() {
                table(o, &["method", "count", "max digits"], &rows, 2);
            }
        }
        _ => o.push_str("  (unavailable)\n"),
    }
    o.push_str("\nfactors found by ECM / P±1\n");
    match v.get("ecm_stats") {
        Some(e) if !e.is_null() => {
            let _ = writeln!(o, "  total  {}", commas(u(e, "total")));
            let rows: Vec<Vec<String>> = arr(e, "types").iter().map(|t| vec![ecm_type_name(i(t, "type")).into(), commas(u(t, "count"))]).collect();
            if !rows.is_empty() {
                table(o, &["method", "count"], &rows, 2);
            }
        }
        _ => o.push_str("  (unavailable)\n"),
    }
    o.push_str("\nimports\n");
    let imports = arr(v, "import_status");
    if imports.is_empty() {
        o.push_str("  (none)\n");
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let rows: Vec<Vec<String>> = imports
            .iter()
            .map(|r| {
                let lr = u(r, "last_run");
                let when = if lr > 0 { ago(now.saturating_sub(lr)) } else { "-".into() };
                vec![str_of(r, "name"), when, str_of(r, "detail")]
            })
            .collect();
        table(o, &["source", "last import", "details"], &rows, 2);
    }
}

fn r_digit_dist(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "rows")
        .iter()
        .map(|r| vec![n(r, "a"), commas_i(i(r, "p")), commas_i(i(r, "prp")), commas_i(i(r, "c")), commas_i(i(r, "cf")), commas_i(i(r, "u"))])
        .collect();
    if rows.is_empty() {
        o.push_str("(no rows in this range)\n");
    } else {
        table(o, &["digits", "P", "PRP", "C", "CF", "U"], &rows, 0);
    }
}

fn r_factor_tables(o: &mut String, v: &Value) {
    let cats = arr(v, "categories");
    if cats.is_empty() {
        o.push_str("(no factor tables)\n");
    }
    for (idx, c) in cats.iter().enumerate() {
        if idx > 0 {
            o.push('\n');
        }
        let _ = writeln!(o, "[{}] {}", u(c, "id"), s(c, "name"));
        let rows: Vec<Vec<String>> = arr(c, "entries").iter().map(|e| vec![s(e, "name").to_string(), s(e, "formula").to_string()]).collect();
        if !rows.is_empty() {
            table(o, &["name", "formula"], &rows, 2);
        }
    }
}

fn r_factor_of(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> =
        arr(v, "parents").iter().map(|r| vec![fid_label(r, "fid"), commas(u(r, "digits")), number_text(r)]).collect();
    if rows.is_empty() {
        o.push_str("(not a factor of any stored number)\n");
    } else {
        table(o, &["id", "digits", "number"], &rows, 0);
    }
    if b(v, "truncated") {
        o.push_str("(more parents exist; raise --limit)\n");
    }
}

fn r_list_by_type(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "rows").iter().map(|r| vec![fid_label(r, "fid"), commas(u(r, "digits")), number_text(r)]).collect();
    if rows.is_empty() {
        o.push_str("(no rows)\n");
    } else {
        table(o, &["id", "digits", "number"], &rows, 0);
    }
    if b(v, "has_more") {
        o.push_str("(more rows available, raise --offset)\n");
    }
}

fn r_ecm_list(o: &mut String, v: &Value) {
    let rows: Vec<Vec<String>> = arr(v, "factors")
        .iter()
        .map(|f| {
            let who = if !s(f, "submitter").is_empty() {
                s(f, "submitter").to_string()
            } else if u(f, "uid") == 0 {
                "-".into()
            } else {
                format!("uid {}", u(f, "uid"))
            };
            vec![
                fid_label(f, "fid"),
                commas(u(f, "digits")),
                ecm_type_name(i(f, "type")).into(),
                n(f, "b1"),
                n(f, "b2"),
                n(f, "sigma"),
                date_utc(i(f, "ts")),
                who,
                number_text(f),
            ]
        })
        .collect();
    if rows.is_empty() {
        o.push_str("(no factors)\n");
    } else {
        table(o, &["id", "digits", "method", "B1", "B2", "sigma", "found", "by", "number"], &rows, 0);
    }
}

fn r_group_order(o: &mut String, v: &Value) {
    if !b(v, "ok") {
        let _ = writeln!(o, "error: {}", s(v, "error"));
        return;
    }
    let mut kv = Kv::new();
    kv.add("prime", s(v, "number"));
    kv.add("digits", n(v, "digits"));
    kv.add("param", n(v, "param"));
    kv.add("sigma", n(v, "sigma"));
    kv.add("group order", n(v, "order"));
    let facs: Vec<String> = arr(v, "factors")
        .iter()
        .map(|f| {
            let e = u(f, "e");
            if e > 1 { format!("{}^{e}", n(f, "p")) } else { n(f, "p") }
        })
        .collect();
    kv.add("factored", facs.join(" * "));
    kv.add_if(has(v, "cofactor"), "cofactor", n(v, "cofactor"));
    kv.add_if(has(v, "largest_prime"), "largest prime", n(v, "largest_prime"));
    kv.add_if(has(v, "montgomery_a"), "Montgomery A", n(v, "montgomery_a"));
    kv.add_if(has(v, "weierstrass_a4"), "Weierstrass a4", n(v, "weierstrass_a4"));
    kv.write(o, 0);
}

fn r_download(o: &mut String, v: &Value) {
    for x in arr(v, "numbers") {
        let _ = writeln!(o, "{}", x.as_str().unwrap_or(""));
    }
}

// ---- account -----------------------------------------------------------------------------------

fn r_session(o: &mut String, v: &Value) {
    if !b(v, "ok") {
        let e = s(v, "error");
        let _ = writeln!(o, "failed{}", if e.is_empty() { String::new() } else { format!(": {e}") });
        return;
    }
    let mut kv = Kv::new();
    kv.add("login", s(v, "login"));
    kv.add("name", s(v, "fullname"));
    kv.add("uid", n(v, "uid"));
    kv.add("permissions", perm_desc(u(v, "perm")));
    kv.add_if(has(v, "token"), "token", s(v, "token"));
    kv.write(o, 0);
}

fn r_whoami(o: &mut String, v: &Value) {
    if !b(v, "found") {
        o.push_str("token not recognised (anonymous)\n");
        return;
    }
    let mut kv = Kv::new();
    kv.add("login", s(v, "login"));
    kv.add("name", s(v, "fullname"));
    kv.add("uid", n(v, "uid"));
    kv.add("permissions", perm_desc(u(v, "perm")));
    kv.write(o, 0);
}

fn r_quota(o: &mut String, v: &Value) {
    let mut kv = Kv::new();
    if b(v, "bypass") {
        kv.add("quotas", "exempt (limit-bypass account)");
    } else if !b(v, "enabled") {
        kv.add("quotas", "not enforced");
    } else {
        kv.add("quotas", "enforced");
    }
    let cap = |used: String, cap_v: u64, cap_txt: String| if cap_v == 0 { format!("{used} (no limit)") } else { format!("{used} / {cap_txt}") };
    kv.add("ids created", cap(commas(u(v, "ids_used")), u(v, "ids_cap"), commas(u(v, "ids_cap"))));
    kv.add("wall time", cap(ms_h(u(v, "wall_used_ms")), u(v, "wall_cap_ms"), ms_h(u(v, "wall_cap_ms"))));
    kv.add("bandwidth", cap(bytes_h(u(v, "bytes_used")), u(v, "bytes_cap"), bytes_h(u(v, "bytes_cap"))));
    kv.add("background checks", cap(ms_h(u(v, "check_used_ms")), u(v, "check_cap_ms"), ms_h(u(v, "check_cap_ms"))));
    kv.add("window resets in", secs_h(u(v, "reset_secs")));
    kv.write(o, 0);
}
