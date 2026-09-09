//! `usage-probe` — can live readings work on this machine?
//!
//!   usage-probe            read Claude Code's login, fetch, print the numbers
//!   usage-probe --where    say where the credential was found, and stop
//!   usage-probe --why      say what each credential lookup did, and stop
//!
//! Answers the question the settings toggle asks, without touching settings,
//! so a failure can be diagnosed from a terminal. The credential is never
//! printed, logged, or passed as an argument.

use token_ledger::ledger::usageapi;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);

    if flag("-h") || flag("--help") {
        eprintln!("{}", include_str!("usage-probe.rs").lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").trim_start())
            .collect::<Vec<_>>().join("\n"));
        return;
    }

    if flag("--why") {
        for (account, outcome) in usageapi::credential_attempts() {
            println!("  {account:24} {outcome}");
        }
        return;
    }

    match usageapi::credential_source() {
        Ok(src) => println!("credential: {}", src.label()),
        Err(e) => {
            eprintln!("credential: {}", e.message());
            std::process::exit(1);
        }
    }
    if flag("--where") {
        return;
    }

    println!("GET {}", usageapi::ENDPOINT);
    match usageapi::fetch() {
        Ok(c) => {
            println!("ok — fetched {}", c.fetched_at.to_rfc3339());
            for (name, r) in [("session", c.session), ("weekly", c.weekly), ("fable", c.fable)] {
                match r {
                    Some(r) => println!(
                        "  {name:<8} {:>5.0}%  resets {}",
                        r.percent,
                        r.resets_at.map(|t| t.to_rfc3339()).unwrap_or_else(|| "—".into())
                    ),
                    None => println!("  {name:<8}     —"),
                }
            }
        }
        Err(e) => {
            eprintln!("failed: {}", e.message());
            std::process::exit(1);
        }
    }
}
