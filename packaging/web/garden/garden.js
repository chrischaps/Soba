// The Patch Garden (#393): pages through the link service's garden, newest
// first, and draws each plant's patch as a small cable diagram. Modules are
// shapes in their real positions and header colours, and cables are curves
// in their signal's colour, all from modules.json (#390), scaled to fit the
// card. A plant whose payload won't inflate or parse is skipped quietly.
"use strict";

const LINKS_API = "https://soba-links.chaps.workers.dev";
// Patches this big drop buffers in the browser (#387): Canopy, Clair de
// Satellite and Orrery, by module count or by size
const HEAVY_MODULES = 85;
const HEAVY_BYTES = 180 * 1024;
const SVG = "http://www.w3.org/2000/svg";
// Room around the drawing, and the header strip, in patch units
const MARGIN = 60;
const HEADER = 26;
const PORT_TOP = 44;

// A local `wrangler dev` with ?api=, only when the page itself is local, so
// no address of the real page can show someone else's garden
function api() {
    const local = ["127.0.0.1", "localhost"].includes(location.hostname);
    const asked = new URLSearchParams(location.search).get("api");
    return local && asked ? asked.replace(/\/+$/, "") : LINKS_API;
}

/// The patch a payload (`1.<data>`) holds: the same steps as
/// patch_link::patch_from_fragment and the Worker's inflatePayload.
async function inflate(payload) {
    const dot = payload.indexOf(".");
    if (dot < 0 || payload.slice(0, dot) !== "1") throw new Error("not a version 1 payload");
    const binary = atob(payload.slice(dot + 1).replace(/-/g, "+").replace(/_/g, "/"));
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    const patch = JSON.parse(await new Response(stream).text());
    if (!patch || !Array.isArray(patch.nodes)) throw new Error("not a patch");
    return patch;
}

function nominalHeight(table, rows, knobs, display) {
    const n = table.nominal;
    return n.base + n.row * rows + n.knob_row * Math.ceil(knobs / n.knobs_per_row) + (display ? n.display : 0);
}

/// What the drawing needs of each top-level node and group: where it is,
/// how big, its colour, and where each of its jacks sits.
function boxes(table, patch) {
    const width = table.nominal.width;
    const found = new Map();
    const jacks = (ports, x, y, h) => {
        const at = new Map();
        const step = Math.min(table.nominal.row, (h - PORT_TOP - 12) / Math.max(1, ports.length));
        ports.forEach((port, i) => at.set(port.name, { y: y + PORT_TOP + step * i, type: port.type }));
        return at;
    };
    for (const node of patch.nodes) {
        const module = table.modules[node.module_id];
        const [x, y] = node.position ?? [0, 0];
        const h = module ? module.height : nominalHeight(table, 4, 4, false);
        found.set(node.id, {
            x, y, w: width, h,
            color: module ? module.color : table.categories.Utility,
            inputs: jacks(module ? module.inputs : [], x, y, h),
            outputs: jacks(module ? module.outputs : [], x + width, y, h),
        });
    }
    for (const group of patch.groups ?? []) {
        const [x, y] = group.position ?? [0, 0];
        const inputs = group.inputs ?? [];
        const outputs = group.outputs ?? [];
        const pinned = (group.nodes ?? []).reduce((sum, node) => sum + Object.keys(node.pinned ?? {}).length, 0);
        const h = nominalHeight(table, Math.max(inputs.length, outputs.length), pinned, true);
        found.set(group.id, {
            x, y, w: width, h, group: true,
            color: table.groups.color,
            inputs: jacks(inputs, x, y, h),
            outputs: jacks(outputs, x + width, y, h),
        });
    }
    return found;
}

/// How many modules a patch holds, those inside its groups too.
function moduleCount(patch) {
    const inside = (groups) => (groups ?? []).reduce((sum, g) => sum + (g.nodes ?? []).length + inside(g.groups), 0);
    return patch.nodes.length + inside(patch.groups);
}

