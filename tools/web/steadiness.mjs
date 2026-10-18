// How steady is the browser build's sound? (WEB.md, "How steady is it")
//
// Serves a trunk build (dist/), opens each patch as an embed in each browser,
// presses Play and listens for 30 seconds: the first half untouched, the
// second with the canvas dragged back and forth so the whole graph repaints
// every frame. index.html's sobaAudioStats() counts the late buffers
// (dropouts) and how long the app takes to render one on the page's thread.
//
//   (cd tools/web && npm install && npx playwright install firefox)
//   trunk build --release
//   node tools/web/steadiness.mjs [--dist dist] [--browsers chrome,firefox]
//        [--patches canopy,orrery] [--seconds 30]
//
// Chrome and Edge are the installed ones; Firefox is Playwright's own.
// Patches with a Keyboard are played with three keys held (Z, C, B).

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { chromium, firefox } from "playwright";

const args = process.argv.slice(2);
const option = (name, fallback) => {
    const i = args.indexOf(`--${name}`);
    return i >= 0 ? args[i + 1] : fallback;
};
const dist = resolve(option("dist", "dist"));
const browsers = option("browsers", "chrome,firefox").split(",");
const patches = option("patches", "lush-pad,canopy,clair-de-satellite,orrery").split(",");
const seconds = Number(option("seconds", "30"));
const held = option("keys", "lush-pad").split(",");

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".wav": "audio/wav", ".json": "application/json" };
const server = createServer(async (req, res) => {
    const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
    try {
        const file = path || "index.html";
        const body = await readFile(join(dist, file));
        res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
        res.end(body);
    } catch {
        res.writeHead(404).end();
    }
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const origin = `http://127.0.0.1:${server.address().port}`;

const launch = {
    chrome: () => chromium.launch({ channel: "chrome", args: ["--autoplay-policy=no-user-gesture-required"] }),
    edge: () => chromium.launch({ channel: "msedge", args: ["--autoplay-policy=no-user-gesture-required"] }),
    firefox: () => firefox.launch({ firefoxUserPrefs: { "media.autoplay.default": 0 } }),
};

const wait = (ms) => new Promise((ok) => setTimeout(ok, ms));
const stats = (page) => page.evaluate(() => ({ ...window.sobaAudioStats(), renders: window.sobaRenders.length }));
// The mean and 95th percentile of the buffers rendered between two snapshots
const spread = (all, from, to) => {
    const times = all.slice(from, to).sort((a, b) => a - b);
    const mean = times.reduce((a, b) => a + b, 0) / Math.max(1, times.length);
    return [mean.toFixed(1), (times[Math.floor(times.length * 0.95)] ?? 0).toFixed(1)];
};

async function measure(browserName, patch) {
    const browser = await launch[browserName]();
    const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
    // Every buffer's render time, not just index.html's running average:
    // this wraps the "ended" handler before index.html's own wrapper does
    await page.addInitScript(() => {
        window.sobaRenders = [];
        const listen = AudioBufferSourceNode.prototype.addEventListener;
        AudioBufferSourceNode.prototype.addEventListener = function (type, fn, ...rest) {
            if (type !== "ended") return listen.call(this, type, fn, ...rest);
            return listen.call(this, type, function (...args) {
                const t0 = performance.now();
                const result = fn.apply(this, args);
                window.sobaRenders.push(performance.now() - t0);
                return result;
            }, ...rest);
        };
    });
    await page.goto(`${origin}/?patch=${patch}`);
    // The app is up once it has taken the loading screen away
    await page.waitForFunction(() => !document.getElementById("loading") || document.getElementById("loading").style.display === "none" || !document.getElementById("loading").isConnected, null, { timeout: 120000 }).catch(() => {});
    await wait(3000);
    const canvas = page.locator("canvas#soba");
    const box = await canvas.boundingBox();
    const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    // The embed's Play button, top left (draw_embed_bar)
    await page.mouse.click(box.x + 50, box.y + 33);
    if (held.includes(patch)) for (const key of ["z", "c", "b"]) await page.keyboard.down(key);

    // Samples are fetched as a patch first plays one: start counting once sound flows
    await wait(2000);
    const begin = await stats(page);
    await wait((seconds / 2) * 1000);
    const still = await stats(page);

    // Pan back and forth with the middle button: the whole graph repaints every frame
    await page.mouse.move(cx, cy);
    await page.mouse.down({ button: "middle" });
    const end = Date.now() + (seconds / 2) * 1000;
    for (let t = 0; Date.now() < end; t += 1) {
        await page.mouse.move(cx + Math.sin(t / 10) * 300, cy + Math.cos(t / 13) * 120);
        await wait(16);
    }
    await page.mouse.up({ button: "middle" });
    const moving = await stats(page);
    const renders = await page.evaluate(() => window.sobaRenders);
    if (option("shot")) await page.screenshot({ path: `${option("shot")}-${browserName}-${patch}.png` });
    await browser.close();
    return {
        browser: browserName,
        patch,
        buffers: moving.buffers - begin.buffers,
        lateStill: still.late - begin.late,
        lateMoving: moving.late - still.late,
        renderStill: spread(renders, begin.renders, still.renders),
        renderMoving: spread(renders, still.renders, moving.renders),
        // The status bar's meter (#374): the page's share of each buffer, smoothed
        loadStill: still.load,
        loadMoving: moving.load,
        peak: +moving.peak.toFixed(3),
        contexts: moving.contexts.join(","),
    };
}

const rows = [];
for (const browserName of browsers) {
    for (const patch of patches) {
        const row = await measure(browserName, patch);
        console.log(JSON.stringify(row));
        rows.push(row);
    }
}
server.close();
console.log("\n| Browser | Patch | Late (still) | Late (dragging) | Render still (mean, p95) | Render dragging (mean, p95) | Load (still, dragging) | Buffers | Peak |");
console.log("|---|---|---|---|---|---|---|---|---|");
for (const r of rows) {
    console.log(`| ${r.browser} | ${r.patch} | ${r.lateStill} | ${r.lateMoving} | ${r.renderStill.join(", ")} ms | ${r.renderMoving.join(", ")} ms | ${r.loadStill}%, ${r.loadMoving}% | ${r.buffers} | ${r.peak} |`);
}
