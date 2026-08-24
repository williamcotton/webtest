import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { AppBridge } from "../src/index.js";

const manifest = JSON.parse(fs.readFileSync(new URL("../../../protocol/examples/app-schema.json", import.meta.url)));

test("registration is schema-bound and rejects duplicates", () => {
  const bridge = new AppBridge(manifest).register("create_user", () => ({}));
  assert.throws(() => bridge.register("create_user", () => ({})), /already registered/);
  assert.throws(() => bridge.register("missing", () => ({})), /not in the manifest/);
});

test("manifest metadata and documentation are bounded", () => {
  assert.throws(() => new AppBridge({ ...manifest, sdk: "" }), /SDK identity/);
  const invalid = structuredClone(manifest);
  invalid.functions.create_user.documentation = "invalid\u0000documentation";
  assert.throws(() => new AppBridge(invalid), /control character/);
});

test("schema export is deterministic across object insertion order", () => {
  const reverseKeys = (value) => Array.isArray(value) ? value.map(reverseKeys)
    : value && typeof value === "object"
      ? Object.fromEntries(Object.entries(value).reverse().map(([key, item]) => [key, reverseKeys(item)]))
      : value;
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "webtest-node-sdk-"));
  try {
    const first = path.join(directory, "first.json");
    const second = path.join(directory, "second.json");
    new AppBridge(manifest).exportSchema(first);
    new AppBridge(reverseKeys(manifest)).exportSchema(second);
    assert.deepEqual(fs.readFileSync(first), fs.readFileSync(second));
    assert.deepEqual(JSON.parse(fs.readFileSync(first)), manifest);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
