mod advance;
mod client;
mod config;
mod render;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Arg, ArgAction, Command, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use client::{Call, Client, Error, Result, target};
use config::Config;

/// Ids at or below this are literal values, not stored rows (fdb_core::ids::STARTDB).
const STARTDB: u64 = 1_000_000_000_000_000_000;

const TARGET_HELP: &str = "A number: an expression (2^127-1, 150!+1, 10^80+7, 12345) or a stored id written id:N";
const TYPE_HELP: &str = "Which sequence family: a name or code aliquot (1), hp10 = home prime base 10, ihp3 = inverse home \
prime base 3, lpf2+1 = largest prime factor²+1,(see `fdb seq types`)";
const START_HELP: &str = "Starting number (a value up to 10^18, an expression evaluating to one, or a stored id as id:N)";
const PASSWORD_HELP: &str =
    "Password. Prompted when omitted (or read from stdin when piped); avoid -p, which exposes it to `ps` and shell history";

const LONG_ABOUT: &str = "\
Command-line client for the factordb JSON-RPC API.

Numbers can be addressed by expression (2^131-1, 10^80+7, 150!+1, 12345) or by stored id (id:123456).
Expressions accept + - * / ^ %, ! (factorial), # / ## (primorial), I(n) / lucas(n) and parentheses.

