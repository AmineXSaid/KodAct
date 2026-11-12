#!/usr/bin/env python3
"""A tiny MCP server on stdio for the terminal matrix: two tools, one of which kills the server.

    mcp_stub.py            answers initialize, tools/list and tools/call
    mcp_stub.py --slow N   waits N seconds before answering initialize
"""
import json
import sys
import time


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    slow = float(sys.argv[sys.argv.index("--slow") + 1]) if "--slow" in sys.argv else 0
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        mid, method = msg.get("id"), msg.get("method")
        if mid is None:
            continue
        if method == "initialize":
            time.sleep(slow)
            send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": msg["params"].get("protocolVersion", "2025-03-26"),
                                                          "capabilities": {"tools": {}}, "serverInfo": {"name": "matrix-stub", "version": "1"}}})
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [
                {"name": "say", "description": "Say the text back.", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
                {"name": "die", "description": "Exit the server.", "inputSchema": {"type": "object", "properties": {}}}]}})
        elif method == "tools/call":
            name = msg["params"]["name"]
            if name == "die":
                sys.exit(1)
            text = msg["params"].get("arguments", {}).get("text", "")
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": f"MCP-SAID {text}"}]}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {}})


if __name__ == "__main__":
    main()
