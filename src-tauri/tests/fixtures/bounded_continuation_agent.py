"""Deterministic ACP peer for strict continuation and ordering tests."""

import json
import sys


mode, log_path = sys.argv[1:]
turn = 0


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")


for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if not method:
        continue
    params = message.get("params", {})
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(
            json.dumps(
                {
                    "method": method,
                    "params": {
                        key: params[key]
                        for key in ("sessionId", "modeId", "configId", "value")
                        if key in params
                    },
                }
            )
            + "\n"
        )

    result = {}
    error = None
    if method == "initialize":
        result = {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": mode == "load_fail",
                "sessionCapabilities": {} if mode == "unsupported" else {"resume": {}},
            },
            "authMethods": [],
        }
    elif method == "session/resume":
        if mode == "load_fail":
            error = {"code": -32601, "message": "injected resume failure"}
        else:
            result = {
                "modes": {
                    "currentModeId": "default",
                    "availableModes": [
                        {"id": "default", "name": "Default"},
                        {"id": "plan", "name": "Plan"},
                    ],
                },
                "configOptions": [
                    {
                        "type": "select",
                        "id": "model",
                        "name": "Model",
                        "currentValue": "default-model",
                        "options": [
                            {"value": "default-model", "name": "Default"},
                            {"value": "source-model", "name": "Source"},
                        ],
                    }
                ],
            }
    elif method == "session/load":
        error = {"code": -32603, "message": "injected load failure"}
    elif method == "session/new":
        result = {"sessionId": "unexpected-new-session"}
    elif method == "session/set_config_option":
        result = {
            "configOptions": [
                {
                    "type": "select",
                    "id": "model",
                    "name": "Model",
                    "currentValue": "source-model",
                    "options": [{"value": "source-model", "name": "Source"}],
                }
            ]
        }
    elif method == "session/prompt":
        turn += 1
        session_id = params["sessionId"]
        notification = {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": f"immediate reply {turn}"},
                },
            },
        }
        response = {
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {"stopReason": "end_turn"},
        }
        # One write and one flush makes both items ready together at the host.
        sys.stdout.write(json.dumps(notification) + "\n" + json.dumps(response) + "\n")
        sys.stdout.flush()
        continue

    if "id" in message:
        response = {"jsonrpc": "2.0", "id": message["id"]}
        response["error" if error else "result"] = error if error else result
        send(response)
        sys.stdout.flush()
