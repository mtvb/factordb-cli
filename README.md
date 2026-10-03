# fdb command-line client for the factordb JSON-RPC API

`fdb` talks to the `fdb-rpc` service (`/rpc`, JSON-RPC 2.0 over HTTP or HTTPS) and exposes every
public method documented on the site's API page as a subcommand. 
Output is human-readable by default; `--json` prints the raw `result`, `--compact` prints it on one line.

## Build

    cargo build --release          
    cargo install --path .
    cargo test         

## Endpoint, token, config

Settings resolve as flag > environment > config file > default:

| setting  | flag        | environment   | default                                   |
|----------|-------------|---------------|-------------------------------------------|
| endpoint | `--url`     | `FDB_RPC_URL` | `http://factordb.com/rpc`                 |
| token    | `--token`   | `FDB_TOKEN`   | none (anonymous); `''` forces anonymous   |
| timeout  | `--timeout` |                | reads 120 s, writes unlimited (see below) |

The config file is `$FDB_CONFIG`, else `$XDG_CONFIG_HOME/fdb/config.toml`, else
`~/.config/fdb/config.toml`. `fdb login <user>` writes the account's API token there (written
atomically with mode 0600). `fdb config show` prints the effective settings and where each came
from; `fdb config set|unset url|token|timeout` edits the file; `fdb config path` prints its path.
These work even when the file is broken (`show`/`path`) so it can be repaired.

The token is sent as `X-Fdb-User-Token`. The server treats an unknown token as anonymous without
complaint, so commands where identity matters check it first: `cert upload` and
`regenerate-token` fail with "token not recognised" rather than quietly acting anonymously, and
`logout` reports a token that was already invalid.

**Token rules.** `login` and `register` save the new token (a note is printed when `--token` or
`FDB_TOKEN` will still take precedence). `regenerate-token` invalidates the old token; the new one
replaces it in the config file only when the old one was the file's token (or the flag/environment
carried that same token); otherwise the new token is printed and the file is left alone. `--no-save`
always prints instead of saving. `logout` invalidates the token and removes it from the file.

**Timeouts.** `--timeout SECS` applies to every call (0 = none). Without it, read calls give up
after 120 s while write calls (`report`, `prove`, `prp-test`, `proof-progress`, `seq extend`,
`seq view`, `cert upload`, `id --create`, `call`) wait for the server, which finishes the work
either way. A saved `timeout` in the config file is validated: only finite values from 0 up.

## Addressing numbers

A `<TARGET>` is an expression - `2^127-1`, `150!`, `10^80+7`, `12345`, or a stored id written
`id:1100000000024481675` (also `fid:N`; `#N` works too but must be quoted in a shell).
Expressions accept `+ - * / ^ %`, `!`, `#`/`##` (primorial), `I(n)`/`lucas(n)` and parentheses.

Values up to 10^18 are literal ids: they are never stored, so `fdb id 12345 --create` reports
`12345 (literal)` with `created no`. Larger values get a `#fid`; `created no` on one of those means
it already existed, and the existing id and status are returned.

A sequence `<START>` is a value up to 10^18, an expression evaluating to one, or a stored number
given as `id:N` (on the wire a start above 10^18 *is* a stored id, so plain values that large are
refused).

**Which sequence family.** Every `seq` subcommand takes `--type` (alias `--sequence`), as a name or
a code: `aliquot` (1, the default), `hp10` = home prime base 10 (also `home-prime-10`), `ihp3` =
inverse home prime base 3, `lpf2+1`, `lpf3-1` = largest-prime-factor sequences, the aliquot
variants `augmented`, `quasi`, `unitary`, `coreful`, `biunitary`, `exponential`, `infinitary`,
Conway's climb to a prime `climb2` .. `climb36` (the factorization read literally, primes with
their exponents), the prime-stopping maps `sigma-1`, `phi+1`, `cototient`, and the concatenation
rules `dhp<b>` / `dihp<b>` (distinct primes, ascending / descending), `schp<b>` (second-class home
prime, one copy of the least prime dropped) and `mdc<b>` (mid-divisor concatenation) for the bases
`b` = 2, 3, 8, 10, 16. Names are case-insensitive; `fdb seq types` lists all codes and names.

## Commands

