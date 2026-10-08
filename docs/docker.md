# Running rdf-compare in a container

The image packages the same `rdf-compare` binary as the release archives. By
default it starts the web viewer (`serve`) on port 8080 and lets the browser
load RDF files from a volume mounted at `/data`. Every CLI invocation works
too: pass arguments and they replace the default `serve` command.

Deploying the image (Kubernetes manifests, Helm charts, ingress, auth) is out
of scope for this project. The notes below cover what a deployment needs to
know about the image.

## Image

| | |
| --- | --- |
| Registry | `ghcr.io/matdata-eu/rdf-compare` |
| Tags | `main`, `sha-<short>`, and `<version>` / `<major>.<minor>` for `v*` tags |
| Base | `gcr.io/distroless/cc-debian12:nonroot` (no shell, no package manager) |
| User | `65532:65532` (`nonroot`) |
| Port | `8080` |
| Volume / workdir | `/data` |
| Entrypoint | `rdf-compare` |
| Default command | `serve --no-open` |

Build it locally with:

```sh
docker build -t rdf-compare .
```

The image is built for `linux/amd64` in CI. For `arm64`, build on an arm64
host (or with `docker buildx build --platform linux/arm64`; under QEMU the
release build is slow).

## Configuration

| Environment variable | Flag | Image default | Meaning |
| --- | --- | --- | --- |
| `RDF_COMPARE_DATA_DIR` | `serve --data-dir` | `/data` | Directory the browser loader may read from. |
| `RDF_COMPARE_BIND` | `serve --bind` | `0.0.0.0:8080` | Listen address of the viewer. |
| `RDF_COMPARE_CACHE_SIZE` | `serve --cache-size` | `4` | Number of computed diffs kept in memory. |

With a data directory set, the viewer:

- lists every RDF file under it (by extension, recursively, hidden entries
  skipped) as suggestions in the *Load files…* form;
- resolves paths in the page URL (and typed in the loader) relative to it;
- rejects any path that resolves outside it, including through `..` or
  symlinks.

Paths given on the command line (`--file-a`, `--file-b`, `--diff`) are not
restricted; they are chosen by whoever starts the container. Without
`--data-dir` (the default outside the container), the browser can load any
path the process can read, as before.

## Examples

Web viewer over a local folder:

```sh
docker run --rm -p 8080:8080 -v "$PWD/rdf:/data:ro" ghcr.io/matdata-eu/rdf-compare
# open http://localhost:8080 and pick files in "Load files…", or link
# straight to a diff:
#   http://localhost:8080/?a=old.ttl&b=new.ttl
#   http://localhost:8080/?diff=diffs/2026-10.trig
```

Pre-load two files (relative paths resolve against `/data`, the working
directory):

```sh
docker run --rm -p 8080:8080 -v "$PWD/rdf:/data:ro" ghcr.io/matdata-eu/rdf-compare \
    serve --no-open --file-a old.ttl --file-b new.ttl
```

One-shot diff, as in CI (no server):

```sh
docker run --rm -v "$PWD/rdf:/data:ro" ghcr.io/matdata-eu/rdf-compare \
    old.ttl new.ttl --ci --quiet > diff.trig
```

Writing the diff with `-o` needs a writable mount, owned by or writable for
uid 65532 (or run with `--user "$(id -u):$(id -g)"`).

## Notes for Kubernetes

These are pointers, not a supported manifest.

- **Volume.** Mount the PersistentVolumeClaim (or NFS, CSI, object-storage
  FUSE driver, ConfigMap for small files…) that holds the RDF files at `/data`,
  `readOnly: true`. The viewer never writes to it. Use `subPath` or
  `RDF_COMPARE_DATA_DIR` to expose only part of a larger volume. Files must be
  readable by uid 65532; set `securityContext.fsGroup` if the volume's
  permissions need it.
- **Security context.** The image already runs as non-root and needs no
  capabilities or writable filesystem, so `runAsNonRoot: true`,
  `readOnlyRootFilesystem: true`, `allowPrivilegeEscalation: false` and
  `capabilities.drop: [ALL]` all work.
- **Probes.** `GET /api/meta` returns `200` as soon as the server listens and
  is cheap; use it for readiness and liveness (`httpGet`, port 8080). Startup
  can take a while if `--file-a`/`--file-b` are preloaded, since the diff is
  computed before the port opens; add a `startupProbe` with a generous
  `failureThreshold` in that case.
- **Shutdown.** The server exits promptly on `SIGTERM`, so the default
  termination grace period is enough.
- **Diffs are addressed by URL.** Each diff is named by its page URL
  (`?a=…&b=…` or `?diff=…`), so users can look at different diffs at the same
  time and share links. The first request for a URL computes the diff; it is
  then kept in an in-memory LRU cache (`RDF_COMPARE_CACHE_SIZE`, default 4)
  keyed by path, size and modification time, so a file replaced on the volume
  is diffed again. The server holds no other state, so several replicas
  behind a Service work without sticky sessions; each replica computes a diff
  the first time it is asked for it. Sticky sessions (for example
  `sessionAffinity: ClientIP`) avoid that repeated work for large files.
- **Memory.** A diff keeps file A as a hash set while it is computed, and the
  cached result holds both "only in" sets. Size `resources.limits.memory` for
  `RDF_COMPARE_CACHE_SIZE` times the largest diff you expect, plus one
  computation in flight per concurrent new URL; measure with
  `/usr/bin/time -v rdf-compare A B -o /dev/null` locally. A container that
  runs out of memory is OOM-killed and restarted with an empty cache.
- **No authentication.** The viewer has no login and shows the full content
  of any file under the data directory. Keep it on an internal network or put
  it behind an authenticating ingress / proxy (for example oauth2-proxy).
- **Batch use.** For scheduled comparisons (for example a nightly diff of two
  exports), run the image as a `Job`/`CronJob` with `args: [old.ttl, new.ttl,
  -o, /out/diff.trig, --ci]` and a writable output volume; exit code `1`
  signals differences, `2` an error.
