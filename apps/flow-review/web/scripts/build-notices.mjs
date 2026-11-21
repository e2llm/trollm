import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Include the complete production lock tree, conservatively including type packages. */
export function collectNotices(root, title = "trollm flow review", extraPaths = []) {
  const lock = JSON.parse(
    readFileSync(join(root, "package-lock.json"), "utf8"),
  );
  const notices = [];
  for (const [path, entry] of Object.entries(lock.packages).sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    if (!path || (entry.dev && !extraPaths.includes(path))) continue;
    if (!path.startsWith("node_modules/") || path.split("/").includes(".."))
      throw new Error(`Unsafe dependency path: ${path}`);
    const directory = join(root, path),
      metadata = JSON.parse(
        readFileSync(join(directory, "package.json"), "utf8"),
      );
    if (metadata.version !== entry.version)
      throw new Error(`Installed dependency does not match lock: ${path}`);
    const files = readdirSync(directory)
      .filter(
        (name) =>
          /^(licen[sc]e|copying|notice)([._-].*)?$/i.test(name) &&
          statSync(join(directory, name)).isFile(),
      )
      .sort();
    if (!files.length)
      throw new Error(
        `Missing license text for ${metadata.name}@${metadata.version}; supply the upstream license before distributing.`,
      );
    const texts = files.map((name) => {
      const text = readFileSync(join(directory, name), "utf8").trim();
      if (!text) throw new Error(`Empty license text: ${path}/${name}`);
      return `--- ${name} ---\n${text}`;
    });
    notices.push(
      `${metadata.name}@${metadata.version}\nDeclared license: ${typeof metadata.license === "string" ? metadata.license : "see upstream text"}\n\n${texts.join("\n\n")}`,
    );
  }
  return `${title} — browser dependency notices\n\nExact installed production dependencies and explicitly included build-runtime helpers from package-lock.json.\nLicense and attribution texts are reproduced from upstream package files.\n\n${notices.join("\n\n" + "=".repeat(78) + "\n\n")}\n`;
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const root = dirname(dirname(fileURLToPath(import.meta.url)));
  const text = collectNotices(root);
  writeFileSync(join(root, "dist", "third-party-notices.txt"), text);
  console.log(
    `Wrote third-party-notices.txt (${Buffer.byteLength(text)} bytes)`,
  );
}