| command | RPC method | notes |
|---|---|---|
| `id <EXPR> [--create]` | get_id | `--create` stores the number (write); values <= 10^18 are literals and never stored |
| `number <TARGET> [--decimal] [--detail 0..2 \| --full]` | get_number | aliases `get`, `show`; default detail 1 (with factors); detail 2 adds primality, algebraic form and, for a composite in the scanner queue, its trial-division / ECM effort |
| `factors <TARGET>` | get_factors | |
| `primality <TARGET>` | primality | while a certificate is being verified, prints its progress (steps done, elapsed, a digit-weighted work estimate) |
| `algebraic <TARGET>` | algebraic_factors | |
| `family <EXPR> [--start N] [--limit N]` | get_family | `x` is the variable, e.g. `2^x-1` |
| `report <TARGET> [FACTOR] [--file F] [--credit]` | report_factors | write; `--file -` reads stdin; `--credit` records the factors as your contributions (token required; a factor counts when it is new for an existing number and both it and its cofactor have at least 30 digits) |
| `report-file <FILE> [--batch N] [--from-line L] [--dry-run] [--credit]` | report_factors | write; bulk submit, one `NUMBER=FACTOR` per line (`NUMBER` an expression or an id written `#N`; `-` reads stdin); sent as JSON-RPC batches of `--batch` numbers per request (default 100); a `#` not followed by a digit starts a comment; malformed lines are skipped with a warning; `--from-line` resumes a stopped run |
| `prove <TARGET>` | prove | write; large numbers queue |
| `proof-progress <TARGET>` | proof_progress | write (may create the N±1 ids) |
| `proof-state <TARGET> [--wait]` | proof_state | `--wait` polls until dequeued |
| `proof-list [--type 0..3] [--min-digits] [--descending] [--skip] [--limit]` | proof_list | |
| `prp-test <TARGET>` | prp_test | write |
| `prp-test-info <TARGET>` | prp_test_info | |
| `cert get <TARGET> [-o FILE]` | get_certificate | the stored bytes, exactly, to stdout (metadata on stderr) or to FILE |
| `cert upload <FILE>` | upload_certificate | write; `-` = stdin (once); see below |
| `cert list [--min-digits] [--pending] [--descending] [--skip] [--limit] [--user UID] [--software ID]` | cert_list | smallest first; `--descending` for largest first; a certificate under verification shows `processing done/total`; `--user 0` = anonymous uploads |
| `cert chain <TARGET> [--skip N] [--limit N]` | cert_chain | the whole chain by default; `--skip`/`--limit` page it (a paged reply says `steps a-b of total`) |
| `cert stats` | cert_stats | totals, plus one `verifying #id` line per verification in flight |
| `cert top [--by user\|software] [--sort score\|n\|size]` | cert_top / cert_software_top | the certificate leaderboards: top 100 uploaders, or the programs and their top 100 versions; score = sum of (digits/1000)^4 over verified certificates |
| `seq get <START> [--from N] [--type T]` | get_sequence | elf-style `index . value = factors`; `--type` = family name or code |
| `seq sizes <START> [--type T]` | sequence_sizes | |
| `seq status <START> [--type T]` | sequence_status | |
| `seq view <START> [--part all\|last\|last20\|range] [--fr N] [--type T]` | sequence_view | write (advances the frontier first) |
| `seq extend <START> [--steps N] [--type T]` | extend_sequence | write |
| `seq advance <START> [--type T] [--threads N] [--from L] [--to L] [--terms N] [--max-digits D] [--ecm PATH] [--heartbeat S] [--no-submit] [--credit]` | sequence_view + report_factors | alias `work`; factors the open frontier locally with gmp-ecm over the t20..t65 ladder and reports each factor (write); `--credit` as in `report` |
| `seq list [--limit] [--offset] [--type] [--category] [--end KIND] [--sort] [--dir] [--driver CODE] [--guide V]` | list_sequences | `--guide` filters on the frontier's exact guide value (finer than `--driver`) |
| `seq of <TARGET>` | sequence_of | |
| `seq types` | the `--type` codes and names (aliquot and its variants, hp2..hp36, ihp2..ihp36, lpf, climb2..climb36, sigma-1, phi+1, cototient, dhp/dihp/schp/mdc on bases 2, 3, 8, 10, 16) |
| `status` | status | the whole status page |
| `stats` | stats | |
| `smallest` | smallest | |
| `comb-progress` | comb_progress | |
| `digit-distribution [--start] [--count]` | digit_distribution | stored numbers begin at 19 digits |
| `factor-tables` | factor_tables | |
| `list <TABLE> [--min-digits] [--offset] [--limit]` | list_by_type | P, PRP, C, U, CF |
| `ecm-list [--type 0..4] [--min-digits] [--by-time] [--descending] [--skip] [--limit]` | ecm_list | |
| `ecm-group-order <PRIME> --sigma S [--param 0-3]` | ecm_group_order | alias `group-order`; `--param` (alias `--curve`) picks the GMP-ECM parametrization, see below |
| `download <TABLE> <DIGITS> [--count 1..50000] [--random] [-o FILE]` | download | one number per line; nothing at all when empty |
| `login <USER> [-p PASS] [--no-save]` | login | prompts for the password (or reads stdin when piped); saves the token |
| `register <USER> [--name N] [-p PASS] [--no-save]` | register | |
| `whoami [--session TOKEN]` | whoami | |
| `regenerate-token [--no-save]` | regenerate_token | see token rules |
| `logout` | logout | also forgets the saved token |
| `quota` | quota_status | alias `quota-status` |
| `contributions [--skip] [--limit]` | contributions | your credited factors, newest first (token required); earned with `report --credit` / `seq advance --credit` or the website's "Credit my factors" setting |
| `health` | health | |
| `call <METHOD> [PARAMS-JSON]` | any public method | escape hatch for new methods; `-` = stdin |
| `batch [FILE]` | (batch request) | JSON array of `{method, params}` or one object per line |
| `config show \| path \| set KEY VALUE \| unset KEY` | KEY = url, token, timeout |

