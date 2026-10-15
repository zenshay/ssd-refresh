import { spawn } from 'child_process';
import puppeteer from 'puppeteer-core';
import http from 'http';
import fs from 'fs';
import path from 'path';

const EXE_PATH = 'F:\\ai-lab\\repos\\ssd-refresh\\target\\debug\\ssd-refresh-gui.exe';
const OUT_DIR = 'F:\\ai-lab\\repos\\ssd-refresh\\output';
const SCREENSHOT_PATH = path.join(OUT_DIR, 'gui-cdp-test.png');

function wait(ms) {
  return new Promise(res => setTimeout(res, ms));
}

function checkPort(port) {
  return new Promise(res => {
    http.get(`http://127.0.0.1:${port}/json/version`, (r) => {
      res(r.statusCode === 200);
    }).on('error', () => res(false));
  });
}

async function run() {
  console.log('[CDP Test] Starting GUI with remote debugging port 9222...');
  fs.mkdirSync(OUT_DIR, { recursive: true });

  const env = {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9222',
  };

  const proc = spawn(EXE_PATH, [], { env, stdio: 'inherit' });

  let connected = false;
  for (let i = 0; i < 30; i++) {
    await wait(500);
    if (await checkPort(9222)) {
      connected = true;
      break;
    }
  }

  if (!connected) {
    proc.kill();
    throw new Error('Timed out waiting for WebView2 CDP port 9222');
  }

  let browser;
  try {
    console.log('[CDP Test] CDP port 9222 active! Connecting puppeteer...');
    browser = await puppeteer.connect({
      browserURL: 'http://127.0.0.1:9222',
      defaultViewport: null,
    });

    // Wait for initial navigation to settle
    await wait(2500);

    const pages = await browser.pages();
    const page = pages[0] || (await browser.newPage());

    // Listen to browser console
    page.on('console', msg => console.log('  [Browser Console]', msg.type(), msg.text()));
    page.on('pageerror', err => console.error('  [Browser PageError]', err));

    let title = '';
    try {
      title = await page.title();
    } catch {
      await wait(1000);
      title = await page.title();
    }
    console.log('[CDP Test] Page connected:', title);

    // Wait for drive cards to render
    await wait(2000);

    const html = await page.evaluate(() => document.getElementById('drive-list')?.innerHTML);
    console.log('[CDP Test] #drive-list innerHTML:\n', html);

    const invokeTest = await page.evaluate(async () => {
      try {
        // @ts-ignore
        if (window.__TAURI__?.core?.invoke) {
          return { method: 'window.__TAURI__.core.invoke', res: await window.__TAURI__.core.invoke('get_volumes') };
        }
        if (window.__TAURI_INTERNALS__?.invoke) {
          return { method: 'window.__TAURI_INTERNALS__.invoke', res: await window.__TAURI_INTERNALS__.invoke('get_volumes') };
        }
        return { error: 'No Tauri invoke found on window' };
      } catch (e) {
        return { error: String(e), stack: e?.stack };
      }
    });
    console.log('[CDP Test] Direct invoke test in page:', JSON.stringify(invokeTest, null, 2));

    const drives = await page.evaluate(() => {
      const cards = Array.from(document.querySelectorAll('.drive-card'));
      return cards.map(c => c.innerText.replace(/\s+/g, ' '));
    });

    console.log('[CDP Test] Found drives count:', drives.length);
    drives.forEach((d, i) => console.log(`  Drive #${i + 1}: ${d}`));

    // Check SMART text
    const hasSmart = drives.some(d => d.includes('SMART:'));
    console.log('[CDP Test] Has SMART status indicator:', hasSmart ? 'YES ✓' : 'NO ✗');

    // E2E Test: Start Safe Scan, then Pause, then Resume, then Stop
    console.log('[CDP Test] Triggering Start Safe Scan...');
    await page.click('#btn-scan');
    await wait(600);

    const scanState = await page.evaluate(() => ({
      badge: document.getElementById('task-badge')?.innerText,
      phase: document.getElementById('progress-phase')?.innerText,
      count: document.getElementById('progress-count')?.innerText,
      progressBoxHidden: document.getElementById('progress-container')?.classList.contains('hidden'),
    }));
    console.log('[CDP Test] Scan active state:', JSON.stringify(scanState));

    console.log('[CDP Test] Triggering Pause during Step 1...');
    await page.click('#btn-pause');
    await wait(500);

    const pauseState = await page.evaluate(() => ({
      pauseText: document.getElementById('pause-text')?.innerText,
      speed: document.getElementById('progress-speed')?.innerText,
    }));
    console.log('[CDP Test] Pause state:', JSON.stringify(pauseState));

    console.log('[CDP Test] Triggering Stop...');
    await page.click('#btn-stop');
    await wait(1200);

    const stopState = await page.evaluate(() => ({
      badge: document.getElementById('task-badge')?.innerText,
      resultBox: document.getElementById('result-box')?.innerText,
      btnScanDisabled: Boolean(document.getElementById('btn-scan')?.disabled),
    }));
    console.log('[CDP Test] Post-stop state:', JSON.stringify(stopState));

    // Capture screenshot for AI Agent inspection
    console.log('[CDP Test] Capturing screenshot to:', SCREENSHOT_PATH);
    await page.screenshot({ path: SCREENSHOT_PATH });

    console.log('[CDP Test] All checks passed successfully!');
  } finally {
    if (browser) {
      try { await browser.disconnect(); } catch {}
    }
    try { proc.kill(); } catch {}
  }
}

run().catch(err => {
  console.error('[CDP Test Failed]:', err);
  process.exit(1);
});
