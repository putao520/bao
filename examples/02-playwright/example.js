// Example 02 — Playwright (Node.js) 连接 Bao over CDP
//
// 前置条件:
//   1. 启动 Bao CDP Server(独立终端):
//        bao browser --cdp-port 9222
//      或在项目根:
//        cargo run -p bao_bin -- browser --cdp-port 9222
//   2. 安装 Playwright:
//        npm i playwright
//
// 运行:
//   node example.js

const { chromium } = require('playwright');

const CDP_ENDPOINT = 'http://127.0.0.1:9222';

(async () => {
  console.log('[02-playwright] Connecting to Bao over CDP ...');
  // connectOverCDP 不会启动新的 Chrome 进程,而是连接到已有 CDP endpoint。
  // Playwright 会以为连的是 Chrome,实际背后是 servo。
  const browser = await chromium.connectOverCDP(CDP_ENDPOINT);

  const ctx = browser.contexts()[0] || (await browser.newContext());
  const page = await ctx.newPage();

  await page.goto('https://example.com', { waitUntil: 'domcontentloaded' });

  const title = await page.title();
  const ua = await page.evaluate(() => navigator.userAgent);

  console.log('[02-playwright] Page title:', title);
  console.log('[02-playwright] User agent:', ua);

  await page.screenshot({ path: 'bao-02-playwright.png', fullPage: true });
  console.log('[02-playwright] Screenshot saved: bao-02-playwright.png');

  await browser.close();
  console.log('[02-playwright] Done');
})().catch((err) => {
  console.error('[02-playwright] Failed:', err.message);
  console.error('  Did you start `bao browser --cdp-port 9222` first?');
  process.exit(1);
});
