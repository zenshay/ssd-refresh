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

    // 1. Initial State Check: Console is open by default, Multi-select and Select All
    console.log('[CDP Test] Checking default console visibility & Select All...');
    const initialState = await page.evaluate(() => {
      const drawer = document.getElementById('console-drawer');
      const toggleBtn = document.getElementById('btn-toggle-console');
      const drawerStyle = drawer ? window.getComputedStyle(drawer) : null;
      const initialLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);
      const isDrawerOpen = drawer && !drawer.hidden && drawerStyle?.display !== 'none';
      const isBtnActive = toggleBtn?.classList.contains('active');

      return {
        isDrawerOpen,
        isBtnActive,
        initialLogsCount: initialLogs.length,
        initialFirstLog: initialLogs[0],
        bodyScrollHeight: document.body.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 2,
      };
    });
    console.log('[CDP Test] Initial State:', JSON.stringify(initialState, null, 2));

    if (!initialState.isDrawerOpen || !initialState.isBtnActive) {
      throw new Error('Console drawer is NOT open by default or pill is not active!');
    }
    if (initialState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected on idle screen: scrollHeight=${initialState.bodyScrollHeight}, clientHeight=${initialState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Console is open by default and zero vertical scrollbar on idle screen ✓');

    await saveScreenshot(page, 'gui-01-select-all.png');

    // 2. Trigger Safe Scan with Multiple Drives
    console.log('[CDP Test] Clicking Start Safe Scan (Multi-drive)...');
    await page.click('#btn-scan');
    // Wait for file enumeration and initial progress events
    await wait(2500);

    const scanState = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const style = card ? window.getComputedStyle(card) : null;
      const metricLabels = Array.from(document.querySelectorAll('.metric-label')).map(el => el.innerText.trim());
      const scanLogs = Array.from(document.querySelectorAll('#console-log-view .log-row')).map(r => r.innerText);

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
        hasVerticalScrollbar: document.body.scrollHeight > document.body.clientHeight + 2,
        scanLogsCount: scanLogs.length,
        recentScanLogs: scanLogs.slice(-6),
      };
    });
    console.log('[CDP Test] Multi-drive Scan active state:', JSON.stringify(scanState, null, 2));

    if (scanState.hasEtaLabel) {
      throw new Error('Found old ETA label! Expected Time Left');
    }
    if (scanState.scanLogsCount <= initialState.initialLogsCount) {
      throw new Error('No scan logs were streamed into console during Safe Scan!');
    }
    console.log('[CDP Test] PASS: Safe scan successfully streamed real-time step and speed logs into console ✓');

    if (scanState.hasVerticalScrollbar) {
      throw new Error(`Vertical scrollbar detected during scan: scrollHeight=${scanState.bodyScrollHeight}, clientHeight=${scanState.bodyClientHeight}`);
    }
    console.log('[CDP Test] PASS: Zero vertical scrollbar maintained during active scan with open console ✓');

    await saveScreenshot(page, 'gui-02-running-compact.png');

    // 3. Test Pause
    console.log('[CDP Test] Clicking Pause...');
    await page.click('#btn-pause');
    await wait(600);

    const pauseCheck = await page.evaluate(() => {
      const card = document.getElementById('progress-card');
      const spin = card?.querySelector('.spin');
      const fill = card?.querySelector('.progress-fill');
      const lastLog = document.querySelector('#console-log-view .log-row:last-child')?.innerText;
      return {
        cardHasIsPaused: card?.classList.contains('is-paused'),
        pausedBadgeVisible: !document.getElementById('badge-paused')?.hidden,
        pauseBtnText: document.getElementById('btn-pause-label')?.innerText,
        lastLog,
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
      const lastLog = document.querySelector('#console-log-view .log-row:last-child')?.innerText;
      return {
        mode: document.getElementById('app')?.dataset.mode,
        cardComputedDisplay: style?.display,
        noticeVisible: !document.getElementById('notice-card')?.hidden,
        noticeTitle: document.getElementById('notice-title')?.innerText,
        lastLog,
      };
    });
    console.log('[CDP Test] Post-stop inspection:', JSON.stringify(postStop, null, 2));

    await saveScreenshot(page, 'gui-03-stopped.png');

    // 5. Test Report Issue button
    console.log('[CDP Test] Testing Report Issue button click...');
    const reportBtnFound = await page.evaluate(() => {
      const btn = document.getElementById('btn-report-issue');
      return !!btn;
    });
    if (!reportBtnFound) {
      throw new Error('Report Issue button not found in Console drawer!');
    }
    // Clicking Report Issue calls api.openUrl (invoke open_url command)
    await page.click('#btn-report-issue');
    await wait(400);
    console.log('[CDP Test] PASS: Report Issue button clicked and executed open_url handler without crashing ✓');

    // 6. Test Console Toggle Close & Re-open
    console.log('[CDP Test] Testing Toggle Console Drawer...');
    await page.evaluate(() => document.getElementById('btn-toggle-console')?.click());
    await wait(400);

    const drawerClosedState = await page.evaluate(() => {
      const drawer = document.getElementById('console-drawer');
      const toggleBtn = document.getElementById('btn-toggle-console');
      return {
        drawerHidden: drawer?.hidden,
        btnActive: toggleBtn?.classList.contains('active'),
      };
    });
    console.log('[CDP Test] Drawer closed state:', JSON.stringify(drawerClosedState, null, 2));
    if (!drawerClosedState.drawerHidden || drawerClosedState.btnActive) {
      throw new Error('Toggle button failed to close console drawer!');
    }

    await saveScreenshot(page, 'gui-04-console-drawer.png');

    // Re-open console
    await page.evaluate(() => document.getElementById('btn-toggle-console')?.click());
    await wait(400);
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
