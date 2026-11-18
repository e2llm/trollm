# Run the flow review companion

The planned public image is **`ghcr.io/e2llm/trollm-flow-review`** for
**Linux/amd64**. Public pulling has not yet been verified. Once the first
publication and anonymous-pull check succeed, find tags and digests on the
[GitHub package page](https://github.com/orgs/e2llm/packages/container/package/trollm-flow-review)
or the repository's **Packages** sidebar. The image's full commit tag names the
public GitHub source commit; `latest` follows successfully checked current-main
builds. Pin a digest for repeatable deployment.

After public availability is confirmed, substitute the digest emitted by that
successful publication (no registry login is needed for a public package):

```sh
FLOW_REVIEW_IMAGE='ghcr.io/e2llm/trollm-flow-review@sha256:PUBLISHED_DIGEST'
podman pull "$FLOW_REVIEW_IMAGE"
python3 apps/flow-review/deploy/launch.py --packages /srv/trollm/packages \
  --image "$FLOW_REVIEW_IMAGE" --port 8080
```

Use rootless Podman with cgroups v2, Python 3 and PyYAML (the repository's
`scripts/requirements.txt` includes it). For a local build, run from the repository root:

```sh
podman build --format docker --ignorefile apps/flow-review/.containerignore \
  -f apps/flow-review/Containerfile -t localhost/trollm-flow-review:dev .
python3 apps/flow-review/deploy/launch.py --packages /srv/trollm/packages \
  --image localhost/trollm-flow-review:dev --port 8080
```

Open `http://127.0.0.1:8080`. Use a published full commit SHA tag or digest for
repeatable deployments. `--print` renders the exact pod configuration for review;
`pod.yaml` can also be copied and customized directly. `--bind` controls the
host interface and defaults to loopback. `podman pod rm -f trollm-flow-review`
stops and removes the pod. Restart with the launcher after changing the image.
The server drains active HTTP requests after SIGTERM or Ctrl-C. The runtime image
uses official distroless Debian 13 with GNU libraries, the Rust server and built
browser assets; it contains no shell or external health-check utility. Both the
image healthcheck and pod liveness probe execute the server with `--healthcheck`.

One container serves the browser and API. It runs as UID/GID 10001, drops all
capabilities, disallows privilege escalation and has a read-only root filesystem.
`/packages` is mounted read-only; make source directories readable by the mapped
container user. The launcher does not chmod or relabel host sources. On SELinux
hosts, arrange an appropriate container-readable label for this dedicated source
directory before launching. Never map a home directory or credential store.

`/tmp` is a memory-backed transient volume. The Kubernetes spec requests a
512 MiB volume limit; Podman 5.8.2 ignores `emptyDir.sizeLimit`. On Podman, the
enforced 1 GiB total container memory limit also bounds memory consumed by tmpfs.
The server accepts at most two concurrent 64 MiB imports. Uploaded imports are staged
under `/tmp/trollm-flow-review` and removed after inspection, including failures.
Removing the pod discards the volume; crashes may leave staging data until pod
removal. There is no shared review database. Comments/baselines remain in browser
page state; export feedback before reloading or leaving the page. The pod limits
memory to 1 GiB and CPU to two cores. Rootless resource enforcement requires
cgroup delegation; verify `podman inspect` on the deployment host. Health and
readiness use `/healthz`. Podman implements the liveness probe; the launcher
waits for HTTP readiness because Podman does not implement Kubernetes
`readinessProbe`. The numeric UID enforces non-root execution even where Podman
does not implement `runAsNonRoot`. View failures with `podman pod logs trollm-flow-review`.

Use trusted hosting. For network access, place the loopback service behind your
TLS/access-control reverse proxy; the application has no login or tenant boundary.
Package text is served to whoever can access this instance. No model weights,
Android build environment, external renderer, or live MCP connection is needed.

Run the actual container acceptance check on the local Linux Podman runtime host:

```sh
python3 apps/flow-review/deploy/smoke.py --image localhost/trollm-flow-review:dev
```

It uses its own disposable pod and validates actual non-root execution, read-only
mounts/root filesystem, memory enforcement, tmpfs, assets, catalog inspection,
tar upload and staging cleanup. A successful HTTP-only check does not establish
these container properties. The smoke reads the real container PID's Linux
`/proc` status, UID mapping, mount table and cgroup v2 membership, then reads the
actual cgroup memory/CPU limits. It copies the ordinary staged directory through
Podman to verify cleanup; no shell/helper binary is injected into the image.

The operator must be allowed to read these host process/cgroup files. Remote
Podman clients and hosts without those permissions are unsupported for this
acceptance check; it fails explicitly rather than accepting configuration-only
assertions. The unchanged smoke passed with both rootful and rootless Podman
5.8.2 on Linux with systemd and cgroup v2. The rootless run used existing
subordinate UID/GID ranges, user-namespace helpers and delegated CPU/memory
controllers; it needed no additional process-inspection privileges on that host.
Other hosts must provide those prerequisites and allow the inspection described
above. The smoke removes its own pod after success/failure.

The launcher and smoke accept `PODMAN` as a command plus flags (parsed without a
shell), for example `PODMAN="podman --root /tmp/review-storage --runroot
/tmp/review-run --storage-driver vfs"`. Rootless deployment remains the default;
acceptance applies to the host and execution mode actually tested.
