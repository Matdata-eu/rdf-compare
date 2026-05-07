# rdf-compare

A small, fast CLI to compute the diff between two RDF files. The output is itself an
RDF dataset — a [TriG](https://www.w3.org/TR/trig/) or
[N-Quads](https://www.w3.org/TR/n-quads/) document containing two named graphs:

- the named graph for **file A** holds triples present in A but not in B;
- the named graph for **file B** holds triples present in B but not in A.

Triples that appear in both files are omitted (they are the "common core").

## Features

- **Streaming.** File B is streamed; only file A is loaded into memory (as a hash
  set of triples), so peak memory is `O(|A|)`.
- **Multiple input formats** auto-detected from the extension:
  N-Triples (`.nt`), Turtle (`.ttl`), RDF/XML (`.rdf`, `.owl`, `.xml`),
  TriG (`.trig`), N-Quads (`.nq`).
- **Transparent gzip** for any input ending in `.gz`.
- **Two output formats:** TriG (default) and N-Quads.
- **Filename-derived graph IRIs** (`urn:rdf-compare:source:<basename>`) with
  automatic `:1` / `:2` disambiguation when both files share a basename.
- **CI mode** (`--ci`) exits non-zero when any difference is found.
- **Blank-node-safe.** Triples touching blank nodes are skipped (without a
  canonicalisation step they cannot be reliably diffed); the count of skipped
  triples is reported in the summary.

## Install

From source (requires a stable Rust toolchain):

```sh
cargo install --path .
```

Or, once published:

```sh
cargo install rdf-compare
```

## Usage

```sh
rdf-compare <FILE_A> <FILE_B> [OPTIONS]
```

Examples:

```sh
# Diff two Turtle files, write TriG to stdout
rdf-compare a.ttl b.ttl

# Cross-format diff, write N-Quads to a file
rdf-compare snapshot-old.nt.gz snapshot-new.ttl \
    --output-format nq -o diff.nq

# Use in CI: exit 1 when files differ
rdf-compare expected.ttl actual.ttl --ci --quiet -o /dev/null

# Start local webviewer UI
rdf-compare a.ttl b.ttl --webviewer
```

### Options

| Flag | Description |
| --- | --- |
| `--format-a <FMT>` | Force input format for file A (`nt`, `ttl`, `rdf`/`xml`, `trig`, `nq`). |
| `--format-b <FMT>` | Force input format for file B. |
| `-o`, `--output <FILE>` | Write output to `FILE` instead of stdout. |
| `--output-format <FMT>` | `trig` (default) or `nq`. |
| `--graph-a <IRI>` | Override the named-graph IRI for "only-in-A" triples. |
| `--graph-b <IRI>` | Override the named-graph IRI for "only-in-B" triples. |
| `--quiet` | Suppress the summary line on stderr. |
| `--ci` | Exit with code 1 if any differences are found. |
| `--webviewer` | Start local webviewer UI instead of writing diff output. |
| `--webviewer-host <HOST>` | Webviewer bind host (default `127.0.0.1`). |
| `--webviewer-port <PORT>` | Webviewer bind port (default `7878`). |

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success (or, without `--ci`, completed even if differences exist). |
| `1` | `--ci` was set and at least one difference was found. |
| `2` | Error (I/O, parse, etc.). |

## Example output

Given:

```turtle
# a.ttl
@prefix ex: <http://example.org/> .
ex:s1 ex:p "v1" .
ex:s3 ex:p "vA" .
```

```turtle
# b.ttl
@prefix ex: <http://example.org/> .
ex:s1 ex:p "v1" .
ex:s3 ex:p "vB" .
ex:s4 ex:p "v4" .
```

`rdf-compare a.ttl b.ttl` produces:

```trig
<urn:rdf-compare:source:a> {
    <http://example.org/s3> <http://example.org/p> "vA" .
}
<urn:rdf-compare:source:b> {
    <http://example.org/s3> <http://example.org/p> "vB" .
    <http://example.org/s4> <http://example.org/p> "v4" .
}
```

…with a summary on stderr:

```
A: a.ttl  triples=2  only-in-A=1  skipped-bnodes=0
B: b.ttl  triples=3  only-in-B=2  skipped-bnodes=0
common=1
```

## Limitations

- **Blank nodes are not diffed.** Computing equivalence for triples involving
  blank nodes requires graph canonicalisation (e.g. RDFC-1.0). Such triples are
  reported as `skipped-bnodes` and excluded from both the comparison and the
  output.
- **Quad inputs are flattened.** The graph component of N-Quads / TriG inputs is
  dropped; only the (s, p, o) triples are compared.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

## License

Apache-2.0.