function element(name, attributes, parent) {
    const el = document.createElementNS(SVG, name);
    for (const [key, value] of Object.entries(attributes)) el.setAttribute(key, value);
    if (parent) parent.appendChild(el);
    return el;
}

/// The patch as an SVG cable diagram, fitted to its card.
function drawing(table, patch, title) {
    const found = boxes(table, patch);
    const frames = patch.frames ?? [];
    let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
    const grow = (x, y, w, h) => {
        left = Math.min(left, x);
        top = Math.min(top, y);
        right = Math.max(right, x + w);
        bottom = Math.max(bottom, y + h);
    };
    for (const box of found.values()) grow(box.x, box.y, box.w, box.h);
    for (const frame of frames) grow(...frame.position, ...frame.size);
    if (!isFinite(left)) grow(0, 0, 400, 300);

    // A lone module shouldn't fill the card: the view is never smaller than
    // a few modules across, so a drone looks as sparse as it is
    let w = right - left + 2 * MARGIN;
    let h = bottom - top + 2 * MARGIN;
    const least = 4.5 * table.nominal.width;
    const cx = (left + right) / 2, cy = (top + bottom) / 2;
    w = Math.max(w, least, (h * 4) / 3);
    h = Math.max(h, (w * 3) / 4);

    const cables = (patch.connections ?? []).length;
    const svg = element("svg", {
        viewBox: `${cx - w / 2} ${cy - h / 2} ${w} ${h}`,
        preserveAspectRatio: "xMidYMid meet",
        role: "img",
        "aria-label": `${title}: ${moduleCount(patch)} modules and ${cables} cables`,
    });

    for (const frame of frames) {
        const tint = table.frames.tints[frame.color] ?? table.frames.tints[table.frames.default];
        element("rect", {
            class: "frame", x: frame.position[0], y: frame.position[1], width: frame.size[0], height: frame.size[1],
            rx: 14, fill: tint, "fill-opacity": 0.07, stroke: tint, "stroke-opacity": 0.3,
        }, svg);
    }

    for (const box of found.values()) {
        const g = element("g", {}, svg);
        element("rect", { x: box.x, y: box.y, width: box.w, height: box.h, rx: 10, fill: box.group ? "#33283a" : "#2c2c46" }, g);
        // The header strip: a rounded top over a square-cornered bottom
        element("path", {
            d: `M${box.x} ${box.y + HEADER}V${box.y + 10}a10 10 0 0 1 10 -10H${box.x + box.w - 10}a10 10 0 0 1 10 10V${box.y + HEADER}Z`,
            fill: box.color,
        }, g);
    }

    // Cables over the modules, as the card is too small to hide them behind
    const wires = element("g", {}, svg);
    for (const cable of patch.connections ?? []) {
        const from = found.get(cable.from_node)?.outputs.get(cable.from_port);
        const to = found.get(cable.to_node)?.inputs.get(cable.to_port);
        const source = found.get(cable.from_node);
        const target = found.get(cable.to_node);
        if (!source || !target) continue;
        const x1 = source.x + source.w, y1 = from ? from.y : source.y + source.h / 2;
        const x2 = target.x, y2 = to ? to.y : target.y + target.h / 2;
        const type = from?.type ?? to?.type ?? "Control";
        const color = table.signals[type] ?? table.signals.Control;
        // The editor's bezier: out to the right, in from the left. Capped, so
        // a cable running back across a big patch doesn't swing off the card
        const reach = Math.min(Math.max(60, Math.abs(x2 - x1) * 0.5), 360);
        const d = `M${x1} ${y1}C${x1 + reach} ${y1} ${x2 - reach} ${y2} ${x2} ${y2}`;
        element("path", { class: "cable glow", d, stroke: color }, wires);
        element("path", { class: "cable line", d, stroke: color }, wires);
    }
    return svg;
}

const dateFormat = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });

