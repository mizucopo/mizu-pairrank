import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { checkNotices, generateNotices } from "./third-party-notices.mjs";

const repository = fileURLToPath(new URL("../", import.meta.url));
const license = readFileSync(resolve(repository, "node_modules/pdfmake/LICENSE"), "utf8");
const hash = (text) => createHash("sha256").update(text).digest("hex");

function fixture(t) {
  const root = mkdtempSync(resolve(tmpdir(), "pairrank-notices-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const write = (path, value) => {
    const destination = resolve(root, path);
    mkdirSync(dirname(destination), { recursive: true });
    writeFileSync(destination, value);
  };
  const writeJson = (path, value) => write(path, JSON.stringify(value));
  const packages = {
    "node_modules/pdfmake": { version: "fixture", integrity: "pinned" },
    "node_modules/runtime": { version: "1.0.0" },
    "node_modules/development": { version: "1.0.0", dev: true },
  };
  for (const [path, entry] of Object.entries(packages)) {
    writeJson(`${path}/package.json`, {
      name: path.slice(13),
      version: entry.version,
      license: "MIT",
    });
    write(`${path}/LICENSE`, license);
  }
  const map = {
    sources: ["webpack://pdfmake/./node_modules/runtime/index.js"],
    sourcesContent: ["/* Copyright original author */"],
  };
  const bundle = "fixture bundled code";
  const sourceMap = JSON.stringify(map);
  write("node_modules/pdfmake/build/pdfmake.js", bundle);
  write("node_modules/pdfmake/build/pdfmake.js.map", sourceMap);
  write("node_modules/pdfmake/src/3rd-party/svg-to-pdfkit/LICENSE", license);
  const record = { source: "fixture upstream license", text: license };
  const inputs = {
    pdfmakeVersion: "fixture",
    bundleSha256: hash(bundle),
    sourceMapSha256: hash(sourceMap),
    bundlePackages: Object.fromEntries(
      ["runtime", "webpack", "base64-arraybuffer"].map((name) => [
        name,
        { license: "MIT", licenseSourceVersion: "fixture", records: [record] },
      ]),
    ),
    missingLicensePackages: {},
    additionalTerms: [record],
  };
  writeJson("package-lock.json", { packages });
  writeJson("scripts/third-party-license-inputs.json", inputs);
  return { root, write, writeJson, packages, map, inputs };
}

test("collects production and vendored attribution while excluding development-only dependencies", (t) => {
  const { root } = fixture(t);
  const text = generateNotices(root);
  assert.match(text, /runtime@1\.0\.0/);
  assert.match(text, /SVG-to-PDFKit/);
  assert.match(text, /Copyright original author/);
  assert.doesNotMatch(text, /development@/);
  assert.equal(
    text.split("Permission is hereby granted").length,
    2,
    "identical license bodies appear once",
  );
});

test("rejects a missing license, then accepts the repaired license file", (t) => {
  const { root, write } = fixture(t);
  rmSync(resolve(root, "node_modules/runtime/LICENSE"));
  assert.throws(() => generateNotices(root), /Missing license text/);
  write("node_modules/runtime/LICENSE", license);
  assert.doesNotThrow(() => generateNotices(root));
});

test("rejects unknown licensing and installed versions different from the lockfile", (t) => {
  const { root, writeJson } = fixture(t);
  writeJson("node_modules/runtime/package.json", {
    name: "runtime",
    version: "1.0.0",
    license: "UNKNOWN",
  });
  assert.throws(() => generateNotices(root), /Unknown license/);
  writeJson("node_modules/runtime/package.json", {
    name: "runtime",
    version: "2.0.0",
    license: "MIT",
  });
  assert.throws(() => generateNotices(root), /Install differs from lockfile/);
});

test("uses reviewed missing-file inputs only for the pinned version and integrity", (t) => {
  const { root, writeJson, packages, inputs } = fixture(t);
  rmSync(resolve(root, "node_modules/runtime/LICENSE"));
  packages["node_modules/runtime"].integrity = "reviewed integrity";
  inputs.missingLicensePackages.runtime = {
    version: "1.0.0",
    integrity: "reviewed integrity",
    records: [{ source: "reviewed upstream declaration", text: license }],
  };
  writeJson("package-lock.json", { packages });
  writeJson("scripts/third-party-license-inputs.json", inputs);
  assert.doesNotThrow(() => generateNotices(root));
  packages["node_modules/runtime"].integrity = "changed package bytes";
  writeJson("package-lock.json", { packages });
  assert.throws(() => generateNotices(root), /Missing license text/);
  packages["node_modules/runtime"] = { version: "2.0.0", integrity: "reviewed integrity" };
  writeJson("package-lock.json", { packages });
  writeJson("node_modules/runtime/package.json", {
    name: "runtime",
    version: "2.0.0",
    license: "MIT",
  });
  assert.throws(() => generateNotices(root), /Missing license text/);
});

test("rejects NOTICE text and lockfile dependency drift, then accepts regenerated output", (t) => {
  const { root, write, writeJson, packages } = fixture(t);
  const notice = generateNotices(root);
  write("src/assets/THIRD_PARTY_NOTICES.txt", notice + "changed license wording\n");
  assert.throws(() => checkNotices(root), /notices are stale/);
  write("src/assets/THIRD_PARTY_NOTICES.txt", notice);
  assert.doesNotThrow(() => checkNotices(root));
  packages["node_modules/new-runtime"] = { version: "1.0.0" };
  writeJson("package-lock.json", { packages });
  writeJson("node_modules/new-runtime/package.json", {
    name: "new-runtime",
    version: "1.0.0",
    license: "MIT",
  });
  write("node_modules/new-runtime/LICENSE", license);
  assert.throws(() => checkNotices(root), /notices are stale/);
  write("src/assets/THIRD_PARTY_NOTICES.txt", generateNotices(root));
  assert.doesNotThrow(() => checkNotices(root));
});

test("accepts Windows CRLF output and license sources while detecting actual wording changes", (t) => {
  const { root, write } = fixture(t);
  const notice = generateNotices(root);
  write("src/assets/THIRD_PARTY_NOTICES.txt", notice.replace(/\n/g, "\r\n"));
  write("node_modules/runtime/LICENSE", license.replace(/\r?\n/g, "\r\n"));
  assert.doesNotThrow(() => checkNotices(root));
  write(
    "src/assets/THIRD_PARTY_NOTICES.txt",
    notice.replace("Permission", "Changed permission").replace(/\n/g, "\r\n"),
  );
  assert.throws(() => checkNotices(root), /notices are stale/);
});

test("rejects changed bundles and unreviewed package inventories", (t) => {
  const { root, write, writeJson, map, inputs } = fixture(t);
  write("node_modules/pdfmake/build/pdfmake.js", "new bundle bytes");
  assert.throws(() => generateNotices(root), /pdfmake bundle changed/);
  inputs.bundleSha256 = hash("new bundle bytes");
  map.sources.push("webpack://pdfmake/./node_modules/unreviewed/index.js");
  const sourceMap = JSON.stringify(map);
  write("node_modules/pdfmake/build/pdfmake.js.map", sourceMap);
  inputs.sourceMapSha256 = hash(sourceMap);
  writeJson("scripts/third-party-license-inputs.json", inputs);
  assert.throws(() => generateNotices(root), /package inventory differs/);
});

test("shipping inputs retain both OFLs and the complete NOTICE on Windows and macOS", () => {
  const files = ["NotoSansJP-OFL.txt", "NotoEmoji-OFL.txt", "THIRD_PARTY_NOTICES.txt"];
  const config = JSON.parse(readFileSync(resolve(repository, "src-tauri/tauri.conf.json"), "utf8"));
  assert.deepEqual(
    config.bundle.resources,
    Object.fromEntries(files.map((name) => [`../src/assets/${name}`, `licenses/${name}`])),
  );
  const workflow = readFileSync(resolve(repository, ".github/workflows/tauri-build.yml"), "utf8");
  const windows = workflow
    .split("Create Windows distribution ZIP")[1]
    .split("Create macOS distribution ZIP")[0];
  assert.deepEqual(
    [...windows.matchAll(/"src\/assets\/([^"]+)"/g)].map((match) => match[1]),
    files,
  );
  for (const name of files)
    assert.ok(readFileSync(resolve(repository, "src/assets", name)).length > 0);
  assert.match(workflow, /Contents\/Resources\/licenses\/\$license/);
  assert.match(workflow, /cmp -s "src\/assets\/\$license" "\$resource"/);
});
