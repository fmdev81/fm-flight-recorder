// Runs the axis tick planner of static/app.js without a browser:
//   node server/tests/js/ticks.test.mjs
// The planner must never loop on a range that is not a finite, positive
// span (an endless loop there freezes the tab), and it must stay bounded.
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const src = fs.readFileSync(fileURLToPath(new URL("../../static/app.js", import.meta.url)), "utf8");
const noop = () => {};
const ctx = {
  console,
  Date,
  Math,
  URL,
  window: { __FR_TEST__: {}, addEventListener: noop },
  document: { readyState: "loading", addEventListener: noop, body: { addEventListener: noop } },
};
ctx.window.document = ctx.document;
vm.createContext(ctx);
vm.runInContext(src, ctx);
const plan = ctx.window.__FR_TEST__.tickPlan;
assert.equal(typeof plan, "function");

// A normal range: ticks inside it, sorted, no duplicates.
let p = plan(480, 1080, null);
assert.ok(p.ticks.length >= 4 && p.ticks.length <= 30, `ticks: ${p.ticks.length}`);
assert.ok(p.ticks.every((t) => t.m >= 480 && t.m <= 1080));
assert.equal(JSON.stringify(p.ticks.map((t) => t.m)), JSON.stringify(p.ticks.map((t) => t.m).sort((a, b) => a - b)));
assert.equal(new Set(p.ticks.map((t) => t.t)).size, p.ticks.length);
assert.equal(p.secs, false);

// A short range shows seconds.
p = plan(700, 701, null);
assert.equal(p.secs, true);
assert.ok(p.ticks.length > 0);

// The tick next to now is left out.
const withNow = plan(480, 1080, 724.9);
assert.ok(withNow.ticks.every((t) => Math.abs(t.m - 724.9) >= 600 * 0.04));

// Ranges that are not a finite, positive span: no ticks, and it returns.
for (const [lo, hi] of [[0, Infinity], [-Infinity, 5], [-Infinity, Infinity], [NaN, 5], [5, NaN], [10, 5], [5, 5], [0, 0]]) {
  const t0 = Date.now();
  assert.equal(plan(lo, hi, null).ticks.length, 0, `${lo}..${hi}`);
  assert.ok(Date.now() - t0 < 200);
}

// A huge span is bounded.
p = plan(0, 1e12, null);
assert.ok(p.ticks.length <= 400, `bounded: ${p.ticks.length}`);
console.log("ticks.test.mjs ok");
