import assert from "node:assert/strict";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
    defaultVsixPath,
    installedExtensionIdentifier,
} from "./package-info.mjs";

const vsix = path.resolve(process.argv[2] ?? defaultVsixPath);
const cursor = process.env.CURSOR_BIN || "cursor";
const install = spawnSync(cursor, ["--install-extension", vsix, "--force"], {
    encoding: "utf8",
    stdio: "pipe",
});
assert.equal(
    install.status,
    0,
    `Cursor could not install the VSIX:\n${install.stdout}${install.stderr}`,
);
const list = spawnSync(cursor, ["--list-extensions", "--show-versions"], {
    encoding: "utf8",
    stdio: "pipe",
});
assert.equal(list.status, 0, `Cursor could not list extensions: ${list.stderr}`);
assert.ok(
    list.stdout.split(/\r?\n/).includes(installedExtensionIdentifier),
    `Cursor did not list ${installedExtensionIdentifier}:\n${list.stdout}`,
);
console.log(`Cursor install smoke passed: ${installedExtensionIdentifier}`);
