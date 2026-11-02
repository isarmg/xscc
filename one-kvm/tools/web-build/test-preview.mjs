// Test-only Node adapter. Browser resources are served by the Rust production tool.
import { spawn } from "node:child_process";
import { isIP } from "node:net";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const productRoot = fileURLToPath(new URL("../../", import.meta.url));

export async function preview(options = {}) {
  const host = options.preview?.host || "127.0.0.1";
  const port = options.preview?.port ?? 0;
  if (typeof host !== "string" || !Number.isInteger(port) || port < 0 || port > 65535) {
    throw new Error("Invalid Rust preview bind address");
  }
  const bind = `${host.includes(":") ? `[${host}]` : host}:${port}`;
  const args = ["preview", "--product-root", productRoot, "--bind", bind];
  if (options.build?.outDir) {
    args.push("--dist", resolve(options.root || process.cwd(), options.build.outDir));
  }
  const executable = process.env.XCSS_WEB_BUILD_BIN;
  const command = executable || "cargo";
  const commandArgs = executable ? args : [
    "+1.99.0", "run", "--quiet", "--locked", "--manifest-path",
    resolve(productRoot, "tools/web-build/Cargo.toml"), "--", ...args,
  ];
  const child = spawn(command, commandArgs, {
    cwd: productRoot, env: process.env, stdio: ["ignore", "pipe", "pipe"],
    detached: process.platform !== "win32",
  });
  let stderr = "";
  child.stderr.on("data", chunk => { stderr = (stderr + chunk).slice(-32_768); });
  let exited = false;
  let stopRequested = false;
  const exitedPromise = new Promise(resolveExit => child.once("exit", (code, signal) => {
    exited = true; resolveExit({ code, signal });
  }));
  function stop() {
    if (exited || stopRequested || !child.pid) return;
    stopRequested = true;
    try {
      if (process.platform === "win32") child.kill("SIGTERM");
      else process.kill(-child.pid, "SIGTERM");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  process.once("exit", stop);
  let endpoint;
  try {
    endpoint = await new Promise((resolveReady, rejectReady) => {
      let pending = "";
      const timeout = setTimeout(() => rejectReady(new Error(`Rust preview readiness timed out\n${stderr}`)), 600_000);
      const cleanup = () => clearTimeout(timeout);
      child.once("error", error => { cleanup(); rejectReady(error); });
      child.once("exit", (code, signal) => {
        cleanup(); rejectReady(new Error(`Rust preview exited before readiness (${code ?? signal})\n${stderr}`));
      });
      child.stdout.on("data", chunk => {
        pending += chunk;
        if (pending.length > 128_000) {
          cleanup(); rejectReady(new Error("Rust preview emitted an oversized readiness record")); return;
        }
        let newline;
        while ((newline = pending.indexOf("\n")) >= 0) {
          const line = pending.slice(0, newline); pending = pending.slice(newline + 1);
          let record;
          try { record = JSON.parse(line); } catch { continue; }
          if (record.event !== "listening") continue;
          try {
            const url = new URL(record.address);
            if (url.protocol !== "http:" || !url.port || !Number.isInteger(Number(url.port))) throw new Error("Invalid Rust preview address");
            cleanup(); resolveReady(url);
          } catch (error) { cleanup(); rejectReady(error); }
        }
      });
    });
  } catch (error) {
    stop(); process.removeListener("exit", stop); throw error;
  }
  const address = { address: endpoint.hostname, port: Number(endpoint.port), family: isIP(endpoint.hostname) === 6 ? "IPv6" : "IPv4" };
  let closing;
  function close(callback) {
    closing ??= (async () => { stop(); await exitedPromise; process.removeListener("exit", stop); })();
    if (callback) closing.then(() => callback(), callback);
    return closing;
  }
  return { httpServer: { address: () => address, close }, resolvedUrls: { local: [endpoint.href] }, close };
}
