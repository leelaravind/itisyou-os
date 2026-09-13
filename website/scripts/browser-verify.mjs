#!/usr/bin/env node
/**
 * Real-browser verification of a deployed (or locally previewed) site.
 *
 * Drives a headless Chromium over the DevTools protocol — no dependencies, no
 * extension — and fails loudly on anything a visitor would hit:
 *
 *   * a route that does not answer 200 (and a missing page that is not a 404);
 *   * console errors, uncaught exceptions, and browser log errors (CSP
 *     violations and blocked resources arrive here);
 *   * failed or >= 400 sub-resource requests;
 *   * horizontal overflow at desktop (1440) and phone (390) widths;
 *   * an internal link that does not resolve;
 *   * expected text (version, commit) missing from the rendered page.
 *
 * HTTP status alone is not verification (docs/DEPLOYMENT.md); this is the
 * browser-side half of it. Screenshots are written as evidence, never as a
 * substitute for the functional checks above.
 *
 * Usage:
 *   node scripts/browser-verify.mjs --base https://os.itisyou.app \
 *     [--expect-text "v0.8.1"]... [--out <dir>] [--chrome <path>]
 */
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const args = process.argv.slice(2);
function opt(name, fallback) {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback;
}
function opts(name) {
  const out = [];
  for (let i = 0; i < args.length; i++) if (args[i] === name && i + 1 < args.length) out.push(args[i + 1]);
  return out;
}

const base = (opt('--base') ?? '').replace(/\/+$/, '');
if (!base) {
  console.error('usage: browser-verify.mjs --base <url> [--expect-text <text>]... [--out <dir>]');
  process.exit(2);
}
const expectTexts = opts('--expect-text');
const scratch = process.env.ITISYOU_SCRATCH || 'E:\\claude-tmp';
const outDir = resolve(opt('--out', join(scratch, 'browser-verify')));
const chromeCandidates = [
  opt('--chrome'),
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
].filter(Boolean);
const chromePath = chromeCandidates.find((p) => existsSync(p));
if (!chromePath) {
  console.error('browser-verify: no Chromium browser found (pass --chrome)');
  process.exit(2);
}

const ROUTES = [
  '/',
  '/build/',
  '/architecture/',
  '/platform/',
  '/security/',
  '/roadmap/',
  '/engineering/',
  '/docs/',
  '/releases/',
  '/changelog/',
  '/philosophy/',
  '/faq/',
  '/source/',
  ...opts('--route'),
];
const VIEWPORTS = [
  { name: 'desktop', width: 1440, height: 900, mobile: false, scale: 1 },
  { name: 'phone', width: 390, height: 844, mobile: true, scale: 2 },
];

mkdirSync(outDir, { recursive: true });
const profile = join(scratch, `chrome-profile-${process.pid}`);
rmSync(profile, { recursive: true, force: true });
mkdirSync(profile, { recursive: true });

