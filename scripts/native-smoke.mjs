// Real Windows Tauri/WebView2 smoke test. No browser or backend mock is used.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const webdriverUrl = "http://127.0.0.1:4444";
const elementKey = "element-6066-11e4-a52e-4f735466cecf";
const toolRoot = path.join(projectRoot, ".scratch", ".tmp", "native-smoke-tools");
const runId = randomUUID().slice(0, 8);
const executableName = `personal-document-manager-smoke-${runId}.exe`;
const appIdentifier = `com.local.personal-document-manager.smoke${runId}`;
const runDir = path.join(os.tmpdir(), `pdm-native-smoke-${runId}`);
const application = path.join(runDir, executableName);
const libraryPath = path.join(runDir, "library");
const sourcePath = path.join(runDir, "source", `smoke-${runId}.txt`);
const searchToken = `native_smoke_${runId}`;
const sampleContent = `Native Tauri smoke sample. Search token: ${searchToken}.`;
const title = path.basename(sourcePath, ".txt");
const collectionName = `Smoke collection ${runId}`;
const report = {
  status: "not-run",
  startedAt: new Date().toISOString(),
  platform: process.platform,
  buildType: "debug",
  appIdentifier,
  nativeFilePickerCovered: false,
  libraryWizardCovered: false,
  importButtonCovered: false,
  initializationMode: "Real Tauri IPC creates the test library and imports the sample; native UI verifies later steps.",
  application,
  libraryPath,
  sourcePath,
  runDir,
  steps: []
};

let sessionId = null;
let driverProcess = null;
let driverClosePromise = null;
let activeStep = "preflight";
let cleaningUp = false;
let unexpectedDriverExit = null;

function log(message) {
  process.stdout.write(`[native-smoke] ${message}\n`);
}

function saveReport() {
  fs.mkdirSync(runDir, { recursive: true });
  fs.writeFileSync(
    path.join(runDir, "report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
    "utf8"
  );
}

function commandVersion(command, args = ["--version"]) {
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    encoding: "utf8",
    windowsHide: true,
    timeout: 10_000
  });
  return result.status === 0 ? `${result.stdout}\n${result.stderr}`.trim() : null;
}

function findTool(envName, candidates, probeArgs = ["--version"]) {
  const names = [process.env[envName], ...candidates].filter(Boolean);
  for (const name of names) {
    if (commandVersion(name, probeArgs)) return name;
  }
  return null;
}

function preflight() {
  assert.equal(process.platform, "win32", "Native smoke test requires Windows.");
  const tauriDriver = findTool("TAURI_DRIVER_PATH", [
    path.join(toolRoot, "bin", "tauri-driver.exe"),
    path.join(os.homedir(), ".cargo", "bin", "tauri-driver.exe"),
    "tauri-driver.exe"
  ], ["--help"]);
  assert.ok(
    tauriDriver,
    "tauri-driver 2.0.6 is missing. Install it with: cargo install tauri-driver --version 2.0.6 --locked"
  );
  const manifest = path.join(path.dirname(path.dirname(tauriDriver)), ".crates2.json");
  assert.ok(
    fs.existsSync(manifest),
    `Cannot verify tauri-driver 2.0.6: missing Cargo install manifest at ${manifest}. Set TAURI_DRIVER_PATH to a Cargo-installed binary.`
  );
  const installed = JSON.parse(fs.readFileSync(manifest, "utf8")).installs;
  assert.ok(
    Object.keys(installed).some((name) => name.startsWith("tauri-driver 2.0.6 ")),
    "Expected Cargo-installed tauri-driver 2.0.6. Install with: cargo install tauri-driver --version 2.0.6 --locked"
  );
  const tauriVersion = "2.0.6";

  const edgeDriver = findTool("MSEDGEDRIVER_PATH", [
    path.join(toolRoot, "msedgedriver.exe"),
    "msedgedriver.exe"
  ]);
  assert.ok(
    edgeDriver,
    "msedgedriver.exe is missing. Download the EdgeDriver matching this computer's Edge version and set MSEDGEDRIVER_PATH; see docs/acceptance/native-smoke.md."
  );
  const driverVersion = commandVersion(edgeDriver)?.match(/\b\d+\.\d+\.\d+\.\d+\b/)?.[0];
  assert.ok(driverVersion, `Cannot read EdgeDriver version from ${edgeDriver}.`);
  const edgeBinary = [
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe"
  ].find((candidate) => fs.existsSync(candidate));
  assert.ok(edgeBinary, "Microsoft Edge is missing; Windows Tauri WebDriver needs Edge.");
  const edgeVersion = commandVersion("powershell.exe", [
    "-NoProfile",
    "-Command",
    `(Get-Item -LiteralPath '${edgeBinary.replaceAll("'", "''")}').VersionInfo.ProductVersion`
  ]);
  assert.equal(
    driverVersion,
    edgeVersion,
    `EdgeDriver ${driverVersion} must match installed Edge ${edgeVersion}.`
  );
  Object.assign(report, { tauriVersion, edgeVersion, driverVersion, tauriDriver, edgeDriver });
  return { tauriDriver, edgeDriver };
}