function card(table, plant, patch) {
    const modules = moduleCount(patch);
    const heavy = modules >= HEAVY_MODULES || plant.bytes >= HEAVY_BYTES;
    const article = document.createElement("article");
    article.className = "card";

    const picture = document.createElement("div");
    picture.className = "drawing";
    picture.appendChild(drawing(table, patch, plant.title));
    article.appendChild(picture);

    const about = document.createElement("div");
    about.className = "about";
    const title = document.createElement("h2");
    title.textContent = plant.title;
    about.appendChild(title);
    if (plant.name) {
        const by = document.createElement("p");
        by.className = "by";
        by.textContent = plant.name;
        about.appendChild(by);
    }
    const meta = document.createElement("p");
    meta.className = "meta";
    const when = document.createElement("time");
    when.dateTime = new Date(plant.planted_at).toISOString();
    when.textContent = dateFormat.format(plant.planted_at);
    const count = document.createElement("span");
    count.textContent = modules === 1 ? "1 module" : `${modules} modules`;
    meta.append(when, count);
    if (heavy) {
        article.classList.add("heavy");
        const badge = document.createElement("span");
        badge.className = "badge heavy";
        badge.textContent = "Heavy";
        badge.title = "A big patch: it may stutter in the browser, and plays best in the desktop app";
        meta.appendChild(badge);
    }
    about.appendChild(meta);

    const open = document.createElement("a");
    open.className = "open";
    open.href = `/s/${encodeURIComponent(plant.id)}`;
    open.textContent = "Open in Soba";
    about.appendChild(open);
    article.appendChild(about);
    return article;
}

async function main() {
    const garden = document.getElementById("garden");
    const status = document.getElementById("status");
    const more = document.getElementById("more");
    const base = api();
    // One listener plays every card (listen.js, #394)
    const listening = new Listening();
    listening.watch(garden);

    let table;
    try {
        table = await (await fetch("modules.json")).json();
    } catch {
        status.textContent = "The garden couldn't load what it draws with. Try again in a moment.";
        return;
    }
    // Every card, in the garden and on a vine (vine.js, #397), and each one
    // plays as the garden's do
    const grow = (plant, patch) => {
        const grown = card(table, plant, patch);
        listening.attach(grown, plant.payload, grown.classList.contains("heavy"));
        return grown;
    };
    const vine = new Vine(base, grow);
    listening.watch(vine.body);
    vine.dialog.addEventListener("close", () => listening.stop());

    let before = "";
    let loading = false;
    let shown = 0;
    let ended = false;

    async function page() {
        if (loading || ended) return;
        loading = true;
        try {
            const answer = await fetch(before ? `${base}/v1/garden?before=${before}` : `${base}/v1/garden`);
            if (!answer.ok) throw new Error(String(answer.status));
            const { plants, before: next } = await answer.json();
            const patches = await Promise.all(plants.map((plant) => inflate(plant.payload).catch(() => null)));
            plants.forEach((plant, i) => {
                if (!patches[i]) return;
                try {
                    garden.appendChild(vine.entwine(grow(plant, patches[i]), plant));
                    shown += 1;
                } catch {
                    // A patch the drawing can't make sense of is left out
                }
            });
            if (next === null || next === undefined) {
                ended = true;
                observer.disconnect();
                status.textContent = shown === 0
                    ? "Nothing has been planted yet."
                    : shown === 1 ? "One patch grows here so far." : `That's the whole garden: ${shown} patches.`;
            } else {
                before = String(next);
                status.textContent = "";
            }
        } catch {
            status.textContent = "The garden didn't answer.";
            const again = document.createElement("button");
            again.textContent = "Try again";
            again.addEventListener("click", () => {
                status.textContent = "Gathering the garden…";
                page();
            });
            status.appendChild(again);
        } finally {
            loading = false;
        }
        // The next page while the sentinel is still in view (a tall screen)
        if (!ended && more.getBoundingClientRect().top < innerHeight + 800 && status.textContent === "") page();
    }

    const observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) page();
    }, { rootMargin: "800px 0px" });
    observer.observe(more);
    page();
}

main();