const chrome = spawn(
  chromePath,
  [
    '--headless=new',
    '--remote-debugging-port=0',
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--disable-gpu',
    '--disable-background-networking',
    '--window-size=1440,900',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

let wsUrl = null;
for (let i = 0; i < 100 && !wsUrl; i++) {
  await sleep(100);
  const portFile = join(profile, 'DevToolsActivePort');
  if (existsSync(portFile)) {
    const [port, path] = readFileSync(portFile, 'utf8').trim().split(/\r?\n/);
    if (port && path) wsUrl = `ws://127.0.0.1:${port}${path}`;
  }
}
if (!wsUrl) {
  chrome.kill();
  console.error('browser-verify: Chromium did not expose a DevTools endpoint');
  process.exit(2);
}

// --- minimal CDP client over the browser endpoint (flattened sessions) -----
const ws = new WebSocket(wsUrl);
await new Promise((ok, fail) => {
  ws.onopen = ok;
  ws.onerror = fail;
});
let nextId = 1;
const pending = new Map();
const listeners = new Set();
ws.onmessage = (ev) => {
  const msg = JSON.parse(typeof ev.data === 'string' ? ev.data : ev.data.toString());
  if (msg.id && pending.has(msg.id)) {
    const { ok, fail } = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) fail(new Error(`${msg.error.message} (${msg.error.code})`));
    else ok(msg.result);
    return;
  }
  for (const l of listeners) l(msg);
};
function send(method, params = {}, sessionId) {
  const id = nextId++;
  const payload = { id, method, params };
  if (sessionId) payload.sessionId = sessionId;
  ws.send(JSON.stringify(payload));
  return new Promise((ok, fail) => pending.set(id, { ok, fail }));
}

const failures = [];
const report = { base, chrome: chromePath, startedAt: new Date().toISOString(), pages: [] };
const linkTargets = new Set();

async function visit(route, vp) {
  const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  const events = { console: [], exceptions: [], log: [], failedRequests: [], badResponses: [], mainStatus: null };
  let loaded = false;
  const onEvent = (msg) => {
    if (msg.sessionId !== sessionId) return;
    const p = msg.params || {};
    switch (msg.method) {
      case 'Runtime.consoleAPICalled':
        if (p.type === 'error' || p.type === 'warning' || p.type === 'assert')
          events.console.push(`${p.type}: ${(p.args || []).map((a) => a.value ?? a.description ?? '').join(' ')}`);
        break;
      case 'Runtime.exceptionThrown':
        events.exceptions.push(p.exceptionDetails?.exception?.description || p.exceptionDetails?.text || 'exception');
        break;
      case 'Log.entryAdded':
        if (p.entry?.level === 'error' || p.entry?.level === 'warning')
          events.log.push(`${p.entry.level} ${p.entry.source}: ${p.entry.text} ${p.entry.url ?? ''}`.trim());
        break;
      case 'Network.loadingFailed':
        if (!p.canceled) events.failedRequests.push(`${p.errorText} ${p.blockedReason ?? ''} (${p.type ?? ''})`.trim());
        break;
      case 'Network.responseReceived':
        if (p.type === 'Document' && events.mainStatus === null) events.mainStatus = p.response.status;
        else if (p.response.status >= 400) events.badResponses.push(`${p.response.status} ${p.response.url}`);
        break;
      case 'Page.loadEventFired':
        loaded = true;
        break;
      default:
    }
  };
  listeners.add(onEvent);
  await send('Page.enable', {}, sessionId);
  await send('Runtime.enable', {}, sessionId);
  await send('Log.enable', {}, sessionId);
  await send('Network.enable', {}, sessionId);
  await send(
    'Emulation.setDeviceMetricsOverride',
    { width: vp.width, height: vp.height, deviceScaleFactor: vp.scale, mobile: vp.mobile },
    sessionId,
  );
  await send('Page.navigate', { url: base + route }, sessionId);
  for (let i = 0; i < 200 && !loaded; i++) await sleep(50);
  await sleep(400);
  const probe = await send(
    'Runtime.evaluate',
    {
      returnByValue: true,
      expression: `(() => ({
        title: document.title,
        h1: (document.querySelector('h1')?.textContent || '').trim().slice(0, 120),
        text: document.body ? document.body.innerText : '',
        overflowX: document.documentElement.scrollWidth - window.innerWidth,
        scripts: document.querySelectorAll('script').length,
        lang: document.documentElement.lang,
        mainLandmark: !!document.querySelector('main'),
        links: [...document.querySelectorAll('a[href]')].map(a => a.getAttribute('href')),
        imagesMissingAlt: [...document.querySelectorAll('img')].filter(i => !i.hasAttribute('alt')).length,
        brokenImages: [...document.querySelectorAll('img')].filter(i => i.complete && i.naturalWidth === 0).length,
      }))()`,
    },
    sessionId,
  );
  const page = probe.result.value;
  const shot = await send('Page.captureScreenshot', { format: 'png' }, sessionId);
  const shotName = `${vp.name}${route.replace(/[^a-z0-9]+/gi, '_') || '_root'}.png`;
  writeFileSync(join(outDir, shotName), Buffer.from(shot.data, 'base64'));
  listeners.delete(onEvent);
  await send('Target.closeTarget', { targetId });

  const problems = [];
  if (!loaded) problems.push('load event never fired');
  if (events.mainStatus !== 200) problems.push(`document status ${events.mainStatus}`);
  if (events.console.length) problems.push(`console: ${events.console.join(' | ')}`);
  if (events.exceptions.length) problems.push(`exceptions: ${events.exceptions.join(' | ')}`);
  if (events.log.length) problems.push(`browser log: ${events.log.join(' | ')}`);
  if (events.failedRequests.length) problems.push(`failed requests: ${events.failedRequests.join(' | ')}`);
  if (events.badResponses.length) problems.push(`bad responses: ${events.badResponses.join(' | ')}`);
  if (page.overflowX > 1) problems.push(`horizontal overflow ${page.overflowX}px`);
  if (!page.mainLandmark) problems.push('no <main> landmark');
  if (!page.lang) problems.push('no document language');
  if (page.imagesMissingAlt) problems.push(`${page.imagesMissingAlt} <img> without alt`);
  if (page.brokenImages) problems.push(`${page.brokenImages} broken <img>`);
  if (route === '/') for (const t of expectTexts) if (!page.text.includes(t)) problems.push(`expected text missing: "${t}"`);
  for (const href of page.links) {
    if (!href || href.startsWith('#') || /^(mailto|tel):/.test(href)) continue;
    const abs = new URL(href, base + route);
    if (abs.origin === new URL(base).origin) linkTargets.add(abs.pathname);
  }
  report.pages.push({
    route,
    viewport: vp.name,
    status: events.mainStatus,
    title: page.title,
    h1: page.h1,
    scripts: page.scripts,
    overflowX: page.overflowX,
    screenshot: shotName,
    problems,
  });
  for (const p of problems) failures.push(`${vp.name} ${route}: ${p}`);
}

try {
  await send('Target.setDiscoverTargets', { discover: false });
  for (const vp of VIEWPORTS) for (const route of ROUTES) await visit(route, vp);

  // A missing page must be a real 404 that still renders the site's own page.
  const nf = await fetch(`${base}/definitely-not-a-page-${Date.now()}`);
  const nfBody = await nf.text();
  report.notFound = { status: nf.status, rendersSitePage: /ITISYOU/i.test(nfBody) };
  if (nf.status !== 404) failures.push(`missing route returned ${nf.status}, expected 404`);
  if (!report.notFound.rendersSitePage) failures.push('404 response is not the site 404 page');

  // Every internal link discovered on every page must resolve.
  report.links = [];
  for (const path of [...linkTargets].sort()) {
    const res = await fetch(base + path, { redirect: 'follow' });
    report.links.push({ path, status: res.status });
    if (res.status !== 200) failures.push(`broken internal link ${path} -> ${res.status}`);
  }

  // Security headers on the document itself.
  const head = await fetch(base + '/');
  const need = {
    'content-security-policy': /default-src 'none'/,
    'x-content-type-options': /nosniff/,
    'x-frame-options': /DENY/,
    'referrer-policy': /no-referrer/,
  };
  report.headers = {};
  for (const [h, re] of Object.entries(need)) {
    const v = head.headers.get(h);
    report.headers[h] = v;
    if (!v || !re.test(v)) failures.push(`header ${h} missing or wrong: ${v}`);
  }
} catch (err) {
  failures.push(`verifier error: ${err.stack || err}`);
} finally {
  try {
    await send('Browser.close');
  } catch {
    chrome.kill();
  }
  ws.close();
}

report.finishedAt = new Date().toISOString();
report.failures = failures;
writeFileSync(join(outDir, 'report.json'), JSON.stringify(report, null, 2));
await sleep(300);
rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });

const pages = report.pages.length;
const links = report.links?.length ?? 0;
if (failures.length) {
  console.log(`BROWSER-VERIFY: FAILED (${failures.length} problems over ${pages} page loads, ${links} links)`);
  for (const f of failures) console.log(`  - ${f}`);
  console.log(`evidence: ${outDir}`);
  process.exit(1);
}
console.log(
  `BROWSER-VERIFY: OK ${base} — ${pages} page loads (desktop+phone), ${links} internal links, ` +
    `0 console errors, 0 failed requests, 404 ok, headers ok; evidence: ${outDir}`,
);
