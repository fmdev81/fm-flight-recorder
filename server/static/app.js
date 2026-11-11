// FM Flight Recorder dashboard: the client lanes (tooltip, arrow keys, range
// navigator, REC clock), charts (uPlot), hover readout, the table kit (sort, filter), theme switch, copy,
// relative times and live updates.
// No framework, no build step.
(function () {
  "use strict";

  function css(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }

  function fmtMs(ms) {
    var s = ms / 1000;
    if (s < 59.95) return s.toFixed(1) + " s";
    var t = Math.round(s);
    if (t < 3600) return Math.floor(t / 60) + " m " + String(t % 60).padStart(2, "0") + " s";
    var m = Math.round(t / 60);
    return Math.floor(m / 60) + " h " + String(m % 60).padStart(2, "0") + " m";
  }

  function axisMs(v) {
    if (v === 0) return "0";
    var s = v / 1000;
    if (s >= 3600) return +(s / 3600).toFixed(1) + " h";
    if (s >= 120) return +(s / 60).toFixed(1) + " m";
    return (s < 10 ? s.toFixed(s % 1 ? 1 : 0) : Math.round(s)) + " s";
  }

  // Nice y steps: ms for times (1 s ... 1 h), whole numbers for counts.
  var MS_STEPS = [500, 1000, 2000, 5000, 10000, 15000, 30000, 60000, 120000, 300000, 600000, 900000, 1800000, 3600000, 7200000];
  var N_STEPS = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 5000, 10000];
  function ySplits(isMs) {
    return function (u, ai, min, max) {
      var steps = isMs ? MS_STEPS : N_STEPS, step = steps[steps.length - 1];
      for (var i = 0; i < steps.length; i++) { if (max / steps[i] <= 2) { step = steps[i]; break; } }
      var out = [];
      for (var v = 0; v <= max + 1e-9; v += step) out.push(v);
      return out;
    };
  }

  // ---------------------------------------------------------------- charts
  var charts = [];

  function readout(card, spec, i) {
    var r = card && card.querySelector(".readout");
    if (!r || i == null || i < 0) return;
    var title = spec.titles[i] || spec.labels[i];
    if (spec.kind === "stack") {
      var srv = spec.srv[i], cli = spec.cli[i], tot = srv + cli;
      var pct = tot > 0 ? Math.round((srv / tot) * 100) : 0;
      r.innerHTML = "";
      add(r, "span", "lab", title);
      add(r, "span", "", fmtMs(tot) + " frozen");
      add(r, "span", "", spec.n[i] + " freezes");
      key(r, "sw sw-s", "Server " + fmtMs(srv) + " (" + pct + "%)");
      key(r, "sw sw-c", "Client " + fmtMs(cli));
    } else {
      r.innerHTML = "";
      add(r, "span", "lab", title);
      add(r, "span", "", spec.n[i] + (spec.unit === "ms" ? " ms" : " freezes"));
    }
  }

  function add(parent, tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    e.textContent = text;
    parent.appendChild(e);
    return e;
  }

  function key(parent, sw, text) {
    var k = add(parent, "span", "key", "");
    add(k, "span", sw, "");
    k.appendChild(document.createTextNode(text));
  }

  function peak(spec) {
    var best = -1, bi = spec.labels.length - 1;
    for (var i = 0; i < spec.labels.length; i++) {
      var v = spec.kind === "stack" ? spec.srv[i] + spec.cli[i] : spec.n[i];
      if (v > best) { best = v; bi = i; }
    }
    return bi;
  }

  function draw(el) {
    var spec;
    try { spec = JSON.parse(el.getAttribute("data-chart")); } catch (e) { return; }
    if (!window.uPlot || !spec.labels.length) return;
    var card = el.closest(".card");
    var n = spec.labels.length;
    var xs = [];
    for (var i = 0; i < n; i++) xs.push(i);
    var server = css("--server"), client = css("--client"), ink2 = css("--ink2"),
        faint = css("--faint");
    var FONT = "11px 'IBM Plex Mono', ui-monospace, monospace";
    var bars = uPlot.paths.bars({ size: [0.72, 64], gap: 2 });
    var data, series;
    if (spec.kind === "stack") {
      var tot = spec.srv.map(function (s, i) { return s + spec.cli[i]; });
      data = [xs, tot, spec.srv];
      series = [{},
        { label: "Client time", fill: client, stroke: client, width: 0, paths: bars, points: { show: false } },
        { label: "Server wait", fill: server, stroke: server, width: 0, paths: bars, points: { show: false } }];
    } else {
      // Counts and latency are neutral: blue and orange mean server and client only.
      var c = withAlpha(ink2, 0.7);
      data = [xs, spec.n];
      series = [{}, { label: "Freezes", fill: c, stroke: c, width: 0, paths: bars, points: { show: false } }];
    }
    var yfmt = spec.kind === "stack" ? axisMs : function (v) { return String(Math.round(v)); };
    var opts = {
      width: Math.max(el.clientWidth, 120),
      height: spec.height || 160,
      legend: { show: false },
      cursor: { points: { show: false }, drag: { x: false, y: false }, y: false },
      select: { show: false },
      scales: { x: { time: false, range: [-0.5, n - 0.5] }, y: { range: function (u, a, b) { return [0, b > 0 ? b * 1.08 : 1]; } } },
      axes: [
        { stroke: faint, grid: { show: false }, ticks: { show: false }, font: FONT,
          splits: function () { return xs; },
          values: function (u, splits) {
            var every = Math.ceil(n / Math.max(1, Math.floor(u.width / 44)));
            return splits.map(function (i) { return i % every === 0 ? spec.labels[i] : ""; });
          } },
        { stroke: faint, size: 48, font: FONT,
          grid: { show: false }, ticks: { show: false }, border: { show: false },
          splits: ySplits(spec.kind === "stack"),
          values: function (u, splits) { return splits.map(yfmt); } }
      ],
      series: series,
      hooks: {
        setCursor: [function (u) { readout(card, spec, u.cursor.idx == null ? peak(spec) : u.cursor.idx); }]
      }
    };
    el.innerHTML = "";
    var u = new uPlot(opts, data, el);
    readout(card, spec, peak(spec));
    charts.push({ el: el, u: u });
  }

  function withAlpha(c, a) {
    var m = /^#([0-9a-f]{6})$/i.exec(c);
    if (!m) return c;
    var n = parseInt(m[1], 16);
    return "rgba(" + (n >> 16) + "," + ((n >> 8) & 255) + "," + (n & 255) + "," + a + ")";
  }

  function drawAll() {
    charts.forEach(function (c) { c.u.destroy(); });
    charts = [];
    document.querySelectorAll(".chart[data-chart]").forEach(draw);
  }

  var rz;
  window.addEventListener("resize", function () {
    clearTimeout(rz);
    rz = setTimeout(function () {
      charts.forEach(function (c) { c.u.setSize({ width: Math.max(c.el.clientWidth, 120), height: c.u.height }); });
    }, 120);
  });

  // ---------------------------------------------------------------- theme
  // Dark is the default; the button cycles Dark -> Light -> Auto.
  var THEME_NAME = { dark: "Dark", light: "Light", auto: "Auto" };

  function theme() {
    var t = null;
    try { t = localStorage.getItem("fr-theme"); } catch (e) { /* no storage */ }
    return t === "light" || t === "auto" ? t : "dark";
  }

  function showTheme(t) {
    var b = document.getElementById("theme");
    if (b) {
      b.textContent = "Theme · " + THEME_NAME[t];
      b.setAttribute("aria-label", "Theme: " + THEME_NAME[t].toLowerCase() + ". Change theme");
    }
    document.querySelectorAll("[data-theme-set]").forEach(function (x) {
      x.setAttribute("aria-pressed", x.getAttribute("data-theme-set") === t ? "true" : "false");
    });
  }

  function setTheme(t) {
    var root = document.documentElement;
    if (t === "auto") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", t);
    try { localStorage.setItem("fr-theme", t); } catch (e) { /* no storage */ }
    showTheme(t);
    drawAll();
  }

  // ---------------------------------------------------------------- sidebar
  // The toggle collapses the sidebar to an icon rail (wide screens only).
  // The state is in localStorage ("fr-side"); the inline script in base.html
  // sets the class before the first paint. Without JS the toggle stays hidden
  // and the sidebar stays open. Shortcut: "[" (not while typing).
  function sideRail() { return document.documentElement.classList.contains("side-rail"); }

  function showSide() {
    var b = document.getElementById("side-toggle");
    if (!b) return;
    var rail = sideRail();
    b.hidden = false;
    b.setAttribute("aria-expanded", rail ? "false" : "true");
    b.setAttribute("title", (rail ? "Expand" : "Collapse") + " sidebar ([)");
    b.querySelector("span").textContent = rail ? "Expand" : "Collapse";
    document.querySelectorAll("#side-nav a").forEach(function (a) {
      if (rail) a.setAttribute("title", a.textContent.trim());
      else a.removeAttribute("title");
    });
  }

  function setSide(rail) {
    document.documentElement.classList.toggle("side-rail", rail);
    try { localStorage.setItem("fr-side", rail ? "rail" : "open"); } catch (e) { /* no storage */ }
    showSide();
    // Charts and lanes measure their width.
    window.dispatchEvent(new Event("resize"));
  }

  document.addEventListener("keydown", function (e) {
    if (e.key !== "[" || e.ctrlKey || e.metaKey || e.altKey) return;
    var t = e.target;
    if (t && (t.isContentEditable || /^(INPUT|SELECT|TEXTAREA)$/.test(t.tagName))) return;
    var b = document.getElementById("side-toggle");
    if (!b || b.hidden || !b.offsetParent) return;
    e.preventDefault();
    setSide(!sideRail());
  });

  // ---------------------------------------------------------------- setup
  function stepText() {
    var pre = document.getElementById("step-calc");
    if (!pre) return;
    var base = pre.getAttribute("data-base");
    var cb = document.getElementById("step-client");
    var name = document.getElementById("step-client-name");
    var extra = cb && cb.checked && name && name.value.trim() ? "; client=" + name.value.trim().replace(/[;"]/g, "") : "";
    var calc = base.replace(/" \)$/, extra + "\" )");
    pre.textContent = calc;
  }

  document.addEventListener("click", function (e) {
    var t = e.target.closest("button, a");
    if (!t) return;
    if (t.id === "side-toggle") {
      setSide(!sideRail());
    } else if (t.id === "theme") {
      setTheme({ dark: "light", light: "auto", auto: "dark" }[theme()] || "dark");
    } else if (t.hasAttribute("data-theme-set")) {
      setTheme(t.getAttribute("data-theme-set"));
    } else if (t.hasAttribute("data-preset") && !e.metaKey && !e.ctrlKey && !e.shiftKey && e.button === 0) {
      var pf = t.closest("[data-lanes].zoomable");
      if (pf) { e.preventDefault(); animateTo(pf, presetView(pf, t.getAttribute("data-preset"))); }
    } else if (t.classList.contains("tks")) {
      var tk = t.closest("[data-tk]");
      if (tk && e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey) {
        e.preventDefault();
        tkSort(tk, t.closest("th").getAttribute("data-key"));
      }
    } else if (t.id === "token-toggle") {
      var inp = document.getElementById("srv-token");
      var show = inp.type === "password";
      inp.type = show ? "text" : "password";
      t.textContent = show ? "Hide" : "Show";
    } else if (t.id === "step-copy") {
      var txt = document.getElementById("step-calc").textContent;
      var done = function () { t.textContent = "Copied"; setTimeout(function () { t.textContent = "Copy"; }, 1500); };
      if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(txt).then(done);
      else {
        var ta = document.createElement("textarea");
        ta.value = txt; document.body.appendChild(ta); ta.select();
        try { document.execCommand("copy"); done(); } catch (err) { /* no copy */ }
        document.body.removeChild(ta);
      }
    }
  });

  document.addEventListener("change", function (e) {
    var t = e.target;
    if (t.matches("select[data-autosubmit]")) t.form.submit();
    if (t.id === "step-client") stepText();
  });
  document.addEventListener("input", function (e) {
    if (e.target.id === "step-client-name") stepText();
  });
  // No swap of the lanes during a drag on the navigator.
  document.body.addEventListener("htmx:beforeSwap", function (e) {
    var tg = e.detail && e.detail.target;
    if (nav && tg && nav.f === tg) e.detail.shouldSwap = false;
  });
  document.body.addEventListener("htmx:afterSwap", function (e) {
    // "Load more" grew the list: live updates leave it alone from now on.
    var tg = e.detail && e.detail.target;
    if (tg && /-body$/.test(tg.id)) {
      var fl = tg.closest("[data-tk]");
      if (fl) fl.setAttribute("data-grown", "1");
    }
    stepText();
    initLanes();
    fadeAll();
  });

  // ---------------------------------------------------------------- lanes
  // Tooltip on hover and focus, roving tabindex (one Tab stop per lane), the
  // REC clock and the playhead. Without JS every block is a plain link.
  function tipFor(fz) {
    var lz = fz.closest(".lz");
    if (!lz) return;
    hideTip(lz);
    var d = fz.dataset;
    var tip = document.createElement("div");
    tip.className = "lztip";
    tip.setAttribute("role", "tooltip");
    if (d.bin) {
      add(tip, "span", "tt", d.bin);
      add(tip, "span", "", "Longest " + d.d + " at " + d.t);
    } else {
      add(tip, "span", "tt", d.t);
      add(tip, "span", "", "Frozen " + d.d);
    }
    var l1 = add(tip, "span", "tl1", "");
    if (d.from) add(l1, "span", "", d.from + " → ");
    l1.appendChild(document.createTextNode(d.layout));
    add(tip, "span", "", "Script: " + d.script);
    add(tip, "span", "", "Account: " + d.acct + " · " + d.client);
    var sp = add(tip, "span", "", "");
    key(sp, "sw sw-s", "Server " + d.srv + (d.pct ? " (" + d.pct + ")" : ""));
    sp.appendChild(document.createTextNode(" · "));
    key(sp, "sw sw-c", "Client " + d.cli);
    lz.appendChild(tip);
    var lr = lz.getBoundingClientRect(), fr = fz.getBoundingClientRect();
    var left = fr.left - lr.left;
    left = Math.max(0, Math.min(left, lr.width - tip.offsetWidth));
    tip.style.left = left + "px";
    // Below the block, unless it would run past the lanes and cover the
    // summary line and its buttons: then above the block.
    var below = fr.bottom - lr.top + 8;
    var top = below + tip.offsetHeight <= lr.height ? below : fr.top - lr.top - tip.offsetHeight - 8;
    tip.style.top = top + "px";
    tip.classList.toggle("up", top < below);
  }

  function hideTip(lz) {
    (lz || document).querySelectorAll(".lztip").forEach(function (t) { t.remove(); });
  }

  function moveTo(from, to) {
    if (!to) return;
    var lane = from.closest(".ln");
    if (lane) lane.querySelectorAll(".fz").forEach(function (b) { b.setAttribute("tabindex", "-1"); });
    var lane2 = to.closest(".ln");
    if (lane2) lane2.querySelectorAll(".fz").forEach(function (b) { b.setAttribute("tabindex", "-1"); });
    to.setAttribute("tabindex", "0");
    to.focus();
  }

  function nearest(lane, x) {
    var best = null, bd = Infinity;
    lane.querySelectorAll(".fz").forEach(function (b) {
      var d = Math.abs(b.getBoundingClientRect().left - x);
      if (d < bd) { bd = d; best = b; }
    });
    return best;
  }

  document.addEventListener("keydown", function (e) {
    var fz = e.target.closest && e.target.closest(".fz");
    if (!fz) return;
    var lane = fz.closest(".ln");
    var blocks = Array.prototype.slice.call(lane.querySelectorAll(".fz"));
    var i = blocks.indexOf(fz);
    var go = null;
    if (e.key === "ArrowRight") go = blocks[i + 1];
    else if (e.key === "ArrowLeft") go = blocks[i - 1];
    else if (e.key === "Home") go = blocks[0];
    else if (e.key === "End") go = blocks[blocks.length - 1];
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      var lanes = Array.prototype.slice.call(fz.closest(".lz").querySelectorAll(".ln"));
      var j = lanes.indexOf(lane), x = fz.getBoundingClientRect().left;
      var step = e.key === "ArrowDown" ? 1 : -1;
      for (var k = j + step; k >= 0 && k < lanes.length; k += step) {
        go = nearest(lanes[k], x);
        if (go) break;
      }
    } else if (e.key === "Escape") { hideTip(fz.closest(".lz")); return; }
    else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); tipFor(fz); return; }
    else return;
    e.preventDefault();
    moveTo(fz, go);
  });
  document.addEventListener("mouseover", function (e) {
    var fz = e.target.closest && e.target.closest(".fz");
    if (fz) tipFor(fz);
  });
  document.addEventListener("mouseout", function (e) {
    var fz = e.target.closest && e.target.closest(".fz");
    if (fz && !fz.contains(e.relatedTarget) && document.activeElement !== fz) hideTip(fz.closest(".lz"));
  });
  document.addEventListener("focusin", function (e) {
    var fz = e.target.closest && e.target.closest(".fz");
    if (fz) tipFor(fz);
  });
  document.addEventListener("focusout", function (e) {
    var fz = e.target.closest && e.target.closest(".fz");
    if (fz) hideTip(fz.closest(".lz"));
  });

  // The REC clock ticks from the server time at render (no client clock drift);
  // the playhead moves once a minute.
  function initLanes(root) {
    (root || document).querySelectorAll("[data-lanes]").forEach(function (f) {
      if (!f.dataset.t0) f.dataset.t0 = String(Date.now());
      if (skew === null && f.dataset.now) skew = +f.dataset.now - Date.now();
      // The JS tooltip replaces the title (the no-JS hover text).
      f.querySelectorAll(".fz[title]").forEach(function (b) { b.removeAttribute("title"); });
      if (!f.classList.contains("strip") && f.querySelector("[data-nav]")) {
        f.classList.add("zoomable");
        layout(f, viewOf(f));
      } else declutter(f);
    });
  }

  // ---------------------------------------------------------------- range
  // The navigator under the lanes: the whole day window with a mini plot of
  // all freezes, and the visible range on it (a window with two handles).
  // Drag a handle to change one end, drag the window to move the range, and
  // click or drag on the bar outside the window to pick a new range. The
  // lanes are laid out here from the minutes in data-m, data-dm, data-a and
  // data-b, so they follow the drag with no fetch. When the drag ends, the
  // URL gets the range (span=LO-HI, or zoom=Nm when the range ends at now)
  // and the page fetches the lanes again for the caption and the totals.
  // The preset buttons are links, so they work without JS.
  var MIN_SPAN = 1, FULL_SCALE = 60, GLYPH_SCALE = 240;
  var views = {};   // figure id -> {lo, hi, follow}
  var nav = null;   // the drag in progress
  var refetch = { seq: 0, timer: null };

  function fnum(f, k) { var v = parseFloat(f.dataset[k]); return isNaN(v) ? null : v; }
  function nowMin(f) {
    var n = fnum(f, "nowm");
    if (n === null) return null;
    return n + (Date.now() - (+f.dataset.t0 || Date.now())) / 60000;
  }
  function extOf(f) {
    var a = fnum(f, "xlo"), b = fnum(f, "xhi"), n = nowMin(f);
    if (n !== null && views[f.id] && views[f.id].follow) b = Math.max(b, Math.min(1440, n + followPad(views[f.id].follow)));
    return [a, b];
  }
  function followPad(x) { return Math.max(0.5, x * 0.06); }
  function followView(f, x) {
    var n = nowMin(f);
    // A tab left open past midnight: n is beyond the day until the page
    // loads the new day (tick asks for it). Keep a valid range meanwhile.
    var hi = Math.min(1440, n + followPad(x)), lo = Math.max(0, Math.min(n - x, hi - MIN_SPAN));
    return { lo: lo, hi: hi, follow: x };
  }
  // Keep the range in the day window, at least MIN_SPAN long; a move keeps
  // the length.
  function clampView(lo, hi, ext) {
    var w = Math.min(Math.max(hi - lo, MIN_SPAN), ext[1] - ext[0]);
    lo = Math.max(ext[0], Math.min(lo, ext[1] - w));
    return { lo: lo, hi: lo + w, follow: 0 };
  }
  // A range that holds now and ends at now (or at the end of the bar)
  // follows now: it becomes "the last N minutes".
  function settle(f, v) {
    var n = nowMin(f), ext = extOf(f);
    if (isDay(f, v)) return { lo: v.lo, hi: v.hi, follow: 0 };
    if (n === null || n < v.lo || n > v.hi) return { lo: v.lo, hi: v.hi, follow: 0 };
    var span = v.hi - v.lo;
    if (v.hi >= ext[1] - 0.01 || v.hi - n <= Math.max(0.5, span * 0.1)) return followView(f, Math.max(MIN_SPAN, n - v.lo));
    return { lo: v.lo, hi: v.hi, follow: 0 };
  }
  function isDay(f, v) { return !v.follow && Math.abs(v.lo - fnum(f, "dlo")) < 0.01 && Math.abs(v.hi - fnum(f, "dhi")) < 0.01; }
  function viewOf(f) {
    var v = views[f.id];
    if (!v) {
      var fo = fnum(f, "follow") || 0;
      v = fo > 0 && nowMin(f) !== null ? followView(f, fo) : { lo: fnum(f, "lo"), hi: fnum(f, "hi"), follow: 0 };
      views[f.id] = v;
    } else if (v.follow) {
      v = views[f.id] = followView(f, v.follow);
    }
    return v;
  }

  function hm(m, secs) {
    var t = Math.max(0, Math.round(m * 60));
    var p = function (n) { return String(n).padStart(2, "0"); };
    return p(Math.floor(t / 3600)) + ":" + p(Math.floor(t / 60) % 60) + (secs ? ":" + p(t % 60) : "");
  }
  // The same text as span_text() in lanes.rs.
  function spanText(span) {
    var s = Math.round(span * 60);
    if (s < 60) return s + " s";
    if (s < 600 && s % 60) return Math.floor(s / 60) + " min " + String(s % 60).padStart(2, "0") + " s";
    var m = Math.round(span), h = Math.floor(m / 60), r = m % 60;
    if (!h) return r + " min";
    return r ? h + " h " + String(r).padStart(2, "0") + " min" : h + " h";
  }
  function minPx(gw, span) {
    var t = Math.max(0, Math.min(1, (span - FULL_SCALE) / (GLYPH_SCALE - FULL_SCALE)));
    return 4 + Math.max(0, gw - 4) * t;
  }
  // Axis ticks: the same steps as ticks() in lanes.rs. A pure function (the
  // test in tests/js runs it). It never loops on a range that is not a
  // finite, positive span: that was a way to freeze the tab.
  var STEPS = [5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 14400, 21600];
  var MAX_TICKS = 200;
  function tickPlan(lo, hi, nowM) {
    var out = [];
    if (!isFinite(lo) || !isFinite(hi) || hi <= lo) return { secs: false, ticks: out };
    var span = (hi - lo) * 60;
    var pick = function (n) { for (var i = 0; i < STEPS.length; i++) if (span / STEPS[i] <= n) return STEPS[i]; return 21600; };
    var s1 = pick(8), s2 = pick(4), seen = {};
    [s1, s2].forEach(function (step) {
      var t = Math.ceil(lo * 60 / step) * step, guard = 0;
      for (; t <= hi * 60 && guard < MAX_TICKS; t += step, guard++) {
        if (seen[t]) continue;
        seen[t] = 1;
        var m = t / 60;
        if (nowM !== null && Math.abs(m - nowM) < (hi - lo) * 0.04) continue;
        out.push({ t: t, m: m, nw: !!(t % s1), nn: !!(t % s2) });
      }
    });
    out.sort(function (a, b) { return a.m - b.m; });
    return { secs: s1 < 60, ticks: out };
  }
  function drawTicks(f, lo, hi, nowM) {
    var trk = f.querySelector(".lzax .trk");
    if (!trk) return;
    trk.querySelectorAll(".tick").forEach(function (t) { t.remove(); });
    var plan = tickPlan(lo, hi, nowM), first = trk.firstChild;
    plan.ticks.forEach(function (o) {
      var el = document.createElement("span");
      el.className = "tick" + (o.nw ? " nw" : "") + (o.nn ? " nn" : "");
      el.style.left = ((o.m - lo) / (hi - lo) * 100).toFixed(3) + "%";
      el.textContent = hm(o.m, plan.secs);
      trk.insertBefore(el, first);
    });
  }

  // Lay out the lanes, the axis, the playhead and the navigator for view v.
  function layout(f, v) {
    var lo = v.lo, hi = v.hi, span = Math.max(hi - lo, 1e-6);
    var X = function (m) { return ((m - lo) / span * 100).toFixed(3) + "%"; };
    f.querySelectorAll(".ln .fz[data-m]").forEach(function (b) {
      var m = +b.dataset.m, dm = +b.dataset.dm;
      b.style.left = X(m);
      b.style.width = "max(" + minPx(+b.dataset.gw || 6, span).toFixed(1) + "px, " + (dm / span * 100).toFixed(4) + "%)";
    });
    f.querySelectorAll(".ln .ss[data-a]").forEach(function (s) {
      var a = +s.dataset.a, b = +s.dataset.b;
      s.style.left = X(a);
      s.style.width = (Math.max(0.2, (b - a) / span * 100)).toFixed(3) + "%";
    });
    var n = nowMin(f), inView = n !== null && n >= lo && n <= hi;
    f.querySelectorAll(".ln .nowl").forEach(function (l) { if (n !== null) l.style.left = X(n); });
    var ph = f.querySelector(".ph"), rec = f.querySelector(".rec");
    if (ph) { ph.hidden = !inView; if (inView) ph.style.setProperty("--f", ((n - lo) / span).toFixed(5)); }
    if (rec) { rec.hidden = !inView; if (inView) { rec.style.left = X(n); rec.classList.toggle("rec-l", (n - lo) / span > 0.7); } }
    drawTicks(f, lo, hi, inView && rec ? n : null);
    f.classList.toggle("scaled", span <= FULL_SCALE);
    var note = f.querySelector(".lnote");
    if (note) note.textContent = span <= FULL_SCALE ? "Positions and widths to time scale" : "Positions to time scale; short freezes drawn wider";
    // The label, the presets and Reset.
    var abbr = f.dataset.abbr || "", secs = span < 10;
    var range = hm(lo, secs) + "–" + hm(hi, secs) + (abbr ? " " + abbr : "");
    var stext = spanText(v.follow || span);
    var lab = f.querySelector(".zlab");
    if (lab) {
      lab.querySelector("b").textContent = range;
      lab.querySelector(".zspan").textContent = stext;
      lab.querySelector(".zfol").hidden = !v.follow;
    }
    var day = isDay(f, v), w = v.follow || span;
    f.querySelectorAll(".seg.zoom [data-preset]").forEach(function (a) {
      var W = presetMin(a.dataset.preset);
      a.setAttribute("aria-pressed", String(W === null ? day : Math.abs(w - W) < W * 0.02));
    });
    var rs = f.querySelector(".zreset");
    if (rs) rs.classList.toggle("off", day);
    // The navigator.
    var ext = extOf(f), elen = Math.max(ext[1] - ext[0], 1e-6);
    var win = f.querySelector(".nwin");
    if (win) {
      win.style.left = ((lo - ext[0]) / elen * 100).toFixed(3) + "%";
      win.style.width = ((hi - lo) / elen * 100).toFixed(3) + "%";
      win.setAttribute("aria-valuenow", lo.toFixed(2));
      win.setAttribute("aria-valuetext", range + ", " + stext + (v.follow ? ", follows now" : ""));
    }
    var nn = f.querySelector(".navnow");
    if (nn && n !== null) nn.style.left = ((n - ext[0]) / elen * 100).toFixed(3) + "%";
    declutter(f);
  }
  function presetMin(label) {
    return { "15 min": 15, "1 h": 60, "4 h": 240 }[label] || null;
  }

  function setView(f, v, commitNow) {
    views[f.id] = v;
    if (f._raf) cancelAnimationFrame(f._raf);
    f._raf = requestAnimationFrame(function () { f._raf = 0; layout(f, v); });
    if (commitNow) commit(f);
  }

  // A short move to the target (or none with reduced motion).
  function animateTo(f, to) {
    var from = viewOf(f);
    var reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) { setView(f, to, true); return; }
    var t0 = performance.now(), dur = 180;
    (function step(t) {
      var k = Math.min(1, (t - t0) / dur), e = 1 - Math.pow(1 - k, 3);
      var v = k < 1 ? { lo: from.lo + (to.lo - from.lo) * e, hi: from.hi + (to.hi - from.hi) * e, follow: 0 } : to;
      views[f.id] = v;
      layout(f, v);
      if (k < 1) requestAnimationFrame(step); else commit(f);
    })(t0);
  }

  // The target of a preset: Day is the day window. 15 min / 1 h / 4 h keep
  // the center of the range; when the range holds now (or is the day), they
  // show the last N minutes and follow now.
  function presetView(f, label) {
    var v = viewOf(f), W = presetMin(label);
    if (W === null) return { lo: fnum(f, "dlo"), hi: fnum(f, "dhi"), follow: 0 };
    var n = nowMin(f);
    if (n !== null && (isDay(f, v) || v.follow || (n >= v.lo && n <= v.hi + 0.5))) return followView(f, W);
    var c = (v.lo + v.hi) / 2;
    return clampView(c - W / 2, c + W / 2, extOf(f));
  }

  function urlFor(f, v) {
    var url = new URL(location.href);
    url.searchParams.delete("zoom");
    url.searchParams.delete("span");
    if (v.follow) {
      var x = Math.round(v.follow * 100) / 100;
      url.searchParams.set("zoom", { 15: "15m", 60: "1h", 240: "4h" }[x] || x + "m");
    } else if (!isDay(f, v)) {
      url.searchParams.set("span", v.lo.toFixed(2) + "-" + v.hi.toFixed(2));
    }
    return url;
  }

  // The URL keeps the range; the lanes come again from the server (caption,
  // totals, bins), and the live updates use the new URL.
  function commit(f) {
    var url = urlFor(f, views[f.id]);
    try { history.replaceState(null, "", url.pathname + url.search); } catch (e) { /* the page still works */ }
    clearTimeout(refetch.timer);
    refetch.timer = setTimeout(function () { lanesFetch(url.href); }, 250);
  }

  function lanesFetch(href) {
    var seq = ++refetch.seq;
    fetch(href, { cache: "no-store", credentials: "same-origin" })
      .then(function (r) { return r.ok ? r.text() : null; })
      .then(function (t) {
        if (!t || seq !== refetch.seq || nav) return;
        var doc = new DOMParser().parseFromString(t, "text/html");
        document.querySelectorAll("[data-lanes][id]").forEach(function (el) {
          var nu = doc.getElementById(el.id);
          if (!nu) return;
          live.src[el.id] = nu.outerHTML;
          swapLanes(el, nu);
        });
        initLanes();
      })
      .catch(function () { /* the next change or live update tries again */ });
  }

  // Swap a lanes figure; keep the focus on the navigator or on the block.
  function swapLanes(el, nu) {
    var a = document.activeElement, focusWin = a && a.classList && a.classList.contains("nwin") && el.contains(a);
    var focusId = a && a.classList && a.classList.contains("fz") && el.contains(a) ? a.getAttribute("data-id") : null;
    hideTip(el);
    var imp = document.importNode(nu, true);
    el.replaceWith(imp);
    if (window.htmx) window.htmx.process(imp);
    if (focusWin) { var w = imp.querySelector(".nwin"); if (w) w.focus({ preventScroll: true }); }
    if (focusId) { var b = imp.querySelector('.fz[data-id="' + focusId + '"]'); if (b) b.focus({ preventScroll: true }); }
    return imp;
  }

  function navMinute(f, x) {
    var bar = f.querySelector(".lnbar").getBoundingClientRect(), ext = extOf(f);
    var fr = Math.max(0, Math.min(1, (x - bar.left) / Math.max(1, bar.width)));
    return { m: ext[0] + fr * (ext[1] - ext[0]), perPx: (ext[1] - ext[0]) / Math.max(1, bar.width), ext: ext };
  }

  document.addEventListener("pointerdown", function (e) {
    if (e.button !== 0 || nav) return;
    var bar = e.target.closest && e.target.closest("[data-nav]");
    var f = bar && bar.closest("[data-lanes]");
    if (!f) return;
    var h = e.target.closest(".nh"), inWin = e.target.closest(".nwin");
    var a = navMinute(f, e.clientX), v0 = viewOf(f);
    // A window that fills the bar cannot move: a drag on it picks a range.
    var full = v0.hi - v0.lo >= a.ext[1] - a.ext[0] - 0.01;
    nav = { f: f, id: e.pointerId, mode: h ? h.dataset.h : inWin && !full ? "move" : "new", x0: e.clientX, v0: v0, m0: a.m, moved: false };
    try { bar.setPointerCapture(e.pointerId); } catch (err) { /* old browsers */ }
    f.classList.add("dragging");
    hideTip(f);
    if (inWin) inWin.focus({ preventScroll: true });
    e.preventDefault();
  });
  document.addEventListener("pointermove", function (e) {
    if (!nav || e.pointerId !== nav.id) return;
    var f = nav.f, a = navMinute(f, e.clientX), v0 = nav.v0, ext = a.ext;
    var dm = (e.clientX - nav.x0) * a.perPx, v;
    if (Math.abs(e.clientX - nav.x0) >= 4) nav.moved = true;
    if (nav.mode === "move") v = clampView(v0.lo + dm, v0.hi + dm, ext);
    else if (nav.mode === "l") v = { lo: Math.max(ext[0], Math.min(v0.lo + dm, v0.hi - MIN_SPAN)), hi: v0.hi, follow: 0 };
    else if (nav.mode === "r") v = { lo: v0.lo, hi: Math.min(ext[1], Math.max(v0.hi + dm, v0.lo + MIN_SPAN)), follow: 0 };
    else if (nav.moved) {
      var lo = Math.min(nav.m0, a.m), hi = Math.max(nav.m0, a.m);
      if (hi - lo < MIN_SPAN) hi = lo + MIN_SPAN;
      v = clampView(lo, hi, ext);
    }
    if (v) setView(f, v, false);
  });
  function navEnd(e, cancel) {
    if (!nav || e.pointerId !== nav.id) return;
    var d = nav, f = d.f;
    nav = null;
    f.classList.remove("dragging");
    var v = views[f.id] || d.v0;
    if (cancel || (!d.moved && d.mode !== "new")) {
      // No move (or Esc): no change.
      views[f.id] = d.v0;
      layout(f, d.v0);
      if (live.dirty) liveLater(300);
      return;
    } else if (d.mode === "new" && !d.moved) {
      // A click on the bar: the range moves there, with the same length.
      var w = d.v0.hi - d.v0.lo;
      v = clampView(d.m0 - w / 2, d.m0 + w / 2, extOf(f));
    }
    setView(f, settle(f, v), true);
    if (live.dirty) liveLater(300);
  }
  document.addEventListener("pointerup", function (e) { navEnd(e, false); });
  // A drag that loses its pointer (the tab is hidden, the capture is lost)
  // must end, or the lanes would never swap again.
  document.addEventListener("lostpointercapture", function (e) { navEnd(e, true); });
  document.addEventListener("pointercancel", function (e) { navEnd(e, true); });
  document.addEventListener("dblclick", function (e) {
    var bar = e.target.closest && e.target.closest("[data-nav]");
    var f = bar && bar.closest("[data-lanes]");
    if (f) animateTo(f, presetView(f, "Day"));
  });

  // The keyboard on the window: arrows move it (a tenth of its length),
  // Shift + arrows make it longer or shorter, Page Up / Down move a full
  // length, Home goes to the start, End goes to now (or the end).
  document.addEventListener("keydown", function (e) {
    var win = e.target.closest && e.target.closest(".nwin");
    var f = win && win.closest("[data-lanes]");
    if (!f) return;
    if (e.key === "Escape" && nav) { navEnd({ pointerId: nav.id }, true); return; }
    var v = viewOf(f), ext = extOf(f), w = v.hi - v.lo, c = (v.lo + v.hi) / 2, nv = null;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      var dir = e.key === "ArrowRight" ? 1 : -1;
      if (e.shiftKey) {
        var nw = Math.max(MIN_SPAN, Math.min(ext[1] - ext[0], dir > 0 ? w * 1.25 : w / 1.25));
        nv = v.follow ? followView(f, Math.max(MIN_SPAN, dir > 0 ? v.follow * 1.25 : v.follow / 1.25)) : clampView(c - nw / 2, c + nw / 2, ext);
      } else nv = clampView(v.lo + dir * w * 0.1, v.hi + dir * w * 0.1, ext);
    } else if (e.key === "PageUp" || e.key === "PageDown") {
      var d2 = e.key === "PageDown" ? w : -w;
      nv = clampView(v.lo + d2, v.hi + d2, ext);
    } else if (e.key === "Home") nv = clampView(ext[0], ext[0] + (v.follow || w), ext);
    else if (e.key === "End") nv = nowMin(f) !== null ? followView(f, v.follow || w) : clampView(ext[1] - w, ext[1], ext);
    else return;
    e.preventDefault();
    if (!nv.follow) nv = settle(f, nv);
    setView(f, nv, true);
  });

  // ---------------------------------------------------------------- lanes
  // Hide the axis labels that the REC label or the playhead covers. Labels
  // just after now come hidden from the server; show them when they are clear.
  function declutter(f) {
    var rec = f.querySelector(".rec");
    if (!rec || rec.hidden) {
      f.querySelectorAll(".tick.hid").forEach(function (t) { t.classList.remove("hid"); });
      return;
    }
    var r = rec.getBoundingClientRect();
    var ph = f.querySelector(".ph");
    var px = ph ? ph.getBoundingClientRect().left + 1 : null;
    f.querySelectorAll(".tick").forEach(function (t) {
      var b = t.getBoundingClientRect();
      if (!b.width) return;
      var hit = (b.right > r.left - 10 && b.left < r.right + 10) ||
        (px !== null && b.left - 6 < px && b.right + 6 > px);
      t.classList.toggle("hid", hit);
    });
  }

  function hms(sec) {
    sec = ((Math.floor(sec) % 86400) + 86400) % 86400;
    var p = function (n) { return String(n).padStart(2, "0"); };
    return p(Math.floor(sec / 3600)) + ":" + p(Math.floor(sec / 60) % 60) + ":" + p(sec % 60);
  }

  // ---------------------------------------------------------------- ago
  // "2 min ago" next to the newest rows, from the server clock (skew).
  var skew = null;
  function agoText(ms) {
    var s = Math.max(0, Math.floor((Date.now() + (skew || 0) - ms) / 1000));
    if (s < 10) return "just now";
    if (s < 60) return s + " s ago";
    if (s < 3600) return Math.floor(s / 60) + " min ago";
    if (s < 86400) return Math.floor(s / 3600) + " h " + String(Math.floor(s / 60) % 60).padStart(2, "0") + " min ago";
    return Math.floor(s / 86400) + " d ago";
  }
  function agoAll() {
    document.querySelectorAll("[data-ago]").forEach(function (e) {
      var t = Date.parse(e.getAttribute("data-ago"));
      if (!isNaN(t)) e.textContent = agoText(t);
    });
  }

  var lastMin = -1;
  function tick() {
    var curMin = Math.floor(Date.now() / 60000);
    document.querySelectorAll("[data-lanes]").forEach(function (f) {
      var c0 = f.dataset.clock0, t0 = +f.dataset.t0;
      if (!c0 || !t0) return;
      var el = (Date.now() - t0) / 1000;
      var parts = c0.split(":");
      var base = (+parts[0]) * 3600 + (+parts[1]) * 60 + (+parts[2]);
      var clock = f.querySelector("[data-clock]");
      if (clock) clock.textContent = hms(base + el);
      // Once a minute: the playhead moves, and a range that follows now
      // moves with it.
      if (curMin !== lastMin && f.classList.contains("zoomable") && !(nav && nav.f === f)) layout(f, viewOf(f));
    });
    lastMin = curMin;
  }
  // A tab left open past midnight asks once for the new day.
  var midnightAsked = false;
  function midnight() {
    var past = false;
    document.querySelectorAll("[data-lanes].zoomable").forEach(function (f) {
      var n = nowMin(f);
      if (n !== null && n > 1441) past = true;
    });
    if (past && !midnightAsked) { midnightAsked = true; liveFetch(); }
    if (!past) midnightAsked = false;
  }

  // ---------------------------------------------------------------- tables
  // A right-edge fade while a wide table can still scroll.
  function fade(tw) {
    tw.classList.toggle("more-r", tw.clientWidth > 0 && tw.scrollLeft + tw.clientWidth < tw.scrollWidth - 4);
  }
  function fadeAll() { document.querySelectorAll(".tw").forEach(fade); }
  document.addEventListener("scroll", function (e) {
    var t = e.target;
    if (t && t.classList && t.classList.contains("tw")) fade(t);
  }, true);
  document.addEventListener("toggle", fadeAll, true);
  window.addEventListener("resize", function () {
    fadeAll();
    document.querySelectorAll("[data-lanes]").forEach(declutter);
  });

  // ---------------------------------------------------------------- table kit
  // The list pages (src/table.rs, templates/tk.html). The server renders the
  // rows in the order of the URL (sort=KEY&dir=asc|desc&q=TEXT). Here a click on
  // a header and the filter box sort and filter the rows in the page and
  // keep the same three parameters in the URL, so a link, a reload and a live
  // update keep the state. A table with data-server (the page loads rows in
  // parts) asks the server for the part again instead. The order is the one
  // of table.rs: numbers by number, text by lower-case text, equal rows by the
  // first column.
  function tkFirstDir(th) { return th.hasAttribute("data-num") ? "desc" : "asc"; }

  function tkRead(t) {
    var u = new URL(location.href).searchParams;
    var ths = t.querySelectorAll("thead th[data-key]");
    var sort = t.getAttribute("data-sort"), dir = t.getAttribute("data-dir");
    var k = u.get("sort");
    if (k && Array.prototype.some.call(ths, function (h) { return h.getAttribute("data-key") === k; })) {
      sort = k;
      dir = u.get("dir") === "asc" || u.get("dir") === "desc" ? u.get("dir") : null;
      if (!dir) { var th0 = t.querySelector('thead th[data-key="' + k + '"]'); dir = tkFirstDir(th0); }
    }
    return { sort: sort, dir: dir, q: (u.get("q") || "").trim() };
  }

  function tkUrl(st) {
    var u = new URL(location.href);
    u.searchParams.set("sort", st.sort);
    u.searchParams.set("dir", st.dir);
    if (st.q) u.searchParams.set("q", st.q); else u.searchParams.delete("q");
    u.searchParams.delete("before");
    u.searchParams.delete("shown");
    return u;
  }

  function tkSave(st) {
    var u = tkUrl(st);
    try { history.replaceState(null, "", u.pathname + u.search); } catch (e) { /* the page still works */ }
    return u;
  }

  function tkLinks(t, st) {
    t.querySelectorAll("thead th[data-key]").forEach(function (th) {
      var key = th.getAttribute("data-key"), a = th.querySelector("a.tks");
      if (!key || !a) return;
      var on = key === st.sort;
      var next = on ? (st.dir === "asc" ? "desc" : "asc") : tkFirstDir(th);
      var u = tkUrl({ sort: key, dir: next, q: st.q });
      a.setAttribute("href", u.pathname + u.search);
      th.setAttribute("aria-sort", on ? (st.dir === "asc" ? "ascending" : "descending") : "none");
      a.querySelector(".arr").textContent = on ? (st.dir === "asc" ? "\u25B2" : "\u25BC") : "";
    });
    var f = t.querySelector("form.tkq");
    if (f) { f.elements.sort.value = st.sort; f.elements.dir.value = st.dir; }
    t.setAttribute("data-sort", st.sort);
    t.setAttribute("data-dir", st.dir);
  }

  function tkNoun(t, n) { return t.getAttribute(n === 1 ? "data-one" : "data-many"); }

  function tkApply(t, st) {
    var body = t.querySelector("tbody");
    var th = t.querySelector('thead th[data-key="' + st.sort + '"]');
    if (!th) return;
    var ci = Array.prototype.indexOf.call(th.parentNode.cells, th), num = th.hasAttribute("data-num");
    var terms = st.q.toLowerCase().split(/\s+/).filter(Boolean);
    var rows = Array.prototype.slice.call(body.rows), shown = 0;
    var val = function (r, i) { var v = r.cells[i].getAttribute("data-v") || ""; return num ? parseFloat(v) || 0 : v; };
    rows.forEach(function (r) {
      var s = r.getAttribute("data-s") || "";
      r.hidden = !terms.every(function (x) { return s.indexOf(x) >= 0; });
      if (!r.hidden) shown++;
    });
    var sign = st.dir === "desc" ? -1 : 1;
    rows.sort(function (a, b) {
      var x = val(a, ci), y = val(b, ci), o = x < y ? -1 : x > y ? 1 : 0;
      if (o) return o * sign;
      var p = a.cells[0].getAttribute("data-v") || "", q = b.cells[0].getAttribute("data-v") || "";
      return p < q ? -1 : p > q ? 1 : 0;
    });
    rows.forEach(function (r) { body.appendChild(r); });
    var all = rows.length, n = t.querySelector("[data-tkn]");
    if (n) n.textContent = (shown === all ? shown + " " + tkNoun(t, shown) : shown + " of " + all + " " + tkNoun(t, all));
    var em = t.querySelector("[data-tkempty]");
    if (em) {
      em.hidden = shown > 0;
      em.textContent = st.q ? "No " + t.getAttribute("data-many") + " match \u201C" + st.q + "\u201D" : "No " + t.getAttribute("data-many");
    }
    tkLinks(t, st);
  }

  var tkSeq = 0, tkTimer = null;
  // The page loads rows in parts: ask the server for the part again.
  function tkReload(t, st) {
    var u = tkSave(st), id = t.id, seq = ++tkSeq;
    fetch(u.href, { cache: "no-store", credentials: "same-origin" })
      .then(function (r) { return r.ok ? r.text() : null; })
      .then(function (txt) {
        if (!txt || seq !== tkSeq) return;
        var nu = new DOMParser().parseFromString(txt, "text/html").getElementById(id);
        var cur = document.getElementById(id);
        if (!nu || !cur) return;
        ["tkw", "tkn", "more", "tknote"].forEach(function (c) {
          var a = cur.querySelector("." + c), b = nu.querySelector("." + c);
          if (a && b) { var imp = document.importNode(b, true); a.replaceWith(imp); if (window.htmx) window.htmx.process(imp); }
          else if (a && !b) a.remove();
          else if (!a && b) cur.appendChild(document.importNode(b, true));
        });
        cur.removeAttribute("data-grown");
        live.src[id] = nu.outerHTML;
        tkLinks(cur, tkRead(cur));
      })
      .catch(function () { /* the next change tries again */ });
  }

  function tkSet(t, st, soon) {
    if (t.hasAttribute("data-server")) {
      tkSave(st);
      tkLinks(t, st);
      clearTimeout(tkTimer);
      tkTimer = setTimeout(function () { tkReload(t, st); }, soon ? 0 : 300);
    } else {
      tkSave(st);
      tkApply(t, st);
    }
  }

  function tkSort(t, key) {
    var st = tkRead(t), th = t.querySelector('thead th[data-key="' + key + '"]');
    if (!th) return;
    st.dir = key === st.sort ? (st.dir === "asc" ? "desc" : "asc") : tkFirstDir(th);
    st.sort = key;
    tkSet(t, st, true);
  }

  document.addEventListener("input", function (e) {
    var inp = e.target;
    if (!inp.classList || !inp.classList.contains("tkf")) return;
    var t = inp.closest("[data-tk]");
    if (!t) return;
    var st = tkRead(t);
    st.q = inp.value.trim();
    tkSet(t, st, false);
  });
  document.addEventListener("submit", function (e) {
    if (e.target.classList && e.target.classList.contains("tkq")) e.preventDefault();
  });
  document.addEventListener("keydown", function (e) {
    var inp = e.target;
    if (e.key === "Escape" && inp.classList && inp.classList.contains("tkf") && inp.value) {
      inp.value = "";
      inp.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  // A click on a row opens the detail page (the name is also a link).
  document.addEventListener("click", function (e) {
    var tr = e.target.closest && e.target.closest(".tkw tbody tr[data-href]");
    if (!tr || e.target.closest("a, button, input, select, summary")) return;
    var sel = window.getSelection && window.getSelection();
    if (sel && String(sel).length) return;
    var href = tr.getAttribute("data-href");
    if (e.metaKey || e.ctrlKey || e.button === 1) window.open(href, "_blank");
    else location.href = href;
  });

  // Apply the URL state to every table: the server did, and this keeps the
  // page right after a live swap or a back navigation.
  function tkInit() {
    document.querySelectorAll("[data-tk]").forEach(function (t) {
      if (t.hasAttribute("data-server")) { tkLinks(t, tkRead(t)); return; }
      tkApply(t, tkRead(t));
    });
  }

  // ---------------------------------------------------------------- live
  // One EventSource (GET /api/v1/live) for the page. The server sends a hint
  // when new events are stored or a client connects or goes offline. Then the
  // page fetches its own URL again (same filters) and swaps each [data-live]
  // part (matched by id) whose server HTML changed. At most one fetch each
  // second, and only while the tab is visible; a hidden tab catches up when
  // it shows again. Without JS (or EventSource) the pages are as before.
  var live = { src: {}, busy: false, dirty: false, last: 0, timer: null, hello: false };

  function liveSnap(root) {
    (root || document).querySelectorAll("[data-live][id]").forEach(function (el) {
      live.src[el.id] = el.outerHTML;
    });
  }

  // Parts that the user changed by hand stay as they are.
  function liveKeep(el) {
    if (el.hasAttribute("data-grown")) return true;
    // A drag on the navigator: swap when it ends.
    if (nav && nav.f === el) { live.dirty = true; return true; }
    var a = document.activeElement;
    return !!(a && el.contains(a) && a.matches("input, select, textarea"));
  }

  function liveSwap(doc) {
    var redraw = false;
    document.querySelectorAll("[data-live][id]").forEach(function (el) {
      var nu = doc.getElementById(el.id);
      if (!nu) return;
      var html = nu.outerHTML;
      if (html === live.src[el.id] || liveKeep(el)) return;
      live.src[el.id] = html;
      var open = Array.prototype.map.call(el.querySelectorAll("details"), function (d) { return d.open; });
      nu.querySelectorAll("details").forEach(function (d, i) { if (open[i]) d.open = true; });
      hideTip(el);
      var tw = el.querySelector(".tkw"), sy = tw ? tw.scrollTop : 0, sx = tw ? tw.scrollLeft : 0;
      var seen = {};
      el.querySelectorAll("tr[data-id], .fz[data-id]").forEach(function (x) { seen[x.getAttribute("data-id")] = 1; });
      var imp = document.importNode(nu, true);
      el.replaceWith(imp);
      if (window.htmx) window.htmx.process(imp);
      markNew(imp, seen);
      var tw2 = imp.querySelector(".tkw");
      if (tw2) { tw2.scrollTop = sy; tw2.scrollLeft = sx; }
      if (imp.querySelector(".chart[data-chart]")) redraw = true;
    });
    if (redraw) drawAll();
    tkInit();
    initLanes();
    fadeAll();
    agoAll();
  }

  // Rows and blocks that came with this update: a highlight, and a "new"
  // chip on the row for one minute.
  function markNew(root, seen) {
    root.querySelectorAll("tr[data-id]").forEach(function (tr) {
      if (seen[tr.getAttribute("data-id")]) return;
      tr.classList.add("arrive");
      var td = tr.querySelector("td");
      if (td && !td.querySelector(".newchip")) {
        var c = add(td, "span", "newchip", "new");
        setTimeout(function () { c.remove(); }, 60000);
      }
    });
    root.querySelectorAll(".fz[data-id]").forEach(function (b) {
      if (!seen[b.getAttribute("data-id")]) b.classList.add("arrive");
    });
  }

  function liveLater(ms) {
    live.dirty = true;
    if (!live.timer) live.timer = setTimeout(function () { live.timer = null; liveFetch(); }, ms);
  }

  function liveFetch() {
    if (document.visibilityState !== "visible") { live.dirty = true; return; }
    var wait = live.last + 1000 - Date.now();
    if (live.busy || wait > 0) { liveLater(Math.max(wait, 200)); return; }
    live.busy = true;
    live.dirty = false;
    live.last = Date.now();
    fetch(location.href, { cache: "no-store", credentials: "same-origin" })
      .then(function (r) { return r.ok ? r.text() : null; })
      .then(function (t) { if (t) liveSwap(new DOMParser().parseFromString(t, "text/html")); })
      .catch(function () { /* the next hint tries again */ })
      .then(function () {
        live.busy = false;
        if (live.dirty) liveLater(200);
      });
  }

  // The stream is open only while the tab is visible. A browser allows 6
  // connections to one host; a stream in every hidden tab used them all and
  // the next page of the dashboard waited for a free one (a hung tab). A tab
  // that shows again catches up with one fetch.
  var es = null;
  function liveOpen() {
    if (es || document.visibilityState !== "visible") return;
    es = new EventSource("/api/v1/live");
    es.addEventListener("events", liveFetch);
    es.addEventListener("client", liveFetch);
    // A second hello means the stream came back (server restart): catch up.
    es.addEventListener("hello", function () { if (live.hello) liveFetch(); live.hello = true; });
  }
  function liveClose() {
    if (!es) return;
    es.close();
    es = null;
    live.dirty = true;
  }
  function liveStart() {
    if (!window.EventSource || !window.fetch || !document.querySelector("[data-live][id]")) return;
    liveOpen();
    document.addEventListener("visibilitychange", function () {
      if (document.visibilityState === "visible") { liveOpen(); if (live.dirty) liveFetch(); }
      else liveClose();
    });
    window.addEventListener("pagehide", liveClose);
    window.addEventListener("pageshow", function (e) { if (e.persisted) { liveOpen(); liveFetch(); } });
  }

  function init() {
    // The server HTML of the live parts, before the scripts below change it.
    liveSnap();
    showTheme(theme());
    showSide();
    drawAll();
    tkInit();
    stepText();
    initLanes();
    fadeAll();
    agoAll();
    setInterval(function () { tick(); midnight(); }, 1000);
    setInterval(agoAll, 15000);
    liveStart();
    // Canvas text does not re-render when the web fonts arrive.
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(function () { drawAll(); initLanes(); });
    if (window.matchMedia) {
      var mq = window.matchMedia("(prefers-color-scheme: light)");
      var on = function () { if (theme() === "auto") drawAll(); };
      if (mq.addEventListener) mq.addEventListener("change", on);
    }
  }

  if (window.__FR_TEST__) window.__FR_TEST__.tickPlan = tickPlan;
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
