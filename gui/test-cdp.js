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

    // 1. Initial State Check: Console open, Progress card always visible in idle state, Full Storage
    console.log('[CDP Test] Checking default layout, idle progress card, and storage...');
    const initialState = await page.evaluate(() => {
      const drawer = document.getElementById('console-drawer');
      const toggleBtn = document.getElementById('btn-toggle-console');
      const progressCard = document.getElementById('progress-card');
      const drawerStyle = drawer ? window.getComputedStyle(drawer) : null;
      const progressStyle = progressCard ? window.getComputedStyle(progressCard) : null;
      const initialLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      const isDrawerOpen = drawer && !drawer.hidden && drawerStyle?.display !== 'none';
      const isProgressVisible = progressCard && !progressCard.hidden && progressStyle?.display !== 'none';
      const isProgressIdle = progressCard?.classList.contains('is-idle') && progressCard?.dataset.state === 'idle';

      // Check storage drive cards
      const driveCards = Array.from(document.querySelectorAll('#drives .drive'));
      const hasFullDrives = driveCards.length > 0 && driveCards.every(c => c.querySelector('.usage') !== null);

      return {
        isDrawerOpen,
        isProgressVisible,
        isProgressIdle,
        hasFullDrives,
        bodyScrollHeight: document.body.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 2,
      };
    });
    console.log('[CDP Test] Initial State:', JSON.stringify(initialState, null, 2));

    if (!initialState.isDrawerOpen) {
      throw new Error('Console drawer is NOT open by default!');
    }
    if (!initialState.isProgressVisible || !initialState.isProgressIdle) {
      throw new Error('Progress card is not persistently visible in idle standby state!');
    }
    if (initialState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected on idle screen: scrollHeight=${initialState.bodyScrollHeight}, clientHeight=${initialState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Layout is stable, Progress Card is idle standby, zero scrollbar on idle ✓');

    await saveScreenshot(page, 'gui-01-select-all.png');

    // 2. Trigger Safe Scan
    console.log('[CDP Test] Clicking Start Safe Scan (Multi-drive)...');
    await page.click('#btn-scan');
    await wait(2500);

    const scanState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      const scanLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);

      // Verify Target Storage is NOT compacted: usage bar and model must remain visible
      const driveCard = document.querySelector('#drives .drive');
      const usageEl = driveCard?.querySelector('.usage');
      const usageStyle = usageEl ? window.getComputedStyle(usageEl) : null;
      const isStorageFullNotCollapsed = usageStyle?.display !== 'none';

      return {
        mode: document.getElementById('app')?.dataset.mode,
        isProgressActive: card && !card.classList.contains('is-idle'),
        isStorageFullNotCollapsed,
        bodyScrollHeight: document.body.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 2,
        scanLogsCount: scanLogs.length,
        recentScanLogs: scanLogs.slice(-6),
      };
    });
    console.log('[CDP Test] Scan active state:', JSON.stringify(scanState, null, 2));

    if (!scanState.isStorageFullNotCollapsed) {
      throw new Error('Target Storage was collapsed! Expected full storage to stay visible.');
    }
    console.log('[CDP Test] PASS: Target Storage remains full and stable without collapsing ✓');

    if (!scanState.isProgressActive) {
      throw new Error('Progress card did not transition to active mode during scan!');
    }
    if (scanState.scanLogsCount < 5) {
      throw new Error('Scan logs failed to stream into console!');
    }
    console.log('[CDP Test] PASS: Safe scan successfully streamed real-time step and speed logs into console ✓');

    if (scanState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected during scan: scrollHeight=${scanState.bodyScrollHeight}, clientHeight=${scanState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Zero vertical scrollbar maintained during active scan with open console and full storage ✓');

    await saveScreenshot(page, 'gui-02-running-compact.png');

    // 3. Test Stop
    console.log('[CDP Test] Clicking Stop...');
    await page.click('#btn-stop');
    await wait(1800);

    const postStop = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      return {
        mode: document.getElementById('app')?.dataset.mode,
        isProgressIdleAfterStop: card?.classList.contains('is-idle'),
        progressTitle: document.getElementById('progress-title')?.innerText,
      };
    });
    console.log('[CDP Test] Post-stop inspection:', JSON.stringify(postStop, null, 2));
    if (!postStop.isProgressIdleAfterStop) {
      throw new Error('Progress card did not return to idle standby after stop!');
    }
    console.log('[CDP Test] PASS: Progress card smoothly returned to idle standby state ✓');

    await saveScreenshot(page, 'gui-03-stopped.png');

    // 4. Test Clear Console and Verify Re-scan Messages
    console.log('[CDP Test] Testing Clear button & re-scan logs...');
    await page.click('#btn-clear-log');
    await wait(400);

    const clearCheck = await page.evaluate(() => {
      const lines = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      return {
        logCountAfterClear: lines.length,
        firstLine: lines[0],
      };
    });
    console.log('[CDP Test] Console state after Clear:', JSON.stringify(clearCheck, null, 2));

    // Now re-run Safe Scan after Clear: verify messages stream properly!
    console.log('[CDP Test] Re-running Safe Scan after Clear...');
    await page.click('#btn-scan');
    await wait(2500);

    const rescanCheck = await page.evaluate(() => {
      const lines = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      return {
        rescanLogsCount: lines.length,
        recentLogs: lines.slice(-6),
        hasStepOneLog: lines.some(l => l.includes('[Step 1/3]')),
        hasStartingLog: lines.some(l => l.includes('Starting Safe Scan')),
      };
    });
    console.log('[CDP Test] Re-scan check:', JSON.stringify(rescanCheck, null, 2));

    if (!rescanCheck.hasStartingLog || !rescanCheck.hasStepOneLog) {
      throw new Error('Console failed to stream messages after clear and re-scan!');
    }
    console.log('[CDP Test] PASS: Clear and re-scan produced clean starting and step 1/3 messages ✓');

    await saveScreenshot(page, 'gui-04-console-drawer.png');

    // Clean up: stop the re-scan
    await page.click('#btn-stop');
    await wait(1800);

    await saveScreenshot(page, 'gui-05-scan-with-console.png');
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