function buildApplication() {
  log("Building the isolated Tauri debug application...");
  const built = spawnSync(
    process.execPath,
    [
      path.join(projectRoot, "node_modules", "@tauri-apps", "cli", "tauri.js"),
      "build", "--debug", "--no-bundle",
      "--config", "src-tauri/tauri.smoke.conf.json",
      "--config", JSON.stringify({ identifier: appIdentifier })
    ],
    { cwd: projectRoot, env: process.env, stdio: "inherit", timeout: 20 * 60_000 }
  );
  assert.equal(built.status, 0, `Tauri debug build failed: ${built.error?.message ?? "exit code " + built.status}`);
  const builtApplication = path.join(
    projectRoot, "src-tauri", "target", "debug", "personal-document-manager.exe"
  );
  assert.ok(fs.existsSync(builtApplication), `Missing application: ${builtApplication}`);
  fs.copyFileSync(builtApplication, application);
  report.builtApplication = builtApplication;
}

async function webdriver(method, route, body, timeout = 20_000) {
  const response = await fetch(`${webdriverUrl}${route}`, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(timeout)
  });
  const result = await response.json();
  if (!response.ok || result.value?.error) {
    throw new Error(`${method} ${route}: ${result.value?.message ?? JSON.stringify(result)}`);
  }
  return result.value;
}

