import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { resolve, dirname, relative, sep, basename } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../../..");
const pin = JSON.parse(readFileSync(resolve(here, "toolchains.json")));
const hash = value => createHash("sha256").update(value).digest("hex");

export function validateOutputDirectory(path) {
  path = resolve(path);
  let existing = path;
  while (!existsSync(existing)) existing = dirname(existing);
  const canonical = resolve(realpathSync(existing), relative(existing, path));
  if (canonical.split(sep).some(part => part.toLowerCase() === "target")) throw new Error("capture output must be outside target");
  if (existsSync(path)) throw new Error("capture output directory must be new");
  return path;
}

function argumentsFrom(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    if (!["--artifacts", "--output", "--playwright-module"].includes(name) || !argv[index + 1] || Object.hasOwn(options, name)) {
      throw new Error("usage: node run.mjs --artifacts DIR --output DIR [--playwright-module PACKAGE_DIR]");
    }
    options[name] = resolve(argv[index + 1]);
  }
  if (!options["--artifacts"] || !options["--output"]) throw new Error("--artifacts and --output are required");
  options["--output"] = validateOutputDirectory(options["--output"]);
  return options;
}

export async function startServer(directory) {
  const build = JSON.parse(readFileSync(resolve(directory, "build.json")));
  if (build.mode !== "checker") throw new Error("browser smoke requires the production checker build");
  const routes = new Map();
  const add = (url, path, type) => routes.set(url, { data: readFileSync(path), type, path });
  add("/", resolve(here, "index.html"), "text/html; charset=utf-8");
  add("/smoke.mjs", resolve(here, "smoke.mjs"), "text/javascript");
  add("/runtime/instance.mjs", resolve(root, "tools/s10/wasm/instance.mjs"), "text/javascript");
  add("/runtime/imports.mjs", resolve(root, "tools/s10/wasm/imports.mjs"), "text/javascript");
  add("/fixtures/parser.json", resolve(root, "tools/s10/parser/fixtures.json"), "application/json");
  add("/artifact/build.json", resolve(directory, "build.json"), "application/json");
  for (const [name, type] of [["checker_bg.wasm", "application/wasm"], ["bindings.mjs", "text/javascript"]]) {
    add(`/artifact/${name}`, resolve(directory, name), type);
    const entries = Object.entries(build.artifacts ?? {}).filter(([path]) => path.split("/").at(-1) === name);
    if (entries.length !== 1 || entries[0][1].sha256 !== hash(routes.get(`/artifact/${name}`).data)) {
      throw new Error(`artifact bytes differ from build record: ${name}`);
    }
  }
  const server = createServer((request, response) => {
    const route = routes.get(request.url);
    if (request.method !== "GET" || !route) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { "Content-Type": route.type, "Cache-Control": "no-store" });
    response.end(route.data);
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  return {
    url: `http://127.0.0.1:${server.address().port}/`,
    inputs: Object.fromEntries(Array.from(routes, ([url, value]) => [url, { path: value.path, sha256: hash(value.data), bytes: value.data.length }])),
    preserveAssets(directory) {
      directory = validateOutputDirectory(directory);
      const names = Array.from(routes.values(), value => basename(value.path));
      if (new Set(names).size !== names.length) throw new Error("served assets have colliding archive names");
      mkdirSync(directory);
      const manifest = { version: 1, assets: {} };
      for (const [url, value] of routes) {
        const file = basename(value.path);
        // Persist the very buffers served over HTTP, not a later reread of
        // sources or target. These copies remain valid after target deletion.
        writeFileSync(resolve(directory, file), value.data, { flag: "wx" });
        manifest.assets[url] = { file, sha256: hash(value.data), bytes: value.data.length,
          original_path: value.path, content_type: value.type };
      }
      const data = JSON.stringify(manifest, null, 2) + "\n";
      const path = resolve(directory, "manifest.json");
      writeFileSync(path, data, { flag: "wx" });
      return { path, sha256: hash(data) };
    },
    close: () => new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())),
  };
}

