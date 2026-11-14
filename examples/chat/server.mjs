import http from "node:http";
import { readFile } from "node:fs/promises";
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { WebSocketServer, WebSocket } from "ws";
import { AppBridge } from "@webtest/node";

// This deliberately small, loopback-only demo keeps all state in memory.
const rooms = new Map();
const sessions = new Map();
const sockets = new WebSocketServer({ noServer: true, maxPayload: 4096 });
const assets = new Map(
  await Promise.all(
    [
      ["/", "index.html", "text/html; charset=utf-8"],
      ["/client.js", "client.js", "text/javascript; charset=utf-8"],
      ["/style.css", "style.css", "text/css; charset=utf-8"],
    ].map(async ([path, file, type]) => [
      path,
      { body: await readFile(new URL(file, import.meta.url)), type },
    ]),
  ),
);

function prepareRoom(label, demo = false) {
  if (rooms.size >= 100)
    throw new Error("Room limit reached; restart the demo.");
  const id = demo ? "lobby" : randomUUID();
  const users = ["Alice", "Bob"].map((name) => ({
    name,
    email: `${name.toLowerCase()}@chat.test`,
    password: demo ? "hello-chat" : randomBytes(16).toString("hex"),
  }));
  rooms.set(id, { id, label, users, history: [], nextId: 1 });
  return { path: `/?room=${id}`, alice: users[0], bob: users[1] };
}
prepareRoom("The conversation room", true);

function cookie(request) {
  return request.headers.cookie
    ?.split(";")
    .map((v) => v.trim())
    .find((v) => v.startsWith("chat_session="))
    ?.slice("chat_session=".length);
}
function session(request, room) {
  const value = sessions.get(cookie(request));
  return value?.room === room ? value : undefined;
}
function json(response, status, value) {
  response.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
}
function send(socket, value) {
  if (socket.readyState !== WebSocket.OPEN) return;
  if (socket.bufferedAmount > 65536) return socket.close(1013, "Slow consumer");
  socket.send(JSON.stringify(value));
}
function broadcast(room, value) {
  for (const socket of sockets.clients)
    if (socket.identity.room === room) send(socket, value);
}
function presence(room) {
  const members = [
    ...new Set(
      [...sockets.clients]
        .filter(
          (s) => s.readyState === WebSocket.OPEN && s.identity.room === room,
        )
        .map((s) => s.identity.name),
    ),
  ].sort();
  broadcast(room, { type: "presence", members });
}
async function body(request) {
  let size = 0;
  const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 4096) throw new Error("Request too large");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}
function samePassword(actual, expected) {
  const a = Buffer.from(actual ?? "");
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://localhost");
    if (request.method === "GET" && url.pathname === "/health")
      return json(response, 200, { ok: true });
    if (request.method === "GET" && assets.has(url.pathname)) {
      const asset = assets.get(url.pathname);
      response.writeHead(200, {
        "content-type": asset.type,
        "cache-control": "no-store",
        "content-security-policy":
          "default-src 'self'; connect-src 'self' ws://127.0.0.1:* ws://localhost:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        "x-content-type-options": "nosniff",
      });
      return response.end(asset.body);
    }
    if (url.pathname === "/favicon.ico") {
      response.writeHead(204);
      return response.end();
    }
    if (request.method === "GET" && url.pathname === "/api/session") {
      const room = rooms.get(url.searchParams.get("room"));
      if (!room) return json(response, 404, { error: "Room not found" });
      const identity = session(request, room.id);
      return json(response, 200, {
        room: room.label,
        name: identity?.name ?? null,
      });
    }
    if (request.method === "POST" && url.pathname === "/api/login") {
      // Only same-origin browser requests may change a session.
      if (request.headers.origin !== `http://${request.headers.host}`)
        return json(response, 403, { error: "Invalid origin" });
      const input = await body(request);
      const room = rooms.get(input.room);
      const user = room?.users.find((u) => u.email === input.email);
      if (
        !user ||
        typeof input.password !== "string" ||
        !samePassword(input.password, user.password)
      ) {
        return json(response, 401, { error: "Check your email and password." });
      }
      if (sessions.size >= 1000)
        return json(response, 503, { error: "Session limit reached" });
      const token = randomBytes(24).toString("hex");
      sessions.delete(cookie(request));
      sessions.set(token, { room: room.id, name: user.name });
      response.setHeader(
        "set-cookie",
        `chat_session=${token}; HttpOnly; SameSite=Strict; Path=/`,
      );
      return json(response, 200, { name: user.name, room: room.label });
    }
    json(response, 404, { error: "Not found" });
  } catch {
    if (!response.headersSent)
      json(response, 400, { error: "Invalid request" });
    else response.end();
  }
});
server.requestTimeout = 5000;
server.headersTimeout = 5000;
server.on("upgrade", (request, socket, head) => {
  const url = new URL(request.url, "http://localhost");
  const identity = session(request, url.searchParams.get("room"));
  if (
    url.pathname !== "/chat" ||
    !identity ||
    sockets.clients.size >= 32 ||
    request.headers.origin !== `http://${request.headers.host}`
  ) {
    socket.end("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
    return;
  }
  sockets.handleUpgrade(request, socket, head, (ws) => {
    ws.identity = identity;
    ws.on("error", (error) => console.error("Chat socket:", error.message));
    send(ws, { type: "ready", name: identity.name });
    const room = rooms.get(identity.room);
    for (const message of room.history) send(ws, message);
    presence(identity.room);
    ws.on("message", (bytes, binary) => {
      try {
        const input = JSON.parse(bytes.toString());
        if (
          binary ||
          input.type !== "send" ||
          typeof input.body !== "string" ||
          !input.body.trim() ||
          input.body.length > 512
        )
          throw new Error("Invalid message");
        // Sender identity comes exclusively from the authenticated socket.
        const message = {
          type: "message",
          id: room.nextId++,
          from: identity.name,
          body: input.body.trim(),
          room: room.id,
        };
        room.history.push(message);
        if (room.history.length > 100) room.history.shift();
        broadcast(identity.room, message);
      } catch {
        ws.close(1008, "Invalid message");
      }
    });
    ws.on("close", () => presence(identity.room));
  });
});

let closing;
function close() {
  if (closing) return closing;
  closing = new Promise((resolve) => {
    for (const socket of sockets.clients) socket.terminate();
    sockets.close();
    server.close(resolve);
    server.closeAllConnections();
  });
  return closing;
}
process.on("SIGTERM", () => void close());
process.on("SIGINT", () => void close());
const port = Number(process.env.WEBTEST_APP_PORT ?? process.env.PORT ?? 3110);
server.listen(port, "127.0.0.1", async () => {
  console.log(`Chat is ready at http://127.0.0.1:${server.address().port}`);
  if (process.env.WEBTEST !== "1") return;
  try {
    const manifest = JSON.parse(
      await readFile(
        new URL(".webtest/app-schema.json", import.meta.url),
        "utf8",
      ),
    );
    await new AppBridge(manifest)
      .register("prepare_room", ({ label }) => prepareRoom(label))
      .connectFromEnv();
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
  } finally {
    await close();
  }
});
