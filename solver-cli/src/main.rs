// SPDX-License-Identifier: AGPL-3.0-or-later

use solver_cli::{run, EXIT_INVALID_SPEC, EXIT_OK};
use std::process::ExitCode;

fn main() -> ExitCode {
    if !solver_cli::cpu_runs_this_build() {
        eprintln!("solver-cli: this CPU lacks AVX2, FMA or BMI2, which this build needs (x86-64-v3)");
        return ExitCode::from(solver_cli::EXIT_INTERNAL);
    }
    run_cli()
}

#[inline(never)]
fn run_cli() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let progress = args.iter().any(|a| a == "--progress");
    let arg = match args.iter().find(|a| !a.starts_with("--")) {
        Some(a) => a.clone(),
        None if args.iter().any(|a| a == "--version") => "--version".to_string(),
        _ => {
            eprintln!("usage: solver-cli <spec.json> [--progress] | --version");
            return ExitCode::from(EXIT_INVALID_SPEC);
        }
    };

    if arg == "--version" {
        println!("{}", run::engine_sha());
        return ExitCode::from(EXIT_OK);
    }

    match run::execute_reporting(&arg, progress) {
        Ok(json) => {
            println!("{json}");
            ExitCode::from(EXIT_OK)
        }
        Err(e) => {
            eprintln!("{}", e.message);
            ExitCode::from(e.code)
        }
    }
}
