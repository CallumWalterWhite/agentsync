# Docker sync showcase

Run from the repository root:

```sh
docker compose -f deploy/compose.yaml up --build -d
```

Open **http://localhost:8788**. Click **Sync A → B** or **Sync B → A**.

The page runs real `agentsync snapshot`, `push`, `pull` and verification commands. Each run verifies exact native bytes, repeats the pull to prove idempotency, and checks that both source transcripts remain unchanged. Displayed hashes and counts come from command results.

## What is deployed

A single Linux container runs the Axum relay, the browser showcase, and two isolated CLI stores. This demonstrates machine-to-machine behavior using independent device IDs and filesystem roots. It does not claim to run two physical machines or separate client containers.

Only a synthetic Codex fixture is included. No host directories, provider data, Docker socket, access tokens or private keys are mounted or copied into the image. `.dockerignore` permits only Rust sources/manifests, synthetic fixtures and showcase code into the build context.

The supervisor generates a random relay access token and two age identities at startup. The identity helper passes its output through an anonymous pipe to the supervisor; it must never be run in a logging pipeline. Secrets remain in process memory or child environments. The dashboard only exposes public device/snapshot/hash metadata. Application storage uses container tmpfs, and the container runs as a non-root user with a read-only root filesystem.

The web port is published only on host loopback. The underlying relay is reachable only inside the container. The dashboard accepts localhost hosts and same-origin POST requests; it has no arbitrary command or upload endpoint. There is no user account/login system.

## Manage the deployment

```sh
# Status and health
docker compose -f deploy/compose.yaml ps

# Reset all disposable demo data and generate fresh in-memory keys
docker compose -f deploy/compose.yaml restart

# Stop and remove this deployment
docker compose -f deploy/compose.yaml down
```

A restart intentionally resets all demo snapshots, imports and relay blobs. Nothing from this showcase should be used as a durable backup. Runs are limited to 100 per start; restart to reset.

If port 8788 is occupied:

```sh
AGENTSYNC_SHOWCASE_PORT=8790 docker compose -f deploy/compose.yaml up -d
```

Open http://localhost:8790 instead. Use the same port setting on subsequent Compose commands.

## Verify

```sh
# Real HTTP smoke test in both directions
python3 deploy/showcase/smoke.py

# HTTP host/origin/input boundary tests; no Docker required
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s deploy/showcase -p 'test_*.py'
```

The smoke test accepts an alternate origin as its first argument, for example `http://127.0.0.1:8790`.

## Showcase on a remote Docker host

Copy this repository to your host and run the same Compose command there. Access the loopback-only showcase through a tunnel from your laptop:

```sh
ssh -N -L 8788:127.0.0.1:8788 user@your-host
```

Then open http://localhost:8788. This does not require putting the demo dashboard on the public internet. A public domain deployment would need explicit host/domain configuration, HTTPS and access control; those are not supplied by this showcase.

## Relay image

The Dockerfile also exposes a `relay` target containing only the production relay executable and runtime dependencies:

```sh
docker build -f deploy/Dockerfile --target relay -t agentsync-relay .
```

It runs as UID/GID 10001 with a default data directory `/data/relay`. Deploy it with an existing writable parent, durable private storage and a runtime token. Its listener remains loopback-only: an HTTPS proxy must share its network namespace (or provide a suitable tunnel). Merely publishing the relay's loopback port through Docker will not expose it on the container interface. See [manual sync setup](../docs/machine-sync.md) for the relay contract.
