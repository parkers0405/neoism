// Compiled from neoism-web/src/pixel-wordmark.ts; keep all 15 website effects in sync.
"use strict";
var NeoismWordmark = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // ../neoism-web/src/pixel-wordmark.ts
  var pixel_wordmark_exports = {};
  __export(pixel_wordmark_exports, {
    mountPixelWordmark: () => mountPixelWordmark
  });
  var SVG_NS = "http://www.w3.org/2000/svg";
  var GRID = 31.25;
  var TILE_LIMIT = 1e3;
  var SETTLE_MS = 200;
  var EFFECTS = [
    "scan",
    "scatter",
    "rain",
    "beams",
    "decrypt",
    "crumble",
    "slice",
    "wipe",
    "waves",
    "fireworks",
    "burn",
    "vhs",
    "rise",
    "checkerboard",
    "spiral"
  ];
  var mounts = /* @__PURE__ */ new WeakMap();
  var clamp = (value) => Math.max(0, Math.min(1, value));
  var ease = (value) => 1 - (1 - clamp(value)) ** 3;
  function mountPixelWordmark(root) {
    mounts.get(root)?.();
    const button = root.querySelector("#wordmark-trigger");
    const svg = root.querySelector(".splash-wordmark");
    if (!button || !svg) return () => {
    };
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const bounds = root.getBoundingClientRect();
    let visible = bounds.bottom > 0 && bounds.top < innerHeight;
    let disposed = false;
    let frame = 0;
    let previous;
    let bag = [];
    const chooseEffect = () => {
      if (bag.length === 0) {
        bag = [...EFFECTS];
        for (let i = bag.length - 1; i > 0; i--) {
          const j = Math.floor(Math.random() * (i + 1));
          [bag[i], bag[j]] = [bag[j], bag[i]];
        }
        if (bag[bag.length - 1] === previous) {
          [bag[0], bag[bag.length - 1]] = [bag[bag.length - 1], bag[0]];
        }
      }
      const effect = bag.pop();
      previous = effect;
      return effect;
    };
    let groups = [];
    let tiles = [];
    let sparks = [];
    const restore = () => {
      cancelAnimationFrame(frame);
      frame = 0;
      root.classList.remove("is-pixel-animating");
      delete root.dataset.pixelEffect;
      for (const group of groups) group.remove();
      groups = [];
      tiles = [];
      sparks = [];
    };
    const rect = (parent, size) => {
      const node = document.createElementNS(SVG_NS, "rect");
      node.setAttribute("width", String(size));
      node.setAttribute("height", String(size));
      parent.append(node);
      return node;
    };
    const build = () => {
      const context = document.createElement("canvas").getContext("2d");
      if (!context || typeof Path2D === "undefined") return false;
      const letters = [...svg.querySelectorAll(".wordmark-letter")];
      if (letters.length !== 6) return false;
      for (const [letterIndex, letter] of letters.entries()) {
        const uses = [...letter.children].filter(
          (node) => node instanceof SVGUseElement
        );
        if (uses.length !== 3) throw new Error("Expected three wordmark layers");
        for (const [layerIndex, use] of uses.entries()) {
          const href = use.getAttribute("href");
          const path = href?.startsWith("#") ? [...svg.querySelectorAll("defs path")].find(
            (candidate) => candidate.id === href.slice(1)
          ) : void 0;
          const definition = path?.getAttribute("d");
          if (!definition) throw new Error("Missing local wordmark path");
          const shape = new Path2D(definition);
          const group = document.createElementNS(SVG_NS, "g");
          group.classList.add("pixel-wordmark-layer");
          group.setAttribute("aria-hidden", "true");
          group.setAttribute("fill", "currentColor");
          for (const attribute of ["transform", "opacity"]) {
            const value = use.getAttribute(attribute);
            if (value !== null) group.setAttribute(attribute, value);
          }
          groups.push(group);
          letter.append(group);
          for (let row = 0; row < 7; row++) {
            for (let column = 0; column < 7; column++) {
              const x = column * GRID;
              const y = -250 + row * GRID;
              if (!context.isPointInPath(shape, x + GRID / 2, y + GRID / 2)) continue;
              if (tiles.length >= TILE_LIMIT) throw new Error("Wordmark tile limit exceeded");
              const node = rect(group, GRID);
              node.classList.add("pixel-wordmark-tile");
              node.setAttribute("x", String(x));
              node.setAttribute("y", String(y));
              tiles.push({
                node,
                x,
                y,
                row,
                column,
                letter: letterIndex,
                across: (letterIndex + column / 7) / 6,
                seed: (row * 37 + column * 19 + letterIndex * 13) % 101 / 101
              });
            }
          }
          if (layerIndex === 2) {
            for (let i = 0; i < 2; i++) {
              const node = rect(group, GRID / 5);
              node.classList.add("pixel-wordmark-spark");
              sparks.push({ node, letter: letterIndex, seed: i * 0.43 + letterIndex * 0.07 });
            }
          }
        }
      }
      return tiles.length > 0;
    };
    const paint = (effect, progress) => {
      const p = clamp(progress);
      const beat = Math.floor(p * 18);
      for (const tile of tiles) {
        let dx = 0;
        let dy = 0;
        let opacity = 1;
        let sx = 1;
        let sy = 1;
        let rotation = 0;
        const reveal = (arrival, span = 0.22) => ease((p - arrival) / span);
        switch (effect) {
          case "scan": {
            const built = reveal(tile.across * 0.72);
            sx = sy = 0.15 + built * 0.85;
            opacity = built;
            break;
          }
          case "scatter": {
            const displacement = ease(p / 0.22) * (1 - reveal(0.28 + tile.seed * 0.1, 0.52));
            dx = Math.cos(tile.seed * Math.PI * 2) * 22 * displacement;
            dy = Math.sin(tile.seed * Math.PI * 2) * 22 * displacement;
            sx = sy = 1 - displacement * 0.5;
            opacity = 1 - displacement * 0.35;
            break;
          }
          case "rain": {
            const fall = reveal(tile.seed * 0.35 + tile.across * 0.25, 0.35);
            dy = -GRID * 1.5 * (1 - fall);
            opacity = fall;
            sx = sy = 0.6 + fall * 0.4;
            break;
          }
          case "beams": {
            const built = reveal((tile.column * 3 + tile.letter) % 7 / 7 * 0.55, 0.35);
            sx = built ** 2;
            sy = 1 + Math.sin(built * Math.PI) * 2;
            opacity = clamp(built * 3);
            break;
          }
          case "decrypt": {
            const lock = reveal(tile.seed * 0.5, 0.35);
            const noise = (tile.column * 17 + tile.row * 23 + beat * 11) % 49;
            dx = (noise % 7 - tile.column) * GRID * (1 - lock);
            dy = (Math.floor(noise / 7) - tile.row) * GRID * (1 - lock);
            sx = sy = lock + (1 - lock) * (0.25 + noise % 3 * 0.2);
            opacity = 0.35 + lock * 0.65;
            break;
          }
          case "crumble": {
            const collapse = ease((p - tile.seed * 0.13) / 0.22);
            const rebuild = reveal(0.4 + (6 - tile.row) * 0.035, 0.32);
            const rubble = collapse * (1 - rebuild);
            dy = (6 - tile.row) * GRID * rubble;
            dx = (tile.seed - 0.5) * GRID * rubble;
            rotation = (tile.seed - 0.5) * 150 * rubble;
            sx = sy = 1 - rubble * 0.7;
            opacity = 1 - rubble * 0.3;
            break;
          }
          case "slice": {
            const join = reveal(tile.row * 0.065, 0.38);
            dx = (tile.row % 2 ? 1 : -1) * 7 * GRID * (1 - join);
            sy = 0.25 + join * 0.75;
            opacity = clamp(join * 4);
            break;
          }
          case "wipe": {
            const edge = p * 1.2;
            sx = clamp((edge - tile.across) * 42);
            dx = -GRID * (1 - sx) / 2;
            break;
          }
          case "waves": {
            const crest = p * Math.PI * 5 - tile.across * Math.PI * 4;
            const envelope = (1 - p) ** 2;
            dy = Math.sin(crest) * GRID * 1.4 * envelope;
            sy = 1 + Math.cos(crest) * 0.45 * envelope;
            opacity = reveal(tile.across * 0.25, 0.25);
            break;
          }
          case "fireworks": {
            const flight = clamp((p - tile.letter * 0.04) / 0.7);
            const launch = ease(flight / 0.4);
            const overshoot = Math.sin(flight * Math.PI) * 0.4;
            const travel = launch + overshoot;
            dx = (3 * GRID - tile.x) * (1 - travel);
            dy = (-140 - tile.y) * (1 - travel) - Math.sin(flight * Math.PI) * GRID;
            sx = sy = Math.max(0.08, launch);
            rotation = (1 - flight) * (tile.seed - 0.5) * 180;
            opacity = clamp(flight * 6);
            break;
          }
          case "burn": {
            const scorch = reveal(tile.row * 0.035, 0.18);
            const regrow = reveal(0.42 + (6 - tile.row) * 0.035, 0.3);
            const ash = scorch * (1 - regrow);
            sx = 1 - ash * 0.85;
            sy = 1 - ash * 0.5;
            dy = -ash * GRID * (0.4 + tile.seed);
            opacity = 1 - ash * 0.9;
            break;
          }
          case "vhs": {
            const envelope = (1 - p) ** 2;
            const tracking = Math.sin(tile.row * 4 + beat * 2);
            dx = Math.round(tracking * 2) * GRID * envelope;
            sy = 1 - (tile.row % 2 ? 0.65 : 0.15) * envelope;
            opacity = 1 - ((beat + tile.row) % 3 === 0 ? 0.55 : 0.1) * envelope;
            break;
          }
          case "rise": {
            const lift = reveal(tile.letter * 0.055 + (6 - tile.row) * 0.04, 0.36);
            dy = (6 - tile.row) * GRID * (1 - lift);
            sy = lift;
            opacity = lift;
            break;
          }
          case "checkerboard": {
            const parity = (tile.column + tile.row) % 2;
            const flip = reveal(parity * 0.38 + tile.letter * 0.025, 0.32);
            sx = Math.abs(Math.cos((1 - flip) * Math.PI / 2));
            sy = 0.2 + flip * 0.8;
            rotation = (1 - flip) * (parity ? 90 : -90);
            opacity = flip;
            break;
          }
          case "spiral": {
            const assemble = reveal(tile.seed * 0.18, 0.65);
            const angle = (1 - assemble) * Math.PI * 2;
            const x = tile.x - 3 * GRID;
            const y = tile.y + 140;
            const radius = 0.25 + assemble * 0.75;
            dx = (Math.cos(angle) * x - Math.sin(angle) * y) * radius - x;
            dy = (Math.sin(angle) * x + Math.cos(angle) * y) * radius - y;
            sx = sy = 0.15 + assemble * 0.85;
            rotation = (1 - assemble) * 270;
            opacity = clamp(assemble * 3);
            break;
          }
        }
        const cx = tile.x + GRID / 2;
        const cy = tile.y + GRID / 2;
        tile.node.setAttribute("transform", `translate(${cx + dx} ${cy + dy}) rotate(${rotation}) scale(${sx} ${sy}) translate(${-cx} ${-cy})`);
        tile.node.setAttribute("opacity", String(opacity));
      }
      for (const spark of sparks) {
        let x = 0;
        let y = 0;
        let width = GRID / 5;
        let height = width;
        let opacity = 0;
        if (effect === "scan") {
          const sweep = p / 0.94 * 6 - spark.letter;
          if (sweep >= 0 && sweep <= 1) opacity = (1 - sweep) * 0.85;
          x = clamp(sweep) * 7 * GRID;
          y = -200 + Math.sin(p * 19 + spark.seed * 12) * 45;
        } else if (effect === "beams") {
          const head = clamp((p - spark.letter * 0.035) / 0.72);
          x = Math.floor(spark.seed * 7) * GRID + GRID / 2;
          y = -250 + head * 7 * GRID;
          width = GRID / 8;
          height = GRID * 2;
          opacity = Math.sin(head * Math.PI) * 0.8;
        } else if (effect === "fireworks") {
          const flight = clamp((p - spark.letter * 0.04) / 0.7);
          const angle = spark.seed * Math.PI * 6;
          x = 3 * GRID + Math.cos(angle) * flight * 4 * GRID;
          y = -140 + Math.sin(angle) * flight * 3 * GRID;
          opacity = Math.sin(flight * Math.PI) * (1 - flight);
        } else if (effect === "burn") {
          x = spark.seed * 7 * GRID;
          y = -60 - p * 6 * GRID;
          opacity = Math.sin(p * Math.PI) ** 2 * 0.8;
        }
        spark.node.setAttribute("opacity", String(opacity));
        spark.node.setAttribute("x", String(x));
        spark.node.setAttribute("y", String(y));
        spark.node.setAttribute("width", String(width));
        spark.node.setAttribute("height", String(height));
      }
    };
    const replay = () => {
      restore();
      if (disposed || motion.matches || document.hidden || !visible) return;
      try {
        if (!build()) {
          restore();
          return;
        }
        const effect = chooseEffect();
        const duration = 1700 + Math.random() * 800;
        paint(effect, 0);
        root.dataset.pixelEffect = effect;
        root.classList.add("is-pixel-animating");
        const start = performance.now();
        const tick = (now) => {
          frame = 0;
          if (disposed || motion.matches || document.hidden || !visible || !root.isConnected) {
            restore();
            return;
          }
          try {
            const elapsed = now - start;
            if (elapsed >= duration + SETTLE_MS) {
              restore();
              return;
            }
            paint(effect, elapsed / duration);
            if (elapsed >= duration) {
              root.classList.remove("is-pixel-animating");
              const fade = 1 - (elapsed - duration) / SETTLE_MS;
              for (const group of groups) {
                group.style.opacity = String(fade * Number(group.getAttribute("opacity") ?? 1));
              }
            }
            frame = requestAnimationFrame(tick);
          } catch {
            restore();
          }
        };
        frame = requestAnimationFrame(tick);
      } catch {
        restore();
      }
    };
    const onMotion = () => {
      if (motion.matches) restore();
    };
    const onVisibility = () => {
      if (document.hidden) restore();
    };
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(
      ([entry]) => {
        visible = entry?.isIntersecting ?? false;
        if (!visible) restore();
      }
    );
    observer?.observe(root);
    button.addEventListener("click", replay);
    motion.addEventListener("change", onMotion);
    document.addEventListener("visibilitychange", onVisibility);
    const unmount = () => {
      disposed = true;
      restore();
      observer?.disconnect();
      button.removeEventListener("click", replay);
      motion.removeEventListener("change", onMotion);
      document.removeEventListener("visibilitychange", onVisibility);
      if (mounts.get(root) === unmount) mounts.delete(root);
    };
    mounts.set(root, unmount);
    replay();
    return unmount;
  }
  return __toCommonJS(pixel_wordmark_exports);
})();
(()=>{const root=document.getElementById("hero-title");if(root)NeoismWordmark.mountPixelWordmark(root)})();
