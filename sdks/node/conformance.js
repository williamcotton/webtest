import fs from "node:fs";
import { AppBridge } from "./src/index.js";

const manifest = JSON.parse(fs.readFileSync(new URL("../../protocol/conformance/app-schema.json", import.meta.url)));
const users = [];
const bridge = new AppBridge({ ...manifest, sdk: "@webtest/node", sdk_version: "0.1.0" });
bridge.register("create_user", ({ email, admin = false }) => {
  if (users.some((user) => user.email === email)) {
    throw Object.assign(new Error("a user with that email already exists"), { code: "user.email_taken" });
  }
  const user = { id: users.length + 1, email, admin };
  users.push(user);
  return user;
});
for (const name of Object.keys(manifest.functions).filter((name) => name.startsWith("echo_"))) {
  bridge.register(name, ({ value = null }) => value);
}
bridge.register("wait", ({ delay_ms }, { signal, emit }) => new Promise((resolve, reject) => {
  emit("progress", { phase: "waiting" });
  const timer = setTimeout(() => resolve("completed"), delay_ms);
  signal.addEventListener("abort", () => {
    clearTimeout(timer);
    reject(Object.assign(new Error("call was cancelled"), { code: "call.cancelled" }));
  }, { once: true });
}));
await bridge.connectFromEnv();
