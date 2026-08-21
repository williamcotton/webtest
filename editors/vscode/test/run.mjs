import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { downloadAndUnzipVSCode, runTests } from "@vscode/test-electron";

const extensionDevelopmentPath = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const repository = path.resolve(extensionDevelopmentPath, "../..");
const extensionTestsPath = path.join(extensionDevelopmentPath, "test/suite/index.cjs");
const vscodeExecutablePath =
    process.env.VSCODE_EXECUTABLE_PATH || (await downloadAndUnzipVSCode("stable"));
const chrome = findChrome(repository);
const headedAvailable =
    Boolean(chrome) &&
    (process.platform !== "linux" || Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY));

await runTests({
    vscodeExecutablePath,
    extensionDevelopmentPath,
    extensionTestsPath,
    launchArgs: [repository, "--disable-extensions"],
    extensionTestsEnv: {
        ...process.env,
        ...(chrome ? { WEBTEST_CHROME_PATH: chrome } : {}),
        WEBTEST_SMOKE_HEADED: String(headedAvailable),
    },
});

function findChrome(root) {
    if (process.env.WEBTEST_CHROME_PATH && fs.existsSync(process.env.WEBTEST_CHROME_PATH)) {
        return process.env.WEBTEST_CHROME_PATH;
    }
    const webtest = path.join(root, "target", "debug", process.platform === "win32" ? "webtest.exe" : "webtest");
    if (fs.existsSync(webtest)) {
        const resolved = spawnSync(webtest, ["browser", "path"], {
            cwd: root,
            encoding: "utf8",
        });
        const executable = resolved.stdout?.trim();
        if (resolved.status === 0 && executable && fs.existsSync(executable)) {
            return executable;
        }
    }
    const candidates = [];
    if (process.platform === "darwin") {
        candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    } else if (process.platform === "linux") {
        candidates.push("/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser");
    }
    return candidates.find((candidate) => fs.existsSync(candidate));
}
