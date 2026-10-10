#!/usr/bin/env node
import { readFileSync, writeFileSync, statSync } from "node:fs";
import { basename } from "node:path";

const statePath = process.env.RELEASE_TEST_STATE;
const state = JSON.parse(readFileSync(statePath, "utf8"));
const command = basename(process.argv[1]);
const args = process.argv.slice(2);
state.calls.push([command, ...args]);
const finish = (output = "", code = 0) => {
  writeFileSync(statePath, JSON.stringify(state));
  process.stdout.write(typeof output === "string" ? output : JSON.stringify(output));
  process.exit(code);
};
const reject = (message) => finish(message, 1);
const release = () => state.releases.find((item) => item.tag_name === process.env.TAG);

if (command === "git") {
  if (args[0] === "fetch") finish();
  if (args[0] === "rev-parse" || args[0] === "rev-list") finish("release-sha\n");
  if (args[0] === "show-ref") finish("", state.missingTag ? 1 : 0);
}
if (command === "curl") {
  const item = release();
  const published = item && !item.draft;
  writeFileSync(args[args.indexOf("--output") + 1], JSON.stringify(published ? item : {}));
  finish(published ? "200" : "404");
}
const fields = Object.fromEntries(
  args
    .filter((_, index) => ["-f", "-F"].includes(args[index - 1]))
    .map((field) => {
      const equal = field.indexOf("=");
      return [field.slice(0, equal), field.slice(equal + 1)];
    }),
);
const create = (tag, prerelease) => {
  if (release()) reject("Duplicate draft creation");
  const item = {
    id: 41,
    tag_name: tag,
    draft: true,
    prerelease,
    upload_url:
      "https://uploads.github.invalid/repos/owner/project/releases/41/assets{?name,label}",
    assets: [],
  };
  state.releases.push(item);
  state.created = true;
  return item;
};
if (command === "gh" && args[0] === "api") {
  const endpoint = args.find(
    (argument) => argument.startsWith("/repos/") || argument.startsWith("https://"),
  );
  if (endpoint.startsWith("https://uploads.github.invalid/")) {
    const url = new URL(endpoint);
    const item = state.releases.find(
      (entry) => entry.id === Number(url.pathname.split("/").at(-2)),
    );
    if (!item?.draft || !args.includes("POST") || !args.includes("Content-Type: application/zip"))
      reject("Unsafe upload");
    const name = url.searchParams.get("name");
    if (item.assets.some((asset) => asset.name === name)) reject("Duplicate asset");
    if (state.failUpload === name) {
      delete state.failUpload;
      reject("Interrupted upload");
    }
    const asset = {
      id: 100 + item.assets.length,
      name,
      state: "uploaded",
      size: statSync(args[args.indexOf("--input") + 1]).size,
    };
    item.assets.push(asset);
    finish(asset);
  }
  if (endpoint.includes("/releases/assets/") && args.includes("DELETE")) {
    const id = Number(endpoint.split("/").at(-1));
    const item = state.releases.find((entry) => entry.assets.some((asset) => asset.id === id));
    if (!item?.draft) reject("Only draft assets may change");
    item.assets = item.assets.filter((asset) => asset.id !== id);
    finish();
  }
  if (endpoint.endsWith("/releases?per_page=100")) {
    if (state.listFailure) reject("Draft listing failed");
    if (!args.includes("--paginate") || !args.includes("--slurp")) reject("Missing pagination");
    const visible = state.hideCreated && state.created ? [] : state.releases;
    finish([[], visible]);
  }
  if (endpoint.endsWith("/releases") && args.includes("POST")) {
    if (fields.draft !== "true" || fields.make_latest !== "false") reject("Unsafe creation");
    const item = create(fields.tag_name, fields.prerelease === "true");
    finish({ ...item, ...state.createResponse });
  }
  const item = state.releases.find((entry) => entry.id === Number(endpoint.split("/").at(-1)));
  if (!item) reject("Unknown release ID");
  if (args.includes("PATCH")) {
    if (fields.draft !== "false" || fields.make_latest !== "false") reject("Unsafe publication");
    if (item.assets.length !== 3 || item.assets.some((asset) => asset.state !== "uploaded")) {
      reject("Incomplete assets");
    }
    item.draft = false;
    item.prerelease = fields.prerelease === "true";
  }
  finish(item);
}
reject(`Unexpected command: ${command} ${args.join(" ")}`);
