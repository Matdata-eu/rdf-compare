# syntax=docker/dockerfile:1

# ---- build -------------------------------------------------------------------
FROM rust:1-slim-bookworm AS build
WORKDIR /src

# Build dependencies first so they are cached independently of the sources.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY assets ./assets
COPY THIRD_PARTY_NOTICES.md ./
# Touch the sources so cargo rebuilds the crate instead of reusing the stub.
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked

# ---- runtime -----------------------------------------------------------------
# distroless/cc ships glibc + libgcc only, runs as uid 65532 ("nonroot").
FROM gcr.io/distroless/cc-debian12:nonroot

LABEL org.opencontainers.image.title="rdf-compare" \
      org.opencontainers.image.description="Diff two RDF files; web viewer over a mounted data directory" \
      org.opencontainers.image.source="https://github.com/Matdata-eu/rdf-compare" \
      org.opencontainers.image.authors="Mathias Vanden Auweele <mathias@matdata.eu>" \
      org.opencontainers.image.licenses="Apache-2.0"

COPY --from=build /src/target/release/rdf-compare /usr/local/bin/rdf-compare
COPY LICENSE THIRD_PARTY_NOTICES.md /usr/share/doc/rdf-compare/

# RDF files are expected on a volume mounted here; the browser loader only
# accepts paths inside it.
ENV RDF_COMPARE_DATA_DIR=/data \
    RDF_COMPARE_BIND=0.0.0.0:8080
VOLUME ["/data"]
WORKDIR /data
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/rdf-compare"]
CMD ["serve", "--no-open"]