Settings resolve in this order: flag > environment > config file > default.
  endpoint  --url      FDB_RPC_URL   (default http://127.0.0.1:4059/rpc)
  token     --token    FDB_TOKEN     (set by `fdb login`; sent as X-Fdb-User-Token; '' = anonymous)
  config    $FDB_CONFIG or ~/.config/fdb/config.toml

Without --timeout, reads give up after 120 s; writes (report, prove, prp-test, seq extend/view,
cert upload) wait for the server, which finishes the work either way.

Exit codes: 0 ok · 1 the call failed (RPC error, rejected login, nothing found) · 2 usage ·
3 service unreachable / HTTP error · 4 rate limit or quota exhausted · 5 address blocked.";

#[derive(Parser)]
#[command(name = "fdb", version, about = "Command-line client for the factordb JSON-RPC API", long_about = LONG_ABOUT)]
struct Cli {
    /// RPC endpoint URL
    #[arg(long, global = true, env = "FDB_RPC_URL", value_name = "URL")]
    url: Option<String>,
    /// Account API token, sent as X-Fdb-User-Token ('' = act anonymously)
    #[arg(long, global = true, env = "FDB_TOKEN", hide_env_values = true, value_name = "TOKEN")]
    token: Option<String>,
    /// Print the raw JSON result instead of the human-readable rendering
    #[arg(short, long, global = true)]
    json: bool,
    /// Single-line JSON output (implies --json)
    #[arg(long, global = true)]
    compact: bool,
    /// Request timeout in seconds for every call (0 = none)
    #[arg(long, global = true, value_name = "SECS", value_parser = parse_timeout)]
    timeout: Option<f64>,
    /// Log each request and the server's resource accounting to stderr
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

fn parse_timeout(s: &str) -> std::result::Result<f64, String> {
    let t: f64 = s.trim().parse().map_err(|_| format!("'{s}' is not a number of seconds"))?;
    config::check_timeout(t)
}

#[derive(Subcommand)]
enum Cmd {
    // ---- Numbers & factors ----
    /// Resolve an expression to a factordb id, optionally storing it [get_id]
    Id {
        expr: String,
        /// Store the number and its structure (a write). Values up to 10^18 are literal ids and are
        /// never stored; `created no` on a larger value means it already existed
        #[arg(short, long)]
        create: bool,
    },
    /// Full record of a number: status, size, factors, primality, [get_number]
    #[command(visible_aliases = ["get", "show"])]
    Number {
        #[arg(help = TARGET_HELP)]
        target: String,
        /// Include the full decimal value (when not too large to inline)
        #[arg(long)]
        decimal: bool,
        /// 0 = basic; 1 = also factors; 2 = also primality, algebraic form and sequence membership
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=2), conflicts_with = "full")]
        detail: u8,
        /// Everything (same as --detail 2)
        #[arg(long)]
        full: bool,
    },
    /// The known factorization of a number [get_factors]
    Factors {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Primality state and certificate metadata [primality]
    Primality {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Algebraic factorization (difference/sum of powers, Aurifeuillian) [algebraic_factors]
    Algebraic {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Neighbouring members of a family such as 2^x-1 around an index [get_family]
    Family {
        /// Family expression with x as the variable, e.g. 2^x-1
        expr: String,
        /// First index (may be negative)
        #[arg(long, default_value_t = 1, allow_negative_numbers = true)]
        start: i64,
        /// How many members to list
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Submit found factors of a number; each is verified exactly [report_factors]
    Report {
        #[arg(help = TARGET_HELP)]
        target: String,
        /// Factors, decimal or expressions (e.g. 1009 2^32+1)
        factors: Vec<String>,
        /// Also read factors from FILE, one per line ('-' = stdin)
        #[arg(short, long, value_name = "FILE")]
        file: Option<PathBuf>,
    },

    // ---- Primality proofs ----
    /// Run a special-form proof (Pocklington N-1 / Morrison N+1 / combined); large numbers queue [prove]
    Prove {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Per-method completeness and the cost of proving a number [proof_progress]
    ProofProgress {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Poll a number's proof state after a large proof was queued [proof_state]
    ProofState {
        #[arg(help = TARGET_HELP)]
        target: String,
        /// Keep polling until the number leaves the queue
        #[arg(long)]
        wait: bool,
        /// Polling interval for --wait, in seconds
        #[arg(long, default_value_t = 5, value_name = "SECS")]
        interval: u64,
    },
    /// List numbers proven prime by a special-form test [proof_list]
    ProofList {
        /// 1 = N-1, 2 = N+1, 3 = combined, 0 = all
        #[arg(long = "type", default_value_t = 0, value_name = "T")]
        type_id: i64,
        #[arg(long, default_value_t = 0)]
        min_digits: u64,
        /// Largest first
        #[arg(long)]
        descending: bool,
        #[arg(long, default_value_t = 0)]
        skip: u64,
        /// Maximum 1000
        #[arg(long, default_value_t = 100)]
        limit: u64,
    },

    // ---- Probable-prime tests ----
    /// BPSW probable-prime test of an untested (U) number; settles it as PRP or C [prp_test]
    PrpTest {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Whether a number is PRP-testable and what a queued test would cost [prp_test_info]
    PrpTestInfo {
        #[arg(help = TARGET_HELP)]
        target: String,
    },

    /// Primality certificates: get, upload, list, chain, stats
    #[command(subcommand)]
    Cert(CertCmd),
    /// Aliquot and other sequences: get, sizes, status, view, extend, list, of, types. Every
    /// subcommand takes --type to pick the family (aliquot, hp10, ihp3, lpf2+1; see `fdb seq types`)
    #[command(subcommand)]
    Seq(SeqCmd),

    // ---- Statistics & listings ----
    /// The whole status page: counts, smallest unresolved, scan progress, cert/proof/ECM stats [status]
    Status,
    /// Row counts per table and disk usage [stats]
    Stats,
    /// The smallest unresolved number of each kind [smallest]
    Smallest,
    /// Progress of the small-factor (comb) scanner [comb_progress]
    CombProgress,
    /// Row counts per digit length [digit_distribution]
    DigitDistribution {
        /// First digit length (stored numbers begin at 19 digits)
        #[arg(long, default_value_t = 0)]
        start: u64,
        /// How many digit lengths
        #[arg(long, default_value_t = 100)]
        count: u64,
    },
    /// The factor-table catalogue (backs the Tables page) [factor_tables]
    FactorTables,
    /// List numbers of one table, smallest first [list_by_type]
    List {
        /// P, PRP, C, U or CF
        table: String,
        #[arg(long, default_value_t = 1)]
        min_digits: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        /// Maximum 1000
        #[arg(long, default_value_t = 100)]
        limit: u64,
    },
    /// Factors found by ECM / P-1 / P+1 [ecm_list]
    EcmList {
        /// 1 = ECM (Montgomery), 2 = P-1, 3 = P+1, 4 = ECM (Edwards), 0 = all
        #[arg(long = "type", default_value_t = 0, value_name = "T")]
        type_id: i64,
        #[arg(long, default_value_t = 0)]
        min_digits: u64,
        /// Order by discovery time instead of size
        #[arg(long)]
        by_time: bool,
        #[arg(long)]
        descending: bool,
        #[arg(long, default_value_t = 0)]
        skip: u64,
        #[arg(long, default_value_t = 100)]
        limit: u64,
    },

    // ---- Tools & downloads ----
    /// Elliptic-curve group order #E(F_p) for a GMP-ECM (param, sigma) over a prime [ecm_group_order]
    #[command(visible_alias = "group-order")]
    EcmGroupOrder {
        /// A prime of at most 100 digits (expressions allowed)
        number: String,
        /// The sigma value (with --param 0 it must be > 5)
        #[arg(long)]
        sigma: String,
        /// Curve parametrization, as in `ecm -sigma PARAM:SIGMA`: 0 = Suyama (sigma > 5), 1 = GMP-ECM
        /// default, 2 = batch mode 1, 3 = batch mode 2 / GPU
        #[arg(long, visible_alias = "curve", default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=3), value_name = "0-3")]
        param: u8,
    },
    /// Pull a batch of candidate numbers to work on, one per line [download]
    Download {
        /// C, CF, PRP, U or P
        table: String,
        /// Exact decimal digit count
        digits: u64,
        /// How many (1 to 50000)
        #[arg(long, default_value_t = 1000)]
        count: u64,
        /// Start at a random position within that size instead of the smallest
        #[arg(long)]
        random: bool,
        /// Write the numbers to FILE instead of stdout
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    // ---- Account ----
    /// Log in and save the account's API token to the config file [login]
    Login {
        user: String,
        #[arg(short, long, help = PASSWORD_HELP)]
        password: Option<String>,
        /// Print the token instead of saving it
        #[arg(long)]
        no_save: bool,
    },
    /// Create an account and save its API token [register]
    Register {
        /// Login name
        user: String,
        /// Display name (defaults to the login name)
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long, help = PASSWORD_HELP)]
        password: Option<String>,
        /// Print the token instead of saving it
        #[arg(long)]
        no_save: bool,
    },
    /// Identity behind the current (or a given) API token [whoami]
    Whoami {
        /// Token to look up (default: the configured one)
        #[arg(long, value_name = "TOKEN")]
        session: Option<String>,
    },
    /// Issue a fresh API token (invalidates the old one) and save it [regenerate_token]
    RegenerateToken {
        /// Print the new token instead of saving it
        #[arg(long)]
        no_save: bool,
    },
    /// Invalidate the current API token and forget it [logout]
    Logout,
    /// Your resource usage against the per-client limits [quota_status]
    #[command(visible_alias = "quota-status")]
    Quota,

    // ---- Utility ----
    /// Liveness check [health]
    Health,
    /// Call any public method with JSON params (admin methods are not available)
    Call {
        /// Method name, e.g. get_number
        method: String,
        /// Params as a JSON object, e.g. '{"target":{"expr":"2^61-1"}}' ('-' = stdin; default {})
        params: Option<String>,
    },
    /// Send several calls in one round-trip: a JSON array of {method, params} objects, or one per line
    Batch {
        /// File to read ('-' or omitted = stdin)
        file: Option<PathBuf>,
    },
    /// Show or change the saved endpoint, token and timeout
    #[command(subcommand)]
    Config(ConfigCmd),
    /// The type codes accepted by --type
    #[command(hide = true)]
    SeqTypes,
}

#[derive(Subcommand)]
enum CertCmd {
    /// Download the stored certificate: text to stdout (or FILE), metadata to stderr [get_certificate]
    Get {
        #[arg(help = TARGET_HELP)]
        target: String,
        /// Write the certificate text to FILE
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Upload certificate files (Primo / gmp-ECPP / CM); the number is read from the certificate [upload_certificate]
    Upload {
        /// Certificate files ('-' = stdin, once)
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List certificates, smallest first (--descending for largest first) [cert_list]
    List {
        #[arg(long, default_value_t = 300)]
        min_digits: u64,
        /// Only certificates still awaiting verification
        #[arg(long)]
        pending: bool,
        /// Largest first
        #[arg(long)]
        descending: bool,
        #[arg(long, default_value_t = 0)]
        skip: u64,
        #[arg(long, default_value_t = 100)]
        limit: u64,
    },
    /// The chain of certificates a number's certificate depends on [cert_chain]
    Chain {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Certificate totals [cert_stats]
    Stats,
}

#[derive(Subcommand)]
enum SeqCmd {
    /// The terms of a sequence, elf-style: index . value = factors [get_sequence]
    Get {
        #[arg(help = START_HELP)]
        start: String,
        /// First iteration to return
        #[arg(long, default_value_t = 0)]
        from: u64,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
    },
    /// Digit size of each term (the growth graph's data) [sequence_sizes]
    Sizes {
        #[arg(help = START_HELP)]
        start: String,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
    },
    /// Status of a sequence: length, end, merges [sequence_status]
    Status {
        #[arg(help = START_HELP)]
        start: String,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
    },
    /// Advance the frontier, then show status, membership and a slice of terms (a write) [sequence_view]
    View {
        #[arg(help = START_HELP)]
        start: String,
        /// Which terms: all, last, last20, or range (from --fr)
        #[arg(long, default_value = "last20", value_parser = ["all", "last", "last20", "range"])]
        part: String,
        /// First index for --part range
        #[arg(long, default_value_t = 0)]
        fr: u64,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
    },
    /// Compute and store more terms [extend_sequence]
    Extend {
        #[arg(help = START_HELP)]
        start: String,
        /// How many iterations at most
        #[arg(long, default_value_t = 1000)]
        steps: u32,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
    },
    /// Browse known sequences [list_sequences]
    List {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
        /// Start-value magnitude category (0 = up to 1000, 1 = up to 10000)
        #[arg(long, default_value_t = 0)]
        category: u8,
        /// Only sequences ending this way
        #[arg(long, value_parser = ["open", "merge", "cycle", "terminus", "all"], value_name = "KIND")]
        end: Option<String>,
        /// Sort column
        #[arg(long, value_parser = ["length", "start"])]
        sort: Option<String>,
        /// Sort direction
        #[arg(long, value_parser = ["asc", "desc"])]
        dir: Option<String>,
    },
    /// Which sequences a number is a term of [sequence_of]
    Of {
        #[arg(help = TARGET_HELP)]
        target: String,
    },
    /// Advance an open sequence by factoring its last composite locally with gmp-ecm and reporting the
    /// factors (a write) [sequence_view + report_factors]
    #[command(visible_alias = "work")]
    Advance {
        #[arg(help = START_HELP)]
        start: String,
        #[arg(long = "type", visible_alias = "sequence", default_value = "aliquot", help = TYPE_HELP, value_name = "TYPE", value_parser = parse_seq_type)]
        kind: u8,
        /// gmp-ecm processes to run in parallel (default: all CPUs)
        #[arg(short, long, value_name = "N")]
        threads: Option<usize>,
        /// First ECM level: the digit size of the factors searched for (15, 20, 25, to 65)
        #[arg(long, default_value = "20", value_name = "LEVEL", value_parser = advance::parse_level)]
        from: u8,
        /// Last ECM level; the command stops when it is exhausted without a factor
        #[arg(long, default_value = "40", value_name = "LEVEL", value_parser = advance::parse_level)]
        to: u8,
        /// Stop after advancing this many terms (0 = keep going)
        #[arg(long, default_value_t = 0, value_name = "N")]
        terms: u64,
        /// Stop when the composite has more digits than this (0 = no limit)
        #[arg(long, default_value_t = 0, value_name = "DIGITS")]
        max_digits: u64,
        /// The gmp-ecm binary
        #[arg(long, default_value = "ecm", value_name = "PATH")]
        ecm: String,
        /// Progress line while a level runs, every SECS seconds (0 = none)
        #[arg(long, default_value_t = 60, value_name = "SECS")]
        heartbeat: u64,
        /// Print the first factor found instead of reporting it, then stop
        #[arg(long)]
        no_submit: bool,
    },
    /// The type codes accepted by --type
    Types,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the effective settings (token masked)
    Show,
    /// Print the config file path
    Path,
    /// Save a setting
    Set {
        key: ConfigKey,
        /// The new value (a URL, a token, or a number of seconds; 0 = no timeout)
        value: String,
    },
    /// Remove a setting
    Unset { key: ConfigKey },
}

#[derive(ValueEnum, Clone, Copy)]
enum ConfigKey {
    Url,
    Token,
    Timeout,
}

/// Where an effective setting came from, so token handling can tell a saved token from an
/// overriding flag/environment one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Flag,
    Env,
    File,
    Default,
}
impl Source {
    fn describe(self, flag: &str, env: &str) -> String {
        match self {
            Source::Flag => format!("--{flag}"),
            Source::Env => env.to_string(),
            Source::File => format!("the config file {}", config::path().display()),
            Source::Default => "the default".into(),
        }
    }
}

struct Settings {
    url: String,
    url_source: Source,
    token: Option<String>,
    token_source: Source,
    timeout: f64,
    timeout_source: Source,
}

/// Merge flags, environment, config file and defaults. Clap already validated --timeout; a saved
/// timeout is validated here so a bad value fails with a message instead of a panic.
fn resolve_settings(cli: &Cli, cfg: &Config) -> Result<Settings> {
    let env_is = |var: &str, v: &str| std::env::var(var).map(|e| e == v).unwrap_or(false);
    let (url, url_source) = match (&cli.url, &cfg.url) {
        (Some(u), _) => (u.clone(), if env_is("FDB_RPC_URL", u) { Source::Env } else { Source::Flag }),
        (None, Some(u)) if !u.is_empty() => (u.clone(), Source::File),
        _ => (config::DEFAULT_URL.to_string(), Source::Default),
    };
    // An explicit empty --token / FDB_TOKEN means "act anonymously" and overrides a saved token.
    let (token, token_source) = match (&cli.token, &cfg.token) {
        (Some(t), _) => {
            let src = if env_is("FDB_TOKEN", t) { Source::Env } else { Source::Flag };
            (Some(t.clone()).filter(|t| !t.is_empty()), src)
        }
        (None, Some(t)) if !t.is_empty() => (Some(t.clone()), Source::File),
        _ => (None, Source::Default),
    };
    let (timeout, timeout_source) = match (cli.timeout, cfg.timeout) {
        (Some(t), _) => (t, Source::Flag),
        (None, Some(t)) => match config::check_timeout(t) {
            Ok(t) => (t, Source::File),
            Err(msg) => {
                return Err(Error::Usage(format!(
                    "{}: timeout = {t}: {msg}; run `fdb config unset timeout` or `fdb config set timeout <secs>`",
                    config::path().display()
                )));
            }
        },
        (None, None) => (config::DEFAULT_TIMEOUT, Source::Default),
    };
    Ok(Settings { url, url_source, token, token_source, timeout, timeout_source })
}

/// Everything a command needs: the client, the output mode, and the loaded config (for saving).
struct Ctx {
    client: Client,
    json: bool,
    compact: bool,
    cfg: Config,
    settings: Settings,
}

impl Ctx {
    fn print(&self, method: &str, v: &Value) {
        if self.json {
            self.print_json(v);
        } else {
            print!("{}", render::render(method, v));
        }
    }
    fn print_json(&self, v: &Value) {
        if self.compact {
            println!("{v}");
        } else {
            println!("{}", render::pretty(v));
        }
    }
    /// A status line for a human: stdout normally, stderr in JSON mode so stdout stays valid JSON.
    fn status(&self, msg: impl AsRef<str>) {
        if self.json {
            eprintln!("{}", msg.as_ref());
        } else {
            println!("{}", msg.as_ref());
        }
    }
    /// Call a method and print its result.
    fn simple(&self, method: &str, params: Value) -> Result<()> {
        let v = self.client.call(method, params)?;
        self.print(method, &v);
        Ok(())
    }
    /// The same for a call that does long server-side work (a write).
    fn simple_long(&self, method: &str, params: Value) -> Result<()> {
        let v = self.client.call_long(method, params)?;
        self.print(method, &v);
        Ok(())
    }
    fn token_required(&self, what: &str) -> Result<String> {
        match self.client.token() {
            Some(t) if !t.is_empty() => Ok(t.to_string()),
            _ => Err(Error::Usage(format!("{what} needs an API token: run `fdb login <user>` first, or pass --token"))),
        }
    }
    /// The configured token must resolve to an account (the server silently treats a stale token as
    /// anonymous, which would lose upload credit and limits). Returns the `whoami` record.
    fn verify_token(&self, what: &str) -> Result<Value> {
        let tok = self.token_required(what)?;
        let v = self.client.call("whoami", json!({ "session": tok }))?;
        if v.get("found").and_then(Value::as_bool).unwrap_or(false) {
            Ok(v)
        } else {
            Err(Error::Failed(format!(
                "{what}: the API token from {} is not recognised (stale or regenerated); run `fdb login` again, or pass --token '' to act anonymously",
                self.settings.token_source.describe("token", "FDB_TOKEN")
            )))
        }
    }
    /// Keep a token obtained from login / register / regenerate_token. `rotate` marks a regenerated
    /// token: it replaces the old one in the config file only if the old one is what the file holds
    /// (it came from the file, or the flag/environment carried the same token). A different token
    /// from a flag or the environment is never overwritten; the new one is printed instead.
    fn keep_token(&mut self, token: &str, no_save: bool, rotate: bool) -> Result<()> {
        let path = config::path();
        let src = self.settings.token_source;
        let file_holds_old = self.settings.token.is_some() && self.cfg.token == self.settings.token;
        let save = !no_save && (!rotate || src == Source::File || file_holds_old);
        if save {
            self.cfg.token = Some(token.to_string());
            config::save(&self.cfg)?;
            self.status(format!("token saved to {}", path.display()));
            if matches!(src, Source::Flag | Source::Env) {
                eprintln!(
                    "note: {} still takes precedence over the saved token; update or unset it",
                    src.describe("token", "FDB_TOKEN")
                );
            }
        } else {
            if !self.json {
                println!("token        {token}");
            }
            if no_save {
                self.status("token not saved (--no-save); pass it with --token or FDB_TOKEN");
            } else {
                self.status(format!(
                    "token not saved: the old token came from {}, not from {}; update it there",
                    src.describe("token", "FDB_TOKEN"),
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

fn main() {
    // Behave like a Unix filter: exit quietly when the reader goes away (`fdb seq get | head`)
    // instead of panicking on the broken pipe.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // `fdb --help` / `fdb help` / a bare `fdb` show the full reference (every command with all of
    // its options), generated from the command definitions so it cannot drift from them.
    let reference = reference_text();
    let matches = match Cli::command().override_help(reference.clone()).try_get_matches() {
        Ok(m) => m,
        Err(e) => {
            // `fdb id`, `fdb cert get`: a command named with nothing after it shows its own
            // page (arguments and options) instead of a "missing argument" error.
            if e.kind() == clap::error::ErrorKind::MissingRequiredArgument {
                if let Some(page) = bare_command_help() {
                    print!("{page}");
                    std::process::exit(0);
                }
            }
            e.exit();
        }
    };
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let code = match run(cli, &reference) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("fdb: {e}");
            e.exit_code()
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli, reference: &str) -> Result<()> {
    match &cli.cmd {
        None => {
            print!("{reference}");
            return Ok(());
        }
        // Config management never needs the service, and must keep working when the config file
        // is broken (it is the way to repair it).
        Some(Cmd::Config(cc)) => return run_config(&cli, cc),
        Some(_) => {}
    }
    let cfg = config::load()?;
    let settings = resolve_settings(&cli, &cfg)?;
    let timeout = config::timeout_duration(settings.timeout);
    // Long-running writes wait for the server unless the caller set an explicit timeout.
    let long_timeout = if settings.timeout_source == Source::Flag { timeout } else { None };
    let client = Client::new(settings.url.clone(), settings.token.clone(), timeout, long_timeout, cli.verbose);
    let mut ctx = Ctx { client, json: cli.json || cli.compact, compact: cli.compact, cfg, settings };

    match cli.cmd.expect("checked above") {
        Cmd::Config(_) => unreachable!("handled above"),

        // ---- numbers & factors ----
        Cmd::Id { expr, create } => {
            let params = json!({ "expr": expr, "create": create });
            if create { ctx.simple_long("get_id", params) } else { ctx.simple("get_id", params) }
        }
        Cmd::Number { target: t, decimal, detail, full } => {
            let detail = if full { 2 } else { detail };
            ctx.simple("get_number", json!({ "target": target(&t)?, "decimal": decimal, "detail": detail }))
        }
        Cmd::Factors { target: t } => ctx.simple("get_factors", json!({ "target": target(&t)? })),
        Cmd::Primality { target: t } => ctx.simple("primality", json!({ "target": target(&t)? })),
        Cmd::Algebraic { target: t } => ctx.simple("algebraic_factors", json!({ "target": target(&t)? })),
        Cmd::Family { expr, start, limit } => ctx.simple("get_family", json!({ "expr": expr, "start": start, "limit": limit })),
        Cmd::Report { target: t, factors, file } => {
            let mut list = factors;
            if let Some(f) = file {
                for line in read_input(&f)?.lines() {
                    let l = line.trim();
                    if !l.is_empty() && !l.starts_with('#') {
                        list.push(l.to_string());
                    }
                }
            }
            if list.is_empty() {
                return Err(Error::Usage("no factors given (pass them as arguments or with --file)".into()));
            }
            ctx.simple_long("report_factors", json!({ "target": target(&t)?, "factors": list }))
        }

        // ---- proofs ----
        Cmd::Prove { target: t } => ctx.simple_long("prove", json!({ "target": target(&t)? })),
        Cmd::ProofProgress { target: t } => ctx.simple_long("proof_progress", json!({ "target": target(&t)? })),
        Cmd::ProofState { target: t, wait, interval } => {
            let params = json!({ "target": target(&t)? });
            loop {
                let v = ctx.client.call("proof_state", params.clone())?;
                let queued = v.get("queued").and_then(Value::as_bool).unwrap_or(false);
                if !wait || !queued {
                    ctx.print("proof_state", &v);
                    return Ok(());
                }
                eprintln!(
                    "still queued ({} digits, status {}); polling again in {interval}",
                    v.get("digits").and_then(Value::as_u64).unwrap_or(0),
                    v.get("status").and_then(Value::as_str).unwrap_or("?")
                );
                std::thread::sleep(Duration::from_secs(interval.max(1)));
            }
        }
        Cmd::ProofList { type_id, min_digits, descending, skip, limit } => ctx.simple(
            "proof_list",
            json!({ "type_id": type_id, "min_digits": min_digits, "descending": descending, "skip": skip, "limit": limit }),
        ),

        // ---- PRP tests ----
        Cmd::PrpTest { target: t } => ctx.simple_long("prp_test", json!({ "target": target(&t)? })),
        Cmd::PrpTestInfo { target: t } => ctx.simple("prp_test_info", json!({ "target": target(&t)? })),

        // ---- certificates ----
        Cmd::Cert(c) => run_cert(&ctx, c),

        // ---- sequences ----
        Cmd::Seq(sc) => run_seq(&ctx, sc),
        Cmd::SeqTypes => {
            print_seq_types();
            Ok(())
        }

        // ---- statistics ----
        Cmd::Status => ctx.simple("status", json!({})),
        Cmd::Stats => ctx.simple("stats", json!({})),
        Cmd::Smallest => ctx.simple("smallest", json!({})),
        Cmd::CombProgress => ctx.simple("comb_progress", json!({})),
        Cmd::DigitDistribution { start, count } => ctx.simple("digit_distribution", json!({ "start": start, "count": count })),
        Cmd::FactorTables => ctx.simple("factor_tables", json!({})),
        Cmd::List { table, min_digits, offset, limit } => {
            let table = table_name(&table, &["P", "PRP", "C", "U", "CF"])?;
            ctx.simple("list_by_type", json!({ "table": table, "min_digits": min_digits, "offset": offset, "limit": limit }))
        }
        Cmd::EcmList { type_id, min_digits, by_time, descending, skip, limit } => ctx.simple(
            "ecm_list",
            json!({ "type_id": type_id, "min_digits": min_digits, "by_time": by_time, "descending": descending, "skip": skip, "limit": limit }),
        ),

        // ---- tools ----
        Cmd::EcmGroupOrder { number, sigma, param } => {
            let v = ctx.client.call("ecm_group_order", json!({ "number": number, "param": param, "sigma": sigma }))?;
            ctx.print("ecm_group_order", &v);
            if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                return Err(Error::Failed("group order not computed".into()));
            }
            Ok(())
        }
        Cmd::Download { table, digits, count, random, output } => {
            let table = table_name(&table, &["C", "CF", "PRP", "U", "P"])?;
            if count == 0 || count > 50_000 {
                return Err(Error::Usage(format!("--count must be between 1 and 50000, got {count}")));
            }
            let v = ctx.client.call("download", json!({ "table": table, "digits": digits, "count": count, "random": random }))?;
            match output {
                Some(path) => {
                    let mut text = if ctx.json { render::pretty(&v) } else { render::render("download", &v) };
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    std::fs::write(&path, text)?;
                    ctx.status(format!("{} numbers written to {}", v.get("count").and_then(Value::as_u64).unwrap_or(0), path.display()));
                }
                None => ctx.print("download", &v),
            }
            Ok(())
        }

        // ---- account ----
        Cmd::Login { user, password, no_save } => {
            let pass = match password {
                Some(p) => p,
                None => prompt_password("Password: ")?,
            };
            let v = ctx.client.call("login", json!({ "user": user, "pass": pass }))?;
            if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                if ctx.json {
                    ctx.print("login", &v);
                }
                return Err(Error::Failed("login failed: unknown user name or wrong password".into()));
            }
            finish_session(&mut ctx, "login", &v, no_save, false)
        }
        Cmd::Register { user, name, password, no_save } => {
            let pass = match password {
                Some(p) => p,
                None => {
                    let p1 = prompt_password("Password: ")?;
                    if std::io::stdin().is_terminal() {
                        let p2 = prompt_password("Repeat password: ")?;
                        if p1 != p2 {
                            return Err(Error::Usage("passwords do not match".into()));
                        }
                    }
                    p1
                }
            };
            let name = name.unwrap_or_else(|| user.clone());
            let v = ctx.client.call("register", json!({ "user": user, "pass": pass, "name": name }))?;
            if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                if ctx.json {
                    ctx.print("register", &v);
                }
                let why = v.get("error").and_then(Value::as_str).unwrap_or("rejected");
                return Err(Error::Failed(format!("registration failed: {why}")));
            }
            finish_session(&mut ctx, "register", &v, no_save, false)
        }
        Cmd::Whoami { session } => {
            let session = match session {
                Some(s) => s,
                None => ctx.token_required("whoami")?,
            };
            let v = ctx.client.call("whoami", json!({ "session": session }))?;
            ctx.print("whoami", &v);
            if !v.get("found").and_then(Value::as_bool).unwrap_or(false) {
                return Err(Error::Failed("token not recognised".into()));
            }
            Ok(())
        }
        Cmd::RegenerateToken { no_save } => {
            ctx.verify_token("regenerate-token")?;
            let v = ctx.client.call("regenerate_token", json!({}))?;
            if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                if ctx.json {
                    ctx.print("regenerate_token", &v);
                }
                return Err(Error::Failed("token regeneration failed".into()));
            }
            finish_session(&mut ctx, "regenerate_token", &v, no_save, true)
        }
        Cmd::Logout => {
            let tok = ctx.token_required("logout")?;
            let known = ctx.client.call("whoami", json!({ "session": tok }))?;
            let recognised = known.get("found").and_then(Value::as_bool).unwrap_or(false);
            let v = if recognised { ctx.client.call("logout", json!({ "session": tok }))? } else { json!({ "ok": false, "found": false }) };
            if recognised || ctx.json {
                ctx.print("logout", &v);
            }
            if ctx.cfg.token.as_deref() == Some(tok.as_str()) {
                ctx.cfg.token = None;
                config::save(&ctx.cfg)?;
                ctx.status(format!("token removed from {}", config::path().display()));
            }
            if recognised { Ok(()) } else { Err(Error::Failed("token was not valid (already logged out or regenerated); nothing to invalidate".into())) }
        }
        Cmd::Quota => ctx.simple("quota_status", json!({})),

        // ---- utility ----
        Cmd::Health => ctx.simple("health", json!({})),
        Cmd::Call { method, params } => {
            check_public(&method)?;
            let params = match params.as_deref() {
                None => json!({}),
                Some("-") => parse_params(&read_input(&PathBuf::from("-"))?)?,
                Some(p) => parse_params(p)?,
            };
            ctx.simple_long(&method, params)
        }
        Cmd::Batch { file } => {
            let text = read_input(&file.unwrap_or_else(|| PathBuf::from("-")))?;
            let calls = parse_batch(&text)?;
            if calls.is_empty() {
                return Err(Error::Usage("no calls given".into()));
            }
            let results = ctx.client.batch(&calls)?;
            let mut failed = 0usize;
            if ctx.json {
                let items: Vec<Value> = calls
                    .iter()
                    .zip(&results)
                    .map(|(c, r)| match r {
                        Ok(v) => json!({ "method": c.method, "result": v }),
                        Err(Error::Rpc { code, message }) => {
                            failed += 1;
                            json!({ "method": c.method, "error": { "code": code, "message": message } })
                        }
                        Err(e) => {
                            failed += 1;
                            json!({ "method": c.method, "error": { "message": e.to_string() } })
                        }
                    })
                    .collect();
                ctx.print_json(&Value::Array(items));
            } else {
                for (idx, (c, r)) in calls.iter().zip(&results).enumerate() {
                    println!("== {}: {} ==", idx + 1, c.method);
                    match r {
                        Ok(v) => print!("{}", render::render(&c.method, v)),
                        Err(e) => {
                            failed += 1;
                            println!("error: {e}");
                        }
                    }
                }
            }
            if failed > 0 { Err(Error::Failed(format!("{failed} of {} calls failed", calls.len()))) } else { Ok(()) }
        }
    }
}

fn run_cert(ctx: &Ctx, c: CertCmd) -> Result<()> {
    match c {
        CertCmd::Get { target: t, output } => {
            let v = ctx.client.call("get_certificate", json!({ "target": target(&t)? }))?;
            if !v.get("found").and_then(Value::as_bool).unwrap_or(false) {
                if ctx.json {
                    ctx.print("get_certificate", &v);
                }
                return Err(Error::Failed(format!("no certificate stored for {t}")));
            }
            let data = v.get("data").and_then(Value::as_str);
            let too_large = || Error::Failed("certificate is too large to be returned by the RPC".into());
            match output {
                Some(path) => {
                    let Some(d) = data else { return Err(too_large()) };
                    std::fs::write(&path, d)?;
                    if ctx.json {
                        let mut meta = v.clone();
                        meta.as_object_mut().map(|m| m.remove("data"));
                        ctx.print("get_certificate", &meta);
                    } else {
                        print!("{}", render::render("get_certificate", &v));
                    }
                    ctx.status(format!("written to {}", path.display()));
                }
                None => {
                    if ctx.json {
                        ctx.print("get_certificate", &v);
                    } else {
                        eprint!("{}", render::render("get_certificate", &v));
                        // The stored bytes, exactly: no newline is added, so stdout and -o agree.
                        let Some(d) = data else { return Err(too_large()) };
                        std::io::stdout().lock().write_all(d.as_bytes())?;
                    }
                }
            }
            Ok(())
        }
        CertCmd::Upload { files } => run_cert_upload(ctx, &files),
        CertCmd::List { min_digits, pending, descending, skip, limit } => ctx.simple(
            "cert_list",
            json!({ "min_digits": min_digits, "pending": pending, "descending": descending, "skip": skip, "limit": limit }),
        ),
        CertCmd::Chain { target: t } => ctx.simple("cert_chain", json!({ "target": target(&t)? })),
        CertCmd::Stats => ctx.simple("cert_stats", json!({})),
    }
}

/// Upload each file in turn. A bad file or a rejected certificate is counted and the run continues;
/// only a rate-limit / block stops it (every further upload would fail the same way). Uploads are
/// credited to the token's account, so a configured token must be valid, stale one would silently
/// make the upload anonymous.
fn run_cert_upload(ctx: &Ctx, files: &[PathBuf]) -> Result<()> {
    let session = match ctx.client.token() {
        Some(t) if !t.is_empty() => {
            let who = ctx.verify_token("cert upload")?;
            if !ctx.json {
                eprintln!("uploading as {}", who.get("login").and_then(Value::as_str).unwrap_or("?"));
            }
            t.to_string()
        }
        _ => {
            eprintln!("no API token configured: uploading anonymously (no credit)");
            String::new()
        }
    };
    let multi = files.len() > 1;
    let mut stdin_used = false;
    let mut failed = 0usize;
    let mut stopped: Option<Error> = None;
    let mut items: Vec<Value> = Vec::new();
    for f in files {
        let label = if f.as_os_str() == "-" { "stdin".to_string() } else { f.display().to_string() };
        let data = if f.as_os_str() == "-" {
            if stdin_used {
                failed += 1;
                eprintln!("fdb: {label}: stdin can only be read once");
                items.push(json!({ "file": label, "error": "stdin can only be read once" }));
                continue;
            }
            stdin_used = true;
            read_input(f)
        } else {
            read_input(f)
        };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                failed += 1;
                eprintln!("fdb: {label}: {e}");
                items.push(json!({ "file": label, "error": e.to_string() }));
                continue;
            }
        };
        if multi && !ctx.json {
            println!("== {label} ==");
        }
        match ctx.client.call_long("upload_certificate", json!({ "data": data, "session": session })) {
            Ok(v) => {
                let stored = v.get("stored").and_then(Value::as_bool).unwrap_or(false);
                let already = v.get("already_prime").and_then(Value::as_bool).unwrap_or(false);
                let ok = stored || already;
                if !ok {
                    failed += 1;
                }
                if ctx.json {
                    items.push(json!({ "file": label, "result": v }));
                } else {
                    print!("{}", render::render("upload_certificate", &v));
                }
                if !ok {
                    eprintln!("fdb: {label}: certificate not stored (a certificate for that number is probably already on file)");
                }
            }
            Err(e @ (Error::Limited { .. } | Error::Blocked(_))) => {
                failed += 1;
                items.push(json!({ "file": label, "error": e.to_string() }));
                eprintln!("fdb: {label}: {e}; stopping");
                stopped = Some(e);
                break;
            }
            Err(e) => {
                failed += 1;
                eprintln!("fdb: {label}: {e}");
                items.push(json!({ "file": label, "error": e.to_string() }));
            }
        }
    }
    if ctx.json {
        if multi {
            ctx.print_json(&Value::Array(items));
        } else if let Some(only) = items.into_iter().next() {
            ctx.print_json(only.get("result").unwrap_or(&only));
        }
    }
    if let Some(e) = stopped {
        eprintln!("fdb: {failed} of {} uploads failed or were not attempted", files.len());
        return Err(e);
    }
    if failed > 0 { Err(Error::Failed(format!("{failed} of {} uploads failed", files.len()))) } else { Ok(()) }
}

fn run_seq(ctx: &Ctx, sc: SeqCmd) -> Result<()> {
    match sc {
        SeqCmd::Get { start, from, kind } => {
            let start = resolve_start(ctx, &start)?;
            ctx.simple("get_sequence", json!({ "start": start, "from": from, "type": kind }))
        }
        SeqCmd::Sizes { start, kind } => {
            let start = resolve_start(ctx, &start)?;
            ctx.simple("sequence_sizes", json!({ "start": start, "type": kind }))
        }
        SeqCmd::Status { start, kind } => {
            let start = resolve_start(ctx, &start)?;
            ctx.simple("sequence_status", json!({ "start": start, "type": kind }))
        }
        SeqCmd::View { start, part, fr, kind } => {
            let start = resolve_start(ctx, &start)?;
            ctx.simple_long("sequence_view", json!({ "start": start, "type": kind, "part": part, "fr": fr }))
        }
        SeqCmd::Extend { start, steps, kind } => {
            let start = resolve_start(ctx, &start)?;
            ctx.simple_long("extend_sequence", json!({ "start": start, "steps": steps, "type": kind }))
        }
        SeqCmd::List { limit, offset, kind, category, end, sort, dir } => {
            let mut p = json!({ "limit": limit, "offset": offset, "type": kind, "category": category });
            if let Some(e) = end.filter(|e| e != "all") {
                p["end_kind"] = Value::String(e);
            }
            if let Some(s) = sort {
                p["sort"] = Value::String(s);
            }
            if let Some(d) = dir {
                p["dir"] = Value::String(d);
            }
            ctx.simple("list_sequences", p)
        }
        SeqCmd::Of { target: t } => ctx.simple("sequence_of", json!({ "target": target(&t)? })),
        SeqCmd::Advance { start, kind, threads, from, to, terms, max_digits, ecm, heartbeat, no_submit } => {
            let start = resolve_start(ctx, &start)?;
            let threads = threads
                .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
                .max(1);
            let kind_name = seq_type_catalog().into_iter().find(|(c, _, _)| *c == kind).map(|(_, n, _)| n).unwrap_or_else(|| kind.to_string());
            advance::run(
                ctx,
                &advance::Opts {
                    start,
                    kind,
                    kind_name,
                    threads,
                    from,
                    to,
                    terms,
                    max_digits,
                    ecm,
                    heartbeat: Duration::from_secs(heartbeat),
                    submit: !no_submit,
                },
            )
        }
        SeqCmd::Types => {
            print_seq_types();
            Ok(())
        }
    }
}

/// fdb config: works without the service and with a broken config file (except set/unset, which cannot merge into a file they cannot parse).
fn run_config(cli: &Cli, cc: &ConfigCmd) -> Result<()> {
    let json = cli.json || cli.compact;
    let path = config::path();
    let emit = |v: Value| {
        if cli.compact {
            println!("{v}");
        } else {
            println!("{}", render::pretty(&v));
        }
    };
    match cc {
        ConfigCmd::Path => {
            if json { emit(json!({ "path": path.display().to_string() })) } else { println!("{}", path.display()) }
            Ok(())
        }
        ConfigCmd::Show => {
            let (cfg, problem) = match config::load() {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("{e:#}"))),
            };
            let s = resolve_settings(cli, &cfg).unwrap_or_else(|_| Settings {
                url: cli.url.clone().or_else(|| cfg.url.clone()).unwrap_or_else(|| config::DEFAULT_URL.into()),
                url_source: Source::Default,
                token: None,
                token_source: Source::Default,
                timeout: cli.timeout.unwrap_or(config::DEFAULT_TIMEOUT),
                timeout_source: Source::Default,
            });
            let timeout_txt = if s.timeout > 0.0 { format!("{} s", s.timeout) } else { "none".to_string() };
            if json {
                emit(json!({
                    "config_file": path.display().to_string(),
                    "config_error": problem,
                    "url": s.url, "url_source": source_name(s.url_source),
                    "token": s.token.as_deref().map(config::mask),
                    "token_source": source_name(s.token_source),
                    "timeout_secs": s.timeout, "timeout_source": source_name(s.timeout_source),
                    "saved_timeout_invalid": cfg.timeout.map(|t| config::check_timeout(t).is_err()),
                }));
            } else {
                println!("config file  {}{}", path.display(), problem.map(|p| format!("  (unreadable: {p})")).unwrap_or_default());
                println!("url          {}  [{}]", s.url, source_name(s.url_source));
                println!("token        {}  [{}]", s.token.as_deref().map(config::mask).unwrap_or_else(|| "(none)".into()), source_name(s.token_source));
                println!("timeout      {timeout_txt}  [{}]", source_name(s.timeout_source));
                if let Some(t) = cfg.timeout {
                    if let Err(msg) = config::check_timeout(t) {
                        println!("warning      saved timeout {t} is invalid ({msg}); run `fdb config unset timeout`");
                    }
                }
            }
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = config::load().map_err(|e| Error::Failed(format!("{e:#}; fix or delete the file first")))?;
            match key {
                ConfigKey::Url => cfg.url = Some(value.clone()),
                ConfigKey::Token => cfg.token = Some(value.clone()),
                ConfigKey::Timeout => {
                    let t: f64 = value.parse().map_err(|_| Error::Usage(format!("timeout must be a number of seconds, got '{value}'")))?;
                    cfg.timeout = Some(config::check_timeout(t).map_err(Error::Usage)?);
                }
            }
            config::save(&cfg)?;
            if json { emit(json!({ "saved": path.display().to_string() })) } else { println!("saved to {}", path.display()) }
            Ok(())
        }
        ConfigCmd::Unset { key } => {
            let mut cfg = config::load().map_err(|e| Error::Failed(format!("{e:#}; fix or delete the file first")))?;
            match key {
                ConfigKey::Url => cfg.url = None,
                ConfigKey::Token => cfg.token = None,
                ConfigKey::Timeout => cfg.timeout = None,
            }
            config::save(&cfg)?;
            if json { emit(json!({ "saved": path.display().to_string() })) } else { println!("saved to {}", path.display()) }
            Ok(())
        }
    }
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Flag => "flag",
        Source::Env => "environment",
        Source::File => "config file",
        Source::Default => "default",
    }
}

/// After a successful login / register / regenerate_token: show the identity and keep the token.
fn finish_session(ctx: &mut Ctx, method: &str, v: &Value, no_save: bool, rotate: bool) -> Result<()> {
    let token = v.get("token").and_then(Value::as_str).unwrap_or("").to_string();
    if ctx.json {
        ctx.print(method, v);
    } else {
        let mut shown = v.clone();
        shown.as_object_mut().map(|m| m.remove("token"));
        print!("{}", render::render(method, &shown));
    }
    if token.is_empty() {
        return Err(Error::Failed("the service returned no token".into()));
    }
    ctx.keep_token(&token, no_save, rotate)
}

/// A sequence start: a value up to 10^18 (integer or expression), or a stored id given as `id:N`.
/// On the wire a start above 10^18 is a stored id, so a plain value that large is refused.
fn resolve_start(ctx: &Ctx, s: &str) -> Result<u64> {
    let t = s.trim();
    for prefix in ["id:", "fid:"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return rest.trim().parse::<u64>().map_err(|_| Error::Usage(format!("'{s}': an id must be an integer after '{prefix}'")));
        }
    }
    let n = match t.parse::<u64>() {
        Ok(n) => n,
        Err(_) => {
            let v = ctx.client.call("get_number", json!({ "target": { "expr": t }, "decimal": true, "detail": 0 }))?;
            let dec = v.get("decimal").and_then(Value::as_str).unwrap_or("");
            dec.parse::<u64>().map_err(|_| Error::Usage(format!("'{s}' does not evaluate to a whole number up to 10^18")))?
        }
    };
    if n > STARTDB {
        return Err(Error::Usage(format!(
            "start {n} is above 10^18: sequence starts are values up to 10^18 (a stored number can be given as id:N)"
        )));
    }
    if n < 2 {
        return Err(Error::Usage("the start must be an integer of at least 2".into()));
    }
    Ok(n)
}

fn table_name(given: &str, allowed: &[&str]) -> Result<String> {
    let up = given.trim().to_ascii_uppercase();
    if allowed.contains(&up.as_str()) {
        Ok(up)
    } else {
        Err(Error::Usage(format!("table must be one of {}, got '{given}'", allowed.join(", "))))
    }
}

fn check_public(method: &str) -> Result<()> {
    if method.starts_with("admin_") {
        Err(Error::Usage(format!("'{method}' is an admin method; this tool only exposes the public API")))
    } else {
        Ok(())
    }
}

fn parse_params(text: &str) -> Result<Value> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(t).map_err(|e| Error::Usage(format!("params must be a JSON object: {e}")))?;
    if !v.is_object() {
        return Err(Error::Usage("params must be a JSON object, e.g. '{\"target\":{\"expr\":\"2^61-1\"}}'".into()));
    }
    Ok(v)
}

/// A batch file: a JSON array of `{method, params}` objects, or one such object per line.
fn parse_batch(text: &str) -> Result<Vec<Call>> {
    let items: Vec<Value> = match serde_json::from_str::<Value>(text.trim()) {
        Ok(Value::Array(a)) => a,
        Ok(Value::Object(o)) => vec![Value::Object(o)],
        _ => {
            let mut out = Vec::new();
            for (ln, line) in text.lines().enumerate() {
                let l = line.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let v: Value = serde_json::from_str(l).map_err(|e| Error::Usage(format!("line {}: not a JSON object: {e}", ln + 1)))?;
                out.push(v);
            }
            out
        }
    };
    let mut calls = Vec::with_capacity(items.len());
    for (idx, it) in items.into_iter().enumerate() {
        let method = it
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Usage(format!("call {}: missing \"method\"", idx + 1)))?
            .to_string();
        check_public(&method)?;
        let params = match it.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(p) if p.is_object() => p.clone(),
            Some(_) => return Err(Error::Usage(format!("call {} ({method}): params must be an object", idx + 1))),
        };
        calls.push(Call { method, params });
    }
    Ok(calls)
}

