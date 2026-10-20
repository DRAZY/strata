"""Word Count: an example Strata plugin (API strata.plugin/1).

Shows the word count of each window's current file as a status bar item, kept up to date
as you type, and "Word Count: Show Word Count" opens a panel with words,
lines and characters. It only uses the Python standard library.

Strata talks to the plugin with JSON-RPC 2.0 messages on stdin and stdout,
each after a "Content-Length: N" header (the framing language servers use).
Nothing but messages may be written to stdout; use stderr or the "log"
notification for anything else.
"""

import json
import re
import sys

API = "strata.plugin/1"
_stdin = sys.stdin.buffer
_stdout = sys.stdout.buffer
_next_id = 0
_waiting = []  # messages that arrived while waiting for an answer
_settings = {"count_numbers": True}
_counted = {}  # document id -> version last counted, for documents shown in front


def read_message():
    """The next message from Strata, or None when Strata closes the pipe."""
    length = None
    while True:
        line = _stdin.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.decode("ascii", "replace").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    body = _stdin.read(length)
    if len(body) < length:
        return None
    return json.loads(body.decode("utf-8"))


def send(message):
    message["jsonrpc"] = "2.0"
    body = json.dumps(message).encode("utf-8")
    _stdout.write(b"Content-Length: %d\r\n\r\n" % len(body))
    _stdout.write(body)
    _stdout.flush()


def notify(method, params):
    send({"method": method, "params": params})


def call(method, params):
    """Ask Strata something and wait for the answer: (result, error)."""
    global _next_id
    _next_id += 1
    my_id = _next_id
    send({"id": my_id, "method": method, "params": params})
    while True:
        message = read_message()
        if message is None:
            sys.exit(0)
        if "method" not in message and message.get("id") == my_id:
            return message.get("result"), message.get("error")
        _waiting.append(message)


def count_words(text):
    words = re.findall(r"\S+", text)
    words = [w for w in words if re.search(r"\w", w)]
    if not _settings.get("count_numbers", True):
        words = [w for w in words if not re.fullmatch(r"[\d.,]+", w)]
    return len(words)


def update_status(document_id):
    """Count the words of a document and show them in each window that has
    it in front (an item for one document shows only there)."""
    result, error = call("document/read", {"document": document_id})
    if error:
        return
    doc = result["document"]
    if _counted.get(doc["id"]) == doc["version"]:
        return
    _counted[doc["id"]] = doc["version"]
    n = count_words(result["text"])
    call("ui/statusItem", {
        "id": "words",
        "document": doc["id"],
        "text": "Word Count: {:,}".format(n),
        "tooltip": "Words in {} (from the Word Count plugin)".format(doc["name"]),
        "command": "show",
    })


def show_panel(document_id):
    params = {} if document_id is None else {"document": document_id}
    result, error = call("document/read", params)
    if error:
        call("ui/notify", {"message": "Couldn't read the file: " + error["message"], "level": "warning"})
        return
    text = result["text"]
    lines = text.count("\n") + (1 if text and not text.endswith("\n") else 0)
    summary = (
        "| | Count |\n|:--|--:|\n"
        "| Words | {:,} |\n| Lines | {:,} |\n| Characters | {:,} |\n"
        "| Characters without spaces | {:,} |\n"
    ).format(count_words(text), lines, len(text), len(re.sub(r"\s", "", text)))
    call("ui/showPanel", {"title": result["document"]["name"], "markdown": summary})


def handle(message):
    method = message.get("method")
    params = message.get("params") or {}
    if "id" in message and method is not None:
        # A request from Strata: always answered.
        if method == "initialize":
            _settings.update(params.get("settings") or {})
            send({"id": message["id"], "result": {"api": API}})
        elif method == "command/execute":
            doc = params.get("document") or {}
            if params.get("command") == "show":
                show_panel(doc.get("id"))
            send({"id": message["id"], "result": None})
        elif method == "shutdown":
            send({"id": message["id"], "result": None})
        else:
            send({"id": message["id"], "error": {"code": -32601, "message": "unknown method " + str(method)}})
        return
    if method == "exit":
        sys.exit(0)
    doc_id = (params.get("document") or {}).get("id")
    if method == "document/activated":
        # A window shows this document in front: count it from now on.
        _counted.setdefault(doc_id, None)
        update_status(doc_id)
    elif method in ("document/changed", "document/saved") and doc_id in _counted:
        update_status(doc_id)
    elif method == "document/closed":
        _counted.pop(doc_id, None)
    elif method == "settings/changed":
        _settings.update(params.get("settings") or {})
        for doc_id in list(_counted):
            _counted[doc_id] = None
            update_status(doc_id)


def main():
    while True:
        message = _waiting.pop(0) if _waiting else read_message()
        if message is None:
            return
        handle(message)


if __name__ == "__main__":
    main()
