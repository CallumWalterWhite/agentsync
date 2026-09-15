"""Synthetic-only showcase. Real CLI, real HTTP relay, two isolated local stores."""
import copy
import hashlib
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

RELAY_URL = "http://127.0.0.1:8787"
APP = Path(__file__).resolve().parent


class Demo:
    def __init__(self, root):
        self.root = root
        self.lock = threading.Lock()
        self.token = secrets.token_hex(32)
        self.keys = {}
        self.originals = {}
        self.relay = None
        self.state = {"phase": "ready", "message": "Ready to sync a synthetic session.",
                      "steps": [], "machines": {}, "result": None, "runs": 0}
        for machine in ("a", "b"):
            # Key material travels through an anonymous pipe and stays in process memory.
            generated = subprocess.run(["agentsync-showcase-identity"], capture_output=True,
                                       text=True, check=True, timeout=10).stdout.splitlines()
            if len(generated) != 2:
                raise RuntimeError("Demo key generation failed")
            self.keys[machine] = generated
            home = root / machine
            home.mkdir(mode=0o700)
            project = home / "project"
            project.mkdir(mode=0o700)
            source = home / "codex/sessions/2026/09/13/rollout-2026-09-13T21-52-44-01a09cc2-24e6-75b1-a1dd-a0a759a7d3d4.jsonl"
            source.parent.mkdir(parents=True, mode=0o700)
            fixture = (APP / "fixture.jsonl").read_text().replace("/work/polaris", str(project))
            fixture = fixture.replace("Synthetic harmless fixture", f"Synthetic showcase from machine {machine.upper()}")
            source.write_text(fixture)
            source.chmod(0o400)
            self.originals[machine] = (source, source.read_bytes())
            device = self.cli(machine, "init")
            self.cli(machine, "discover")
            self.state["machines"][machine] = {"name": f"Machine {machine.upper()}",
                                               "device_id": device["id"], "snapshots": 0, "imports": 0}
        environment = {"PATH": os.environ["PATH"], "AGENTSYNC_RELAY_TOKEN": self.token}
        self.relay = subprocess.Popen(["agentsync-server", "--data-dir", str(root / "relay")],
                                      env=environment, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL)
        for _ in range(100):
            if self.relay.poll() is not None:
                raise RuntimeError("Demo relay failed to start")
            try:
                request = Request(f"{RELAY_URL}/v1/transfers/" + "0" * 64,
                                  headers={"Authorization": "Bearer " + self.token})
                with urlopen(request, timeout=1):
                    pass
            except HTTPError as error:
                if error.code == 404:
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise RuntimeError("Demo relay readiness timeout")

    def cli(self, machine, *args):
        home = self.root / machine
        environment = {"PATH": os.environ["PATH"], "HOME": str(home),
                       "AGENTSYNC_RELAY_TOKEN": self.token,
                       "AGENTSYNC_AGE_IDENTITY": self.keys[machine][1]}
        command = ["agentsync", "--json", "--home", str(home / "store"),
                   "--codex-home", str(home / "codex"), "--claude-home", str(home / "claude"), *args]
        result = subprocess.run(command, env=environment, capture_output=True, timeout=30)
        if result.returncode:
            # CLI output is not forwarded: errors must never expose keys or native payloads.
            raise RuntimeError(f"Machine {machine.upper()} could not complete {args[0]}")
        return json.loads(result.stdout)

    def snapshot(self):
        with self.lock:
            return copy.deepcopy(self.state)

    def event(self, message):
        with self.lock:
            self.state["steps"].append({"message": message, "time": time.strftime("%H:%M:%S", time.gmtime())})

    def start(self, source):
        with self.lock:
            if self.state["phase"] == "running":
                return False
            if self.state["runs"] >= 100:
                return False
            self.state.update(phase="running", message="Running real AgentSync commands…", steps=[], result=None)
            self.state["runs"] += 1
        threading.Thread(target=self.run, args=(source,), daemon=True).start()
        return True

    def run(self, source):
        target = "b" if source == "a" else "a"
        try:
            sessions = self.cli(source, "sessions")
            item = self.cli(source, "snapshot", sessions[0]["id"])
            snapshot_id = item["manifest"]["snapshot_id"]
            self.event(f"Machine {source.upper()} created an immutable snapshot.")
            transfer = self.cli(source, "push", snapshot_id, "--server", RELAY_URL,
                                "--recipient", self.keys[target][0])
            encrypted = (self.root / "relay" / (transfer["transfer_sha256"] + ".age")).read_bytes()
            if not encrypted.startswith(b"age-encryption.org/v1\n"):
                raise RuntimeError("Relay encryption format check failed")
            if self.originals[source][1] in encrypted:
                raise RuntimeError("Relay privacy check failed")
            self.event("Relay accepted the encrypted bundle and verified its transfer hash.")
            imported = self.cli(target, "pull", transfer["transfer_sha256"], "--server", RELAY_URL)
            self.cli(target, "bundle", "verify", snapshot_id)
            received = imported["snapshot"]
            if received["manifest"] != item["manifest"]:
                raise RuntimeError("Imported manifest does not match")
            native_bytes = 0
            for obj in item["manifest"]["objects"]:
                relative = "objects/" + obj["sha256"]
                original = (Path(item["directory"]) / relative).read_bytes()
                copied = (Path(received["directory"]) / relative).read_bytes()
                if copied != original or hashlib.sha256(copied).hexdigest() != obj["sha256"]:
                    raise RuntimeError("Imported bytes do not match")
                native_bytes += len(copied)
            self.event(f"Machine {target.upper()} decrypted, imported and verified identical native bytes.")
            before = self.cli(target, "bundle", "list")
            self.cli(target, "pull", transfer["transfer_sha256"], "--server", RELAY_URL)
            after = self.cli(target, "bundle", "list")
            if before != after:
                raise RuntimeError("Repeated pull created a duplicate")
            self.event("A second pull succeeded without creating a duplicate import.")
            for path, original in self.originals.values():
                if path.read_bytes() != original:
                    raise RuntimeError("Source preservation check failed")
            self.event("Both synthetic provider transcripts remain unchanged.")
            machines = {}
            for machine in ("a", "b"):
                session = self.cli(machine, "sessions")[0]["id"]
                machines[machine] = {"name": f"Machine {machine.upper()}",
                                     "device_id": self.cli(machine, "init")["id"],
                                     "snapshots": len(self.cli(machine, "snapshots", session, "--verify")),
                                     "imports": len(self.cli(machine, "bundle", "list"))}
            with self.lock:
                self.state.update(phase="complete", message=f"Verified: Machine {source.upper()} synced to Machine {target.upper()}.",
                                  machines=machines, result={"snapshot_id": snapshot_id,
                                  "transfer_sha256": transfer["transfer_sha256"], "manifest_sha256": item["manifest_sha256"],
                                  "native_bytes": native_bytes, "encrypted_bytes": len(encrypted),
                                  "identical_bytes": True, "repeat_pull_idempotent": True, "sources_preserved": True})
        except Exception:
            with self.lock:
                self.state.update(phase="error", message="Demo verification failed. Restart the showcase and retry.")

    def stop(self):
        if self.relay is not None and self.relay.poll() is None:
            self.relay.terminate()
            try:
                self.relay.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.relay.kill()
                self.relay.wait()


