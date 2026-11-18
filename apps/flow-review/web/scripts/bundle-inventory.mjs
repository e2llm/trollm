import { copyFileSync, existsSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  INVENTORY,
  MODULES,
  PUBLIC_RECEIPT,
  RECEIPT,
  keys,
  outputFiles,
  readContext,
  readJson,
  relativePath,
  requireValue,
  sha256,
  validateInventory,
  validateReceipt,
} from "./audit-evidence.mjs";

export function collectBundle(
  root,
  bundle,
  context,
  receipt,
  now = Date.now(),
) {
  const packages = new Map();
  const files = [];
  let moduleCount = 0;
  for (const output of Object.values(bundle)) {
    relativePath(output.fileName);
    requireValue(
      ![INVENTORY, MODULES, PUBLIC_RECEIPT].includes(output.fileName),
      "Reserved bundle evidence filename",
    );
    files.push({
      path: output.fileName,
      sha256: sha256(output.type === "chunk" ? output.code : output.source),
    });
    if (output.type !== "chunk") continue;
    for (const id of Object.keys(output.modules)) {
      moduleCount++;
      const marker = "/node_modules/",
        index = id.lastIndexOf(marker);
      if (index < 0) continue;
      const parts = id.slice(index + marker.length).split("/");
      const name = parts[0].startsWith("@")
        ? parts.slice(0, 2).join("/")
        : parts[0];
      const directory = id.slice(0, index + marker.length) + name;
      const packagePath = relative(root, directory).split(sep).join("/");
      relativePath(packagePath);
      const metadata = readJson(join(directory, "package.json"));
      requireValue(
        metadata.name === name &&
          context.lock.packages[packagePath]?.version === metadata.version,
        "Output module package differs from the lockfile",
      );
      const entry = packages.get(packagePath) ?? {
        name,
        version: metadata.version,
        path: packagePath,
        modules: 0,
      };
      entry.modules++;
      packages.set(packagePath, entry);
    }
  }
  if (receipt) validateReceipt(receipt, context, now);
  return {
    schema: "trollm.web-bundle-modules.v1",
    generatedAt: new Date(now).toISOString(),
    lockSha256: context.lockSha256,
    policySha256: context.policySha256,
    prebuildNonce: receipt?.nonce ?? null,
    moduleCount,
    packages: [...packages.values()].sort((a, b) =>
      a.path.localeCompare(b.path),
    ),
    files: files.sort((a, b) => a.path.localeCompare(b.path)),
  };
}

export function bundleInventoryPlugin() {
  let root, outputDirectory;
  return {
    name: "trollm-bundle-package-inventory",
    apply: "build",
    enforce: "post",
    configResolved(config) {
      root = config.root;
      outputDirectory = resolve(root, config.build.outDir);
    },
    writeBundle(_options, bundle) {
      const context = readContext(root);
      const receipt = existsSync(join(root, RECEIPT))
        ? readJson(join(root, RECEIPT))
        : null;
      const inventory = collectBundle(root, bundle, context, receipt);
      writeFileSync(
        join(outputDirectory, MODULES),
        JSON.stringify(inventory, null, 2) + "\n",
      );
    },
  };
}

export function finalizeInventory(root, now = Date.now()) {
  const context = readContext(root, now),
    directory = join(root, "dist");
  const raw = readJson(join(directory, MODULES));
  keys(raw, [
    "schema",
    "generatedAt",
    "lockSha256",
    "policySha256",
    "prebuildNonce",
    "moduleCount",
    "packages",
    "files",
  ]);
  requireValue(
    raw.schema === "trollm.web-bundle-modules.v1" &&
      raw.lockSha256 === context.lockSha256 &&
      raw.policySha256 === context.policySha256,
    "Stale output module evidence",
  );
  const recorded = outputFiles(directory);
  requireValue(
    !recorded.some((file) => file.path === PUBLIC_RECEIPT),
    "Unexpected pre-existing public audit receipt",
  );
  requireValue(
    recorded.some((file) => file.path === "third-party-notices.txt") &&
      JSON.stringify(raw.files) ===
        JSON.stringify(
          recorded.filter((file) => file.path !== "third-party-notices.txt"),
        ),
    "Outputs changed after Vite recorded their modules",
  );
  if (raw.prebuildNonce !== null)
    copyFileSync(join(root, RECEIPT), join(directory, PUBLIC_RECEIPT));
  const files = outputFiles(directory);
  const inventory = { ...raw, schema: "trollm.web-bundle-inventory.v1", files };
  if (raw.prebuildNonce !== null) {
    validateInventory(
      inventory,
      context,
      readJson(join(root, RECEIPT)),
      files,
      now,
    );
  }
  writeFileSync(
    join(directory, INVENTORY),
    JSON.stringify(inventory, null, 2) + "\n",
  );
  rmSync(join(directory, MODULES));
  console.log(
    `Recorded ${inventory.packages.length} bundled packages and ${files.length} output hashes${raw.prebuildNonce === null ? " (no prebuild audit; not eligible for the final gate)" : ""}.`,
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    finalizeInventory(dirname(dirname(fileURLToPath(import.meta.url))));
  } catch (error) {
    console.error(`Bundle inventory failed: ${error.message}`);
    process.exitCode = 1;
  }
}