fn read_input(path: &PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| Error::Other(anyhow::anyhow!("cannot read {}: {e}", path.display())))
    }
}

fn prompt_password(prompt: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).map_err(|e| Error::Other(anyhow::anyhow!("reading password: {e}")))
    } else {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// The sequence-type catalogue: (code, short name, description). Codes match the core's
/// `SeqKind::from_code` (28 = Sylvester is not served); the names are this tool's shorthand.
fn seq_type_catalog() -> Vec<(u8, String, String)> {
    const HP_HI: [(u8, u8); 25] = [
        (12, 29), (13, 31), (14, 32), (15, 33), (16, 30), (17, 34), (18, 35), (19, 36), (20, 37), (21, 38), (22, 39), (23, 40),
        (24, 41), (25, 42), (26, 43), (27, 44), (28, 45), (29, 46), (30, 47), (31, 48), (32, 49), (33, 50), (34, 51), (35, 52),
        (36, 53),
    ];
    let mut rows: Vec<(u8, String, String)> = vec![(1, "aliquot".into(), "Aliquot sequence (sum of proper divisors)".into())];
    for b in 2..=11u8 {
        rows.push((b, format!("hp{b}"), format!("Home prime, base {b}")));
    }
    for (b, code) in HP_HI {
        rows.push((code, format!("hp{b}"), format!("Home prime, base {b}")));
    }
    for b in 2..=11u8 {
        rows.push((10 + b, format!("ihp{b}"), format!("Inverse home prime, base {b}")));
    }
    for (code, name, label) in [
        (22, "lpf2+1", "largest prime factor ^2 + 1"),
        (23, "lpf2+2", "largest prime factor ^2 + 2"),
        (24, "lpf2-1", "largest prime factor ^2 - 1"),
        (25, "lpf2-2", "largest prime factor ^2 - 2"),
        (26, "lpf3+1", "largest prime factor ^3 + 1"),
        (27, "lpf3-1", "largest prime factor ^3 - 1"),
    ] {
        rows.push((code, name.into(), label.into()));
    }
    rows.sort_by_key(|(code, _, _)| *code);
    rows
}

/// `--type`: a catalogue code or name. Names are matched ignoring case, spaces, `-` and `_`, and
/// the long forms `home-prime-10` / `inverse-home-prime-3` are accepted too.
fn parse_seq_type(input: &str) -> std::result::Result<u8, String> {
    let catalog = seq_type_catalog();
    let t = input.trim();
    if let Ok(code) = t.parse::<u8>() {
        return if catalog.iter().any(|(c, _, _)| *c == code) {
            Ok(code)
        } else {
            Err(format!("{code} is not a sequence type code (see `fdb seq types`)"))
        };
    }
    let norm = |x: &str| x.to_ascii_lowercase().replace([' ', '-', '_'], "");
    let want = norm(t);
    for (code, name, _) in &catalog {
        let long = name.replacen("ihp", "inversehomeprime", 1).replacen("hp", "homeprime", 1);
        if want == norm(name) || want == long {
            return Ok(*code);
        }
    }
    Err(format!("'{input}' is not a sequence type; use a code or name from `fdb seq types` (e.g. aliquot, hp10, ihp3, lpf2+1)"))
}

fn print_seq_types() {
    let rows: Vec<Vec<String>> = seq_type_catalog().into_iter().map(|(c, n, l)| vec![c.to_string(), n, l]).collect();
    let mut o = String::from("--type accepts the code or the name (case-insensitive):\n");
    render::table(&mut o, &["code", "name", "sequence"], &rows, 0);
    print!("{o}");
}

// ---- generated reference -----------------------------------------------------------------------

/// Command groups in display order; every visible subcommand must appear here (checked in tests).
const GROUPS: &[(&str, &[&str])] = &[
    ("Numbers & factors", &["id", "number", "factors", "primality", "algebraic", "family", "report"]),
    ("Primality proofs", &["prove", "proof-progress", "proof-state", "proof-list"]),
    ("Probable-prime tests", &["prp-test", "prp-test-info"]),
    ("Certificates", &["cert"]),
    ("Sequences", &["seq"]),
    ("Statistics & listings", &["status", "stats", "smallest", "comb-progress", "digit-distribution", "factor-tables", "list", "ecm-list"]),
    ("Tools & downloads", &["ecm-group-order", "download"]),
    ("Account", &["login", "register", "whoami", "regenerate-token", "logout", "quota"]),
    ("Utility", &["health", "call", "batch", "config"]),
];

const REF_WIDTH: usize = 100;

/// The command index shown by a bare `fdb`, `fdb --help` and `fdb help`: every command with its
/// arguments on one line, grouped; options live on each command's own page (`fdb <command>`).
/// Built from the clap definitions so it cannot drift from them.
fn reference_text() -> String {
    use std::fmt::Write;
    let cmd = Cli::command();
    let mut o = String::new();
    let _ = writeln!(o, "fdb {} command-line client for the factordb JSON-RPC API", env!("CARGO_PKG_VERSION"));
    o.push_str("\nUsage: fdb [GLOBAL OPTIONS] <COMMAND> [ARGS]\n");
    o.push_str("       fdb <COMMAND>            the command's own page: its arguments and options\n\n");
    // The intro (addressing, settings, exit codes) without its first line, which the title covers.
    let intro = LONG_ABOUT.split_once('\n').map(|(_, rest)| rest.trim_start_matches('\n')).unwrap_or(LONG_ABOUT);
    o.push_str(intro);
    o.push_str("\n\nGlobal options\n");
    for a in cmd.get_arguments().filter(|a| a.is_global_set() && !a.is_hide_set()) {
        arg_line(&mut o, &opt_syntax(a), a, 2);
    }
    for (heading, names) in GROUPS {
        let _ = writeln!(o, "\n{heading}");
        for name in *names {
            let sub = cmd.find_subcommand(name).unwrap_or_else(|| panic!("reference: no subcommand {name}"));
            if sub.has_subcommands() {
                for s2 in sub.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
                    command_line(&mut o, &format!("{name} {}", s2.get_name()), s2);
                }
            } else {
                command_line(&mut o, name, sub);
            }
        }
    }
    o
}

