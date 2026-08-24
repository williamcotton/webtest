import http, { type IncomingMessage, type ServerResponse } from "node:http";
import fs from "node:fs";
import { AppBridge } from "../../../sdks/node/src/index.js";

type User = { id: number; email: string; admin: boolean };

if (process.env.WEBTEST !== "1") throw new Error("this example bridge is test-only");
const users = new Map<string, User>();
const manifest = JSON.parse(
  fs.readFileSync(new URL(".webtest/app-schema.json", import.meta.url), "utf8"),
);
const bridge = new AppBridge(manifest).register(
  "create_user",
  ({ email, admin = false }: { email: string; admin?: boolean }): User => {
    if (users.has(email)) {
      throw Object.assign(new Error("email already exists"), { code: "user.email_taken" });
    }
    const user = { id: users.size + 1, email, admin };
    users.set(email, user);
    return user;
  },
);

const page = (body: string): string => `<!doctype html><html><body>${body}</body></html>`;
const server = http.createServer((request: IncomingMessage, response: ServerResponse) => {
  if (request.method === "GET" && request.url === "/health") return response.end("ok");
  if (request.method === "GET" && request.url === "/login") {
    response.setHeader("content-type", "text/html");
    return response.end(
      page('<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>'),
    );
  }
  if (request.method === "POST" && request.url === "/login") {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk: string) => {
      body += chunk;
    });
    return request.on("end", () => {
      const email = new URLSearchParams(body).get("email") ?? "";
      response.setHeader("content-type", "text/html");
      response.end(page(users.has(email) ? `<p>Welcome, ${email}</p>` : "<p>Invalid sign in</p>"));
    });
  }
  response.statusCode = 404;
  response.end("not found");
});
server.listen(Number(process.env.PORT || 3101), "127.0.0.1", async () => {
  try {
    await bridge.connectFromEnv();
  } finally {
    server.close();
  }
});
