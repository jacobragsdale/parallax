// Spike 0.3: Chromium DevTools access to a page's workers through the browser's
// session, which Playwright does not expose: throttling, heap usage and garbage
// collection in the engine's worker, and the renderer's resident memory.
import { readFileSync } from "node:fs";

export async function browserSession(browser) {
  const session = await browser.newBrowserCDPSession();
  let next = 1;
  const waiting = new Map();
  session.on("Target.receivedMessageFromTarget", ({ message }) => {
    const parsed = JSON.parse(message);
    const pending = waiting.get(parsed.id);
    if (pending !== undefined) {
      waiting.delete(parsed.id);
      if (parsed.error) pending.reject(new Error(parsed.error.message));
      else pending.resolve(parsed.result);
    }
  });
  /** Attaches to the first target of `type` whose url matches, and returns a send function for it. */
  async function attach(type, match) {
    const { targetInfos } = await session.send("Target.getTargets");
    const target = targetInfos.find((t) => t.type === type && match(t));
    if (target === undefined) throw new Error(`no ${type} target`);
    const { sessionId } = await session.send("Target.attachToTarget", { targetId: target.targetId, flatten: false });
    return (method, params = {}) =>
      new Promise((resolve, reject) => {
        const id = next++;
        waiting.set(id, { resolve, reject });
        session.send("Target.sendMessageToTarget", { sessionId, message: JSON.stringify({ id, method, params }) }).catch(reject);
      });
  }
  /** The resident memory of each renderer process, now and at its peak, in bytes. */
  async function renderers() {
    const { processInfo } = await session.send("SystemInfo.getProcessInfo");
    return processInfo
      .filter((p) => p.type === "renderer")
      .map((p) => {
        const status = readFileSync(`/proc/${p.id}/status`, "utf8");
        const kb = (key) => Number(new RegExp(`${key}:\\s+(\\d+)`).exec(status)?.[1] ?? 0) * 1024;
        return { pid: p.id, rss: kb("VmRSS"), peak: kb("VmHWM") };
      });
  }
  return { session, attach, renderers };
}

export const ARGS = ["--disable-gpu", "--disable-gpu-compositing", "--disable-software-rasterizer"];