/// One index line: `name <ARGS> [options]` in the left column, the description on the right.
fn command_line(o: &mut String, full_name: &str, sub: &Command) {
    use std::fmt::Write;
    let mut syn = format!("  {full_name}");
    for a in sub.get_positionals().filter(|a| !a.is_hide_set()) {
        let _ = write!(syn, " {}", positional_syntax(a));
    }
    if command_options(sub).next().is_some() {
        syn.push_str(" [options]");
    }
    // The description without its trailing "[rpc_method]" tag (the command's page keeps it).
    let mut about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
    if let Some(idx) = about.rfind(" [") {
        let tag = &about[idx + 1..];
        if tag.ends_with(']') && !tag.contains(' ') {
            about.truncate(idx);
        }
    }
    let aliases: Vec<&str> = sub.get_visible_aliases().collect();
    if !aliases.is_empty() {
        let _ = write!(about, " (also: {})", aliases.join(", "));
    }
    let col = 42;
    if syn.chars().count() + 2 > col {
        o.push_str(&syn);
        o.push('\n');
        wrap_into(o, &about, col, REF_WIDTH);
    } else {
        wrap_first(o, &format!("{syn:<col$}"), &about, col, REF_WIDTH);
    }
}

/// A command's own options (not positionals, not the global ones, not help/version).
fn command_options(sub: &Command) -> impl Iterator<Item = &Arg> {
    sub.get_arguments()
        .filter(|a| !a.is_positional() && !a.is_global_set() && !a.is_hide_set())
        .filter(|a| !matches!(a.get_id().as_str(), "help" | "version"))
}

