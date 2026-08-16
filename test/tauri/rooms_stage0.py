#!/usr/bin/env python3
"""Stdlib-only W3C client for the isolated Tauri Rooms Stage0 fixture."""

import argparse
import json
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf"
TAB_KEY_CODE = 48
ENTER_KEY_CODE = 36
ESCAPE_KEY_CODE = 53


def require(condition, message):
    if not condition:
        raise AssertionError(message)


if sys.flags.optimize:
    raise RuntimeError("rooms_stage0.py refuses optimized Python; acceptance checks must remain load-bearing")


class W3CError(RuntimeError):
    pass


class W3C:
    def __init__(self, port):
        self.base = "http://127.0.0.1:{}".format(port)
        self.session_id = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, path, payload=None):
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            self.base + path,
            data=data,
            method=method,
            headers={"content-type": "application/json"},
        )
        try:
            with self.opener.open(request, timeout=5) as response:
                raw = response.read()
        except urllib.error.HTTPError as error:
            raw = error.read().decode("utf-8", "replace")
            raise W3CError("{} {} returned {}: {}".format(method, path, error.code, raw))
        except OSError as error:
            raise W3CError("{} {} failed: {}".format(method, path, error))
        if not raw:
            return None
        decoded = json.loads(raw.decode("utf-8"))
        if isinstance(decoded, dict) and "value" in decoded:
            value = decoded["value"]
            if isinstance(value, dict) and value.get("error"):
                raise W3CError("{} {}: {}".format(method, path, value))
            return value
        return decoded

    def wait_ready(self, timeout=15):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            try:
                value = self.request("GET", "/status")
                if isinstance(value, dict) and value.get("ready") is True:
                    return
            except (W3CError, ValueError) as error:
                last = error
            time.sleep(0.1)
        raise W3CError("embedded WebDriver never became ready: {}".format(last))

    def create_session(self):
        value = self.request("POST", "/session", {"capabilities": {"alwaysMatch": {}}})
        if isinstance(value, dict):
            self.session_id = value.get("sessionId")
        if not self.session_id:
            raise W3CError("POST /session returned no sessionId: {!r}".format(value))

    def path(self, suffix):
        if not self.session_id:
            raise W3CError("session has not been created")
        return "/session/{}{}".format(urllib.parse.quote(self.session_id, safe=""), suffix)

    def delete_session(self):
        if self.session_id:
            session = self.session_id
            self.session_id = None
            self.request("DELETE", "/session/{}".format(urllib.parse.quote(session, safe="")))

    def execute(self, script, args=None):
        return self.request("POST", self.path("/execute/sync"), {
            "script": script,
            "args": args or [],
        })

    def elements(self, selector):
        values = self.request("POST", self.path("/elements"), {
            "using": "css selector",
            "value": selector,
        }) or []
        return [value[ELEMENT_KEY] for value in values]

    def element(self, selector):
        values = self.elements(selector)
        if not values:
            raise W3CError("element not found: {}".format(selector))
        return values[0]

    def element_path(self, element, suffix):
        return self.path("/element/{}{}".format(urllib.parse.quote(element, safe=""), suffix))

    def click(self, element):
        self.request("POST", self.element_path(element, "/click"), {})

    def attribute(self, element, name):
        return self.request("GET", self.element_path(
            element, "/attribute/{}".format(urllib.parse.quote(name, safe=""))))

    def text(self, element):
        return self.request("GET", self.element_path(element, "/text"))

    def displayed(self, element):
        return bool(self.request("GET", self.element_path(element, "/displayed")))

    def css(self, element, name):
        return self.request("GET", self.element_path(
            element, "/css/{}".format(urllib.parse.quote(name, safe=""))))



def wait_for(predicate, message, timeout=15):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (W3CError, KeyError, TypeError, ValueError) as error:
            last = error
        time.sleep(0.1)
    raise AssertionError("{} (last={!r})".format(message, last))


