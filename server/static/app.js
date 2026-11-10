// FM Flight Recorder dashboard: the client lanes (tooltip, arrow keys, zoom,
// REC clock), charts (uPlot), hover readout, table sort, theme switch, copy,
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
    if (t.id === "side-toggle") {
      setSide(!sideRail());
    } else if (t.id === "theme") {
      setTheme({ dark: "light", light: "auto", auto: "dark" }[theme()] || "dark");
    } else if (t.hasAttribute("data-theme-set")) {
      setTheme(t.getAttribute("data-theme-set"));
    } else if (t.hasAttribute("data-zoom") && !e.metaKey && !e.ctrlKey && !e.shiftKey && e.button === 0) {
      e.preventDefault();
      lanesGo(t.getAttribute("href"));
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
    // "Load more" grew the list: live updates leave it alone from now on.
    var tg = e.detail && e.detail.target;
    if (tg && tg.id === "fz-body") {
      var fl = document.getElementById("fz-list");
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
      if (!f.classList.contains("strip")) {
        f.classList.add("zoomable");
        var h = f.querySelector(".zhint");
        if (h) h.hidden = false;
      }
      declutter(f);
    });
  }

  // ---------------------------------------------------------------- zoom
  // The zoom buttons are links (?zoom=1h, ?span=LO-HI). With JS the page
  // fetches the new URL and swaps only the lanes, and keeps the URL, so a
  // live update keeps the zoom. Drag across the lanes to zoom to that span;
  // pinch (Ctrl + wheel) zooms around the pointer.
  function lanesGo(href) {
    var url = new URL(href, location.href);
    try { history.replaceState(null, "", url.pathname + url.search); } catch (e) { location.href = url.href; return; }
    fetch(url.href, { cache: "no-store", credentials: "same-origin" })
      .then(function (r) { return r.ok ? r.text() : null; })
      .then(function (t) {
        if (!t) { location.href = url.href; return; }
        var doc = new DOMParser().parseFromString(t, "text/html");
        document.querySelectorAll("[data-lanes][id]").forEach(function (el) {
          var nu = doc.getElementById(el.id);
          if (!nu) return;
          live.src[el.id] = nu.outerHTML;
          hideTip(el);
          var imp = document.importNode(nu, true);
          el.replaceWith(imp);
          if (window.htmx) window.htmx.process(imp);
        });
        initLanes();
      })
      .catch(function () { location.href = url.href; });
  }

  function spanHref(lo, hi) {
    var url = new URL(location.href);
    url.searchParams.delete("zoom");
    url.searchParams.set("span", lo.toFixed(2) + "-" + hi.toFixed(2));
    return url.pathname + url.search;
  }

  // Minutes of the day under client x, or null outside the track.
  function minuteAt(f, x) {
    var trk = f.querySelector(".lzax .trk");
    if (!trk) return null;
    var r = trk.getBoundingClientRect();
    var lo = +f.dataset.lo, hi = +f.dataset.hi;
    var fr = Math.max(0, Math.min(1, (x - r.left) / Math.max(1, r.width)));
    return { m: lo + fr * (hi - lo), fr: fr, r: r, lo: lo, hi: hi };
  }

  var drag = null;
  document.addEventListener("mousedown", function (e) {
    if (e.button !== 0) return;
    var trk = e.target.closest && e.target.closest(".ln .trk");
    var f = trk && trk.closest("[data-lanes].zoomable");
    if (!f) return;
    var a = minuteAt(f, e.clientX);
    if (!a) return;
    var lz = f.querySelector(".lz");
    var sel = document.createElement("div");
    sel.className = "zsel";
    lz.appendChild(sel);
    drag = { f: f, lz: lz, sel: sel, x0: e.clientX, a: a };
    e.preventDefault();
  });
  document.addEventListener("mousemove", function (e) {
    if (!drag) return;
    var r = drag.a.r, lr = drag.lz.getBoundingClientRect();
    var x0 = Math.max(r.left, Math.min(r.right, drag.x0));
    var x1 = Math.max(r.left, Math.min(r.right, e.clientX));
    drag.sel.style.left = (Math.min(x0, x1) - lr.left) + "px";
    drag.sel.style.width = Math.abs(x1 - x0) + "px";
  });
  document.addEventListener("mouseup", function (e) {
    if (!drag) return;
    var d = drag;
    drag = null;
    d.sel.remove();
    if (Math.abs(e.clientX - d.x0) < 6) return;
    var b = minuteAt(d.f, e.clientX);
    var lo = Math.min(d.a.m, b.m), hi = Math.max(d.a.m, b.m);
    lanesGo(spanHref(lo, hi));
  });

  var pinch = null;
  document.addEventListener("wheel", function (e) {
    if (!e.ctrlKey) return;
    var f = e.target.closest && e.target.closest("[data-lanes].zoomable");
    if (!f) return;
    e.preventDefault();
    var a = minuteAt(f, e.clientX);
    if (!a) return;
    if (!pinch || pinch.f !== f) pinch = { f: f, lo: a.lo, hi: a.hi, t: null };
    var span = pinch.hi - pinch.lo;
    var nu = Math.max(1, Math.min(1440, span * Math.exp(e.deltaY * 0.01)));
    var m = pinch.lo + a.fr * span;
    pinch.lo = Math.max(0, m - a.fr * nu);
    pinch.hi = Math.min(1440, pinch.lo + nu);
    clearTimeout(pinch.t);
    var p = pinch;
    p.t = setTimeout(function () { pinch = null; lanesGo(spanHref(p.lo, p.hi)); }, 250);
  }, { passive: false });

  // Hide the axis labels that the REC label or the playhead covers. Labels
  // just after now come hidden from the server; show them when they are clear.
  function declutter(f) {
    var rec = f.querySelector(".rec");
    if (!rec) return;
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
        if (rec) { rec.style.left = (fr * 100).toFixed(3) + "%"; rec.classList.toggle("rec-l", fr > 0.7); }
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
    if (el.querySelector('.sortb[aria-pressed="true"]')) return true;
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
      var seen = {};
      el.querySelectorAll("tr.fr[data-id], .fz[data-id]").forEach(function (x) { seen[x.getAttribute("data-id")] = 1; });
      var imp = document.importNode(nu, true);
      el.replaceWith(imp);
      if (window.htmx) window.htmx.process(imp);
      markNew(imp, seen);
      if (imp.querySelector(".chart[data-chart]")) redraw = true;
    });
    if (redraw) drawAll();
    initLanes();
    fadeAll();
    agoAll();
  }

  // Rows and blocks that came with this update: a highlight, and a "new"
  // chip on the row for one minute.
  function markNew(root, seen) {
    root.querySelectorAll("tr.fr[data-id]").forEach(function (tr) {
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

  function liveStart() {
    if (!window.EventSource || !window.fetch || !document.querySelector("[data-live][id]")) return;
    var es = new EventSource("/api/v1/live");
    es.addEventListener("events", liveFetch);
    es.addEventListener("client", liveFetch);
    // A second hello means the stream came back (server restart): catch up.
    es.addEventListener("hello", function () { if (live.hello) liveFetch(); live.hello = true; });
    document.addEventListener("visibilitychange", function () {
      if (document.visibilityState === "visible" && live.dirty) liveFetch();
    });
  }

  function init() {
    // The server HTML of the live parts, before the scripts below change it.
    liveSnap();
    showTheme(theme());
    showSide();
    drawAll();
    stepText();
    initLanes();
    fadeAll();
    agoAll();
    setInterval(tick, 1000);
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

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
