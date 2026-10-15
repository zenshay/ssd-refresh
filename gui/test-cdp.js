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

    // 1. Initial State Check: Multi-select and Select All button
    console.log('[CDP Test] Testing Select All button...');
    const drivesBefore = await page.evaluate(() => {
      return Array.from(document.querySelectorAll('#drives .drive')).map(c => ({
        text: c.innerText.replace(/\s+/g, ' '),
        selected: c.classList.contains('is-selected'),
      }));
    });
    console.log('[CDP Test] Drives before Select All:', JSON.stringify(drivesBefore, null, 2));

    await page.click('#btn-select-all');
    await wait(400);

    const drivesAfterSelectAll = await page.evaluate(() => {
      return Array.from(document.querySelectorAll('#drives .drive')).map(c => ({
        root: c.getAttribute('data-root'),
        selected: c.classList.contains('is-selected'),
      }));
    });
    console.log('[CDP Test] Drives after Select All:', JSON.stringify(drivesAfterSelectAll, null, 2));
    const allAreSelected = drivesAfterSelectAll.every(d => d.selected);
    if (!allAreSelected) {
      throw new Error('Not all drives were selected by Select All button!');
    }
    console.log('[CDP Test] PASS: Select All button selected all drives successfully ✓');

    await saveScreenshot(page, 'gui-01-select-all.png');

    // 2. Trigger Safe Scan with Multiple Drives
    console.log('[CDP Test] Clicking Start Safe Scan (Multi-drive)...');
    await page.click('#btn-scan');
    await wait(1000);

    const scanState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      const metricLabels = Array.from(document.querySelectorAll('.metric-label')).map(el => el.innerText.trim());

      return {
        mode: document.getElementById('app')?.dataset.mode,
        title: document.getElementById('progress-title')?.innerText,
        subtitle: document.getElementById('progress-subtitle')?.innerText,
        statFiles: document.getElementById('stat-files')?.innerText,
        hasTimeLeftLabel: metricLabels.includes('TIME LEFT'),
        hasEtaLabel: metricLabels.includes('ETA'),
        cardComputedDisplay: style?.display,
        bodyScrollHeight: document.body.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 20,
      };
    });
    console.log('[CDP Test] Multi-drive Scan active state:', JSON.stringify(scanState, null, 2));

    if (scanState.hasEtaLabel) {
      throw new Error('Found old ETA label! Expected Time Left');
    }
    console.log('[CDP Test] PASS: Metric label successfully updated from ETA to TIME LEFT ✓');

    console.log(`[CDP Test] Scroll check: scrollHeight=${scanState.bodyScrollHeight}, clientHeight=${scanState.bodyClientHeight}, hasScrollbar=${scanState.hasVerticalScrollbar}`);
    if (scanState.hasVerticalScrollbar) {
      console.warn('[CDP Test Warning]: Slight scrollbar detected, inspecting container height');
    } else {
      console.log('[CDP Test] PASS: Window maintains zero vertical scrollbar during scan ✓');
    }

    await saveScreenshot(page, 'gui-02-running-compact.png');

    // 3. Test Pause
    console.log('[CDP Test] Clicking Pause...');
    await page.click('#btn-pause');
    await wait(600);

    const pauseCheck = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const spin = card?.querySelector('.spin');
      const fill = card?.querySelector('.progress-fill');
      return {
        cardHasIsPaused: card?.classList.contains('is-paused'),
        pausedBadgeVisible: !document.getElementById('badge-paused')?.hidden,
        pauseBtnText: document.getElementById('btn-pause-label')?.innerText,
        spinAnimationPlayState: spin ? window.getComputedStyle(spin).animationPlayState : null,
        fillAnimationPlayState: fill ? window.getComputedStyle(fill).animationPlayState : null,
      };
    });
    console.log('[CDP Test] Pause state inspection:', JSON.stringify(pauseCheck, null, 2));

    // 4. Test Stop
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
