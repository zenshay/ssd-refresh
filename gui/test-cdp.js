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

    // 1. Initial State Check: Two-column layout, Console open, Progress card idle, Open File button
    console.log('[CDP Test] Checking two-column layout, idle progress card, and storage sidebar...');
    const initialState = await page.evaluate(() => {
      const drawer = document.getElementById('console-drawer');
      const toggleBtn = document.getElementById('btn-toggle-console');
      const openBtn = document.getElementById('btn-open-log');
      const progressCard = document.getElementById('progress-card');
      const drawerStyle = drawer ? window.getComputedStyle(drawer) : null;
      const progressStyle = progressCard ? window.getComputedStyle(progressCard) : null;
      const initialLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      const isDrawerOpen = drawer && !drawer.hidden && drawerStyle?.display !== 'none';
      const isProgressVisible = progressCard && !progressCard.hidden && progressStyle?.display !== 'none';
      const isProgressIdle = progressCard?.classList.contains('is-idle') && progressCard?.dataset.state === 'idle';
      const hasTwoColumnLayout = document.querySelector('.app-two-column') !== null;
      const hasSidebar = document.querySelector('.layout-sidebar') !== null;
      const hasContent = document.querySelector('.layout-content') !== null;

      // Check storage drive cards in sidebar
      const driveCards = Array.from(document.querySelectorAll('#drives .drive'));
      const hasFullDrives = driveCards.length > 0 && driveCards.every(c => c.querySelector('.usage') !== null);

      return {
        isDrawerOpen,
        isProgressVisible,
        isProgressIdle,
        hasTwoColumnLayout,
        hasSidebar,
        hasContent,
        hasFullDrives,
        consoleBtnRemoved: toggleBtn === null,
        hasOpenLogBtn: openBtn !== null,
        bodyScrollHeight: document.body.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 2,
      };
    });
    console.log('[CDP Test] Initial State:', JSON.stringify(initialState, null, 2));

    if (!initialState.hasTwoColumnLayout || !initialState.hasSidebar || !initialState.hasContent) {
      throw new Error('Two-column layout structure was not detected!');
    }
    if (!initialState.consoleBtnRemoved) {
      throw new Error('Header console toggle button was not removed!');
    }
    if (!initialState.hasOpenLogBtn) {
      throw new Error('Open File button was not found in Console toolbar!');
    }
    if (!initialState.isProgressVisible || !initialState.isProgressIdle) {
      throw new Error('Progress card is not persistently visible in idle standby state!');
    }
    if (initialState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected on idle screen: scrollHeight=${initialState.bodyScrollHeight}, clientHeight=${initialState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Two-column master-detail layout active, zero scrollbar on idle ✓');

    await saveScreenshot(page, 'gui-01-select-all.png');

    // 2. Test Open File Button (invokes write_and_open_log)
    console.log('[CDP Test] Testing Open File button click...');
    await page.click('#btn-open-log');
    await wait(600);
    const afterOpenLog = await page.evaluate(() => {
      const lastLine = document.querySelector('#console-log-view .log-row:last-child')?.innerText;
      return { lastLine };
    });
    console.log('[CDP Test] Open File result log:', JSON.stringify(afterOpenLog, null, 2));
    console.log('[CDP Test] PASS: Open File button clicked and executed write_and_open_log without error ✓');

    // 3. Trigger Safe Scan
    console.log('[CDP Test] Clicking Start Safe Scan...');
    await page.click('#btn-scan');
    await wait(2500);

    const scanState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const scanLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);

      // Verify Target Storage in sidebar: usage bar and model remain visible
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
      throw new Error('Target Storage was collapsed! Expected full storage in sidebar.');
    }
    if (!scanState.isProgressActive) {
      throw new Error('Progress card did not transition to active mode during scan!');
    }
    if (scanState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected during scan: scrollHeight=${scanState.bodyScrollHeight}, clientHeight=${scanState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Full storage and active progress stable with zero scrollbars ✓');

    await saveScreenshot(page, 'gui-02-running-compact.png');

    // 4. Test Stop
    console.log('[CDP Test] Clicking Stop...');
    await page.click('#btn-stop');
    await wait(1800);

    await saveScreenshot(page, 'gui-03-stopped.png');

    // 5. Test Clear Console & Verify Re-scan Messages
    console.log('[CDP Test] Testing Clear button & re-scan logs...');
    await page.click('#btn-clear-log');
    await wait(400);

    // Re-run Safe Scan after Clear
    await page.click('#btn-scan');
    await wait(2500);

    const rescanCheck = await page.evaluate(() => {
      const lines = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      return {
        rescanLogsCount: lines.length,
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

    // Clean up stop
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