/// When the command line is just a (possibly nested) command name with nothing after it, the
/// help page of that command; `None` if the arguments are anything else.
fn bare_command_help() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a.starts_with('-')) {
        return None;
    }
    let mut cur = Cli::command();
    let mut path = vec!["fdb".to_string()];
    for name in &args {
        let next = cur.find_subcommand(name)?.clone();
        path.push(next.get_name().to_string());
        cur = next;
    }
    if cur.has_subcommands() {
        return None; // a group: clap's own "requires a subcommand" message lists them
    }
    Some(cur.bin_name(path.join(" ")).render_help().to_string())
}

/// `<NAME>`, `[NAME]`, `<NAME>...` for a positional argument.
fn positional_syntax(a: &Arg) -> String {
    let name = a.get_value_names().and_then(|v| v.first()).map(|s| s.to_string()).unwrap_or_else(|| a.get_id().to_string().to_uppercase());
    let multiple = matches!(a.get_action(), ArgAction::Append);
    let inner = if multiple { format!("{name}...") } else { name };
    if a.is_required_set() { format!("<{inner}>") } else { format!("[{inner}]") }
}

/// `-c, --create` or `--limit <N>` for an option.
fn opt_syntax(a: &Arg) -> String {
    let mut s = String::new();
    if let Some(c) = a.get_short() {
        s.push_str(&format!("-{c}, "));
    }
    if let Some(l) = a.get_long() {
        s.push_str(&format!("--{l}"));
    }
    if !matches!(a.get_action(), ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count | ArgAction::Help | ArgAction::Version) {
        let vn = a.get_value_names().and_then(|v| v.first()).map(|s| s.to_string()).unwrap_or_else(|| a.get_id().to_string().to_uppercase());
        s.push_str(&format!(" <{vn}>"));
    }
    s
}

