// Hover to listen (#394): the garden's cards play through one hidden copy of
// Soba, the listener at /?listen (#391, WEB.md). Resting the pointer on a
// card for a moment loads and plays its patch; moving to another card fades
// across to it, and leaving the cards fades out. On a phone, and for heavy
// patches anywhere, a tap or click on the drawing plays it instead. A ring
// round the sounding card follows its level.
"use strict";

// How long the pointer rests on a card before it plays, so a quick pass
// across the grid stays quiet
const DWELL_MS = 280;
// The listener's swap fade (listen::FADE_SECONDS): levels until then are
// still the last patch's
const SWAP_MS = 200;
// The ring's range, in dB below full scale, and how fast it rises and falls
// (per frame, at 60 a second)
const FLOOR_DB = 54;
const ATTACK = 0.45;
const RELEASE = 0.12;

// The listener: soba.chaps.dev's own app, or ?app= on a local page (as
// garden.js's ?api=), say a `trunk serve` beside it
function appOrigin() {
    const local = ["127.0.0.1", "localhost"].includes(location.hostname);
    const asked = new URLSearchParams(location.search).get("app");
    if (local && asked) {
        try {
            return new URL(asked).origin;
        } catch {
            // Not an address: the page's own
        }
    }
    return location.origin;
}

class Listening {
    constructor() {
        this.origin = appOrigin();
        this.frame = document.createElement("iframe");
        this.frame.className = "listener";
        this.frame.title = "Soba, playing the garden";
        this.frame.allow = "autoplay";
        this.frame.tabIndex = -1;
        this.frame.setAttribute("aria-hidden", "true");
        // Messages sent before the frame's page loads would go to about:blank
        this.ready = false;
        this.waiting = [];
        this.frame.addEventListener("load", () => {
            this.ready = true;
            this.waiting.splice(0).forEach((message) => this.send(message));
        }, { once: true });
        this.frame.src = `${this.origin}/?listen`;
        document.body.appendChild(this.frame);

        this.sounding = null;   // the card playing, or about to
        this.dwelling = null;   // { card, timer } while the pointer rests
        this.quietUntil = 0;
        this.target = 0;        // the ring's level, 0 to 1
        this.shown = 0;
        this.drawing = false;
        this.gateDismissed = false;
        this.gate = null;

        addEventListener("message", (e) => this.hear(e));
        // A card scrolled out of sight stops sounding (a phone's way out)
        this.seen = new IntersectionObserver((entries) => {
            if (entries.some((entry) => entry.target === this.sounding && !entry.isIntersecting)) this.stop();
        });
    }

    send(message) {
        if (!this.ready) this.waiting.push(message);
        else this.frame.contentWindow.postMessage(message, this.origin);
    }

    /// Lets `card` play its `payload`. A `heavy` card plays only on a click.
    attach(card, payload, heavy) {
        const drawing = card.querySelector(".drawing");
        card.listen = { payload, heavy };
        drawing.setAttribute("role", "button");
        drawing.tabIndex = 0;
        drawing.setAttribute("aria-pressed", "false");
        drawing.setAttribute("aria-label", `Listen to ${card.querySelector("h2").textContent}`);
        if (heavy) {
            const hint = document.createElement("span");
            hint.className = "hint";
            hint.textContent = "Click to listen";
            drawing.appendChild(hint);
        }

        card.addEventListener("pointerenter", (e) => {
            if (e.pointerType === "touch" || heavy) return;
            this.rest(card);
        });
        card.addEventListener("pointerleave", () => {
            if (this.dwelling?.card === card) this.restless();
        });
        drawing.addEventListener("click", () => this.toggle(card));
        drawing.addEventListener("keydown", (e) => {
            if (e.key !== "Enter" && e.key !== " ") return;
            e.preventDefault();
            this.toggle(card);
        });
        this.seen.observe(card);
    }

    /// Leaving the cards altogether (not just the gap between two) fades out.
    watch(grid) {
        grid.addEventListener("pointerleave", (e) => {
            if (e.pointerType === "touch") return;
            this.restless();
            this.stop();
        });
    }

    rest(card) {
        this.restless();
        if (card === this.sounding) return;
        this.dwelling = { card, timer: setTimeout(() => {
            this.dwelling = null;
            this.play(card);
        }, DWELL_MS) };
    }

    restless() {
        if (this.dwelling) clearTimeout(this.dwelling.timer);
        this.dwelling = null;
    }

    toggle(card) {
        this.restless();
        if (card === this.sounding) this.stop();
        else this.play(card);
    }

    play(card) {
        if (card === this.sounding) return;
        const crossing = this.sounding !== null;
        this.mark(this.sounding, false);
        this.sounding = card;
        this.mark(card, true);
        card.classList.add("waking");
        this.target = 0;
        this.shown = crossing ? this.shown : 0;
        this.quietUntil = performance.now() + (crossing ? SWAP_MS : 0);
        this.send({ type: "load", payload: card.listen.payload });
        this.send({ type: "play" });
        this.animate();
    }

    stop() {
        if (!this.sounding) return;
        this.mark(this.sounding, false);
        this.sounding = null;
        this.target = 0;
        this.send({ type: "stop" });
    }

    mark(card, on) {
        if (!card) return;
        card.classList.toggle("sounding", on);
        if (!on) {
            card.classList.remove("waking");
            card.style.removeProperty("--level");
        }
        card.querySelector(".drawing").setAttribute("aria-pressed", String(on));
    }

    hear(e) {
        if (e.source !== this.frame.contentWindow || e.origin !== this.origin) return;
        const message = e.data ?? {};
        if (message.type === "level") {
            if (!this.sounding || performance.now() < this.quietUntil) return;
            const db = 20 * Math.log10(Math.max(Number(message.rms) || 0, 1e-6));
            this.target = Math.min(1, Math.max(0, (db + FLOOR_DB) / FLOOR_DB));
            if (this.target > 0.02) this.sounding.classList.remove("waking");
            this.animate();
        } else if (message.type === "blocked") {
            if (this.sounding) this.ask();
        } else if (message.type === "error") {
            const card = this.sounding;
            this.stop();
            if (card) card.querySelector(".drawing").title = "This patch didn't play here: open it in Soba";
        }
    }

    /// The browser holds the sound until the page is tapped: ask once a visit.
    ask() {
        if (this.gateDismissed || this.gate) return;
        this.gate = document.createElement("button");
        this.gate.className = "allow";
        this.gate.type = "button";
        this.gate.textContent = "Tap to let the garden play";
        this.gate.addEventListener("click", () => {
            this.gateDismissed = true;
            this.gate.remove();
            this.gate = null;
            // This tap is what lets the listener's sound start
            if (this.sounding) this.send({ type: "play" });
        }, { once: true });
        document.body.appendChild(this.gate);
    }

    /// Eases the ring toward the level, a frame at a time, until it's still.
    animate() {
        if (this.drawing) return;
        this.drawing = true;
        const step = () => {
            const rate = this.target > this.shown ? ATTACK : RELEASE;
            this.shown += (this.target - this.shown) * rate;
            if (this.sounding) this.sounding.style.setProperty("--level", this.shown.toFixed(3));
            if (Math.abs(this.target - this.shown) < 0.002) {
                this.drawing = false;
                return;
            }
            requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
    }
}
