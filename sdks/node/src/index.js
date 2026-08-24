import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import process from "node:process";

const DEFAULT_MAX = 1_048_576;

export const SDK_INFO = Object.freeze({
  name: "@webtest/node",
  version: "0.1.0",
  minimumProtocol: 1,
  maximumProtocol: 1,
  generatedSchemaRevision: 1,
  transports: Object.freeze(["unix", "named_pipe", "tcp", "stdio"]),
});

export class AppBridge {
  constructor(manifest, options = {}) {
    validateManifest(manifest);
    this.manifest = structuredClone(manifest);
    this.handlers = new Map();
    this.inFlight = new Map();
    this.maxMessageBytes = options.maxMessageBytes ?? DEFAULT_MAX;
    this.maxPendingCalls = options.maxPendingCalls ?? 1_024;
    this.maxEventsPerCall = options.maxEventsPerCall ?? 1_024;
    this.logger = options.logger ?? ((message) => process.stderr.write(`[webtest] ${message}\n`));
  }

  register(name, handler) {
    if (!this.manifest.functions[name]) throw new Error(`function ${name} is not in the manifest`);
    if (this.handlers.has(name)) throw new Error(`function ${name} is already registered`);
    if (typeof handler !== "function") throw new TypeError("handler must be a function");
    this.handlers.set(name, handler);
    return this;
  }

  exportSchema(destination) {
    fs.mkdirSync(path.dirname(path.resolve(destination)), { recursive: true, mode: 0o700 });
    fs.writeFileSync(destination, `${JSON.stringify(sortJson(this.manifest), null, 2)}\n`, { mode: 0o600 });
  }

  async connectFromEnv() {
    const endpoint = process.env.WEBTEST_BRIDGE ?? "stdio";
    const token = process.env.WEBTEST_TOKEN;
    if (!token) throw new Error("WEBTEST_TOKEN is required; bridge helpers must run only under WebTest");
    const io = await connect(endpoint);
    let state = "await_hello_ok";
    const send = (message) => io.writeFrame(message, this.maxMessageBytes);
    send({ type: "hello", protocol_versions: [1], sdk: SDK_INFO.name, sdk_version: SDK_INFO.version,
      token, capabilities: { cancel: true, events: true } });
    for await (const message of io.frames(this.maxMessageBytes)) {
      if (state === "await_hello_ok") {
        if (message.type === "hello_error") throw new Error(`${message.code}: ${message.message}`);
        if (message.type !== "hello_ok" || message.protocol !== 1) throw new Error("expected protocol-1 hello_ok");
        this.maxMessageBytes = Math.min(this.maxMessageBytes, message.max_message_bytes);
        state = "ready";
        continue;
      }
      if (message.type === "describe") {
        send({ type: "schema", id: message.id, protocol: 1, schema_hash: this.manifest.schema_hash,
          functions: this.manifest.functions });
      } else if (message.type === "call") {
        this.#call(message, send);
      } else if (message.type === "cancel") {
        this.inFlight.get(message.id)?.abort(message.reason);
      } else if (message.type === "ping") {
        send({ type: "pong", id: message.id });
      } else if (message.type === "shutdown") {
        state = "draining";
        for (const controller of this.inFlight.values()) controller.abort("shutdown");
        await Promise.race([
          Promise.allSettled([...this.inFlight.values()].map((controller) => controller.done)),
          new Promise((resolve) => setTimeout(resolve, 1_000)),
        ]);
        send({ type: "shutdown_ok", id: message.id });
        io.close();
        return;
      } else if (message.type !== "pong") {
        throw new Error(`unknown message type ${String(message.type)}`);
      }
    }
    if (state === "ready" && this.inFlight.size) throw new Error("bridge EOF with calls pending");
  }

