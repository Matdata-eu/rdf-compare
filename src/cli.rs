use anyhow::{Result, anyhow, bail};
use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};

/// Compare two RDF files and emit the diff as a quad dataset with two named graphs.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// First (left) RDF file.
    pub file_a: PathBuf,
    /// Second (right) RDF file.
    pub file_b: PathBuf,

    /// Override input format for file A (auto-detected from extension by default).
    #[arg(long = "format-a", value_enum)]
    pub format_a: Option<InputFormat>,

    /// Override input format for file B (auto-detected from extension by default).
    #[arg(long = "format-b", value_enum)]
    pub format_b: Option<InputFormat>,

    /// Output file (defaults to stdout).
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Output serialization.
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Trig)]
    pub output_format: OutputFormat,

    /// Override the named-graph IRI for triples only in file A.
    #[arg(long = "graph-a")]
    pub graph_a: Option<String>,

    /// Override the named-graph IRI for triples only in file B.
    #[arg(long = "graph-b")]
    pub graph_b: Option<String>,

    /// Suppress the summary line on stderr.
    #[arg(long)]
    pub quiet: bool,

    /// Exit with code 1 if any differences are found (useful in CI).
    #[arg(long)]
    pub ci: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputFormat {
    /// N-Triples (.nt)
    Nt,
    /// Turtle (.ttl)
    Ttl,
    /// RDF/XML (.rdf, .owl, .xml)
    #[value(alias = "xml")]
    Rdf,
    /// TriG (.trig) — graph component is dropped
    Trig,
    /// N-Quads (.nq) — graph component is dropped
    Nq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// TriG (Turtle-based, named graphs)
    Trig,
    /// N-Quads (line-based)
    Nq,
}

/// Detect input format from a path. Strips a trailing `.gz` first.
pub fn detect_format(path: &Path) -> Result<InputFormat> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("path has no filename: {}", path.display()))?
        .to_ascii_lowercase();

    let stem = name.strip_suffix(".gz").unwrap_or(&name);
    let ext = stem.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    match ext {
        "nt" | "ntriples" => Ok(InputFormat::Nt),
        "ttl" | "turtle" => Ok(InputFormat::Ttl),
        "rdf" | "owl" | "xml" => Ok(InputFormat::Rdf),
        "trig" => Ok(InputFormat::Trig),
        "nq" | "nquads" => Ok(InputFormat::Nq),
        "" => bail!(
            "could not detect RDF format for {} (no extension); use --format-a/--format-b",
            path.display()
        ),
        other => bail!(
            "unknown RDF extension '.{}' for {}; use --format-a/--format-b",
            other,
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_basic_extensions() {
        assert_eq!(detect_format(Path::new("a.ttl")).unwrap(), InputFormat::Ttl);
        assert_eq!(detect_format(Path::new("a.nt")).unwrap(), InputFormat::Nt);
        assert_eq!(
            detect_format(Path::new("a.rdf")).unwrap(),
            InputFormat::Rdf
        );
        assert_eq!(
            detect_format(Path::new("a.trig")).unwrap(),
            InputFormat::Trig
        );
        assert_eq!(detect_format(Path::new("a.nq")).unwrap(), InputFormat::Nq);
    }

    #[test]
    fn detects_gzipped_extensions() {
        assert_eq!(
            detect_format(Path::new("a.ttl.gz")).unwrap(),
            InputFormat::Ttl
        );
        assert_eq!(
            detect_format(Path::new("a.nt.gz")).unwrap(),
            InputFormat::Nt
        );
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(detect_format(Path::new("a.foo")).is_err());
        assert!(detect_format(Path::new("noext")).is_err());
    }
}
