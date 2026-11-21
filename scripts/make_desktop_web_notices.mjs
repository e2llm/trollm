import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { collectNotices } from "../apps/flow-review/web/scripts/build-notices.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/web");
const lock = readFileSync(join(root, "package-lock.json"));
const text = collectNotices(root, "trollm desktop", [
  "node_modules/vite", "node_modules/rolldown",
]);
const directory = join(root, "dist/notices");
mkdirSync(directory, { recursive: true });
writeFileSync(join(directory, "web.txt"), text);
writeFileSync(join(directory, "web.json"), JSON.stringify({
  format: "trollm.desktop-web-notices.v1",
  lockSha256: createHash("sha256").update(lock).digest("hex"),
  noticesSha256: createHash("sha256").update(text).digest("hex"),
}, null, 2) + "\n");
console.log("Collected desktop browser dependency notices.");
