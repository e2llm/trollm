# Run the flow review companion

The public image is **`ghcr.io/e2llm/trollm-flow-review`** for **Linux/amd64**.
Use `:latest` for the current published viewer. Full commit tags identify the
public GitHub source revisions used to build the images.
[Image tags and digests on GitHub Packages](https://github.com/e2llm/trollm/pkgs/container/trollm-flow-review)

## Start the viewer with Docker

Run these commands on the machine running Docker. If the Linux desktop
runtime is using port 8080, stop it first. The viewer accepts uploaded
packages from the browser.

```sh
docker pull --platform linux/amd64 ghcr.io/e2llm/trollm-flow-review:latest &&
docker run --detach --rm --name trollm-flow-review --platform linux/amd64 \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --memory 1g --cpus 2 --tmpfs /tmp --publish 127.0.0.1:8080:8080 \
  ghcr.io/e2llm/trollm-flow-review:latest
```

Open `http://127.0.0.1:8080`. Choose **Open files / tar** to load an
uncompressed flow bundle or **Open folder** to load its source directory.
When finished, run `docker stop trollm-flow-review`. The container is removed
when it stops. To update the viewer, stop it and repeat the pull and run commands.
Pulling an image does not restart an existing container.

## Start the viewer with Podman

The Podman launcher mounts a package directory for browsing. From the
repository root on Linux/amd64, use rootless **Podman 5.0 or newer** with
cgroups v2, Python 3 and PyYAML. Both the Podman client and runtime must meet
the minimum release for memory-backed volumes. Use `flows/starters` or your
own package directory. PyYAML is listed in `scripts/requirements.txt`.

```sh
FLOW_REVIEW_IMAGE='ghcr.io/e2llm/trollm-flow-review:latest'
podman pull "$FLOW_REVIEW_IMAGE"
python3 apps/flow-review/deploy/launch.py --packages "$PWD/flows/starters" \
  --image "$FLOW_REVIEW_IMAGE" --port 8080
```

Open `http://127.0.0.1:8080` on the host running the viewer. Choose **Open files /
tar** to load an uncompressed flow bundle, **Open folder** for a source directory,
or select a flow from **Mounted packages**. To use a different loopback port,
change `--port` and open that port in the browser.

To stop the viewer, run `podman pod rm -f trollm-flow-review`. To update it,
pull the image again and rerun the launcher to replace the pod. Add `--print`
to inspect the configuration without starting Podman. The example
[pod.yaml](pod.yaml) can also be customized directly.

## Pin a published image

For a repeatable deployment, copy an image digest from
[GitHub Packages](https://github.com/e2llm/trollm/pkgs/container/trollm-flow-review)
and use `ghcr.io/e2llm/trollm-flow-review@sha256:<digest>` in both Docker
commands or as `FLOW_REVIEW_IMAGE` for Podman. A published full commit tag
can also identify a specific build. The example [pod.yaml](pod.yaml) uses a digest.

## Build locally with Podman

Run from the repository root.

```sh
podman build --format docker --ignorefile apps/flow-review/.containerignore \
  -f apps/flow-review/Containerfile -t localhost/trollm-flow-review:dev .
python3 apps/flow-review/deploy/launch.py --packages "$PWD/flows/starters" \
  --image localhost/trollm-flow-review:dev --port 8080
```

## Hosting and data

The container runs as UID/GID 10001 with a read-only root filesystem and
read-only package mounts. Make the package directory readable by the mapped
user. On SELinux hosts, arrange a container-readable label for that directory.
The launcher leaves host file permissions and labels as it found them.

The pod limits memory to 1 GiB and CPU to two cores. Podman needs delegated
cgroup controllers to enforce those limits when running rootless. The 1 GiB
memory limit includes the memory-backed `/tmp` volume used for uploads. The
server removes staged uploads after inspection. Removing the pod discards
temporary storage. Export review feedback before leaving or reloading the page.

The launcher listens on loopback by default and waits for `/healthz`. View
failures with `podman pod logs trollm-flow-review`. To serve a remote browser,
configure a TLS and access-control reverse proxy. Anyone who can reach the
viewer can inspect the packages it serves.

## Dependency notices

The image includes the Rust server's dependency texts at
`/app/notices/THIRD_PARTY_NOTICES.txt` and a SHA-256 inventory at
`/app/notices/third-party-manifest.json`. The locked Linux/amd64 release build
provides the package list. The inventory includes build dependencies and
procedural macros, exact upstream supplements, nested texts for bundled code,
and the installed Rust toolchain's standard-library copyright and license
bundle. The browser dependency notices are at `/app/web/third-party-notices.txt`.
Use `podman cp` or `docker cp` to retrieve these files from a created container.

## Check a local image

From the repository root on the Linux host running Podman, run the container
smoke check against the image you built.

```sh
python3 apps/flow-review/deploy/smoke.py --image localhost/trollm-flow-review:dev
```

The check creates a disposable pod and verifies the effective non-root UID,
visible read-only mounts, tmpfs, kernel resource limits, health, imports and
staging cleanup. It needs access to the container's `/proc` state and cgroup
files on the same host. Rootless checks also require subordinate UID/GID
mappings and delegated CPU and memory controllers. The smoke removes its pod
when it finishes. Set `PODMAN` to a Podman command with flags if the launcher
or smoke needs a separate image store.