def handler_for(demo):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def allowed_host(self):
            try:
                return urlsplit("http://" + self.headers.get("Host", "")).hostname in ("localhost", "127.0.0.1", "::1")
            except ValueError:
                return False

        def reply(self, code, body, content_type="application/json"):
            if isinstance(body, dict):
                body = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self.allowed_host():
                return self.reply(403, {"error": "Unsupported host"})
            if self.path == "/":
                return self.reply(200, (APP / "index.html").read_bytes(), "text/html; charset=utf-8")
            if self.path == "/api/state":
                return self.reply(200, demo.snapshot())
            if self.path == "/health":
                alive = demo.relay.poll() is None
                return self.reply(200 if alive else 503, {"status": "ready" if alive else "unavailable"})
            return self.reply(404, {"error": "Not found"})

        def do_POST(self):
            if not self.allowed_host() or self.headers.get("Origin") != "http://" + self.headers.get("Host", ""):
                return self.reply(403, {"error": "Same-origin request required"})
            if self.path != "/api/sync":
                return self.reply(404, {"error": "Not found"})
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 128:
                    return self.reply(400, {"error": "Invalid request"})
                self.connection.settimeout(5)
                value = json.loads(self.rfile.read(length))
                if not isinstance(value, dict) or set(value) != {"source"} or value["source"] not in ("a", "b"):
                    return self.reply(400, {"error": "Invalid source"})
            except (ValueError, OSError):
                return self.reply(400, {"error": "Invalid request"})
            if not demo.start(value["source"]):
                return self.reply(409, {"error": "Demo busy or run limit reached; restart to reset"})
            self.reply(202, {"status": "started"})
    return Handler


def main():
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="agentsync-showcase-", dir="/demo") as directory:
        demo = Demo(Path(directory))
        server = ThreadingHTTPServer(("0.0.0.0", 8788), handler_for(demo))
        server.timeout = 0.5
        stopped = threading.Event()
        signal.signal(signal.SIGTERM, lambda *_: stopped.set())
        signal.signal(signal.SIGINT, lambda *_: stopped.set())
        print("AgentSync showcase ready on port 8788. Synthetic data only.", flush=True)
        try:
            while not stopped.is_set():
                server.handle_request()
        finally:
            server.server_close()
            demo.stop()


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("AgentSync showcase failed to initialize. Check container health and restart.", flush=True)
        raise SystemExit(1)