/// One argument's line: the syntax, then its help with defaults / choices / env, wrapped.
fn arg_line(o: &mut String, syntax: &str, a: &Arg, indent: usize) {
    let mut help = a.get_help().map(|h| h.to_string()).unwrap_or_default();
    let is_flag = matches!(a.get_action(), ArgAction::SetTrue | ArgAction::SetFalse);
    let choices: Vec<String> = a.get_possible_values().iter().map(|p| p.get_name().to_string()).collect();
    if !choices.is_empty() && !is_flag {
        help.push_str(&format!(" [one of: {}]", choices.join(", ")));
    }
    let defaults: Vec<String> = a.get_default_values().iter().map(|d| d.to_string_lossy().into_owned()).collect();
    if !defaults.is_empty() && !matches!(a.get_action(), ArgAction::SetTrue | ArgAction::SetFalse) {
        help.push_str(&format!(" [default: {}]", defaults.join(", ")));
    }
    if let Some(env) = a.get_env() {
        help.push_str(&format!(" [env: {}]", env.to_string_lossy()));
    }
    let aliases: Vec<String> = a.get_visible_aliases().unwrap_or_default().iter().map(|x| format!("--{x}")).collect();
    if !aliases.is_empty() {
        help.push_str(&format!(" [alias: {}]", aliases.join(", ")));
    }
    let col = 28;
    let pad = " ".repeat(indent);
    if syntax.chars().count() + indent + 2 > col {
        o.push_str(&format!("{pad}{syntax}\n"));
        wrap_into(o, &help, col, REF_WIDTH);
    } else {
        let first = format!("{pad}{syntax:<w$}", w = col - indent);
        wrap_first(o, &first, &help, col, REF_WIDTH);
    }
}

