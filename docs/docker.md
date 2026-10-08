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

With a data directory set, the viewer:

- lists every RDF file under it (by extension, recursively, hidden entries
  skipped) as suggestions in the *Load files…* form;
- resolves paths typed in the browser relative to it;
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
# open http://localhost:8080 and pick files in "Load files…"
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
- **Memory.** A diff keeps file A as a hash set plus both "only in" sets in
  memory, and the loaded diff stays resident until another one is loaded.
  Size `resources.limits.memory` from the largest pair of files you expect;
  measure with `/usr/bin/time -v rdf-compare A B -o /dev/null` locally. A
  container that runs out of memory is OOM-killed and restarted with no diff
  loaded.
- **One diff per instance.** The loaded diff is global to the process: when
  one user loads files, every open browser tab sees the new diff on its next
  refresh. Run one instance per user or team, or treat it as a shared
  read-only view with a preloaded diff. Run a single replica (`replicas: 1`);
  with several replicas behind a Service, requests from one browser can land
  on instances holding different diffs.
- **No authentication.** The viewer has no login and shows the full content
  of any file under the data directory. Keep it on an internal network or put
  it behind an authenticating ingress / proxy (for example oauth2-proxy).
- **Batch use.** For scheduled comparisons (for example a nightly diff of two
  exports), run the image as a `Job`/`CronJob` with `args: [old.ttl, new.ttl,
  -o, /out/diff.trig, --ci]` and a writable output volume; exit code `1`
  signals differences, `2` an error.
