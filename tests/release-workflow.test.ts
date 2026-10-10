import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, delimiter } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

type Release = {
  id: number;
  tag_name: string;
  draft: boolean;
  prerelease: boolean;
  upload_url: string;
  assets: { id: number; name: string; state: string; size: number }[];
};
type State = {
  releases: Release[];
  calls: string[][];
  hideCreated?: boolean;
  listFailure?: boolean;
  missingTag?: boolean;
  createResponse?: Partial<Release>;
  failUpload?: string;
};

const tag = "1.0.1";
const assetNames = ["windows-x64", "windows-arm64", "macos-arm64"].map(
  (platform) => `mizu-pairrank-${tag}-${platform}.zip`,
);
const workflow = readFileSync(".github/workflows/tauri-build.yml", "utf8");
const publishStep = workflow.split("      - name: Create GitHub Release\n")[1];
const runBlock = publishStep?.split("        run: |\n")[1]?.split("\n  promote-latest:")[0];
if (!runBlock) throw new Error("Missing Tauri publication step");
const script = runBlock
  .split("\n")
  .map((line) => line.replace(/^ {10}/, ""))
  .join("\n");
const fixture = readFileSync("tests/fixtures/release-api.mjs", "utf8");

describe("Tauri draft publication", () => {
  let directory: string;
  let statePath: string;
  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), "pairrank-release-"));
    statePath = join(directory, "state.json");
    mkdirSync(join(directory, "bin"));
    for (const command of ["git", "gh", "curl"]) {
      writeFileSync(join(directory, "bin", command), fixture, { mode: 0o755 });
    }
    mkdirSync(join(directory, "release-assets"));
    for (const name of assetNames) writeFileSync(join(directory, "release-assets", name), "ZIP");
    save({ releases: [], calls: [] });
  });
  afterEach(() => rmSync(directory, { recursive: true, force: true }));
  const save = (state: State) => writeFileSync(statePath, JSON.stringify(state));
  const load = () => JSON.parse(readFileSync(statePath, "utf8")) as State;
  const existing = (count: number, draft = true): Release => ({
    id: 41,
    tag_name: tag,
    draft,
    prerelease: false,
    upload_url:
      "https://uploads.github.invalid/repos/owner/project/releases/41/assets{?name,label}",
    assets: assetNames
      .slice(0, count)
      .map((name, index) => ({ id: 100 + index, name, state: "uploaded", size: 100 })),
  });
  const execute = (prerelease = false) =>
    spawnSync("bash", ["-e"], {
      input: script,
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH ?? ""}`,
        RELEASE_TEST_STATE: statePath,
        GITHUB_REPOSITORY: "owner/project",
        GITHUB_API_URL: "https://api.github.invalid",
        GITHUB_OUTPUT: join(directory, "output"),
        GH_TOKEN: "test-token",
        TAG: tag,
        ZIP_PREFIX: `mizu-pairrank-${tag}`,
        IS_PRERELEASE: String(prerelease),
        INSPECT_DRAFT_RELEASES: "true",
        RUNNER_TEMP: directory,
      },
    });
  const mutations = () =>
    load().calls.filter((call) =>
      ["POST", "PATCH", "DELETE"].some((method) => call.includes(method)),
    );

  it("publishes a created draft even when it is absent from the release listing", () => {
    save({ releases: [], calls: [], hideCreated: true });
    const result = execute();
    expect(result.stdout + result.stderr).not.toContain("could not be found");
    expect(result.status).toBe(0);
    expect(load().releases).toEqual([
      {
        ...existing(0),
        draft: false,
        assets: assetNames.map((name, index) => ({
          id: 100 + index,
          name,
          state: "uploaded",
          size: 3,
        })),
      },
    ]);
  });

  it.each([false, true])(
    "creates once and preserves completed assets on rerun (prerelease=%s)",
    (prerelease) => {
      expect(execute(prerelease).status).toBe(0);
      const before = load();
      expect(before.releases[0]?.prerelease).toBe(prerelease);
      expect(execute(prerelease).status).toBe(0);
      expect(load().releases).toEqual(before.releases);
      expect(mutations()).toHaveLength(5);
    },
  );

  it.each([
    { id: 0 },
    { tag_name: "another-tag" },
    { draft: false },
    { prerelease: true },
    { upload_url: "" },
  ])("rejects an inconsistent creation response %j before uploads", (createResponse) => {
    save({ releases: [], calls: [], createResponse });
    expect(execute().status).not.toBe(0);
    expect(load().releases[0]?.draft).toBe(true);
    expect(mutations()).toHaveLength(1);
  });

  it("resumes missing assets in an existing draft without replacing completed ones", () => {
    save({ releases: [existing(1)], calls: [] });
    expect(execute().status).toBe(0);
    expect(load().releases[0]?.assets[0]?.size).toBe(100);
    expect(mutations()).toHaveLength(3);
  });

  it("reuses the same draft after an interrupted upload", () => {
    save({ releases: [], calls: [], failUpload: assetNames[1]! });
    expect(execute().status).not.toBe(0);
    expect(load().releases[0]?.draft).toBe(true);
    expect(execute().status).toBe(0);
    expect(load().releases[0]?.draft).toBe(false);
    expect(
      load().calls.filter(
        (call) => call.includes("POST") && call.includes("/repos/owner/project/releases"),
      ),
    ).toHaveLength(1);
  });

  it("replaces an incomplete draft asset before uploading", () => {
    const draft = existing(1);
    draft.assets[0]!.state = "starter";
    draft.assets[0]!.size = 0;
    save({ releases: [draft], calls: [] });
    expect(execute().status).toBe(0);
    expect(
      load().releases[0]?.assets.every((asset) => asset.state === "uploaded" && asset.size > 0),
    ).toBe(true);
    expect(load().calls.filter((call) => call.includes("DELETE"))).toHaveLength(1);
  });

  it.each([
    { failure: "duplicate", releases: [existing(1), { ...existing(0), id: 42 }] },
    { failure: "listing", releases: [], listFailure: true },
    { failure: "missing-tag", releases: [], missingTag: true },
    { failure: "published-incomplete", releases: [existing(1, false)] },
  ])("blocks mutations when inspection fails: $failure", ({ failure: _failure, ...state }) => {
    save({
      ...state,
      calls: [],
    });
    expect(execute().status).not.toBe(0);
    expect(mutations()).toHaveLength(0);
  });
});