/// Word-wrap text at width, every line indented by indent.
fn wrap_into(o: &mut String, text: &str, indent: usize, width: usize) {
    wrap_first(o, &" ".repeat(indent), text, indent, width);
}

/// Like wrap_into, but the first line starts with first (already padded to indent columns).
fn wrap_first(o: &mut String, first: &str, text: &str, indent: usize, width: usize) {
    let mut line = first.to_string();
    let mut len = first.chars().count();
    let mut fresh = true;
    for word in text.split_whitespace() {
        let wl = word.chars().count();
        if !fresh && len + 1 + wl > width {
            o.push_str(line.trim_end());
            o.push('\n');
            line = " ".repeat(indent);
            len = indent;
            fresh = true;
        }
        if !fresh {
            line.push(' ');
            len += 1;
        }
        line.push_str(word);
        len += wl;
        fresh = false;
    }
    o.push_str(line.trim_end());
    o.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every visible subcommand is placed in a reference group (so nothing is left undocumented).
    #[test]
    fn reference_covers_every_command() {
        let cmd = Cli::command();
        let grouped: Vec<&str> = GROUPS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        for sub in cmd.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
            assert!(grouped.contains(&sub.get_name()), "subcommand {} missing from GROUPS", sub.get_name());
        }
        let text = reference_text();
        assert!(text.contains("  id <EXPR> [options]"), "commands appear with their arguments");
        assert!(text.contains("  cert upload <FILES...>"), "nested commands must appear in the index");
        assert!(!text.contains("--create"), "option details belong on the command's own page");
        let page = Cli::command().find_subcommand("id").unwrap().clone().render_help().to_string();
        assert!(page.contains("--create"), "the command page lists its options");
    }
}
