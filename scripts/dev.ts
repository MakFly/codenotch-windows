// Dev loop for the native app: runs it, and on every saved change under gpui-app/src or
// gpui-app/assets, closes it, rebuilds and starts it again. GPUI has no hot reload, so this
// is a restart; the configuration on disk carries the settings over.
//   bun dev            (extra arguments go to the app, e.g. `bun dev --settings`)
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { watch } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const app = join(root, "gpui-app");
const args = process.argv.slice(2);
let child: ChildProcess | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;

/// cargo starts the app as its own child: end the whole tree, or the old window stays and
/// keeps the executable locked.
function stop() {
  if (child?.pid && child.exitCode === null) {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  }
  child = undefined;
}

function start() {
  stop();
  console.log("\x1b[36m[dev]\x1b[0m building and starting CodeNotch…");
  child = spawn("cargo", ["run", "--manifest-path", join(app, "Cargo.toml"), "--", ...args], {
    cwd: root,
    stdio: "inherit",
  });
}

for (const dir of ["src", "assets"]) {
  watch(join(app, dir), { recursive: true }, (_, file) => {
    if (!file || file.endsWith("~") || file.includes(".tmp")) return;
    clearTimeout(timer);
    // Editors write in bursts (save, format, rename): wait for the last one.
    timer = setTimeout(() => {
      console.log(`\x1b[36m[dev]\x1b[0m ${dir}/${file} changed`);
      start();
    }, 300);
  });
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    stop();
    process.exit(0);
  });
}
start();
