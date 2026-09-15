"""Exercise both directions against the running synthetic Docker showcase."""
import json
import sys
import time
from urllib.request import Request, urlopen

origin = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8788"


def request(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = Request(origin + path, data=data, headers={"Origin": origin, "Content-Type": "application/json"})
    with urlopen(req, timeout=5) as response:
        return json.load(response)


assert request("/health")["status"] == "ready"
initial = request("/api/state")
assert initial["machines"]["a"]["device_id"] != initial["machines"]["b"]["device_id"]
for source, target in (("a", "b"), ("b", "a")):
    before = request("/api/state")
    request("/api/sync", {"source": source})
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        state = request("/api/state")
        if state["phase"] != "running":
            break
        time.sleep(0.25)
    assert state["phase"] == "complete", "Showcase sync did not complete"
    result = state["result"]
    assert all(result[key] for key in ("identical_bytes", "repeat_pull_idempotent", "sources_preserved"))
    assert state["machines"][target]["imports"] == before["machines"][target]["imports"] + 1
    assert state["machines"][source]["snapshots"] == before["machines"][source]["snapshots"] + 1
    assert "AGE-SECRET-KEY" not in json.dumps(state)
    assert result["native_bytes"] > 0 and result["encrypted_bytes"] > result["native_bytes"]
    print(f"Machine {source.upper()} to {target.upper()}: encrypted transfer, exact bytes, repeat import and source preservation passed.")
print("Docker showcase smoke test passed.")
