// Garden vines (#397): where a plant came from and what grew from it. A card
// whose patch was remixed shows its stem count, and a remix says what it was
// "grown from". Either opens the vine: the plant's lineage from the link
// service's GET /v1/plants/<id>/lineage, as a column of cards with a living
// stem drawn down their left, branching where a plant was remixed twice.
"use strict";

// How far each branch steps in, and the room the stem takes, in pixels
const VINE_STEP = 22;
const VINE_GUTTER = 28;
const VINE_CURL = 10;
// The deepest a branch indents: past it, branches line up, so a phone keeps
// room for the cards
const VINE_DEEPEST = 4;

const SPRIG = "M8 15V7M8 9C8 6 6 4 3 4C3 7 5 9 8 9ZM8 7C8 4 10 2 13 2C13 5 11 7 8 7Z";

/// A small sprig, for the stem count and "grown from".
function sprig() {
    const svg = element("svg", { viewBox: "0 0 16 16", "aria-hidden": "true", class: "sprig" });
    element("path", { d: SPRIG }, svg);
    return svg;
}

class Vine {
    /// `grow(plant, patch)` makes a plant's card, as the garden does.
    constructor(base, grow) {
        this.base = base;
        this.grow = grow;
        this.asked = 0;
        this.rows = [];

        this.dialog = document.createElement("dialog");
        this.dialog.className = "vine";
        this.dialog.setAttribute("aria-labelledby", "vine-title");
        const head = document.createElement("header");
        const heading = document.createElement("div");
        this.title = document.createElement("h2");
        this.title.id = "vine-title";
        this.lede = document.createElement("p");
        this.lede.className = "lede";
        heading.append(this.title, this.lede);
        const close = document.createElement("button");
        close.className = "close";
        close.setAttribute("aria-label", "Close the vine");
        close.textContent = "×";
        close.addEventListener("click", () => this.dialog.close());
        head.append(heading, close);

        this.body = document.createElement("div");
        this.body.className = "vine-rows";
        this.stem = element("svg", { class: "stem", "aria-hidden": "true" }, this.body);
        this.status = document.createElement("p");
        this.status.className = "vine-status";
        this.status.setAttribute("role", "status");
        this.dialog.append(head, this.body, this.status);
        document.body.appendChild(this.dialog);

        // A click on the backdrop, which is the dialog itself, closes it
        this.dialog.addEventListener("click", (e) => {
            if (e.target === this.dialog) this.dialog.close();
        });
        // A queued task, so the vine may already be open again on another plant
        this.dialog.addEventListener("close", () => {
            if (this.dialog.open) return;
            this.asked += 1;
            this.clear();
        });
        // Again when the cards reflow, as a phone turns
        new ResizeObserver(() => {
            if (this.body.clientWidth !== this.drawnWidth) this.draw(false);
        }).observe(this.body);
    }

    /// The card, with its stem count and what it grew from, each opening the
    /// vine. Cards inside the vine keep neither: the stem says it.
    entwine(article, plant) {
        const about = article.querySelector(".about");
        if (plant.parent) {
            const grown = document.createElement("p");
            grown.className = "grown";
            if (plant.parent_title) {
                const button = document.createElement("button");
                button.append(sprig(), `Grown from ${plant.parent_title}`);
                button.title = "See the vine it grew on";
                button.addEventListener("click", () => this.open(plant));
                grown.appendChild(button);
            } else {
                grown.append(sprig(), "Grown from a shared patch");
            }
            about.querySelector(".meta").before(grown);
        }
        if (plant.stems > 0) {
            const stems = document.createElement("button");
            stems.className = "stems";
            const remixes = plant.stems === 1 ? "1 remix" : `${plant.stems} remixes`;
            stems.append(sprig(), String(plant.stems));
            stems.title = `${remixes} grew from it`;
            stems.setAttribute("aria-label", `${remixes}: see the vine`);
            stems.addEventListener("click", () => this.open(plant));
            about.querySelector(".meta").appendChild(stems);
        }
        return article;
    }

