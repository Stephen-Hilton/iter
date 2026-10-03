// Playwright config for the iter5 webui. globalSetup builds iter_data, starts it
// on a random port against a throwaway ArangoDB database (iter5_pw_<ts>) and seeds
// it through the API; globalTeardown stops it and drops the database.
const { defineConfig } = require('@playwright/test');
const fs = require('fs');
const path = require('path');

/** The installed Chromium build (the cache may hold a newer revision than this @playwright/test expects). */
function chromiumPath() {
  if (process.env.PW_CHROMIUM) return process.env.PW_CHROMIUM;
  const cache = path.join(process.env.HOME || '', 'Library/Caches/ms-playwright');
  let dirs = [];
  try { dirs = fs.readdirSync(cache).filter((d) => /^chromium-\d+$/.test(d)).sort((a, b) => +b.split('-')[1] - +a.split('-')[1]); } catch (e) { return undefined; }
  for (const d of dirs) {
    for (const rel of ['chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing', 'chrome-mac/Chromium.app/Contents/MacOS/Chromium', 'chrome-linux/chrome', 'chrome-linux64/chrome']) {
      const p = path.join(cache, d, rel);
      if (fs.existsSync(p)) return p;
    }
  }
  return undefined;
}

module.exports = defineConfig({
  testDir: './specs',
  globalSetup: require.resolve('./global-setup.js'),
  globalTeardown: require.resolve('./global-teardown.js'),
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60000,
  expect: { timeout: 10000 },
  reporter: [['list']],
  outputDir: 'test-results',
  use: {
    viewport: { width: 1440, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: { executablePath: chromiumPath() },
  },
});
