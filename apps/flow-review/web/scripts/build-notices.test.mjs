import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectNotices } from "./build-notices.mjs";
function fixture(fn) {
  const root = mkdtempSync(join(tmpdir(), "trollm-notices-"));
  try {
    const pkg = join(root, "node_modules", "sample");
    mkdirSync(pkg, { recursive: true });
    writeFileSync(
      join(root, "package-lock.json"),
      JSON.stringify({
        packages: {
          "": {},
          "node_modules/sample": { version: "1.0.0" },
          "node_modules/dev-only": { dev: true, version: "2.0.0" },
        },
      }),
    );
    writeFileSync(
      join(pkg, "package.json"),
      JSON.stringify({ name: "sample", version: "1.0.0", license: "MIT" }),
    );
    fn(root, pkg);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}
test("retains actual production package copyright and license text", () =>
  fixture((root, pkg) => {
    writeFileSync(
      join(pkg, "LICENSE"),
      "Copyright upstream.\nPermission granted.",
    );
    const text = collectNotices(root);
    assert.match(text, /sample@1.0.0/);
    assert.match(text, /Copyright upstream\.\nPermission granted\./);
    assert.doesNotMatch(text, /dev-only/);
  }));
test("fails clearly when upstream license text is missing", () =>
  fixture((root) =>
    assert.throws(
      () => collectNotices(root),
      /Missing license text for sample@1.0.0/,
    ),
  ));
