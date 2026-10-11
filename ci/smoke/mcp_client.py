#!/usr/bin/env python3
"""Minimal MCP stdio client for the omnibus smoke demos (roadmap 1.8.2).

Speaks newline-delimited JSON-RPC 2.0 to `omnibusd` (rmcp stdio transport):
one request at a time, every response awaited, server-initiated messages
other than pings are ignored. Stdlib only — no third-party MCP SDK.

Import from the demo scripts:

    from mcp_client import McpClient, ToolResult

or use the tiny CLI for manual debugging:

    python3 ci/smoke/mcp_client.py -- ./target/debug/omnibusd call doctor
    python3 ci/smoke/mcp_client.py -- ./target/debug/omnibusd list
"""

from __future__ import annotations

import json
import os
import queue
import re
import subprocess
import sys
import threading
from dataclasses import dataclass
from typing import Any

PROTOCOL_VERSION = "2025-06-18"
CLIENT_NAME = "omnibus-smoke"
CLIENT_VERSION = "0.1.0"

# The closed taxonomy (specs/architecture/error-taxonomy.md) — the daemon
# renders the code name in brackets inside tool-level error text.
CODE_NAMES = [
    "CapabilityRequired",
    "ScopeDenied",
    "FloorUnsupported",
    "AdapterUnavailable",
    "NativeFailure",
    "ElementStale",
    "TimeoutError",
    "Unsupported",
]


class ProtocolError(RuntimeError):
    """The daemon spoke something other than newline-delimited JSON-RPC."""


@dataclass
class ToolResult:
    """One tools/call outcome, decoded from the CallToolResult."""

    is_error: bool
    text: str
    data: Any = None

    def error_code(self) -> str | None:
        """The taxonomy code name carried in an error text, if any."""
        if not self.is_error:
            return None
        for code in CODE_NAMES:
            if f"[{code}]" in self.text or self.text.startswith(f"{code}:"):
                return code
        # Defensive: a bracketed capitalized name we do not know yet.
        match = re.search(r"\[([A-Z][A-Za-z]+)\]", self.text)
        return match.group(1) if match else None


class McpClient:
    """One daemon process, one MCP session; dies with the client process."""

    def __init__(
        self,
        daemon_cmd: list[str],
        startup_timeout_s: float = 90.0,
        call_timeout_s: float = 120.0,
        env_extra: dict[str, str] | None = None,
    ) -> None:
        self.daemon_cmd = daemon_cmd
        self.startup_timeout_s = startup_timeout_s
        self.call_timeout_s = call_timeout_s
        self._next_id = 0
        self._lines: queue.Queue[str] = queue.Queue()
        env = os.environ.copy()
        if env_extra:
            env.update(env_extra)
        self.proc = subprocess.Popen(  # noqa: S603 - fixed argv, smoke tool
            daemon_cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,  # daemon diagnostics flow to the demo's stderr
            text=True,
            bufsize=1,
            env=env,
        )
        self._reader = threading.Thread(target=self._read_lines, daemon=True)
        self._reader.start()

    # --- transport ---------------------------------------------------------

    def _read_lines(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            self._lines.put(line)
        self._lines.put("")  # EOF sentinel

    def _send(self, payload: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        if self.proc.poll() is not None:
            raise ProtocolError(
                f"daemon exited early (code {self.proc.returncode}): {self.daemon_cmd}"
            )
        self.proc.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self.proc.stdin.flush()

    def _request(self, method: str, params: dict[str, Any] | None) -> dict[str, Any]:
        self._next_id += 1
        request_id = self._next_id
        message: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
        }
        if params is not None:
            message["params"] = params
        self._send(message)
        return self._wait_response(request_id, self.call_timeout_s)

    def _wait_response(self, request_id: int, timeout_s: float) -> dict[str, Any]:
        import time

        deadline = time.monotonic() + timeout_s
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"no JSON-RPC response for id={request_id} within {timeout_s}s"
                )
            try:
                raw = self._lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if raw == "":
                raise ProtocolError(
                    f"daemon closed stdout while waiting for id={request_id}"
                )
            raw = raw.strip()
            if not raw:
                continue
            try:
                message = json.loads(raw)
            except json.JSONDecodeError as exc:
                raise ProtocolError(f"non-JSON line from daemon: {raw[:200]!r}") from exc
            if message.get("id") != request_id:
                self._handle_server_message(message)
                continue
            if "error" in message:
                raise ProtocolError(f"JSON-RPC error for id={request_id}: {message['error']}")
            return message.get("result") or {}

    def _handle_server_message(self, message: dict[str, Any]) -> None:
        # Answer server pings; log-and-drop everything else (notifications).
        if message.get("method") == "ping" and "id" in message:
            self._send({"jsonrpc": "2.0", "id": message["id"], "result": {}})

    # --- MCP session -------------------------------------------------------

    def initialize(self) -> dict[str, Any]:
        result = self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def list_tools(self) -> list[str]:
        result = self._request("tools/list", {})
        return [tool.get("name", "") for tool in result.get("tools", [])]

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> ToolResult:
        params: dict[str, Any] = {"name": name}
        if arguments:
            params["arguments"] = arguments
        result = self._request("tools/call", params)
        blocks = result.get("content") or []
        text = "\n".join(
            block.get("text", "") for block in blocks if block.get("type") == "text"
        )
        is_error = bool(result.get("isError", False))
        data: Any = None
        if not is_error and text.strip():
            try:
                data = json.loads(text)
            except json.JSONDecodeError:
                data = None
        return ToolResult(is_error=is_error, text=text, data=data)

    def close(self) -> None:
        """Ends the session: closing stdin makes the daemon exit (no supervisor)."""
        try:
            if self.proc.stdin is not None and not self.proc.stdin.closed:
                self.proc.stdin.close()
        except (BrokenPipeError, OSError):
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=10)

    def __enter__(self) -> "McpClient":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


