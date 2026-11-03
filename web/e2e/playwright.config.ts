import { defineConfig, devices } from "@playwright/test";

/**
 * End-to-end tests run against the reference server, which serves the built demo
 * next to generated datasets. Chromium runs with the GPU disabled, as on the Citrix
 * desktops parallax targets.
 */
export default defineConfig({
  testDir: ".",
  outputDir: "../test-results",
  timeout: 300_000,
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: "http://127.0.0.1:8791", viewport: { width: 1400, height: 900 }, launchOptions: { args: ["--disable-gpu", "--disable-gpu-compositing", "--disable-software-rasterizer"] } },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1400, height: 900 } } }],
  webServer: { command: "target/release/parallax-server --config examples/experiment.toml", cwd: "../..", url: "http://127.0.0.1:8791/health", reuseExistingServer: true, timeout: 120_000 }
});
