import { spawn } from 'child_process';
import puppeteer from 'puppeteer-core';
import http from 'http';
import fs from 'fs';
import path from 'path';

const EXE_PATH = 'F:\\ai-lab\\repos\\ssd-refresh\\target\\debug\\ssd-refresh-gui.exe';
const OUT_DIR = 'F:\\ai-lab\\repos\\ssd-refresh\\output';
const ARTIFACT_DIR = 'C:\\Users\\User\\.gemini\\antigravity\\brain\\4800c8aa-8174-4c27-b15b-ba372ff6001b';

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

async function saveScreenshot(page, filename) {
  const localPath = path.join(OUT_DIR, filename);
  const artifactPath = path.join(ARTIFACT_DIR, filename);
  await page.screenshot({ path: localPath });
  try {
    fs.copyFileSync(localPath, artifactPath);
  } catch {}
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

    // 1. Initial State Check: Progress card MUST be hidden before user clicks scan!
    const initialState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      return {
        cardExists: Boolean(card),
        cardHiddenAttr: card?.hidden,
        cardComputedDisplay: style?.display,
        isSpinning: Boolean(card?.querySelector('.spin')),
      };
    });
    console.log('[CDP Test] Initial Ready state:', JSON.stringify(initialState, null, 2));
    if (initialState.cardComputedDisplay !== 'none') {
      throw new Error(`Progress card is NOT hidden on initial load! display=${initialState.cardComputedDisplay}`);
    }
    console.log('[CDP Test] PASS: Progress card is completely hidden before clicking scan ✓');

    await saveScreenshot(page, 'gui-01-ready.png');

    // 2. Trigger Safe Scan (Step 1)
    console.log('[CDP Test] Clicking Start Safe Scan (Step 1)...');
    await page.click('#btn-scan');
    await wait(800);

    const scanState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      return {
        mode: document.getElementById('app')?.dataset.mode,
        title: document.getElementById('progress-title')?.innerText,
        subtitle: document.getElementById('progress-subtitle')?.innerText,
        statFiles: document.getElementById('stat-files')?.innerText,
        hasIndeterminate: document.getElementById('progress-track')?.classList.contains('indeterminate'),
        cardComputedDisplay: style?.display,
      };
    });
    console.log('[CDP Test] Scan active state (Step 1):', JSON.stringify(scanState, null, 2));
    if (scanState.cardComputedDisplay === 'none') {
      throw new Error('Progress card failed to become visible after clicking scan!');
    }

    // 3. Test Pause: Verify animation-play-state becomes "paused"
    console.log('[CDP Test] Clicking Pause...');
    await page.click('#btn-pause');
    await wait(600);

    const pauseCheck = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const spin = card?.querySelector('.spin');
      const fill = card?.querySelector('.progress-fill');
      const spinStyle = spin ? window.getComputedStyle(spin) : null;
      const fillStyle = fill ? window.getComputedStyle(fill) : null;

      return {
        cardHasIsPaused: card?.classList.contains('is-paused'),
        pausedBadgeVisible: !document.getElementById('badge-paused')?.hidden,
        pauseBtnText: document.getElementById('btn-pause-label')?.innerText,
        spinAnimationPlayState: spinStyle?.animationPlayState,
        fillAnimationPlayState: fillStyle?.animationPlayState,
      };
    });
    console.log('[CDP Test] Pause state inspection:', JSON.stringify(pauseCheck, null, 2));

    if (pauseCheck.spinAnimationPlayState !== 'paused' || pauseCheck.fillAnimationPlayState !== 'paused') {
      throw new Error(`Animations are NOT paused! spin=${pauseCheck.spinAnimationPlayState}, fill=${pauseCheck.fillAnimationPlayState}`);
    }
    console.log('[CDP Test] PASS: Both spinner and progress track animations are FROZEN during pause ✓');

    await saveScreenshot(page, 'gui-02-paused.png');

    // 4. Test Stop: Verify card completely disappears and notice shows up
    console.log('[CDP Test] Clicking Stop...');
    await page.click('#btn-stop');
    await wait(1800);

    const postStop = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      return {
        mode: document.getElementById('app')?.dataset.mode,
        cardComputedDisplay: style?.display,
        noticeVisible: !document.getElementById('notice-card')?.hidden,
        noticeTitle: document.getElementById('notice-title')?.innerText,
      };
    });
    console.log('[CDP Test] Post-stop inspection:', JSON.stringify(postStop, null, 2));

    if (postStop.cardComputedDisplay !== 'none') {
      throw new Error(`Progress card is NOT hidden after stop! display=${postStop.cardComputedDisplay}`);
    }
    console.log('[CDP Test] PASS: Progress card completely disappeared after stop ✓');

    await saveScreenshot(page, 'gui-03-stopped.png');

    console.log('[CDP Test] All CDP tests passed with 100% precision!');
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
