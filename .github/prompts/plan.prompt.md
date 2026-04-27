# Plan: RDF diff CLI (kg-compare)

A small fast Rust CLI that compares two RDF files and emits the differences as an RDF dataset with two named graphs (only-in-A, only-in-B). Built on the Oxigraph crate family for streaming parsers/serializers covering N-Triples, Turtle, RDF/XML, TriG, N-Quads. Blank-node-containing triples are skipped (per decision). Format auto-detected from extension (incl. `.gz`) with override flags. Output is TriG (default) or N-Quads.

## Approach (algorithm)

1. Parse file A in streaming mode; insert each ground triple (no blank nodes) into a `HashSet<Triple>`. Skip triples with a blank node as subject or object; track skipped count.
2. Stream-parse file B triple by triple:
   - If present in A's set → remove it from the set (it's common, discard).
   - Else → emit a `Quad(triple, graph_b)` to the output serializer immediately.
   - Skip blank-node triples; track count.
3. After B is exhausted, drain remaining entries of A's set as `Quad(triple, graph_a)` to the output.
4. Memory cost: O(|A|) only. B is fully streamed. This satisfies the low-memory goal for the common case where one side fits in memory; document this clearly. (No on-disk sort/spill in v1.)

## Steps

### Phase 1 — Project scaffolding
1. `cargo init` a binary crate at the workspace root (name `kg-compare`).
2. Add dependencies in `Cargo.toml`:
   - `oxrdf` — RDF model (NamedNode, Triple, Quad, GraphName, Term, Subject)
   - `oxttl` — streaming readers/writers for N-Triples, Turtle, TriG, N-Quads
   - `oxrdfxml` — RDF/XML reader
   - `clap` (derive feature) — CLI parsing
   - `flate2` — gzip decoding
   - `anyhow` — error context
   - `thiserror` — typed errors (optional, only if needed)
3. Configure release profile with `lto = "thin"`, `codegen-units = 1` for a small fast binary.

### Phase 2 — CLI surface (`src/cli.rs`)
4. Define `Args` with clap derive:
   - positional `file_a: PathBuf`, `file_b: PathBuf`
   - `--format-a <fmt>` / `--format-b <fmt>` overrides (values: `nt`, `ttl`, `rdf`/`xml`, `trig`, `nq`)
   - `--output / -o <path>` (default stdout)
   - `--output-format <trig|nq>` (default `trig`)
   - `--graph-a <iri>` / `--graph-b <iri>` (optional override; default derived from filename — see Phase 4)
   - `--quiet` to suppress stderr summary
   - `--ci` to set exit code 1 on any difference
5. Implement format auto-detection helper: peel `.gz` suffix first, then map remaining extension (`.nt|.ntriples`, `.ttl|.turtle`, `.rdf|.owl|.xml`, `.trig`, `.nq|.nquads`). Error with clear message if unknown and no override.

### Phase 3 — Input readers (`src/input.rs`)
6. `open_reader(path) -> Box<dyn BufRead>`: open file; if path ends in `.gz`, wrap in `flate2::read::MultiGzDecoder`; wrap in `BufReader`.
7. `parse_triples(reader, format, on_triple, on_skipped_bnode)` — dispatches to:
   - `oxttl::NTriplesParser` / `TurtleParser` / `TriGParser` / `NQuadsParser` (their `for_reader` streaming iterator API)
   - `oxrdfxml::RdfXmlParser`
   - For quad formats (TriG, N-Quads): collapse to triples, ignoring graph component (document this; v1 treats inputs as triple sets).
   - For each parsed triple: if subject or object is a blank node → increment skipped counter and continue; else hand to callback.

### Phase 4 — Graph IRI derivation (`src/graph_iri.rs`)
8. Derive a stable IRI per source file (used unless `--graph-a/--graph-b` overrides). Strategy:
   - Take file basename without extension; sanitize (percent-encode non-IRI chars).
   - Build IRI: `urn:kg-compare:source:<sanitized-basename>`.
   - If both files yield the same IRI (e.g. comparing two `data.ttl` from different dirs), append a numeric suffix `:1` / `:2`.
9. Validate result via `oxrdf::NamedNode::new` and bail with helpful message on failure.

