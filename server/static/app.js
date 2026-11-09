// Flight Recorder dashboard: charts (uPlot), hover readout, table sort,
// theme switch, copy. No framework, no build step.
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
      for (var i = 0; i < steps.length; i++) { if (max / steps[i] <= 4) { step = steps[i]; break; } }
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
        line = css("--line"), muted = css("--muted");
    var bars = uPlot.paths.bars({ size: [0.72, 64], gap: 2 });
    var data, series;
    if (spec.kind === "stack") {
      var tot = spec.srv.map(function (s, i) { return s + spec.cli[i]; });
      data = [xs, tot, spec.srv];
      series = [{},
        { label: "Client time", fill: client, stroke: client, width: 0, paths: bars, points: { show: false } },
        { label: "Server wait", fill: server, stroke: server, width: 0, paths: bars, points: { show: false } }];
    } else {
      var c = spec.color === "server" ? server : ink2;
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
        { stroke: muted, grid: { show: false }, ticks: { show: false }, font: "11px ui-monospace, Menlo, monospace",
          splits: function () { return xs; },
          values: function (u, splits) {
            var every = Math.ceil(n / Math.max(1, Math.floor(u.width / 44)));
            return splits.map(function (i) { return i % every === 0 ? spec.labels[i] : ""; });
          } },
        { stroke: muted, size: 48, font: "11px ui-monospace, Menlo, monospace",
          grid: { stroke: line, dash: [3, 3], width: 1 }, ticks: { show: false },
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
  function setTheme(t) {
    var root = document.documentElement;
    if (t === "light" || t === "dark") root.setAttribute("data-theme", t);
    else root.removeAttribute("data-theme");
    try { localStorage.setItem("fr-theme", t); } catch (e) { /* no storage */ }
    var b = document.getElementById("theme");
    if (b) b.textContent = t === "light" ? "Light" : t === "dark" ? "Dark" : "Auto";
    drawAll();
  }

  function theme() {
    var t = "auto";
    try { t = localStorage.getItem("fr-theme") || "auto"; } catch (e) { /* no storage */ }
    return t;
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
      var next = { auto: "light", light: "dark", dark: "auto" }[theme()] || "auto";
      setTheme(next);
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
  document.body.addEventListener("htmx:afterSwap", stepText);

  function init() {
    var b = document.getElementById("theme");
    var t = theme();
    if (b) b.textContent = t === "light" ? "Light" : t === "dark" ? "Dark" : "Auto";
    drawAll();
    stepText();
    if (window.matchMedia) {
      var mq = window.matchMedia("(prefers-color-scheme: dark)");
      var on = function () { if (theme() === "auto") drawAll(); };
      if (mq.addEventListener) mq.addEventListener("change", on);
    }
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
