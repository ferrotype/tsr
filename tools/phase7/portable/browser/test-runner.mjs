import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { startServer, validateOutputDirectory } from "./run.mjs";
import { createInstanceAsync } from "../../../s10/wasm/instance.mjs";

test("async initialization keeps the terminal-trap controller", async () => {
  let initialized = false;
  const module = {};
  const instance = await createInstanceAsync(() => Object.assign(async value => {
    assert.equal(value.module_or_path, module);
    await Promise.resolve();
    initialized = true;
  }, {
    initSync() { throw new Error("sync browser initialization is forbidden"); },
    parse() {
      assert(initialized);
      throw new WebAssembly.RuntimeError("unreachable");
    },
  }), module);
  assert.throws(() => instance.parse(new Uint8Array(), new Uint8Array(), 3), WebAssembly.RuntimeError);
  assert.equal(instance.state, "failed");
  assert.throws(() => instance.parse(new Uint8Array(), new Uint8Array(), 3), /instance is failed/);
  instance.dispose();
  await assert.rejects(createInstanceAsync(() => async () => { throw new Error("initialization failed"); }, module), /initialization failed/);
});

test("capture output rejects target directories, symlink aliases and existing output", () => {
  const temporary = mkdtempSync(resolve(tmpdir(), "r3-browser-output-"));
  try {
    const target = resolve(temporary, "target");
    mkdirSync(target);
    symlinkSync(target, resolve(temporary, "alias"));
    assert.throws(() => validateOutputDirectory(resolve(target, "capture")), /outside target/);
    assert.throws(() => validateOutputDirectory(resolve(temporary, "TARGET/capture")), /outside target/);
    assert.throws(() => validateOutputDirectory(resolve(temporary, "alias/capture")), /outside target/);
    assert.throws(() => validateOutputDirectory(temporary), /must be new/);
    assert.equal(validateOutputDirectory(resolve(temporary, "new/capture")), resolve(temporary, "new/capture"));
    const runner = fileURLToPath(new URL("./run.mjs", import.meta.url));
    const child = spawnSync(process.execPath, [runner, "--artifacts", temporary, "--output", resolve(target, "capture")], { encoding: "utf8" });
    assert.equal(child.error, undefined);
    assert.notEqual(child.status, 0);
    assert.match(child.stderr, /outside target/);
    assert.equal(existsSync(resolve(target, "capture")), false);
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});


test("capture archives the exact served bytes after source artifacts change and disappear", async () => {
  const temporary = mkdtempSync(resolve(tmpdir(), "r3-browser-assets-"));
  const target = resolve(temporary, "target");
  mkdirSync(target);
  const hash = bytes => createHash("sha256").update(bytes).digest("hex");
  const wasm = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]);
  const bindings = Buffer.from("export function createBindings() {}\n");
  writeFileSync(resolve(target, "checker_bg.wasm"), wasm);
  writeFileSync(resolve(target, "bindings.mjs"), bindings);
  writeFileSync(resolve(target, "build.json"), JSON.stringify({ mode: "checker", artifacts: {
    "target/checker_bg.wasm": { sha256: hash(wasm) },
    "target/bindings.mjs": { sha256: hash(bindings) },
  } }));
  let server;
  try {
    server = await startServer(target);
    const expected = structuredClone(server.inputs);
    // Archive creation must use the HTTP snapshot even if target changes or
    // vanishes before the archive is written. No original path may be read.
    writeFileSync(resolve(target, "checker_bg.wasm"), Buffer.from("changed"));
    rmSync(target, { recursive: true });
    const directory = resolve(temporary, "artifacts");
    const archive = server.preserveAssets(directory);
    const data = readFileSync(archive.path);
    assert.equal(hash(data), archive.sha256);
    const manifest = JSON.parse(data);
    assert.equal(manifest.version, 1);
    assert.equal(Object.keys(manifest.assets).length, 8);
    for (const [url, input] of Object.entries(expected)) {
      const asset = manifest.assets[url];
      assert.equal(asset.sha256, input.sha256);
      assert.equal(asset.bytes, input.bytes);
      assert.equal(asset.original_path, input.path);
      const saved = readFileSync(resolve(directory, asset.file));
      assert.equal(hash(saved), input.sha256);
      assert.equal(saved.length, input.bytes);
      const served = await fetch(new URL(url, server.url));
      assert.equal(served.status, 200);
      assert.equal(hash(Buffer.from(await served.arrayBuffer())), input.sha256);
    }
    assert.deepEqual(readFileSync(resolve(directory, "checker_bg.wasm")), wasm);
    assert.deepEqual(readFileSync(resolve(directory, "bindings.mjs")), bindings);
    assert.equal(existsSync(target), false);
    assert.throws(() => server.preserveAssets(directory), /must be new/);
  } finally {
    if (server) await server.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