async function withTimeout(operation, milliseconds) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("browser smoke deadline exceeded")), milliseconds); })]);
  } finally { clearTimeout(timer); }
}

async function main() {
  const options = argumentsFrom(process.argv.slice(2));
  mkdirSync(dirname(options["--output"]), { recursive: true });
  mkdirSync(options["--output"]);
  const require = createRequire(import.meta.url);
  const packagePath = options["--playwright-module"] ?? dirname(require.resolve("playwright/package.json"));
  const packageInfo = JSON.parse(readFileSync(resolve(packagePath, "package.json")));
  if (packageInfo.name !== "playwright" || packageInfo.version !== pin.playwright) throw new Error(`requires Playwright ${pin.playwright}`);
  const playwright = await import(pathToFileURL(resolve(packagePath, "index.mjs")));
  const server = await startServer(options["--artifacts"]);
  let archive;
  try { archive = server.preserveAssets(resolve(options["--output"], "artifacts")); }
  catch (error) { await server.close(); throw error; }
  const report = { version: 1, playwright: packageInfo.version, node: process.version, platform: process.platform, architecture: process.arch,
    required_engines: pin.engines, required_cases: pin.cases, engine_stack_flags: [], inputs: server.inputs,
    archived_assets: { manifest: "artifacts/manifest.json", sha256: archive.sha256 }, engines: [] };
  try {
    for (const name of pin.engines) {
      const row = { engine: name, executable: playwright[name].executablePath(), console: [], page_errors: [] };
      let browser;
      try {
        if (!existsSync(row.executable)) throw new Error(`matching browser executable is unavailable: ${row.executable}`);
        browser = await playwright[name].launch({ headless: true, executablePath: row.executable });
        row.browser_version = browser.version();
        const page = await browser.newPage();
        page.on("console", message => row.console.push({ type: message.type(), text: message.text() }));
        page.on("pageerror", error => row.page_errors.push(String(error.stack ?? error)));
        await page.goto(server.url, { waitUntil: "load", timeout: 60000 });
        await page.waitForFunction(() => typeof globalThis.runSmoke === "function", undefined, { timeout: 60000 });
        row.result = await withTimeout(page.evaluate(() => globalThis.runSmoke()), 60000);
        if (row.result.wasm_sha256 !== report.inputs["/artifact/checker_bg.wasm"].sha256) throw new Error("browser loaded different wasm bytes");
        if (row.page_errors.length > 0) throw new Error("unexpected browser page error");
        if (!Array.isArray(row.result.cases) || JSON.stringify(row.result.cases.map(value => value.id)) !== JSON.stringify(pin.cases)) throw new Error("browser result has missing, duplicate or reordered smoke cases");
        if (row.result.state === "passed" && !row.result.cases.every(value => value.state === "passed")) throw new Error("browser result state disagrees with case failures");
        row.state = row.result.state;
      } catch (error) { row.state = "failed"; row.reason = String(error.stack ?? error); }
      finally {
        if (browser) {
          try { await withTimeout(browser.close(), 10000); }
          catch (error) { row.state = "failed"; row.cleanup_error = String(error.stack ?? error); }
        }
      }
      report.engines.push(row);
      writeFileSync(resolve(options["--output"], "browser-smoke.json"), JSON.stringify(report, null, 2) + "\n");
      process.stderr.write(`${name}: ${row.state}\n`);
    }
  } finally { await server.close(); }
  report.state = report.engines.length === pin.engines.length && report.engines.every(row => row.state === "passed") ? "passed" : "failed";
  writeFileSync(resolve(options["--output"], "browser-smoke.json"), JSON.stringify(report, null, 2) + "\n");
  process.stdout.write(JSON.stringify({ state: report.state, record: resolve(options["--output"], "browser-smoke.json"), engines: report.engines.map(row => ({ engine: row.engine, version: row.browser_version, state: row.state })) }) + "\n");
  if (report.state !== "passed") process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { await main(); }
  catch (error) { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; }
}
