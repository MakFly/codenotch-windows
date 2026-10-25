// Read-only integration smoke check. Never prints account identifiers, tokens or raw responses.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { join } from "node:path";
const executable = process.env.LOCALAPPDATA
  ? join(
      process.env.LOCALAPPDATA,
      "Programs",
      "OpenAI",
      "Codex",
      "bin",
      "codex.exe",
    )
  : "codex";
const child = spawn(executable, ["app-server", "--listen", "stdio://"], {
  stdio: ["pipe", "pipe", "ignore"],
  windowsHide: true,
});
const lines = createInterface({ input: child.stdout });
const responses = new Map<number, any>();
const send = (m: unknown) => child.stdin.write(`${JSON.stringify(m)}\n`);
await new Promise<void>((resolve, reject) => {
  const timer = setTimeout(() => {
    child.kill();
    reject(new Error("Codex timeout after 30 seconds"));
  }, 30000);
  child.on("error", (e) => {
    clearTimeout(timer);
    reject(new Error(`Codex launch failed: ${e.name}`));
  });
  lines.on("line", (line) => {
    let v: any;
    try {
      v = JSON.parse(line);
    } catch {
      return;
    }
    if (v.id === 1) {
      if (v.error) {
        clearTimeout(timer);
        child.kill();
        reject(new Error("Codex initialization refused"));
        return;
      }
      send({ method: "initialized", params: {} });
      send({ id: 2, method: "account/read", params: { refreshToken: false } });
      send({ id: 3, method: "account/rateLimits/read" });
      send({ id: 4, method: "account/usage/read" });
    }
    if ([2, 3, 4].includes(v.id)) responses.set(v.id, v);
    if (responses.size === 3) {
      clearTimeout(timer);
      child.kill();
      const limits = responses.get(3)?.result;
      const usage = responses.get(4)?.result;
      console.log(
        JSON.stringify(
          {
            authenticated: !!responses.get(2)?.result?.account,
            rateLimitsAvailable: !!limits,
            rateLimitBuckets: Object.keys(limits?.rateLimitsByLimitId || {}),
            usageAvailable: !!usage,
            dailyBucketCount: usage?.dailyUsageBuckets?.length ?? null,
            quotaErrorCode: responses.get(3)?.error?.code ?? null,
            usageErrorCode: responses.get(4)?.error?.code ?? null,
          },
          null,
          2,
        ),
      );
      resolve();
    }
  });
  send({
    id: 1,
    method: "initialize",
    params: {
      clientInfo: { name: "smashtoken", title: "SmashToken", version: "0.1.1" },
      capabilities: { experimentalApi: true },
    },
  });
}).finally(() => {
  lines.close();
  child.kill();
});