async function waitFor(condition, description, timeout = 25_000) {
  const until = Date.now() + timeout;
  let lastError;
  while (Date.now() < until) {
    try {
      const value = await condition();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${description}. ${lastError?.message ?? ""}`);
}

async function find(selector, timeout = 20_000) {
  return waitFor(async () => {
    const value = await webdriver("POST", `/session/${sessionId}/element`, {
      using: "css selector",
      value: selector
    });
    return value[elementKey];
  }, selector, timeout);
}

async function click(selector) {
  const element = await find(selector);
  await webdriver("POST", `/session/${sessionId}/element/${element}/click`, {});
}

async function type(selector, value) {
  const element = await find(selector);
  await webdriver("POST", `/session/${sessionId}/element/${element}/value`, {
    text: value,
    value: Array.from(value)
  });
}

async function evaluate(script, args = []) {
  return webdriver("POST", `/session/${sessionId}/execute/sync`, { script, args });
}

async function pageIncludes(selector, text) {
  return evaluate(
    "return [...document.querySelectorAll(arguments[0])].some(node => node.textContent.includes(arguments[1]))",
    [selector, text]
  );
}

async function step(name, action) {
  activeStep = name;
  log(name);
  const entry = { name, startedAt: new Date().toISOString(), status: "running" };
  report.steps.push(entry);
  saveReport();
  try {
    await action();
    entry.status = "passed";
  } catch (error) {
    entry.status = "failed";
    entry.error = error.stack ?? String(error);
    throw error;
  } finally {
    entry.finishedAt = new Date().toISOString();
    saveReport();
  }
}

async function invokeReal(command, payload) {
  const result = await webdriver("POST", `/session/${sessionId}/execute/async`, {
    script: `
      const done = arguments[arguments.length - 1];
      window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
        .then(value => done({ ok: true, value }))
        .catch(error => done({ ok: false, error }));
    `,
    args: [command, payload]
  }, 45_000);
  assert.equal(result.ok, true, `${command} failed: ${JSON.stringify(result.error)}`);
  return result.value;
}

async function reloadWindow() {
  await webdriver("POST", `/session/${sessionId}/refresh`, {});
}

async function startDriver({ tauriDriver, edgeDriver }) {
  try {
    await webdriver("GET", "/status", undefined, 1_000);
    throw new Error("Port 4444 already has a WebDriver server. Stop it before running this smoke test.");
  } catch (error) {
    if (!String(error).includes("fetch failed") && !String(error).includes("terminated")) {
      throw error;
    }
  }
  const env = {
    ...process.env,
    APPDATA: path.join(runDir, "roaming"),
    LOCALAPPDATA: path.join(runDir, "local"),
    WEBVIEW2_USER_DATA_FOLDER: path.join(runDir, "webview2")
  };
  for (const directory of [env.APPDATA, env.LOCALAPPDATA, env.WEBVIEW2_USER_DATA_FOLDER]) {
    fs.mkdirSync(directory, { recursive: true });
  }
  const driverLog = fs.createWriteStream(path.join(runDir, "tauri-driver.log"));
  driverProcess = spawn(tauriDriver, ["--native-driver", edgeDriver], {
    cwd: projectRoot,
    env,
    windowsHide: true
  });
  driverProcess.stdout.pipe(driverLog, { end: false });
  driverProcess.stderr.pipe(driverLog, { end: false });
  driverClosePromise = new Promise((resolve) => {
    driverProcess.once("close", () => driverLog.end(resolve));
  });
  driverProcess.once("exit", (code, signal) => {
    if (!cleaningUp) {
      unexpectedDriverExit = { code, signal };
      log(`tauri-driver exited unexpectedly with code ${code}, signal ${signal}`);
    }
  });
  await waitFor(async () => {
    if (driverProcess.exitCode !== null) {
      throw new Error(`tauri-driver exited with ${driverProcess.exitCode}; see tauri-driver.log`);
    }
    try {
      return await webdriver("GET", "/status", undefined, 2_000);
    } catch {
      return false;
    }
  }, "tauri-driver readiness", 15_000);
  const value = await webdriver("POST", "/session", {
    capabilities: {
      alwaysMatch: {
        browserName: "wry",
        "tauri:options": { application }
      }
    }
  }, 60_000);
  sessionId = value.sessionId;
  assert.ok(sessionId, "tauri-driver did not return a WebDriver session ID.");
}

async function captureFailure(error) {
  report.status = "failed";
  report.failedStep = activeStep;
  report.error = error.stack ?? String(error);
  if (sessionId) {
    try {
      const screenshot = await webdriver("GET", `/session/${sessionId}/screenshot`);
      fs.writeFileSync(path.join(runDir, "failure.png"), Buffer.from(screenshot, "base64"));
    } catch (captureError) {
      report.screenshotError = String(captureError);
    }
    try {
      const html = await evaluate("return document.documentElement.outerHTML");
      fs.writeFileSync(path.join(runDir, "failure.html"), html, "utf8");
    } catch (captureError) {
      report.htmlError = String(captureError);
    }
  }
  saveReport();
}

async function cleanup() {
  cleaningUp = true;
  const errors = [];
  let driverKillError = null;
  if (sessionId) {
    try {
      await webdriver("DELETE", `/session/${sessionId}`);
    } catch (error) {
      report.sessionCloseWarning = String(error);
    }
  }
  // A completed child can have a recycled PID. Only kill its tree while the
  // ChildProcess handle still reports the original driver as running.
  if (
    driverProcess?.pid &&
    driverProcess.exitCode === null &&
    driverProcess.signalCode === null &&
    !driverProcess.killed
  ) {
    const result = spawnSync("taskkill.exe", ["/PID", String(driverProcess.pid), "/T", "/F"], {
      windowsHide: true,
      timeout: 10_000,
      encoding: "utf8"
    });
    if (result.status !== 0) {
      driverKillError = `Could not stop driver process tree: ${result.stderr || result.stdout || result.error?.message || result.status}`;
    }
  }
  if (driverClosePromise) {
    let timeoutHandle;
    const closed = await Promise.race([
      driverClosePromise.then(() => true),
      new Promise((resolve) => {
        timeoutHandle = setTimeout(() => resolve(false), 5_000);
      })
    ]);
    clearTimeout(timeoutHandle);
    if (!closed) {
      if (driverKillError) driverProcess.kill();
      errors.push(driverKillError || "tauri-driver did not close and flush its log within 5 seconds.");
    }
  } else if (driverProcess) {
    errors.push("Cannot confirm that tauri-driver closed and flushed its log.");
  }
  if (unexpectedDriverExit !== null) {
    errors.push(`tauri-driver exited unexpectedly: ${JSON.stringify(unexpectedDriverExit)}.`);
  }

  // The image name includes this run's ID; never target the normal app.
  const runningApplication = () => {
    const result = spawnSync(
      "tasklist.exe",
      ["/FI", `IMAGENAME eq ${executableName}`, "/FO", "CSV", "/NH"],
      { windowsHide: true, timeout: 10_000, encoding: "utf8" }
    );
    if (result.status !== 0) {
      throw new Error(`Cannot verify test app process: ${result.stderr || result.error?.message || result.status}`);
    }
    return result.stdout.toLowerCase().includes(`"${executableName.toLowerCase()}"`);
  };
  try {
    if (runningApplication()) {
      const result = spawnSync("taskkill.exe", ["/IM", executableName, "/F"], {
        windowsHide: true,
        timeout: 10_000,
        encoding: "utf8"
      });
      if (result.status !== 0) {
        errors.push(`Could not stop test app: ${result.stderr || result.stdout || result.error?.message || result.status}`);
      }
    }
    if (runningApplication()) {
      errors.push(`Test app ${executableName} is still running after cleanup.`);
    }
  } catch (error) {
    errors.push(error.message);
  }
  if (errors.length > 0) throw new Error(errors.join(" "));
}

async function main() {
  let currentLibrary = null;
  fs.mkdirSync(path.dirname(sourcePath), { recursive: true });
  fs.mkdirSync(libraryPath, { recursive: true });
  fs.writeFileSync(sourcePath, sampleContent, "utf8");
  saveReport();
  try {
    await step("Check Windows WebDriver prerequisites", async () => {
      const tools = preflight();
      report.tools = tools;
    });
    await step("Build isolated debug Tauri application", async () => buildApplication());
    await step("Start real Tauri window through tauri-driver", async () => {
      await startDriver(report.tools);
      await waitFor(
        () => evaluate("return Boolean(window.__TAURI_INTERNALS__?.invoke)"),
        "native Tauri IPC",
        25_000
      );
      await find("button.button.primary.wide");
    });
    await step("Create isolated library through real Tauri IPC", async () => {
      const created = await invokeReal("create_library", { path: libraryPath });
      assert.equal(path.resolve(created.path).toLowerCase(), path.resolve(libraryPath).toLowerCase());
      currentLibrary = created;
      await reloadWindow();
      await find('button[aria-label="创建根集合"]');
    });
    await step("Import TXT through real Tauri IPC", async () => {
      const batch = await invokeReal("start_import", {
        library: currentLibrary,
        paths: [sourcePath],
        targetCollectionId: null,
        source: "filePicker"
      });
      assert.equal(batch.importedCount, 1, `Import response: ${JSON.stringify(batch)}`);
      await reloadWindow();
      await waitFor(() => pageIncludes(".document-title-cell", title), "imported document");
    });
    await step("Create a collection and move the document", async () => {
      await click('button[aria-label="创建根集合"]');
      await type("#collection-name", collectionName);
      await click('[role="dialog"] button[type="submit"]');
      await waitFor(() => pageIncludes(".collection-select", collectionName), "new collection");
      await click(".primary-nav button:nth-child(1)");
      await waitFor(() => pageIncludes(".document-title-cell", title), "document in all-documents view");
      const collectionId = await evaluate(
        "return [...document.querySelectorAll('[data-collection-id]')].find(node => node.textContent.includes(arguments[0]))?.dataset.collectionId",
        [collectionName]
      );
      assert.ok(collectionId, "Cannot locate the new collection ID in the native window.");
      await click(`select[aria-label="移动 ${title} 到集合"] option[value="${collectionId}"]`);
      await waitFor(
        () => pageIncludes(`.document-row .document-collection`, collectionName),
        "document moved to collection"
      );
    });
    await step("Search indexed text and inspect preview", async () => {
      await type('input[aria-label="搜索文档"]', searchToken);
      await waitFor(() => pageIncludes(".document-title-cell", title), "search result", 35_000);
      await click(`button[aria-label="选择文档 ${title}"]`);
      await waitFor(
        () => pageIncludes('section[aria-label="文档预览"] pre', searchToken),
        "readable text preview"
      );
    });
    await step("Move document to trash and restore it", async () => {
      await click(`aside[aria-label="文档详情"] button[aria-label="将 ${title} 移入回收站"]`);
      await find('[role="dialog"]');
      await click('[role="dialog"] button.button.danger');
      await click(".primary-nav button:nth-child(2)");
      await waitFor(() => pageIncludes(".trash-row", title), "document in trash");
      await click(`button[aria-label="恢复 ${title}"]`);
      await waitFor(() => pageIncludes(".trash-empty-state", "回收站为空"), "empty trash");
      await click(".primary-nav button:nth-child(1)");
      await waitFor(() => pageIncludes(".document-title-cell", title), "restored document");
      await click(`button[aria-label="选择文档 ${title}"]`);
      await waitFor(
        () => pageIncludes('section[aria-label="文档预览"] pre', searchToken),
        "readable restored document"
      );
    });
    await step("Capture the final native window", async () => {
      const screenshot = await webdriver("GET", `/session/${sessionId}/screenshot`);
      fs.writeFileSync(path.join(runDir, "success.png"), Buffer.from(screenshot, "base64"));
      report.successScreenshot = "success.png";
    });
    report.status = "passed";
  } catch (error) {
    await captureFailure(error);
    log(`FAIL at ${activeStep}: ${error.message}`);
    process.exitCode = 1;
  } finally {
    try {
      await cleanup();
    } catch (error) {
      report.status = "failed";
      report.failedStep = "cleanup";
      report.cleanupError = error.stack ?? String(error);
      process.exitCode = 1;
      log(`FAIL at cleanup: ${error.message}`);
    }
    report.finishedAt = new Date().toISOString();
    saveReport();
    if (report.status === "passed") {
      log("PASS: real-window and real-IPC subset completed; native file pickers and creation/import buttons remain untested.");
    }
    log(`Evidence: ${runDir}`);
  }
}

await main();