Global flags: `--url`, `--token`, `-j/--json`, `--compact`, `--timeout SECS`, `-v/--verbose`
(logs each request and the server's `X-Fdb-Resources` accounting to stderr).

**ECM curve parametrization.** `--param` is GMP-ECM's `-param`, exactly as in `ecm -sigma
PARAM:SIGMA`: `0` Suyama (sigma must be > 5), `1` GMP-ECM default (the CLI default, like the web
page), `2` batch mode 1, `3` batch mode 2 / GPU. `--curve` is an alias.

**Certificate uploads.** With a token configured, the token is verified first and the uploads are
credited to that account; without one, the upload is anonymous (a note is printed; `--token ''`
forces this). Each file is uploaded in turn: an unreadable file or a rejected certificate is
reported and counted, and the run continues; only a rate limit / block stops it. The exit code is
1 if any file failed or the server did not store a certificate (for instance because one is already
on file for that number, which is not the same as `already_prime`). `-` (stdin) can be given once.

**Passwords.** Prefer the prompt, or pipe the password on stdin; `-p` puts it in the process list
and shell history.

**JSON mode.** With `--json`/`--compact`, stdout carries only JSON: status lines such as
"token saved to" go to stderr, a multi-file `cert upload` prints one array, and the `config`
commands print objects.

## Examples

    fdb                                   # the command index
    fdb id                                # one command's page (its options)
    fdb number 2^127-1 --full
    fdb factors id:1100000000024482356
    fdb report 1427247692705959880439315947500961989719490561 2^61-1
    fdb report id:1100000000024531041 2^607-1 --credit   # credited to your account
    fdb contributions
    fdb download C 60 --count 100 --random > work.txt
    fdb seq get 276 --from 1900
    fdb seq get 20 --type climb10                       # Conway's climb to a prime
    fdb seq list --type dihp10 --end cycle              # the 713 -> 3123 -> 3473 -> 15123 cycle
    fdb seq advance 276 --threads 16 --to 45 --credit
    fdb cert get id:1100000000024481681 -o m1000.cert
    fdb cert upload *.out
    fdb ecm-group-order 1000003 --sigma 7 --param 0
    fdb -j status | jq .stats
    printf '{"method":"stats"}\n{"method":"get_id","params":{"expr":"2^61-1"}}\n' | fdb batch
    fdb call get_family '{"expr":"2^x-1","start":100,"limit":5}'

## Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | the call failed: JSON-RPC error, rejected login, nothing found, a failed batch item or upload |
| 2 | usage error (bad target, unknown table, admin method, missing token, invalid timeout) |
| 3 | service unreachable, timed out, or a non-JSON / unexpected HTTP response |
| 4 | HTTP 429: request rate or a resource quota exhausted (`Retry-After` is shown) |
| 5 | HTTP 403: this client address is blocked |

`fdb` dies quietly on a closed pipe (`fdb seq get 276 | head`), like other Unix filters.

## Known server-side quirks (not CLI bugs)

Seen through the CLI against fdbtest; they live in `fdb-rpc` / `fdb-service`:
`digit_distribution` returns `count + 1` rows; `download P 0` returns 19-digit primes;
`report_factors` on a small literal accepts a non-dividing factor and a non-number factor yields
`term: Empty`; `seq get 12` shows a `0` term past the end, and `seq extend 12` reports the base-1 leg.
