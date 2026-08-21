import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const extensionDirectory = path.dirname(
    path.dirname(fileURLToPath(import.meta.url)),
);
export const packageManifest = JSON.parse(
    fs.readFileSync(path.join(extensionDirectory, "package.json"), "utf8"),
);
export const vsixFilename = `${packageManifest.name}-${packageManifest.version}.vsix`;
export const defaultVsixPath = path.join(extensionDirectory, vsixFilename);
export const installedExtensionIdentifier =
    `${packageManifest.publisher}.${packageManifest.name}@${packageManifest.version}`;
