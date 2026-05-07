use clap::Parser;
use rdf_compare::cli::Args;
use rdf_compare::diff::{DiffStats, run_diff};
use rdf_compare::webviewer::run_webviewer;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let args = Args::parse();
    let quiet = args.quiet;
    let ci = args.ci;

    let start = Instant::now();
    if args.webviewer {
        match run_webviewer(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("rdf-compare: error: {:#}", err);
                ExitCode::from(2)
            }
        }
    } else {
        match run_diff(&args) {
            Ok(stats) => {
                let elapsed = start.elapsed();
                if !quiet {
                    print_summary(&args, &stats, elapsed);
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
}

fn print_summary(args: &Args, s: &DiffStats, elapsed: std::time::Duration) {
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
    eprintln!("total-time={:.3}s", elapsed.as_secs_f64());
}