def native_key(client, app_pid, key_code, label, expected_id=None, expected_aria_label=None):
    """Send a real macOS key; plugin action synthesis cannot test native traversal."""
    focus = client.execute("""
        return {
          documentFocused: document.visibilityState === 'visible' && document.hasFocus(),
          id: document.activeElement && document.activeElement.id,
          aria: document.activeElement && document.activeElement.getAttribute('aria-label')
        };
    """)
    if not focus["documentFocused"]:
        raise AssertionError("document lost focus immediately before native {}".format(label))
    if expected_id is not None and focus["id"] != expected_id:
        raise AssertionError("active element id before {} was {!r}, expected {!r}".format(
            label, focus["id"], expected_id))
    if expected_aria_label is not None and focus["aria"] != expected_aria_label:
        raise AssertionError("active element label before {} was {!r}, expected {!r}".format(
            label, focus["aria"], expected_aria_label))
    script = """
tell application "System Events"
  set activePid to unix id of first application process whose frontmost is true
  if activePid is not %d then error "frontmost PID " & activePid & " is not acceptance PID %d"
  key code %d
end tell
""" % (app_pid, app_pid, key_code)
    try:
        result = subprocess.run(
            ["/usr/bin/osascript", "-e", script],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
            timeout=10,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(
            "native {} timed out; Accessibility permission may be awaiting approval "
            "in the dedicated login".format(label)
        ) from error
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "unknown System Events error"
        raise RuntimeError(
            "native {} failed; verify the dedicated login grants Accessibility permission "
            "to the Stage0 runner: {}".format(label, detail)
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--room-key", required=True)
    parser.add_argument("--room-name", required=True)
    parser.add_argument("--app-pid", type=int, required=True)
    args = parser.parse_args()

    client = W3C(args.port)
    failure = None
    try:
        client.wait_ready()
        client.create_session()
        workspace = wait_for(
            lambda: client.element('[aria-label="Rooms workspace"]'),
            "Rooms workspace did not mount",
        )
        wait_for(
            lambda: client.execute(
                "return document.visibilityState === 'visible' && document.hasFocus();"
            ),
            "native document never became visible and focused",
        )
        facts = client.execute("""
            return {
              tauri: Boolean(window.__TAURI_INTERNALS__),
              webkit: Boolean(window.webkit && window.webkit.messageHandlers),
              width: window.innerWidth
            };
        """)
        require(facts["tauri"], "must run in the Tauri shell")
        require(facts["webkit"], "must run in a WKWebView")
        require(facts["width"] <= 650, "innerWidth was {}".format(facts["width"]))

        compact = client.element(".rooms-workspace__mobile-nav")
        wait_for(lambda: client.displayed(compact) and client.css(compact, "display") == "flex",
                 "compact navigation was not visible with display:flex")

        toggle = client.element('[aria-label="Toggle room list"]')
        require(client.attribute(toggle, "aria-expanded") == "false",
                "room drawer unexpectedly began expanded")
        client.click(toggle)
        wait_for(lambda: client.attribute(toggle, "aria-expanded") == "true",
                 "room drawer did not open")
        drawer = client.element("#rooms-workspace-room-list")
        wait_for(lambda: client.displayed(drawer) and
                 "rooms-workspace__left--visible" in client.attribute(drawer, "class"),
                 "room drawer did not become visible")

        room_options = [candidate for candidate in client.elements('[role="option"]')
                        if (client.attribute(candidate, "id") or "").startswith("rooms-opt-")]
        require(len(room_options) == 1, "isolated app did not render exactly one room")
        room_option = room_options[0]
        require(client.text(room_option).strip().lstrip("#").strip() == args.room_name,
                "canonical seeded room name was not rendered")
        option_id = client.attribute(room_option, "id")
        require(option_id == "rooms-opt-{}".format(args.room_key),
                "canonical seeded room key was not rendered")

        close = client.element('[aria-label="Close rooms workspace"]')
        client.execute("arguments[0].focus();", [{ELEMENT_KEY: close}])
        native_key(client, args.app_pid, TAB_KEY_CODE, "Tab",
                   expected_aria_label="Close rooms workspace")
        wait_for(lambda: client.execute("return document.activeElement && document.activeElement.id;") == option_id,
                 "native Tab did not move focus to the seeded room")
        native_key(client, args.app_pid, ENTER_KEY_CODE, "Enter", expected_id=option_id)
        wait_for(lambda: client.attribute(room_option, "aria-selected") == "true" and
                 client.attribute(toggle, "aria-expanded") == "false",
                 "native Enter did not select the seeded room and close the drawer")

        client.click(toggle)
        wait_for(lambda: client.attribute(toggle, "aria-expanded") == "true",
                 "drawer did not reopen")
        client.execute("arguments[0].focus();", [{ELEMENT_KEY: close}])
        native_key(client, args.app_pid, TAB_KEY_CODE, "Tab",
                   expected_aria_label="Close rooms workspace")
        wait_for(lambda: client.execute("return document.activeElement && document.activeElement.id;") == option_id,
                 "second native Tab did not focus the seeded room")
        native_key(client, args.app_pid, ESCAPE_KEY_CODE, "Escape", expected_id=option_id)
        wait_for(lambda: client.attribute(toggle, "aria-expanded") == "false" and
                 client.execute("return document.activeElement && document.activeElement.getAttribute('aria-label');") == "Toggle room list",
                 "first native Escape did not close drawer and restore toggle focus")
        native_key(client, args.app_pid, ESCAPE_KEY_CODE, "Escape",
                   expected_aria_label="Toggle room list")
        wait_for(lambda: not client.elements('[aria-label="Rooms workspace"]'),
                 "second native Escape did not close Rooms")
    except BaseException as error:  # preserve test/system-signal failure through cleanup
        failure = error
    finally:
        try:
            client.delete_session()
        except BaseException as error:
            failure = failure or error
    if failure:
        raise failure


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda _signum, _frame: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        main()
    except BaseException as error:
        print("W3C acceptance failed: {}".format(error), file=sys.stderr)
        raise