# --- small assertion helpers shared by the demo scripts ---------------------


def expect_ok(result: ToolResult, label: str) -> Any:
    """Asserts a non-error tool result; returns the decoded JSON payload."""
    if result.is_error:
        raise AssertionError(f"{label}: expected success, got error: {result.text[:300]}")
    if result.data is None:
        raise AssertionError(f"{label}: expected JSON content, got: {result.text[:300]}")
    print(f"PASS: {label}")
    return result.data


def expect_refusal(result: ToolResult, label: str, codes: tuple[str, ...]) -> str:
    """Asserts an honest typed refusal: isError plus one of the given codes."""
    if not result.is_error:
        raise AssertionError(
            f"{label}: expected an honest refusal ({'/'.join(codes)}), got success: "
            f"{result.text[:300]}"
        )
    code = result.error_code()
    if code not in codes:
        raise AssertionError(
            f"{label}: expected refusal code {'/'.join(codes)}, got {code!r}: "
            f"{result.text[:300]}"
        )
    print(f"PASS: {label} ({code})")
    return code


def find_tool(tools: list[str], name: str) -> bool:
    return name in tools


def capabilities_present(doc: Any, capability: str) -> bool:
    """Searches a capabilities() document (recursively) for a fact's status.

    The exact envelope may evolve; any object carrying `name == capability`
    with a `status` field counts. `present`/`degraded` mean callable;
    `hidden`/`opt-in-required`/absent mean the honest-refusal paths apply.
    """
    if isinstance(doc, dict):
        if doc.get("name") == capability and "status" in doc:
            return doc.get("status") in ("present", "degraded")
        return any(capabilities_present(value, capability) for value in doc.values())
    if isinstance(doc, list):
        return any(capabilities_present(item, capability) for item in doc)
    return False


def main(argv: list[str]) -> int:
    """CLI: `mcp_client.py [--] DAEMON [list|call TOOL [json-args]]`."""
    args = argv
    if args and args[0] == "--":
        args = args[1:]
    if len(args) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    daemon, command = args[0], args[1]
    with McpClient([daemon]) as client:
        client.initialize()
        if command == "list":
            print(json.dumps(client.list_tools(), indent=2))
            return 0
        if command == "call":
            if len(args) < 3:
                print("usage: mcp_client.py DAEMON call TOOL [JSON-ARGS]", file=sys.stderr)
                return 2
            tool = args[2]
            arguments = json.loads(args[3]) if len(args) > 3 else {}
            result = client.call(tool, arguments)
            status = "ERROR" if result.is_error else "OK"
            print(f"[{status}] {result.text}")
            return 1 if result.is_error else 0
    print(f"unknown command: {command}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