    async open(plant) {
        const asked = ++this.asked;
        this.clear();
        this.title.textContent = plant.title;
        this.lede.textContent = "Where it came from, and what grew from it";
        this.status.textContent = "Following the vine…";
        if (!this.dialog.open) this.dialog.showModal();
        try {
            const answer = await fetch(`${this.base}/v1/plants/${encodeURIComponent(plant.id)}/lineage`);
            if (!answer.ok) throw new Error(String(answer.status));
            const { id, root, cut } = await answer.json();
            const rows = [];
            const walk = (node, depth, parent) => {
                const row = { node, depth: Math.min(depth, VINE_DEEPEST), parent };
                rows.push(row);
                // A line of remixes stays in line, and a plant remixed twice branches
                const step = node.children.length > 1 ? 1 : 0;
                for (const child of node.children) walk(child, depth + step, row);
            };
            walk(root, 0, null);
            const patches = await Promise.all(rows.map((row) => inflate(row.node.payload).catch(() => null)));
            if (asked !== this.asked) return;

            for (const [i, row] of rows.entries()) {
                // A patch that won't draw leaves its place, so the vine still joins up
                let article;
                try {
                    if (!patches[i]) throw new Error("no patch");
                    // A root grown from a link that isn't a plant still says so
                    const shown = row.parent ? { ...row.node, parent: null } : { ...row.node, parent_title: null };
                    article = this.entwine(this.grow(shown, patches[i]), shown);
                } catch {
                    article = document.createElement("article");
                    article.className = "card withered";
                    article.textContent = row.node.title;
                }
                if (row.node.id === id) {
                    article.classList.add("here");
                    article.setAttribute("aria-current", "true");
                }
                article.style.setProperty("--depth", row.depth);
                article.style.animationDelay = `${i * 70}ms`;
                row.article = article;
                this.body.appendChild(article);
            }
            this.rows = rows;
            const remixes = rows.length - 1;
            this.lede.textContent = remixes === 0 ? "Nothing has grown from it yet" : `${rows.length} plants on one vine`;
            this.status.textContent = cut ? "More grew from it than the vine can hold." : "";
            this.draw(true);
            this.dialog.querySelector(".here")?.scrollIntoView({ block: "nearest" });
        } catch {
            if (asked !== this.asked) return;
            this.status.textContent = "The vine didn't answer. Try again in a moment.";
        }
    }

    clear() {
        for (const row of this.rows) row.article.remove();
        this.rows = [];
        this.stem.replaceChildren();
        this.status.textContent = "";
    }

    /// The stem: from each plant's point on it, down and curling in to each
    /// plant grown from it, with a leaf on each long reach and a jack where
    /// it meets the card. `growing` lets it grow in, row by row.
    draw(growing) {
        if (this.rows.length === 0) return;
        this.stem.replaceChildren();
        this.drawnWidth = this.body.clientWidth;
        this.stem.setAttribute("width", this.body.scrollWidth);
        this.stem.setAttribute("height", this.body.scrollHeight);
        const at = (row) => {
            const card = row.article;
            const picture = card.querySelector(".drawing");
            const middle = picture ? picture.offsetHeight / 2 : card.offsetHeight / 2;
            return { x: card.offsetLeft - VINE_GUTTER / 2, edge: card.offsetLeft, y: card.offsetTop + middle };
        };
        const animate = growing && !matchMedia("(prefers-reduced-motion: reduce)").matches;
        this.rows.forEach((row, i) => {
            const here = at(row);
            const delay = `${i * 70 + 120}ms`;
            const bud = (el) => {
                if (!animate) return;
                el.style.animationDelay = delay;
                el.classList.add("budding");
            };
            let d;
            if (row.parent) {
                const from = at(row.parent);
                const curl = Math.min(VINE_CURL, (here.y - from.y) / 2);
                d = `M${from.x} ${from.y}V${here.y - curl}Q${from.x} ${here.y} ${from.x + curl} ${here.y}H${here.edge}`;
                if (here.y - from.y > 72) bud(leaf(this.stem, from.x, (from.y + here.y) / 2, i % 2 === 0));
            } else {
                // The root's own short shoot, from a seed
                d = `M${here.x} ${here.y}H${here.edge}`;
                bud(element("circle", { class: "seed", cx: here.x, cy: here.y, r: 4 }, this.stem));
            }
            for (const kind of ["glow", "line"]) {
                const path = element("path", { class: `vine-stem ${kind}`, d }, this.stem);
                if (animate) {
                    const length = path.getTotalLength();
                    path.style.strokeDasharray = length;
                    path.style.strokeDashoffset = length;
                    path.style.animationDelay = delay;
                    path.classList.add("growing");
                }
            }
            bud(element("circle", { class: "jack", cx: here.edge, cy: here.y, r: 3 }, this.stem));
        });
    }
}

/// A small leaf on the stem at (x, y), to the right or the left.
function leaf(svg, x, y, right) {
    const turn = right ? -35 : 215;
    return element("path", {
        class: "leaf",
        d: "M0 0C4 -6 11 -6 14 0C11 6 4 6 0 0Z",
        transform: `translate(${x} ${y}) rotate(${turn})`,
    }, svg);
}
