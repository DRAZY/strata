// Sort Unique: an example Strata plugin (API strata.plugin/1), in Node with
// no packages. "Sort Unique: Sort Lines Uniquely" sorts the lines of the
// selection (or of the whole file, with nothing selected) and drops
// duplicates, as one edit, so Undo takes it back.
//
// Strata talks to the plugin with JSON-RPC 2.0 messages on stdin and stdout,
// each after a "Content-Length: N" header. Nothing but messages may be
// written to stdout.

"use strict";

const API = "strata.plugin/1";
let buffer = Buffer.alloc(0);
let nextId = 0;
const waiting = new Map(); // request id -> resolve
let settings = { ignore_case: false };

function send(message) {
  message.jsonrpc = "2.0";
  const body = Buffer.from(JSON.stringify(message), "utf8");
  process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`);
  process.stdout.write(body);
}

function call(method, params) {
  const id = ++nextId;
  send({ id, method, params });
  return new Promise((resolve) => waiting.set(id, resolve));
}

function lineBreaks(text) {
  return (text.match(/\r\n|\n|\r/g) || []).length;
}

async function sortUnique(documentId) {
  const read = await call("document/read", documentId == null ? {} : { document: documentId });
  if (read.error) {
    return;
  }
  const { document, text, selections, lineEnding } = read.result;
  const lines = text.split(/\r\n|\n|\r/);
  const sel = (selections || []).find((s) => s.start.line !== s.end.line || s.start.character !== s.end.character);
  let first = 0;
  let last = lines.length - 1;
  let keepFinalBreak = false;
  if (sel) {
    first = sel.start.line;
    last = sel.end.line;
    // A selection ending at the start of a line doesn't include that line.
    if (sel.end.character === 0 && last > first) {
      last -= 1;
    }
  } else if (lines.length > 1 && lines[lines.length - 1] === "") {
    // The file ends with a line break: it stays at the end.
    last -= 1;
    keepFinalBreak = true;
  }
  const chosen = lines.slice(first, last + 1);
  const seen = new Set();
  const unique = [];
  for (const line of [...chosen].sort()) {
    const key = settings.ignore_case ? line.toLowerCase() : line;
    if (!seen.has(key)) {
      seen.add(key);
      unique.push(line);
    }
  }
  const removed = chosen.length - unique.length;
  // A column past the end of a line means its end, so the line's length
  // (in code points, not JavaScript's UTF-16 units) isn't needed.
  const end = keepFinalBreak ? { line: lineBreaks(text), character: 0 } : { line: last, character: 1e9 };
  const newText = unique.join(lineEnding) + (keepFinalBreak ? lineEnding : "");
  const edit = { range: { start: { line: first, character: 0 }, end }, newText };
  const applied = await call("document/applyEdits", { document: document.id, version: document.version, edits: [edit] });
  if (applied.error) {
    await call("ui/notify", { message: "Couldn't sort: " + applied.error.message, level: "warning" });
    return;
  }
  const what = removed === 1 ? "1 duplicate line" : `${removed} duplicate lines`;
  await call("ui/notify", { message: `Sorted ${chosen.length} lines and removed ${what}` });
}

async function handle(message) {
  const { method, params = {} } = message;
  if (method === undefined) {
    const resolve = waiting.get(message.id);
    if (resolve) {
      waiting.delete(message.id);
      resolve(message);
    }
    return;
  }
  if (message.id !== undefined) {
    if (method === "initialize") {
      Object.assign(settings, params.settings || {});
      send({ id: message.id, result: { api: API } });
    } else if (method === "command/execute") {
      if (params.command === "sort-unique") {
        await sortUnique(params.document ? params.document.id : null);
      }
      send({ id: message.id, result: null });
    } else if (method === "shutdown") {
      send({ id: message.id, result: null });
    } else {
      send({ id: message.id, error: { code: -32601, message: `unknown method ${method}` } });
    }
    return;
  }
  if (method === "exit") {
    process.exit(0);
  } else if (method === "settings/changed") {
    Object.assign(settings, params.settings || {});
  }
}

process.stdin.on("data", (chunk) => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const end = buffer.indexOf("\r\n\r\n");
    if (end < 0) {
      return;
    }
    const header = buffer.subarray(0, end).toString("ascii");
    const match = /content-length:\s*(\d+)/i.exec(header);
    const length = match ? Number(match[1]) : 0;
    if (buffer.length < end + 4 + length) {
      return;
    }
    const body = buffer.subarray(end + 4, end + 4 + length).toString("utf8");
    buffer = buffer.subarray(end + 4 + length);
    if (length > 0) {
      handle(JSON.parse(body)).catch((e) => process.stderr.write(String(e) + "\n"));
    }
  }
});
process.stdin.on("end", () => process.exit(0));