  #call(message, send) {
    if (this.inFlight.has(message.id)) throw new Error(`duplicate request ID ${message.id}`);
    if (this.inFlight.size >= this.maxPendingCalls) throw new Error("too many pending calls");
    if (!Number.isSafeInteger(message.id) || message.id < 0) throw new Error("invalid request ID");
    if (!Number.isSafeInteger(message.deadline_ms) || message.deadline_ms < 1) throw new Error("invalid call deadline");
    const schema = this.manifest.functions[message.function];
    const handler = this.handlers.get(message.function);
    if (!schema || !handler) {
      send({ type: "error", id: message.id, code: "function.unknown", message: "unknown function",
        retryable: false, data: {} });
      return;
    }
    const controller = new AbortController();
    this.inFlight.set(message.id, controller);
    let eventCount = 0;
    const deadline = setTimeout(() => controller.abort("deadline"), message.deadline_ms);
    const done = (async () => {
      try {
        const args = withDefaults(schema.params, message.arguments);
        validateValue(schema.params, args, "$.arguments");
        const value = await handler(args, {
          signal: controller.signal,
          emit: (kind, value) => {
            if (++eventCount > this.maxEventsPerCall) throw new Error("too many call events");
            validateTransferable(value, "$.event.value");
            send({ type: "event", call_id: message.id, kind: bounded(String(kind), 128), value });
          },
        });
        if (controller.signal.aborted) {
          throw Object.assign(new Error("call was cancelled"), {
            code: controller.signal.reason === "deadline" ? "call.deadline" : "call.cancelled",
          });
        }
        validateValue(schema.returns, value, "$.result");
        send({ type: "result", id: message.id, value });
      } catch (error) {
        let data = error?.data ?? {};
        try { validateTransferable(data, "$.error.data"); } catch { data = {}; }
        const cancellationCode = controller.signal.reason === "deadline" ? "call.deadline" : "call.cancelled";
        send({ type: "error", id: message.id, code: error?.code ?? (controller.signal.aborted ? cancellationCode : "application.error"),
          message: bounded(error?.message ?? String(error), 4096), retryable: Boolean(error?.retryable), data });
      } finally {
        clearTimeout(deadline);
        this.inFlight.delete(message.id);
      }
    })();
    controller.done = done;
  }
}

function sortJson(value) {
  if (Array.isArray(value)) return value.map(sortJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sortJson(value[key])]));
  }
  return value;
}

function validateManifest(manifest) {
  if (manifest?.manifest_version !== 1 || manifest.protocol !== 1 || manifest.provider !== "app") {
    throw new Error("invalid protocol-1 app manifest");
  }
  if (typeof manifest.sdk !== "string" || !manifest.sdk || Buffer.byteLength(manifest.sdk) > 128
      || typeof manifest.sdk_version !== "string" || !manifest.sdk_version
      || Buffer.byteLength(manifest.sdk_version) > 64) {
    throw new Error("manifest SDK identity is missing or too large");
  }
  if (!/^blake3:[0-9a-f]{64}$/.test(manifest.schema_hash)) throw new Error("invalid schema hash");
  const functions = Object.entries(manifest.functions ?? {});
  if (functions.length > 10_000) throw new Error("manifest has too many functions");
  for (const [name, schema] of functions) {
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(name)) throw new Error(`invalid function name ${name}`);
    validateDocumentation(schema.documentation ?? "", `documentation for ${name}`);
    if (schema.params?.type !== "object") throw new Error(`function ${name} parameters must be an object`);
    validateSchemaShape(schema.params, `$.functions.${name}.params`);
    validateSchemaShape(schema.returns, `$.functions.${name}.returns`);
    for (const [fieldName, field] of Object.entries(schema.params.fields)) {
      if ("default" in field) {
        if (!field.optional) throw new Error(`${name}.${fieldName} default requires optional`);
        validateValue(field, field.default, `$.functions.${name}.params.${fieldName}.default`);
      }
    }
  }
}

function validateSchemaShape(schema, path, depth = 0) {
  if (!schema || typeof schema !== "object" || depth > 32) throw new Error(`${path} is not a bounded schema`);
  if (["null", "boolean", "integer", "float", "string"].includes(schema.type)) return;
  if (schema.type === "array") return validateSchemaShape(schema.items, `${path}.items`, depth + 1);
  if (schema.type === "optional") return validateSchemaShape(schema.item, `${path}.item`, depth + 1);
  if (schema.type === "alias") return validateSchemaShape(schema.base, `${path}.base`, depth + 1);
  if (schema.type === "object" && schema.fields && Object.keys(schema.fields).length <= 10_000) {
    for (const [name, field] of Object.entries(schema.fields)) {
      if (!/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(name)) throw new Error(`${path} has invalid field ${name}`);
      validateDocumentation(field.documentation ?? "", `${path}.${name} documentation`);
      validateSchemaShape(field, `${path}.${name}`, depth + 1);
    }
    return;
  }
  throw new Error(`${path} has unknown or invalid type ${String(schema.type)}`);
}

function validateDocumentation(value, subject) {
  if (typeof value !== "string" || Buffer.byteLength(value) > 1_024) {
    throw new Error(`${subject} is not bounded text`);
  }
  if (/[ --]/u.test(value)) {
    throw new Error(`${subject} contains a control character`);
  }
}

function withDefaults(schema, value) {
  const result = { ...(value ?? {}) };
  if (schema.type === "object") for (const [name, field] of Object.entries(schema.fields)) {
    if (!(name in result) && "default" in field) result[name] = structuredClone(field.default);
  }
  return result;
}

