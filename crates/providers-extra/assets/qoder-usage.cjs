// CodeNotch Qoder usage sidecar.
//
// Install next to the official SDK:  npm install @qoder-ai/qoder-agent-sdk
// Run by CodeNotch hidden, with a 120 s budget. Prints exactly one JSON object on stdout:
//   {"usage": <UsageInfo|null>}                       exit 0
//   {"error": "<code>", "message": "<text>"}          exit 1
// Codes: sdk_missing, needsAuth, daemon_failed, query_failed.
// Read only: getUsageInfo() is an account query; it starts no inference and spends no credits.
// Sign-in is the local qodercli session (qodercliAuth()); no other credential store is read.

'use strict';

const AUTH_WORDS = /auth|login|sign.?in|credential|token|unauthori[sz]ed|forbidden/i;

function finish(result, code) {
  process.stdout.write(JSON.stringify(result));
  process.exitCode = code;
}

function failure(kind, err) {
  const message = (err && err.message) ? String(err.message) : String(err);
  return { error: AUTH_WORDS.test(message) ? 'needsAuth' : kind, message: message.slice(0, 200) };
}

async function main() {
  let sdk;
  try {
    sdk = require('@qoder-ai/qoder-agent-sdk');
  } catch (err) {
    return finish({ error: 'sdk_missing', message: 'the Qoder SDK is not installed here' }, 1);
  }

  let daemon;
  try {
    // A cold daemon start can take well over the SDK's default handshake budget.
    daemon = await sdk.startDaemon({ auth: sdk.qodercliAuth(), initializeTimeoutMs: 60000 });
  } catch (err) {
    return finish(failure('daemon_failed', err), 1);
  }

  try {
    if (typeof daemon.auth === 'function') {
      let signedIn = false;
      try {
        signedIn = await daemon.auth();
      } catch (err) {
        return finish({ error: 'needsAuth', message: 'qodercli sign-in could not be used' }, 1);
      }
      if (!signedIn) {
        return finish({ error: 'needsAuth', message: 'qodercli has no stored sign-in' }, 1);
      }
    }
    try {
      const usage = await daemon.getUsageInfo();
      return finish({ usage: usage === undefined ? null : usage }, 0);
    } catch (err) {
      return finish(failure('query_failed', err), 1);
    }
  } finally {
    try { await daemon.stop(); } catch (_) { /* the job object cleans up anyway */ }
  }
}

// Exit explicitly: a daemon handle left open must not keep Node (and the 120 s budget) running.
main()
  .catch((err) => finish(failure('query_failed', err), 1))
  .finally(() => process.exit(process.exitCode || 0));
