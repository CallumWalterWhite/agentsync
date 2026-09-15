"""HTTP boundary tests; no provider data, Docker socket or real keys are accessed."""
import http.client
import json
from http.server import ThreadingHTTPServer
import threading
import unittest
from unittest.mock import Mock
from server import handler_for


class HttpBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.demo = Mock()
        self.demo.snapshot.return_value = {"phase": "ready"}
        self.demo.start.return_value = True
        self.demo.relay.poll.return_value = None
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(self.demo))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.address = "127.0.0.1:" + str(self.server.server_port)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, path, body=None, host=None, origin=None):
        connection = http.client.HTTPConnection(self.address, timeout=5)
        headers = {"Host": host or self.address}
        if origin:
            headers["Origin"] = origin
        connection.request("POST" if body is not None else "GET", path, body, headers)
        response = connection.getresponse()
        status, value = response.status, json.loads(response.read())
        connection.close()
        return status, value

    def test_host_and_origin_rejections_never_start_work(self):
        self.assertEqual(self.request("/api/state", host="attacker.example")[0], 403)
        for origin in (None, "https://attacker.example"):
            self.assertEqual(self.request("/api/sync", '{"source":"a"}', origin=origin)[0], 403)
        self.demo.start.assert_not_called()

    def test_only_fixed_source_values_are_accepted(self):
        for body in ('{"source":"../../other"}', '{"source":"a","command":"anything"}', '[]', 'x' * 129):
            self.assertEqual(self.request("/api/sync", body, origin="http://" + self.address)[0], 400)
        self.demo.start.assert_not_called()
        self.assertEqual(self.request("/api/sync", '{"source":"b"}', origin="http://" + self.address)[0], 202)
        self.demo.start.assert_called_once_with("b")

    def test_busy_and_health_statuses(self):
        self.demo.start.return_value = False
        self.assertEqual(self.request("/api/sync", '{"source":"a"}', origin="http://" + self.address)[0], 409)
        self.assertEqual(self.request("/health")[0], 200)
        self.demo.relay.poll.return_value = 1
        self.assertEqual(self.request("/health")[0], 503)

    def test_public_state_and_unknown_route(self):
        self.assertEqual(self.request("/api/state"), (200, {"phase": "ready"}))
        self.assertEqual(self.request("/unknown")[0], 404)


if __name__ == "__main__":
    unittest.main()