function validateValue(schema, value, path, depth = 0) {
  if (depth > 32) throw new Error(`${path} exceeds value depth`);
  if (schema.type === "alias") return validateValue(schema.base, value, path, depth + 1);
  if (schema.type === "optional" && value === null) return;
  if (schema.type === "optional") return validateValue(schema.item, value, path, depth + 1);
  const ok = schema.type === "null" ? value === null
    : schema.type === "boolean" ? typeof value === "boolean"
    : schema.type === "integer" ? Number.isSafeInteger(value)
    : schema.type === "float" ? typeof value === "number" && Number.isFinite(value)
    : schema.type === "string" ? typeof value === "string" && Buffer.byteLength(value) <= DEFAULT_MAX
    : schema.type === "array" ? Array.isArray(value)
    : schema.type === "object" ? value && typeof value === "object" && !Array.isArray(value)
    : false;
  if (!ok) throw new Error(`${path} does not match ${schema.type}`);
  if (schema.type === "array") {
    if (value.length > 10_000) throw new Error(`${path} has too many items`);
    value.forEach((item, index) => validateValue(schema.items, item, `${path}[${index}]`, depth + 1));
  }
  if (schema.type === "object") {
    if (Object.keys(value).length > 10_000) throw new Error(`${path} has too many fields`);
    for (const key of Object.keys(value)) if (!schema.fields[key]) throw new Error(`${path}.${key} is not declared`);
    for (const [name, field] of Object.entries(schema.fields)) {
      if (!(name in value) && !field.optional) throw new Error(`${path}.${name} is required`);
      if (name in value) validateValue(field, value[name], `${path}.${name}`, depth + 1);
    }
  }
}

function validateTransferable(value, path, depth = 0) {
  if (depth > 32) throw new Error(`${path} exceeds value depth`);
  if (value === null || typeof value === "boolean") return;
  if (typeof value === "number" && Number.isFinite(value)) return;
  if (typeof value === "string") {
    if (Buffer.byteLength(value) > DEFAULT_MAX) throw new Error(`${path} string is too large`);
    return;
  }
  if (Array.isArray(value)) {
    if (value.length > 10_000) throw new Error(`${path} has too many items`);
    value.forEach((item, index) => validateTransferable(item, `${path}[${index}]`, depth + 1));
    return;
  }
  if (value && typeof value === "object") {
    const entries = Object.entries(value);
    if (entries.length > 10_000) throw new Error(`${path} has too many fields`);
    entries.forEach(([name, item]) => validateTransferable(item, `${path}.${name}`, depth + 1));
    return;
  }
  throw new Error(`${path} is not transferable`);
}

function bounded(value, bytes) {
  while (Buffer.byteLength(value) > bytes) value = value.slice(0, -1);
  return value;
}

async function connect(endpoint) {
  if (endpoint === "stdio" || endpoint === "stdio:") return frameIo(process.stdin, process.stdout, () => {});
  let socket;
  if (endpoint.startsWith("unix:")) socket = net.createConnection(endpoint.slice(5));
  else if (endpoint.startsWith("tcp://")) {
    const url = new URL(endpoint);
    if (url.hostname !== "127.0.0.1" && url.hostname !== "localhost" && url.hostname !== "[::1]") throw new Error("refusing non-loopback bridge endpoint");
    socket = net.createConnection(Number(url.port), url.hostname);
  } else if (endpoint.startsWith("pipe:")) socket = net.createConnection(endpoint.slice(5));
  else throw new Error(`unsupported bridge endpoint ${endpoint}`);
  await new Promise((resolve, reject) => socket.once("connect", resolve).once("error", reject));
  return frameIo(socket, socket, () => socket.end());
}

function frameIo(readable, writable, close) {
  return {
    writeFrame(message, max) {
      const encoded = JSON.stringify(message);
      if (Buffer.byteLength(encoded) > max) throw new Error("frame too large");
      writable.write(`${encoded}\n`);
    },
    async *frames(max) {
      let buffered = Buffer.alloc(0);
      const decoder = new TextDecoder("utf-8", { fatal: true });
      for await (const chunk of readable) {
        buffered = Buffer.concat([buffered, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)]);
        if (buffered.length > max + 1 && !buffered.includes(0x0a)) throw new Error("frame too large");
        let newline;
        while ((newline = buffered.indexOf(0x0a)) >= 0) {
          let line = buffered.subarray(0, newline);
          buffered = buffered.slice(newline + 1);
          if (line.at(-1) === 0x0d) line = line.subarray(0, -1);
          if (line.length > max) throw new Error("frame too large");
          const value = JSON.parse(decoder.decode(line));
          if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("message must be an object");
          yield value;
        }
      }
      if (buffered.length) throw new Error("truncated bridge frame");
    },
    close,
  };
}