### Phase 5 — Diff core (`src/diff.rs`)
10. `run_diff(args) -> Result<DiffStats>`:
    - Build graph IRIs (`graph_a`, `graph_b`).
    - Open output writer (file or stdout) and create the appropriate serializer:
      - TriG → `oxttl::TriGSerializer` (streaming `for_writer`)
      - N-Quads → `oxttl::NQuadsSerializer`
    - Parse A → `HashSet<Triple>` (and skipped-bnode counter `a_skipped`).
    - Stream B; for each triple decide common vs only-in-B; write only-in-B quads as we go (`b_only` counter, `b_skipped` counter).
    - Drain remaining A entries as only-in-A quads (`a_only` counter).
    - Finish/flush serializer.
    - Return `DiffStats { a_total, b_total, common, a_only, b_only, a_skipped, b_skipped }`.

### Phase 6 — Reporting & exit code (`src/main.rs`)
11. Wire `main()`: parse args, call `run_diff`, on success print summary to stderr unless `--quiet`:
    ```
    A: <path>  triples=<n>  only-in-A=<n>  skipped-bnodes=<n>
    B: <path>  triples=<n>  only-in-B=<n>  skipped-bnodes=<n>
    common=<n>
    ```
12. Exit code: `0` if no differences (or `--ci` not set), `1` if `--ci` and (`a_only > 0 || b_only > 0`), `2` on errors.

### Phase 7 — Tests (`tests/`)
13. Integration test helper that runs the binary against fixture pairs in `tests/fixtures/` and asserts output dataset.
14. Cases: identical files; disjoint files; mixed Turtle vs N-Triples; gzipped input; RDF/XML input; format override flag; blank-node skip behavior; `--ci` exit code; same-basename collision producing distinct graphs; output to TriG and N-Quads.

## Relevant files

- `Cargo.toml` — crate metadata + dependency list (above).
- `src/main.rs` — entrypoint, error → exit-code mapping, summary printing.
- `src/cli.rs` — `clap` derive `Args` struct + format enum + auto-detect helper.
- `src/input.rs` — gzip-aware reader + format-dispatching streaming parser.
- `src/graph_iri.rs` — filename → IRI derivation + collision suffix.
- `src/diff.rs` — `run_diff`, `DiffStats`, set-based diff algorithm.
- `tests/cli.rs` + `tests/fixtures/*` — end-to-end tests.

## Verification

1. `cargo build --release` produces a single binary; check size (target ≤ a few MB).
2. `cargo test` — all integration cases pass.
3. Manual: round-trip a known pair `a.ttl` / `b.ttl` and verify TriG output contains exactly the expected quads in two named graphs.
4. Performance smoke: run against a 1M-triple N-Triples file vs a perturbed copy; confirm completion in seconds and bounded memory (RSS ≈ size of A's triple set).
5. Confirm exit codes: `0` on identical; `1` on diff with `--ci`; non-zero with stderr error on malformed input.
6. Confirm `.gz` inputs work transparently and that `--format-a nt` overrides extension correctly.

## Decisions

- **Stack**: Rust + oxrdf/oxttl/oxrdfxml.
- **Output**: TriG (default) and N-Quads, selectable with `--output-format`.
- **Graph naming**: derived from filename basename (`urn:kg-compare:source:<name>`), with `--graph-a/--graph-b` override and automatic `:1`/`:2` suffix on collision.
- **Blank nodes**: triples involving blank nodes are skipped on both sides; counted and reported in the summary.
- **Quad input formats** (TriG/N-Quads): graph component is dropped; inputs are treated as triple sets in v1.
- **Memory model**: file A loaded into a `HashSet<Triple>`; file B fully streamed. No on-disk spill in v1.
- **Excluded from v1**: blank-node isomorphism, RDF-star, dataset-level diff (per-graph), config file, parallel parsing.

## Further Considerations

1. Should the output dataset also include provenance metadata (e.g. a default-graph triple stating source file paths and timestamps)? Recommendation: **yes, minimal** — emit a few `void:Dataset` / `dct:source` triples in the default graph so the diff is self-describing. Option A: include by default. Option B: behind `--with-metadata`. Option C: never. *Recommended: A.*
2. If both inputs are quad formats, would you eventually want a true dataset diff (per named graph)? Out of scope for v1; flag for v2 if needed.
