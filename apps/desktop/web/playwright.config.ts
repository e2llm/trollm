import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests/browser",
  workers: 1,
  use: {
    baseURL: process.env.TROLLM_DESKTOP_BROWSER_URL ?? "http://127.0.0.1:4174",
    launchOptions: {
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
    },
    trace: "off",
  },
  webServer: process.env.TROLLM_DESKTOP_BROWSER_URL
    ? undefined
    : {
        command: "npm run dev -- --port 4174",
        url: "http://127.0.0.1:4174",
        reuseExistingServer: !process.env.CI,
      },
  reporter: "list",
});
