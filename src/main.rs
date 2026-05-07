use clap::Parser;
use rdf_compare::cli::{Args, Cli, Command, ServeArgs};
use rdf_compare::diff::{DiffStats, compute_diff, write_diff};
use rdf_compare::web::{self, Preload};
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(Command::Serve(s)) = cli.command {
        return run_serve(s);
    }

    let args = cli.diff;
    if args.file_a.is_none() || args.file_b.is_none() {
        eprintln!(
            "error: the following required arguments were not provided:\n  \
             <FILE_A>\n  <FILE_B>\n\nUsage: rdf-compare <FILE_A> <FILE_B>\n\n\
             For more information, try '--help'."
        );
        return ExitCode::from(2);
    }
    if args.view {
        run_view(args)
    } else {
        run_default(args)
    }
}

fn run_default(args: Args) -> ExitCode {
    let start = Instant::now();
    let inputs = rdf_compare::diff::DiffInputs {
        file_a: args.file_a.clone().unwrap(),
        file_b: args.file_b.clone().unwrap(),
        format_a: args.format_a,
        format_b: args.format_b,
        graph_a: args.graph_a.clone(),
        graph_b: args.graph_b.clone(),
    };
    match compute_diff(&inputs).and_then(|r| {
        write_diff(&r, args.output.as_deref(), args.output_format)?;
        Ok(r.stats)
    }) {
        Ok(stats) => {
            let elapsed = start.elapsed();
            if !args.quiet {
                print_summary(&args, &stats, elapsed);
            }
            if args.ci && stats.has_differences() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => {
            eprintln!("rdf-compare: error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run_view(args: Args) -> ExitCode {
    let start = Instant::now();
    let inputs = rdf_compare::diff::DiffInputs {
        file_a: args.file_a.clone().unwrap(),
        file_b: args.file_b.clone().unwrap(),
        format_a: args.format_a,
        format_b: args.format_b,
        graph_a: args.graph_a.clone(),
        graph_b: args.graph_b.clone(),
    };
    let result = match compute_diff(&inputs) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("rdf-compare: error: {err:#}");
            return ExitCode::from(2);
        }
    };
    if args.output.is_some()
        && let Err(err) = write_diff(&result, args.output.as_deref(), args.output_format)
    {
        eprintln!("rdf-compare: error: {err:#}");
        return ExitCode::from(2);
    }
    let stats = result.stats;
    if !args.quiet {
        print_summary(&args, &stats, start.elapsed());
    }
    if let Err(err) = web::run_blocking(&args.bind, !args.no_open, Preload::Loaded(result)) {
        eprintln!("rdf-compare: error: {err:#}");
        return ExitCode::from(2);
    }
    if args.ci && stats.has_differences() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_serve(s: ServeArgs) -> ExitCode {
    let preload = if let Some(diff) = s.diff {
        Preload::Diff {
            diff,
            format: None,
            graph_a: s.graph_a,
            graph_b: s.graph_b,
        }
    } else if let (Some(a), Some(b)) = (s.file_a, s.file_b) {
        Preload::Files {
            file_a: a,
            file_b: b,
            format_a: s.format_a,
            format_b: s.format_b,
            graph_a: s.graph_a,
            graph_b: s.graph_b,
        }
    } else {
        Preload::None
    };
    if let Err(err) = web::run_blocking(&s.bind, !s.no_open, preload) {
        eprintln!("rdf-compare: error: {err:#}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn print_summary(args: &Args, s: &DiffStats, elapsed: std::time::Duration) {
    eprintln!(
        "A: {}  triples={}  only-in-A={}  skipped-bnodes={}",
        args.file_a.as_ref().unwrap().display(),
        s.a_total,
        s.a_only,
        s.a_skipped_bnodes
    );
    eprintln!(
        "B: {}  triples={}  only-in-B={}  skipped-bnodes={}",
        args.file_b.as_ref().unwrap().display(),
        s.b_total,
        s.b_only,
        s.b_skipped_bnodes
    );
    eprintln!("common={}", s.common);
    eprintln!("total-time={:.3}s", elapsed.as_secs_f64());
}
