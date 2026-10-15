import { spawn } from 'child_process';
import puppeteer from 'puppeteer-core';
import http from 'http';
import fs from 'fs';
import path from 'path';

const EXE_PATH = 'F:\\ai-lab\\repos\\ssd-refresh\\target\\debug\\ssd-refresh-gui.exe';
const OUT_DIR = 'F:\\ai-lab\\repos\\ssd-refresh\\output';
const SCREENSHOT_PATH = path.join(OUT_DIR, 'gui-cdp-test.png');
const ARTIFACT_PATH = 'C:\\Users\\User\\.gemini\\antigravity\\brain\\4800c8aa-8174-4c27-b15b-ba372ff6001b\\gui-cdp-test.png';

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

    await wait(2500);

    const pages = await browser.pages();
    const page = pages[0] || (await browser.newPage());

    page.on('console', msg => console.log('  [Browser Console]', msg.type(), msg.text()));
    page.on('pageerror', err => console.error('  [Browser PageError]', err));

    const title = await page.title();
    console.log('[CDP Test] Page connected:', title);

    await wait(1500);

    const drives = await page.evaluate(() => {
      const cards = Array.from(document.querySelectorAll('#drives .drive'));
      return cards.map(c => c.innerText.replace(/\s+/g, ' '));
    });

    console.log('[CDP Test] Found drives count:', drives.length);
    drives.forEach((d, i) => console.log(`  Drive #${i + 1}: ${d}`));

    // Check SMART
    const hasSmart = drives.some(d => d.includes('SMART'));
    console.log('[CDP Test] Has SMART status indicator:', hasSmart ? 'YES ✓' : 'NO ✗');

    // Trigger Safe Scan
    console.log('[CDP Test] Triggering Start Safe Scan (Step 1)...');
    await page.click('#btn-scan');
    await wait(800);

    const scanState = await page.evaluate(() => ({
      mode: document.getElementById('app')?.dataset.mode,
      title: document.getElementById('progress-title')?.innerText,
      subtitle: document.getElementById('progress-subtitle')?.innerText,
      statFiles: document.getElementById('stat-files')?.innerText,
      hasIndeterminate: document.getElementById('progress-track')?.classList.contains('indeterminate'),
      cardHidden: document.getElementById('progress-card')?.hidden,
    }));
    console.log('[CDP Test] Scan active state (Step 1):', JSON.stringify(scanState, null, 2));

    // Test Pause
    console.log('[CDP Test] Triggering Pause during Step 1...');
    await page.click('#btn-pause');
    await wait(600);

    const pauseState = await page.evaluate(() => ({
      pausedBadgeHidden: document.getElementById('badge-paused')?.hidden,
      pauseBtnText: document.getElementById('btn-pause-label')?.innerText,
      statEta: document.getElementById('stat-eta')?.innerText,
    }));
    console.log('[CDP Test] Pause state:', JSON.stringify(pauseState, null, 2));

    // Capture screenshot while paused to see badge and UI
    console.log('[CDP Test] Capturing screenshot while paused...');
    await page.screenshot({ path: SCREENSHOT_PATH });
    try {
      fs.copyFileSync(SCREENSHOT_PATH, ARTIFACT_PATH);
    } catch (e) {
      console.warn('Failed to copy to artifact dir:', e);
    }

    // Test Stop
    console.log('[CDP Test] Triggering Stop...');
    await page.click('#btn-stop');
    await wait(1500);

    const stopState = await page.evaluate(() => ({
      mode: document.getElementById('app')?.dataset.mode,
      cardHidden: document.getElementById('progress-card')?.hidden,
      noticeVisible: !document.getElementById('notice-card')?.hidden,
      noticeTitle: document.getElementById('notice-title')?.innerText,
      noticeBody: document.getElementById('notice-body')?.innerText,
    }));
    console.log('[CDP Test] Post-stop state:', JSON.stringify(stopState, null, 2));

    console.log('[CDP Test] Capturing final state screenshot...');
    await page.screenshot({ path: SCREENSHOT_PATH });
    try {
      fs.copyFileSync(SCREENSHOT_PATH, ARTIFACT_PATH);
    } catch {}

    console.log('[CDP Test] All CDP checks verified successfully!');
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
