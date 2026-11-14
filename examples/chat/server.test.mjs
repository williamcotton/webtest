import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import test from "node:test";
import { WebSocket } from "ws";

test(
  "independent sessions relay authenticated messages and replay shared room history",
  { timeout: 15000 },
  async (t) => {
    const child = spawn(process.execPath, ["server.mjs"], {
      cwd: new URL(".", import.meta.url),
      env: { ...process.env, WEBTEST: "0", WEBTEST_APP_PORT: "0" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (bytes) => {
      stderr = (stderr + bytes).slice(-4096);
    });
    t.after(async () => {
      if (child.exitCode !== null) return;
      const exited = once(child, "exit");
      child.kill("SIGTERM");
      const timer = setTimeout(() => child.kill("SIGKILL"), 2000);
      await exited;
      clearTimeout(timer);
    });
    const lines = createInterface({ input: child.stdout });
    const origin = await new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`App startup timed out: ${stderr}`)),
        5000,
      );
      child.once("error", reject);
      child.once("exit", (code) => {
        clearTimeout(timer);
        reject(new Error(`App exited ${code}: ${stderr}`));
      });
      lines.on("line", (line) => {
        const match = line.match(
          /Chat is ready at (http:\/\/127\.0\.0\.1:\d+)/,
        );
        if (match) {
          clearTimeout(timer);
          resolve(match[1]);
        }
      });
    });
    const login = async (email) => {
      const response = await fetch(`${origin}/api/login`, {
        method: "POST",
        headers: { "content-type": "application/json", origin },
        body: JSON.stringify({ room: "lobby", email, password: "hello-chat" }),
      });
      assert.equal(response.status, 200);
      return response.headers.get("set-cookie").split(";")[0];
    };
    const aliceCookie = await login("alice@chat.test");
    const bobCookie = await login("bob@chat.test");
    assert.notEqual(aliceCookie, bobCookie);
    for (const [cookie, name] of [
      [aliceCookie, "Alice"],
      [bobCookie, "Bob"],
      ["", null],
    ]) {
      const response = await fetch(`${origin}/api/session?room=lobby`, {
        headers: { cookie },
      });
      assert.equal((await response.json()).name, name);
    }
    function connect(cookie) {
      const socket = new WebSocket(
        `${origin.replace("http:", "ws:")}/chat?room=lobby`,
        { headers: { cookie, origin } },
      );
      const history = [];
      const pending = new Set();
      socket.on("error", () => {});
      socket.on("message", (bytes) => {
        const value = JSON.parse(bytes.toString());
        history.push(value);
        for (const waiter of pending)
          if (waiter.predicate(value)) waiter.resolve(value);
      });
      t.after(() => socket.terminate());
      const until = (predicate) => {
        const found = history.find(predicate);
        if (found) return Promise.resolve(found);
        return new Promise((resolve, reject) => {
          const waiter = {
            predicate,
            resolve: (value) => {
              clearTimeout(timer);
              pending.delete(waiter);
              resolve(value);
            },
          };
          const timer = setTimeout(() => {
            pending.delete(waiter);
            reject(new Error("Expected chat event missing"));
          }, 3000);
          pending.add(waiter);
        });
      };
      return { socket, until };
    }
    const alice = connect(aliceCookie);
    const bob = connect(bobCookie);
    await Promise.all([
      alice.until((v) => v.type === "ready"),
      bob.until((v) => v.type === "ready"),
    ]);
    await alice.until((v) => v.type === "presence" && v.members.length === 2);
    alice.socket.send(
      JSON.stringify({
        type: "send",
        from: "Mallory",
        body: "Coffee? ☕ <hello>",
      }),
    );
    const greeting = await bob.until((v) => v.type === "message");
    assert.equal(
      greeting.from,
      "Alice",
      "client input cannot impersonate the sender",
    );
    assert.equal(greeting.body, "Coffee? ☕ <hello>");
    bob.socket.send(JSON.stringify({ type: "send", body: "Yes, Alice!" }));
    const reply = await alice.until(
      (v) => v.type === "message" && v.from === "Bob",
    );
    assert.equal(reply.body, "Yes, Alice!");
    alice.socket.close();
    const reloaded = connect(aliceCookie);
    assert.equal(
      (await reloaded.until((v) => v.type === "ready")).name,
      "Alice",
    );
    assert.equal(
      (
        await reloaded.until(
          (v) => v.type === "message" && v.id === greeting.id,
        )
      ).body,
      greeting.body,
    );
    assert.equal(
      (await reloaded.until((v) => v.type === "message" && v.id === reply.id))
        .body,
      reply.body,
    );
    assert.equal(
      stderr,
      "",
      "the app should close without background socket errors",
    );
  },
);
