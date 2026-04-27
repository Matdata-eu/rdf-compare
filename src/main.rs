use clap::Parser;
use rdf_compare::cli::Args;
use rdf_compare::diff::{DiffStats, run_diff};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = Args::parse();
    let quiet = args.quiet;
    let ci = args.ci;

    match run_diff(&args) {
        Ok(stats) => {
            if !quiet {
                print_summary(&args, &stats);
            }
            if ci && stats.has_differences() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => {
            eprintln!("rdf-compare: error: {:#}", err);
            ExitCode::from(2)
        }
    }
}

fn print_summary(args: &Args, s: &DiffStats) {
    eprintln!(
        "A: {}  triples={}  only-in-A={}  skipped-bnodes={}",
        args.file_a.display(),
        s.a_total,
        s.a_only,
        s.a_skipped_bnodes
    );
    eprintln!(
        "B: {}  triples={}  only-in-B={}  skipped-bnodes={}",
        args.file_b.display(),
        s.b_total,
        s.b_only,
        s.b_skipped_bnodes
    );
    eprintln!("common={}", s.common);
}
