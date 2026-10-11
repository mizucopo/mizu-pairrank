import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../", import.meta.url));
const noticePath = "src/assets/THIRD_PARTY_NOTICES.txt";
const knownLicenses = new Set([
  "MIT",
  "Apache-2.0",
  "Apache-2.0 OR MIT",
  "ISC",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "0BSD",
  "BlueOak-1.0.0",
  "(MIT AND Zlib)",
  "Zlib",
]);
const licenseFile = /^(licen[cs]e|copying|notice|copyright)/i;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const json = (root, path) => JSON.parse(readFileSync(resolve(root, path), "utf8"));
function compare(a, b) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

function checkedRecords(label, records) {
  if (!records?.length || records.some(({ source, text }) => !source || !text?.trim())) {
    throw new Error(`Missing license text or provenance: ${label}`);
  }
  return records;
}

export function generateNotices(root = repository) {
  const lock = json(root, "package-lock.json");
  const inputs = json(root, "scripts/third-party-license-inputs.json");
  const sections = [];
  const production = Object.entries(lock.packages).filter(([path, entry]) => path && !entry.dev);
  for (const [path, entry] of production.sort(([a], [b]) => compare(a, b))) {
    const installed = json(root, `${path}/package.json`);
    const label = `${installed.name}@${entry.version}`;
    if (installed.version !== entry.version)
      throw new Error(`Install differs from lockfile: ${label}`);
    let records = readdirSync(resolve(root, path), { withFileTypes: true })
      .filter((file) => file.isFile() && licenseFile.test(file.name))
      .sort((a, b) => compare(a.name, b.name))
      .map((file) => ({
        source: `npm:${label}/${file.name}`,
        text: readFileSync(resolve(root, path, file.name), "utf8"),
      }));
    const fallback = inputs.missingLicensePackages[installed.name];
    if (
      !records.length &&
      fallback?.version === entry.version &&
      fallback.integrity === entry.integrity
    ) {
      records = fallback.records;
    }
    const license =
      installed.license ??
      (records.some(({ text }) => /^\s*(?:The )?MIT License/i.test(text)) ? "MIT" : "UNKNOWN");
    if (!knownLicenses.has(license)) throw new Error(`Unknown license: ${label} (${license})`);
    sections.push({
      label: `${label}\nInstalled path: ${path}\nDeclared license: ${license}`,
      records: checkedRecords(label, records),
    });
  }

  const pdfmake = json(root, "node_modules/pdfmake/package.json");
  const bundle = readFileSync(resolve(root, "node_modules/pdfmake/build/pdfmake.js"));
  const sourceMap = readFileSync(resolve(root, "node_modules/pdfmake/build/pdfmake.js.map"));
  if (
    pdfmake.version !== inputs.pdfmakeVersion ||
    sha256(bundle) !== inputs.bundleSha256 ||
    sha256(sourceMap) !== inputs.sourceMapSha256
  ) {
    throw new Error("pdfmake bundle changed: review and update the offline licensing inputs");
  }
  const map = JSON.parse(sourceMap.toString("utf8"));
  const names = new Set(["webpack", "base64-arraybuffer"]);
  for (const source of map.sources) {
    for (const suffix of source.split("/node_modules/").slice(1)) {
      const parts = suffix.split("/");
      names.add(parts[0].startsWith("@") ? parts.slice(0, 2).join("/") : parts[0]);
    }
  }
  if (
    JSON.stringify([...names].sort()) !== JSON.stringify(Object.keys(inputs.bundlePackages).sort())
  ) {
    throw new Error("pdfmake bundle package inventory differs from the reviewed licensing inputs");
  }
  for (const name of [...names].sort()) {
    const component = inputs.bundlePackages[name];
    if (!knownLicenses.has(component.license)) throw new Error(`Unknown bundled license: ${name}`);
    sections.push({
      label: `${name} (included in pdfmake@${pdfmake.version})\nComponent version: not asserted by the published bundle\nLicense text source version: ${component.licenseSourceVersion}\nDeclared license: ${component.license}`,
      records: checkedRecords(name, component.records),
    });
  }
  const retained = new Set();
  for (const source of map.sourcesContent) {
    for (const [comment] of (source ?? "").matchAll(/\/\*[\s\S]*?\*\/|(?:^|\n)\s*\/\/[^\n]*/g)) {
      if (/copyright|©|licensed under|@license/i.test(comment)) retained.add(comment.trim());
    }
  }
  sections.push({
    label: "Original copyright/license comments retained from the pdfmake source map",
    records: [
      {
        source: `pdfmake@${pdfmake.version}/build/pdfmake.js.map`,
        text: [...retained].sort().join("\n\n"),
      },
    ],
  });
  sections.push({
    label: "SVG-to-PDFKit (vendored by pdfmake)",
    records: [
      {
        source: `pdfmake@${pdfmake.version}/src/3rd-party/svg-to-pdfkit/LICENSE`,
        text: readFileSync(
          resolve(root, "node_modules/pdfmake/src/3rd-party/svg-to-pdfkit/LICENSE"),
          "utf8",
        ),
      },
    ],
  });
  sections.push({
    label: "Additional upstream data and source terms",
    records: checkedRecords("additional terms", inputs.additionalTerms),
  });
  const texts = new Map();
  const inventory = sections.map(({ label, records }) => {
    const references = records.map(({ source, text }) => {
      const normalized = text.replace(/\r\n/g, "\n").trim();
      if (!texts.has(normalized)) texts.set(normalized, texts.size + 1);
      return `Source: ${source}\nLicense/notice text: ${texts.get(normalized)}`;
    });
    return `${"=".repeat(78)}\n${label}\n\n${references.join("\n\n")}\n`;
  });
  return (
    "THIRD-PARTY NOTICES\n\nGenerated by npm run notices:generate. Do not edit this file.\n" +
    "Includes all lockfile production JavaScript dependencies and the components embedded in pdfmake's browser bundle.\n" +
    "Development-only application dependencies are excluded. Font licenses are distributed separately.\n" +
    `pdfmake bundle SHA-256: ${inputs.bundleSha256}\npdfmake source-map SHA-256: ${inputs.sourceMapSha256}\n\n` +
    inventory.join("\n") +
    "\nLICENSE AND NOTICE TEXTS\n\n" +
    [...texts]
      .map(([text, id]) => `${"=".repeat(78)}\nLicense/notice text: ${id}\n\n${text}\n`)
      .join("\n")
  );
}

export function checkNotices(root = repository) {
  const expected = generateNotices(root);
  const actual = readFileSync(resolve(root, noticePath), "utf8").replace(/\r\n/g, "\n");
  if (actual !== expected)
    throw new Error(
      "Third-party notices are stale: run npm run notices:generate and review the result",
    );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv[2] === "--write") {
      writeFileSync(resolve(repository, noticePath), generateNotices());
    } else if (process.argv[2] === "--check") {
      checkNotices();
    } else {
      throw new Error("Usage: node scripts/third-party-notices.mjs --write|--check");
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
