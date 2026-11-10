// FM Flight Recorder dashboard: the client lanes (tooltip, arrow keys, REC
// clock), charts (uPlot), hover readout, table sort, theme switch, copy.
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

  // ---------------------------------------------------------------- tables
  function sortTable(btn) {
    var table = btn.closest("table");
    var col = +btn.getAttribute("data-col");
    var body = table.tBodies[0];
    var rows = Array.prototype.slice.call(body.rows);
    var asc = btn.getAttribute("aria-pressed") === "true" && btn.getAttribute("data-dir") === "desc";
    rows.sort(function (a, b) {
      var x = +(a.cells[col].getAttribute("data-v") || 0), y = +(b.cells[col].getAttribute("data-v") || 0);
      return asc ? x - y : y - x;
    });
    rows.forEach(function (r) { body.appendChild(r); });
    table.querySelectorAll(".sortb").forEach(function (b) { b.setAttribute("aria-pressed", "false"); });
    btn.setAttribute("aria-pressed", "true");
    btn.setAttribute("data-dir", asc ? "asc" : "desc");
  }

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
    if (t.id === "theme") {
      setTheme({ dark: "light", light: "auto", auto: "dark" }[theme()] || "dark");
    } else if (t.hasAttribute("data-theme-set")) {
      setTheme(t.getAttribute("data-theme-set"));
    } else if (t.classList.contains("sortb")) {
      sortTable(t);
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
  document.body.addEventListener("htmx:afterSwap", function (e) {
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
      add(tip, "span", "tt", d.t + " · " + d.d);
    }
    var l1 = add(tip, "span", "tl1", "");
    if (d.from) add(l1, "span", "", d.from + " → ");
    l1.appendChild(document.createTextNode(d.layout));
    add(tip, "span", "", d.client + " · " + d.acct + " · " + d.script);
    add(tip, "span", "", "Server " + d.srv + " · Client " + d.cli);
    add(tip, "span", "th", d.bin ? "Enter opens the longest freeze" : "Enter opens the freeze");
    lz.appendChild(tip);
    var lr = lz.getBoundingClientRect(), fr = fz.getBoundingClientRect();
    var left = fr.left - lr.left;
    left = Math.max(0, Math.min(left, lr.width - tip.offsetWidth));
    tip.style.left = left + "px";
    tip.style.top = (fr.bottom - lr.top + 8) + "px";
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
      declutter(f);
    });
  }

  // Hide the axis labels that the REC label covers.
  function declutter(f) {
    var rec = f.querySelector(".rec");
    if (!rec) return;
    var r = rec.getBoundingClientRect();
    f.querySelectorAll(".tick").forEach(function (t) {
      t.classList.remove("hid");
      var b = t.getBoundingClientRect();
      if (b.width && b.right > r.left - 8 && b.left < r.right + 8) t.classList.add("hid");
    });
  }

  function hms(sec) {
    sec = ((Math.floor(sec) % 86400) + 86400) % 86400;
    var p = function (n) { return String(n).padStart(2, "0"); };
    return p(Math.floor(sec / 3600)) + ":" + p(Math.floor(sec / 60) % 60) + ":" + p(sec % 60);
  }

  var lastMin = -1;
  function tick() {
    var nowMin = Math.floor(Date.now() / 60000);
    document.querySelectorAll("[data-lanes]").forEach(function (f) {
      var c0 = f.dataset.clock0, t0 = +f.dataset.t0;
      if (!c0 || !t0) return;
      var el = (Date.now() - t0) / 1000;
      var parts = c0.split(":");
      var base = (+parts[0]) * 3600 + (+parts[1]) * 60 + (+parts[2]);
      var clock = f.querySelector("[data-clock]");
      if (clock) clock.textContent = hms(base + el);
      if (nowMin !== lastMin && f.dataset.f0) {
        var fr = Math.min(1, +f.dataset.f0 + el / 60 / Math.max(1, +f.dataset.win));
        var ph = f.querySelector(".ph");
        if (ph) ph.style.setProperty("--f", fr.toFixed(5));
        var rec = f.querySelector(".rec");
        if (rec) rec.style.left = (fr * 100).toFixed(3) + "%";
        declutter(f);
      }
    });
    lastMin = nowMin;
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

  function init() {
    showTheme(theme());
    drawAll();
    stepText();
    initLanes();
    fadeAll();
    setInterval(tick, 1000);
    // Canvas text does not re-render when the web fonts arrive.
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(function () { drawAll(); initLanes(); });
    if (window.matchMedia) {
      var mq = window.matchMedia("(prefers-color-scheme: light)");
      var on = function () { if (theme() === "auto") drawAll(); };
      if (mq.addEventListener) mq.addEventListener("change", on);
    }
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
