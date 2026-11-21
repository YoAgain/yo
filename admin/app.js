// Login: a word on why the last try did not work. Dashboard: the server every three seconds, the hubs every ten.
const $ = (id) => document.getElementById(id);

// <support-list> The support chat's list, pure (admin/test_support.py runs it in node): the installs that wrote to
// the team, "open" (something of theirs not handled yet, the default) or "answered" (nothing open); newest first.
// A thread is a random install id (32 hex, hub schema 9), never an account: shown by its first 6 hex characters.
const installShort = (hex) => String(hex || "").slice(0, 6).toLowerCase();
const supportList = ({ threads, filter = "open" }) => threads
  .filter((t) => /^[0-9a-f]{32}$/i.test(t.install || "") && (filter === "open" ? t.unread > 0 : !(t.unread > 0)))
  .map((t) => { const id = t.install.toLowerCase(); return { id, name: installShort(id), thread: t, open: t.unread || 0, at: t.at || 0 }; })
  .sort((a, b) => b.at - a.at);
// Crash-free installs (estimated), in %: 1 - installs with a crash in the range / phones seen in it. The hub has no
// app start per install id, so the phones seen stand for the installs in use: an estimate; null without a count (an
// older hub) or without phones.
const crashFree = (installs, users) => (installs == null || !users ? null : Math.max(0, 100 - (100 * installs) / users));
// </support-list>
// A crash or error group's title and text from its payload; "expected" ones (known to happen) count but stay hidden.
const diagInfo = (d) => {
  let p = {};
  try { p = JSON.parse(d.payload || "{}") || {}; } catch {}
  const title = d.kind === "crash" ? (p.exception || p.termination || "Crash")
    : [p.domain, p.code, p.place].filter((x) => x != null && x !== "").join(" · ") || "Error";
  const text = [p.details, p.frames, p.message].filter(Boolean).join("\n\n") || d.payload || "";
  return { title, text, expected: p.severity === "expected" };
};
// What a group is about without its build or line (as users.problem_cause): a new build opens no new problem.
const diagCause = (d) => {
  let p = {};
  try { p = JSON.parse(d.payload || "{}") || {}; } catch {}
  const place = String(p.place ?? "").replace(/^\w+\//, "").replace(/:\d+/g, "");
  return JSON.stringify(d.kind === "crash" ? [d.kind, p.exception ?? "", p.termination ?? ""] : [d.kind, p.domain ?? "", p.code ?? "", place]);
};

if (document.body.classList.contains("login")) {
  const q = location.search, note = $("note");
  if (q.includes("wrong")) { note.textContent = "Wrong user or password."; note.hidden = false; }
  if (q.includes("locked")) { note.textContent = "Too many tries. Wait 15 minutes."; note.hidden = false; }
} else {
  // ---- small helpers ----------------------------------------------------------------------------------------------
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const set = (id, text) => { const e = $(id); if (e) e.textContent = text; else console.warn(`no #${id}`); };
  const part = (fn) => { try { fn(); } catch (e) { console.warn(e); } };
  const state = (id, cls) => { $(id).classList.remove("ok", "bad", "warn"); if (cls) $(id).classList.add(cls); };
  // Services in one row: "all running", or the ones down by name; a click opens all four.
  const SVCS = [["t-hub", "Hub"], ["t-dev", "Dev"], ["t-tunnel", "Tunnel"], ["t-cli", "CLI"]];
  const svcSum = () => {
    const n = $("svc-n");
    if (!n) return;
    const down = SVCS.filter(([id]) => $(id)?.classList.contains("bad")).map(([, name]) => name);
    const known = SVCS.some(([id]) => $(id)?.classList.contains("ok") || $(id)?.classList.contains("bad"));
    n.textContent = !known ? "–" : down.length ? `${down.join(", ")} down` : "all running";
    n.className = `cn ${!known ? "" : down.length ? "bad" : "ok"}`.trim();
  };
  new MutationObserver(svcSum).observe(document.querySelector(".svcbar") || document.body, { subtree: true, attributes: true, attributeFilter: ["class"] });
  const bytes = (n) => {
    if (n == null) return "–";
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return `${n.toFixed(n < 10 && i ? 1 : 0)} ${u[i]}`;
  };
  const rate = (n) => (n == null ? "–" : `${bytes(n)}/s`);
  const num = (n) => (n == null ? "–" : Number(n).toLocaleString());
  const span = (s) => {
    s = Math.floor(s);
    const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
    return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
  };
  const date = (ms) => (ms ? new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "–");
  // "5 min", "3 h", "2 days", "3 weeks", "2 months"; the exact time goes in a tooltip.
  const rel = (ms) => {
    if (!ms) return "–";
    const m = Math.max(0, (Date.now() - ms) / 60000), d = m / 1440;
    if (m < 1) return "just now";
    if (m < 60) return `${Math.floor(m)} min`;
    if (d < 1) return `${Math.floor(m / 60)} h`;
    const unit = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
    if (d < 7) return unit(Math.floor(d), "day");
    if (d < 60) return unit(Math.floor(d / 7), "week");
    return unit(Math.floor(d / 30), "month");
  };
  // Last seen counts to the day only (the hub writes it once a day, on purpose).
  const seenDay = (ms) => (!ms ? "–" : Date.now() - ms < 86400000 ? "today" : Date.now() - ms < 2 * 86400000 ? "yesterday" : rel(ms));
  const clock = (ms) => {
    if (!ms) return "";
    const d = new Date(ms), today = new Date().toDateString() === d.toDateString();
    return today ? d.toLocaleTimeString(undefined, { timeStyle: "short" }) : d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  };
  const cell = (tr, text, cls, title) => {
    const td = tr.insertCell();
    if (text instanceof Node) td.append(text); else td.textContent = text ?? "–";
    if (cls) td.className = cls;
    if (title) hint(td, title);
    return td;
  };
  const tag = (text, cls) => el("span", `tag ${cls || ""}`, text);
  // A name or an id in a table: frosted by the Database widget's eye (a span: a cell's own filter is not drawn by
  // every browser). PII_COL: the columns holding names, ids, keys or someone's words.
  const pii = (text) => el("span", "pii", text);
  const PII_COL = /(^|_)(id|uuid|name|spelling|skeleton|handle|username|key|token|hash|text|title|note|payload|body|commitment)s?($|_)/i;
  // A round tick: done or not; a click toggles it (the change is the caller's).
  const tickButton = (done, label, onToggle) => {
    const b = el("button", `tick${done ? " on" : ""}`);
    b.type = "button";
    b.setAttribute("aria-pressed", String(done));
    b.setAttribute("aria-label", label);
    b.title = done ? "done (click to reopen)" : "mark done";
    b.innerHTML = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7"/></svg>';
    b.addEventListener("click", async (e) => {
      e.stopPropagation();
      b.disabled = true;
      try { await onToggle(!done); } finally { b.disabled = false; }
    });
    return b;
  };
  // Initials on a colour picked from the name: the same person, the same colour.
  const avatar = (node, name, team) => {
    node.replaceChildren();
    node.className = node.className.replace(/\bteam\b/, "").trim();
    if (team) { node.classList.add("team"); node.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 6h11M9 12h11M9 18h11M3.5 6l1.2 1.2L7 5M3.5 12l1.2 1.2L7 11M3.5 18l1.2 1.2L7 17"/></svg>'; return node; }
    const words = (name || "?").replace(/[^\p{L}\p{N} ]/gu, " ").trim().split(/\s+/);
    node.textContent = ((words[0] || "?")[0] + (words[1] ? words[1][0] : (words[0] || "")[1] || "")).toUpperCase();
    let h = 0;
    for (const c of name || "") h = (h * 31 + c.codePointAt(0)) % 360;
    node.style.setProperty("--h", h);
    return node;
  };
  const postJSON = async (path, data) => {
    const r = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data) });
    if (!r.ok) throw new Error("Change failed. Try again.");
    return r;
  };

  // ---- the hub shown: live (default) or dev, remembered in this browser; key 0 switches ----------------------------
  let which = "live", last = null, lastDone = new Set(), reservedData = null;
  try { if (localStorage.getItem("hub") === "dev") which = "dev"; } catch {}
  const switchHub = () => {
    which = which === "live" ? "dev" : "live";
    try { localStorage.setItem("hub", which); } catch {}
    showHub();
    liveConnect();
    if (last) render();
  };
  // The logo is the switch: "Yo" for live, "Dev" for dev.
  function showHub() {
    set("hub-word", which === "dev" ? "Dev" : "Yo");
    document.body.classList.toggle("dev", which === "dev");
  }
  showHub();
  $("hub-word").addEventListener("click", switchHub);
  const setRange = (r) => {
    if (r === rangeKey) return;
    rangeKey = r; hourView = r === -1; range = hourView ? 1 : r;
    try { localStorage.setItem("range", String(rangeKey)); } catch {}
    drawRange();
    if (last) render();
  };
  $("range").addEventListener("click", (e) => { const b = e.target.closest("button[data-r]"); if (b) setRange(+b.dataset.r); });

  // ---- server: every three seconds --------------------------------------------------------------------------------
  // The server's last hour (a point a minute, kept by admin.py): bars under CPU, memory, disk and network; blue,
  // orange above 70 %, red above 90 %.
  const serverBars = (all) => {
    // As many bars as Usage shows (7, 30, or 12 months): the hour in as many slices, each its average.
    const n = range === 1 ? 6 : range >= 7 && range <= 30 ? range : 12, size = Math.max(1, Math.ceil(all.length / n)), hist = [];
    for (let i = 0; i < all.length; i += size) {
      const part = all.slice(i, i + size), avg = (k) => part.reduce((t, p) => t + (p[k] || 0), 0) / part.length;
      hist.push([part[part.length - 1][0], avg(1), avg(2), avg(3), avg(4), avg(5)]);
    }
    const names = hist.map(([t]) => clock(t));
    const pctRow = (key, i) => {
      const v = hist.map((p) => p[i] ?? 0);
      spark(key, null, v, names, (k, x) => `${Math.round(x)} %`, undefined, 100);
      [...$(`${key}-spark`).children].forEach((b, k) => b.classList.add(v[k] > 90 ? "hot" : v[k] > 70 ? "warm" : "cool"));
    };
    // CPU, memory, disk: the live value as one gauge (no history), orange above 70 %, red above 90 %.
    const gauge = (key, i) => {
      // The last minute's average, not the newest sample alone (a single sample jumps).
      const t1 = all.length ? all[all.length - 1][0] : 0, win = all.filter((p) => p[0] >= t1 - 60000 && p[i] != null);
      const avg = win.length ? win.reduce((n, p) => n + p[i], 0) / win.length : 0;
      const v = Math.max(0, Math.min(100, avg)), row = $(`c-${key}`);
      $(`${key}-spark`).hidden = true;
      let g = row.querySelector(".gauge");
      if (!g) { g = el("span", "gauge"); g.append(el("span")); row.append(g); }
      g.firstChild.style.width = `${v}%`;
      g.className = `gauge ${v > 90 ? "hot" : v > 70 ? "warm" : "cool"}`;
      sparkData[key] = { values: hist.map((p) => p[i] ?? 0), names, tip: (k, x) => `${Math.round(x)} %`, scale: 100, cls: "cool" };
    };
    gauge("cpu", 1); gauge("mem", 2); gauge("disk", 3);
    spark("net", null, hist.map((p) => (p[4] || 0) + (p[5] || 0)), names, (k) => `↓ ${rate(hist[k][4])} ↑ ${rate(hist[k][5])}`);
    [...$("net-spark").children].forEach((b) => b.classList.add("cool"));
  };
  // A service chip: its dot; state, uptime, memory, restarts and version in its card.
  const svcInfo = {};
  let srvNow = null, updData = null;
  // The tab's title counts only what does not run (a service down), never reports or crash groups.
  const svcDown = {};
  const drawTitle = () => { const n = Object.values(svcDown).filter(Boolean).length; document.title = n ? `(${n} down) Yo admin` : "Yo admin"; };
  const service = (id, u) => {
    u = u || {};
    const ok = u.state === "active";
    svcDown[id] = !ok; drawTitle();
    if ($(`t-${id}`)) state(`t-${id}`, ok ? "ok" : "bad");
    svcInfo[id] = { ok, rows: [
      ["State", ok ? "running" : u.state || "not running", ok ? "ok" : "bad"],
      ["Up", ok && u.since ? span(Date.now() / 1000 - u.since) : "–"],
      ["Memory", u.memory != null ? bytes(u.memory) : "–"],
      ["Restarts", num(u.restarts || 0), u.restarts ? "warn" : ""],
      ...(u.version ? [["Version", u.version.split(" ")[0]]] : []),
    ] };
    if (id === "yogpt") drawCli();
  };
  // CLI: YoGPT and the status bot together, green only while both run (the status bot: on, its last report sent).
  function drawCli() {
    const y = svcInfo.yogpt, b = svcInfo.stbot;
    state("t-cli", !y || !b ? null : y.ok && b.ok ? "ok" : "bad");
    svcInfo.cli = { parts: [["YoGPT", y], ["Status bot", b]], select: true };
  }
  async function tick() {
    if (document.visibilityState !== "visible") return;
    let s;
    try {
      const r = await fetch("/stats.json", { cache: "no-store" });
      if (r.status === 401 || r.status === 403) { location.reload(); return; }
      s = await r.json();
      $("stale").hidden = true;
    } catch { $("stale").hidden = false; return; }
    set("host", s.host || "");
    srvNow = s;
    part(() => serverBars(s.history || []));
    reservedData = s.reserved;
    part(() => {
      const c = s.cpu || {};
      set("cpu-n", c.percent == null ? "–" : `${Math.round(c.percent)} %`);
      $("c-cpu").classList.toggle("hot", c.percent > 90);
      $("c-cpu").dataset.tip = [c.cores ? `${c.cores} cores` : "", c.load?.length ? `load ${c.load.map((x) => x.toFixed(2)).join(" · ")}` : "", s.uptime == null ? "" : `up ${span(s.uptime)}`].filter(Boolean).join(" · ");
    });
    part(() => {
      const m = s.memory;
      if (!m) { set("mem-n", "–"); return; }
      const used = m.total - m.available;
      set("mem-n", `${Math.round((100 * used) / m.total)} %`); $("c-mem").classList.toggle("hot", used / m.total > 0.9);
      $("c-mem").dataset.tip = `${bytes(used)} of ${bytes(m.total)}${m.swap_total ? ` · swap ${bytes(m.swap_total - m.swap_free)}` : ""}`;
    });
    part(() => {
      const d = s.disk;
      if (!d) { set("disk-n", "–"); return; }
      set("disk-n", `${Math.round((100 * d.used) / d.total)} %`); $("c-disk").classList.toggle("hot", d.used / d.total > 0.9);
      $("c-disk").dataset.tip = `of ${bytes(d.total)} · ${bytes(d.free)} free`;
    });
    part(() => {
      const n = s.network || {};
      // Below 100 KB/s in KB/s ("79K"), from there in Mbit/s with one decimal ("1.4M").
      const k = (v) => (v == null ? "–" : v >= 100000 ? `${((v * 8) / 1e6).toFixed(1)}M` : `${Math.round(v / 1000)}K`);
      set("net-n", `↓${k(n.rx_per_s)} ↑${k(n.tx_per_s)}`);
      $("c-net").dataset.tip = "K = KB/s · M = Mbit/s";
    });
    part(() => {
      const r = s.reboot;
      set("reboot-text", r && r.needed ? "reboot needed" : "");
      $("reboot-text").dataset.tip = r && r.needed && r.packages?.length ? r.packages.join(", ") : "";
    });
    part(() => {
      const version = s.hub && s.hub.version;
      service("hub", s.hub);
      set("foot-hub", version ? ` · hub ${version.split(" ")[0]}` : "");
    });
    part(() => service("dev", s.hub_dev));
    part(() => service("tunnel", s.tunnel));
    part(() => {
      service("yogpt", s.yogpt);
      const people = last?.live?.yogpt_contacts;
      if (s.yogpt?.state === "active" && people != null) svcInfo.yogpt.rows.push(["Knows", `${num(people)} people`]);
    });
    if (last) part(drawTables);
    redrawDetail(["server", "svc"]);
  }

  // Updates: a dot in the row (green: all fine, no package updates); a click opens its detail.
  const updates = (u) => {
    const stale = !u || Date.now() / 1000 - u.at > 1800;
    state("t-system", stale ? "warn" : u.status);
    const summary = stale ? "Update data unavailable" : u.apt.error || u.apt.packages == null ? "Package check unavailable"
      : u.apt.packages ? `${u.apt.packages} updates (${u.apt.security} security)` : "Up to date";
    set("upd-n", stale ? "unknown" : u.apt.packages ? `${u.apt.packages} (${u.apt.security} security)${u.reboot ? " · reboot" : ""}` : u.reboot ? "reboot needed" : "up to date");
    state("c-upd", stale ? "warn" : u.status);
    updData = u ? { ...u, summary, stale } : null;
  };

  // ---- app builds: the platform word ("iOS", "Android", "Bot"), green when current, yellow when an update is out,
  // plain when unknown; build and current build only in the tooltip. Worked out by the collector: current is the highest
  // iOS build two users report; a build only one user has is "fresh" (counts as current).
  const build = (app) => (app || "").split("/").pop();
  const parse = (app) => {
    const [p, b] = (app || "").split("/");
    return p && /^\d+$/.test(b || "") ? [p, Number(b)] : null;
  };
  // The hub's minimum (YO_MIN_APP, "ios/…"): a build below it is refused by the hub and must update.
  let minApp = [];
  const findNewest = (h) => { minApp = String(h?.min_app || "").split(",").map((m) => parse(m.trim())).filter(Boolean); };
  const appInfo = (app) => last?.app_builds?.[app] || (app ? { label: build(app), state: null, title: app } : null);
  const belowMin = (app) => { const v = parse(app); return !!v && minApp.some((m) => m[0] === v[0] && v[1] < m[1]); };
  // "fresh", "current", "update" or null (unknown, or no app).
  const appState = (app) => (belowMin(app) ? "update" : appInfo(app)?.state || null);
  const appTitle = (app, uid) => {
    const i = appInfo(app);
    return i ? `${i.title}${belowMin(app) ? " · below the hub's minimum: refused, must update" : ""}` : "";
  };
  const appTag = (app, uid) => {
    const i = appInfo(app), a = appState(app, uid);
    const t = el("span", a ? `app-${a}` : i ? null : "muted", i ? i.label : "–");
    hint(t, appTitle(app, uid));
    return t;
  };

  // ---- small charts (SVG, drawn to the size of their viewBox) ---------------------------------------------------
  const SVG = "http://www.w3.org/2000/svg";
  const svg = (name, attrs) => {
    const e = document.createElementNS(SVG, name);
    for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, v);
    return e;
  };
  const days14 = () => Array.from({ length: 14 }, (_, i) => Math.floor(Date.now() / 86400000) - 13 + i);
  const dayName = (d) => new Date(d * 86400000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
  // The last 24 hours, the current one last: their clock times.
  const hourNames = () => {
    const hour0 = Math.floor(Date.now() / 3600000) - 23;
    return Array.from({ length: 24 }, (_, i) => new Date((hour0 + i) * 3600000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }));
  };
  // One count row: the number, and small bars (one per hour or day, an empty one a faint dash); over the bars, the
  // slot under the pointer or finger shows its time and value in place ("14:00 · 12").
  // CUM: rows whose bars rise with the running total over the range (Media: GB sent so far).
  // The bars of a row in a W x H box: rounded, small gaps, an empty slot a faint dash (also the live strip's).
  const bars = (values, W, H, scale) => {
    const slot = W / values.length, top = scale || Math.max(1, ...values.map((v) => v || 0));
    return values.map((v, i) => {
      const h = v ? Math.max(2, (v / top) * (H - 2)) : 1.5;
      return svg("rect", { x: (i * slot + slot * 0.16).toFixed(1), width: (slot * 0.68).toFixed(1), y: (H - h).toFixed(1), height: h.toFixed(1), rx: 1.2, class: v ? "b" : "b zero" });
    });
  };
  let problemTotals = null;
  const sparkData = {}, FMT = { md: bytes }, CUM = { md: true };
  const spark = (key, big, values, names, tip, title, scale) => {
    if (CUM[key]) { let t = 0; values = values.map((v) => (v == null ? v : (t += v))); }
    if (big !== null) set(`${key}-n`, big === undefined ? "–" : (FMT[key] || num)(big));
    if (title !== undefined) $(`c-${key}`).dataset.tip = title || "";
    $(`${key}-spark`).replaceChildren(...bars(values, 140, 28, scale));
    sparkData[key] = { values, names, tip, scale, fmt: FMT[key] };
  };
  // The one range of Usage and Attention (1, 7 or 30 days, or 0: since the start), kept in this browser; the switch
  // next to the logo.
  // 1H (-1) is 1D plus the last hour in five-minute bars wherever the hub counts per minute (the events); the other
  // charts keep their 1D view, so `range` is 1 then and `hourView` says to use the minutes.
  const RANGES = [-1, 1, 7, 30, 0];
  let rangeKey = 7, range = 7, hourView = false;
  try { const r = localStorage.getItem("range"); if (r !== null && RANGES.includes(+r)) { rangeKey = +r; hourView = rangeKey === -1; range = hourView ? 1 : rangeKey; } } catch {}
  // An event kind's last hour as twelve five-minute counts (null without minutes); names are the slots' end clocks.
  const minuteSlots = (k) => {
    const m = hubNow?.events?.[k]?.minutes;
    if (!hourView || !Array.isArray(m) || m.length < 60) return null;
    const end = Math.floor(Date.now() / 60000) * 60000 + 60000;  // the running minute's end; five minutes each, newest last
    return { values: Array.from({ length: 12 }, (_, b) => m.slice(b * 5, b * 5 + 5).reduce((n, v) => n + (v || 0), 0)), names: Array.from({ length: 12 }, (_, b) => clock(end - (11 - b) * 300000)) };
  };
  const today0 = () => Math.floor(Date.now() / 86400000);
  // A list of the last days' values (today last) as a map by UTC day.
  const year = (arr) => (Array.isArray(arr) ? new Map(arr.map((v, i) => [today0() - arr.length + 1 + i, v])) : new Map());
  // A row in the range: the total with its trend against the range before, and bars (a day each; a month each for a
  // year). out: a second count for the hover. bad: more is worse (Attention: red bars, a rise in red).
  // hours: the last 24 hours (the current one last) for the 1-day range: a bar per four hours, the total of those 24 hours;
  // a row without hours (the hub keeps errors per day only) shows today's count and no bars.
  const rangeRow = (key, perDay, word, tip, out, bad, hours, outHours, total, minutes) => {
    // No bars where there is nothing to draw (a row kept per day only, over the last 24 hours).
    $(`${key}-spark`).toggleAttribute("hidden", range === 1 && !Array.isArray(hours));
    if (hourView && minutes) {
      const ms = minuteSlots(minutes);
      if (ms) {
        $(`${key}-spark`).removeAttribute("hidden");
        spark(key, ms.values.reduce((n, v) => n + v, 0), ms.values, ms.names, (i, v) => `${(FMT[key] || num)(v)}${word ? ` ${word}` : ""}`, `${tip} · last hour, five minutes each`);
        $(`c-${key}`).classList.toggle("bad-up", !!bad);
        return;
      }
    }
    if (range === 1) {
      // Six blocks of four hours, the current one last.
      const h0 = Math.floor(Date.now() / 3600000) - 23, has = Array.isArray(hours);
      const block = (arr) => Array.from({ length: 6 }, (_, b) => arr.slice(-24).slice(b * 4, b * 4 + 4).reduce((n, v) => n + (v || 0), 0));
      const names = Array.from({ length: 6 }, (_, b) => `${clock((h0 + b * 4) * 3600000)} – ${clock((h0 + b * 4 + 4) * 3600000)}`);
      const values = has ? block(hours) : Array(6).fill(null), outs = Array.isArray(outHours) ? block(outHours) : [];
      const now = has ? values.reduce((n, v) => n + v, 0) : perDay?.get(today0()) || 0;
      spark(key, now, values, names, (i, v) => `${(FMT[key] || num)(v)}${word ? ` ${word}` : ""}${out ? ` · ${num(outs[i] || 0)} out` : ""}`, `${tip} · ${has ? "last 24 hours" : "today (per day only)"}`);
      $(`c-${key}`).classList.toggle("bad-up", !!bad);
      return;
    }
    const val = (m, d) => m?.get(d) || 0, sum = (ds, m) => ds.reduce((n, d) => n + val(m, d), 0);
    if (range === 0) {
      // Since the start: from the first day with a count, at most 12 bars of equal length (a week each while that fits).
      const first = Math.min(today0() - 6, ...[...(perDay?.keys() || [])].filter((d) => perDay.get(d) > 0));
      const span = today0() - first + 1, n = Math.min(12, Math.ceil(span / 7)), size = Math.max(7, Math.ceil(span / n));
      const slots = Array.from({ length: Math.ceil(span / size) }, (_, i) => {
        const end = today0() - (Math.ceil(span / size) - 1 - i) * size;
        const ds = Array.from({ length: size }, (_, j) => end - size + 1 + j);
        return [`${dayName(ds[0])} – ${dayName(end)}`, ds];
      });
      const values = slots.map(([, ds]) => sum(ds, perDay)), outs = slots.map(([, ds]) => sum(ds, out));
      const all = total ?? [...(perDay?.values() || [])].reduce((a, v) => a + (v || 0), 0);
      spark(key, all, values, slots.map(([x]) => x), (i, v) => `${(FMT[key] || num)(v)}${word ? ` ${word}` : ""}${out ? ` · ${num(outs[i])} out` : ""}`, `${tip} · since ${dayName(first)}`);
      $(`c-${key}`).classList.toggle("bad-up", !!bad);
      return;
    }
    const days = Array.from({ length: range }, (_, i) => today0() - range + 1 + i), before = days.map((d) => d - range);
    let slots;
    if (range <= 30) slots = days.map((d) => [dayName(d), [d]]);
    else {
      const months = new Map();
      for (const d of days) { const k = new Date(d * 86400000).toLocaleDateString(undefined, { month: "short", year: "2-digit" }); (months.get(k) || months.set(k, []).get(k)).push(d); }
      slots = [...months];
    }
    const values = slots.map(([, ds]) => sum(ds, perDay)), outs = slots.map(([, ds]) => sum(ds, out));
    const now = sum(days, perDay), was = sum(before, perDay);
    spark(key, now, values, slots.map(([n]) => n), (i, v) => `${(FMT[key] || num)(v)}${word ? ` ${word}` : ""}${out ? ` · ${num(outs[i])} out` : ""}`, `${tip} · ${range} days · before: ${(FMT[key] || num)(was)}`);
    const row = $(`c-${key}`);
    row.classList.toggle("bad-up", !!bad);
  };
  // The range's bars as rangeRow draws them: [name, days] each, or for 1 day [name, null, its 4 of the last 24 hours].
  const rangeSlots = (perDay) => {
    if (range === 1) {
      const h0 = Math.floor(Date.now() / 3600000) - 23;
      return Array.from({ length: 6 }, (_, b) => [`${clock((h0 + b * 4) * 3600000)} – ${clock((h0 + b * 4 + 4) * 3600000)}`, null, [0, 1, 2, 3].map((i) => b * 4 + i)]);
    }
    if (range === 0) {
      const first = Math.min(today0() - 6, ...[...(perDay?.keys() || [])].filter((d) => perDay.get(d) > 0));
      const span = today0() - first + 1, size = Math.max(7, Math.ceil(span / Math.min(12, Math.ceil(span / 7)))), n = Math.ceil(span / size);
      return Array.from({ length: n }, (_, i) => {
        const end = today0() - (n - 1 - i) * size, ds = Array.from({ length: size }, (_, j) => end - size + 1 + j);
        return [`${dayName(ds[0])} – ${dayName(end)}`, ds];
      });
    }
    return Array.from({ length: range }, (_, i) => today0() - range + 1 + i).map((d) => [dayName(d), [d]]);
  };
  const slotSum = ([, ds, hs], perDay, hours) => (hs ? hs.reduce((n, i) => n + ((hours || []).slice(-24)[i] || 0), 0)
    : ds.reduce((n, d) => n + (perDay?.get(d) || 0), 0));
  const pctText = (x) => (x == null ? "–" : x > 0 && x < 0.1 ? "< 0.1 %" : `${x >= 99.95 || x < 10 ? x.toFixed(x >= 99.95 || x === 0 ? 0 : 1) : Math.round(x)} %`);
  // Health rows in the counts card, graph only (the value on hover): Delivery (p95 of how long a Yo took to its device),
  // Push (the share the services took), Hub errors (5xx answers), in the range; with nothing counted yet, no bars and
  // "no data yet" in the hover. The rest goes into hovers: Deleted (in the range) and DAU/MAU under Accounts,
  // retention D1/D7/D30 under Yos, crash-free (est.) with Problems.
  // Speed: how long from the hub taking a Yo to the friend's phone having it, in four buckets; the last one is mostly a
  // phone that was offline (the Yo waited at the hub), not a slow hub.
  const LAT = ["latency-1s", "latency-5s", "latency-60s", "latency-slow"];
  const LAT_NAME = ["Within 1 s", "1–5 s", "5–60 s", "Later (phone offline)"];
  let speedData = null, healthData = null, pushData = null;
  const areaSub = {};
  const healthRows = (h, ym, yh) => {
    const span = range === 1 ? "last 24 hours" : range ? `${range} days` : "since the start";
    const all = (key) => ym(key), yo = ym("yo-out"), names24 = hourNames(), slots = range === 1 ? names24.map((n, i) => [n, null, [i]]) : rangeSlots(yo), names = slots.map(([n]) => n);
    const sums = (keys) => slots.map((s) => keys.map((k) => slotSum(s, all(k), yh(k))));
    const whole = (rows) => rows.reduce((a, r) => a.map((v, i) => v + r[i]), rows[0].map(() => 0));
    const none = (key, what) => { sparkData[key].empty = `No data yet: the hub has counted no ${what} in this range`; $(`c-${key}`).dataset.tip = "no data yet"; };
    // Speed: per bar the share of Yos on the friend's phone within 1 s (higher is better); the hover splits them all.
    const lat = sums(LAT), latAll = whole(lat), latN = (b) => b.reduce((a, v) => a + v, 0);
    const fast = (b) => (latN(b) ? (100 * b[0]) / latN(b) : 0);
    spark("dl", null, lat.map(fast), names, (i) => (latN(lat[i]) ? `${num(latN(lat[i]))} Yos · ${pctText(fast(lat[i]))} within 1 s` : "no Yos"),
      latN(latAll) ? `${pctText(fast(latAll))} within 1 s` : "", 100);
    speedData = { lat, all: latAll, names, span, slots };
    if (!latN(latAll)) none("dl", "delivered Yos");
    // Push: the share the services took.
    const ps = sums(["push-ok", "push-fail"]), psAll = whole(ps), share = ([ok, bad]) => (ok + bad ? (100 * ok) / (ok + bad) : null);
    spark("ps", null, ps.map((p) => share(p) ?? 0), names,
      (i) => (ps[i][0] + ps[i][1] ? `${pctText(share(ps[i]))} taken · ${num(ps[i][1])} failed of ${num(ps[i][0] + ps[i][1])}` : "no pushes"),
      `Push: taken by APNs and FCM · ${pctText(share(psAll))} · ${num(psAll[1])} failed (in Errors) · ${span}`, 100);
    if (!(psAll[0] + psAll[1])) none("ps", "pushes");
    // Push pacing (spec 6.7): kept for the card inside Sent's detail.
    pushData = { sent: ps.map(([a, b]) => a + b), folded: sums(["push-folded"]).map(([n]) => n), paced: sums(["push-paced"]).map(([n]) => n), names, span };
    // Hub errors: 5xx answers per bar (a bar's height its count); their share, 4xx and 429 in the hover.
    const he = sums(["http-5xx", "http", "http-4xx", "http-429"]), heAll = whole(he), rate = ([e, n]) => (n ? (100 * e) / n : 0);
    spark("he", null, he.map(([e]) => e), names,
      (i) => (he[i][1] ? `${num(he[i][0])} 5xx (${pctText(rate(he[i]))}) · ${num(he[i][2])} 4xx · ${num(he[i][3])} 429 of ${num(he[i][1])}` : "no answers"),
      `Hub errors: answers that were 5xx · ${num(heAll[0])} (${pctText(rate(heAll))}) of ${num(heAll[1])} · ${span}`);
    if (!heAll[1]) none("he", "answers");
    $("c-he").classList.add("bad-up");
    // Accounts' detail: DAU/MAU (users seen today of those seen in 30 days) and the accounts deleted in the range.
    const gone = sums(["account-gone"]).reduce((a, [n]) => a + n, 0);
    const dau = h.active_day ?? 0, mau = h.active_month ?? 0, stick = mau ? (100 * dau) / mau : null;
    areaSub.uc = [["DAU / MAU", `${pctText(stick)}`, stick == null ? null : stick / 100], [`Deleted, ${span}`, num(gone), null]];
    // Yos' detail: retention, of the accounts made 1 (7, 30) to 90 days ago those seen that many days after signing up.
    const rt = h.retention || {};
    areaSub.yc = ["d1", "d7", "d30"].map((k) => [`Retention ${k.toUpperCase()}`, rt[k]?.[1] ? pctText((100 * rt[k][0]) / rt[k][1]) : "–", rt[k]?.[1] ? rt[k][0] / rt[k][1] : null]);
    // Problems' detail: crash-free installs (est.) in the range, the installs with a crash against the phones seen in it.
    const cf = h.crash_free?.[String(range)] || null, crashInstalls = cf ? cf[0] : null, seen = cf ? cf[1] : 0;
    const pi = h.problem_installs?.[String(range)] || null, days = { 1: 2, 7: 7, 30: 30, 0: 90 }[range], from = today0() - days + 1;
    const reports = (kind) => (h.diagnostic_days || []).filter((r) => r.day >= from && (!kind || r.kind === kind)).reduce((n, r) => n + r.count, 0);
    problemTotals = { reports: reports(), installs: pi ? pi[1] : null, seen, errInstalls: pi ? pi[0] : null, crashInstalls, platforms: pi ? [["iOS", pi[2], pi[3], pi[4]], ["Android", pi[5], pi[6], pi[7]]] : [] };
    healthData = { span, ps, psAll, he, heAll, names, crashInstalls, seen, free: crashFree(crashInstalls, seen), gone, dau, mau, stick, heRate: rate(heAll) };
  };
  // The two chart widgets: a running total over the range (a point an hour, a day, or for Total up to 60 points),
  // ending at the total since the start; a click opens their detail.
  const areaData = {};
  const runningTotal = (perDay, hours, total, kind) => {
    let names, incs;
    const ms = kind && minuteSlots(kind);
    if (ms) { names = ms.names; incs = ms.values; }
    else if (range === 1) {
      const h0 = Math.floor(Date.now() / 3600000) - 23;
      names = Array.from({ length: 24 }, (_, i) => clock((h0 + i + 1) * 3600000));
      incs = Array.isArray(hours) ? hours.slice(-24).map((v) => v || 0) : Array(24).fill(0);
    } else {
      const first = range ? today0() - range + 1 : Math.min(today0() - 6, ...[...(perDay?.keys() || [])].filter((d) => perDay.get(d) > 0));
      const span = today0() - first + 1, step = Math.max(1, Math.ceil(span / 60)), n = Math.ceil(span / step);
      const ends = Array.from({ length: n }, (_, i) => today0() - (n - 1 - i) * step);
      names = ends.map(dayName);
      incs = ends.map((e) => { let t = 0; for (let d = e - step + 1; d <= e; d++) t += perDay?.get(d) || 0; return t; });
    }
    const values = []; let v = total;
    for (let i = incs.length - 1; i >= 0; i--) { values[i] = v; v -= incs[i]; }
    return { names, incs, values: [Math.max(0, v), ...values], labels: [ms ? "1 hour ago" : range === 1 ? "24 hours ago" : "before", ...names] };
  };
  // The hover popup of every chart: the slot under the pointer with its values (text "slot · value · value"; the first
  // line bold), beside the pointer; mark(i) highlights that slot in the chart (-1 when the pointer leaves).
  const tipBox = el("div", "ctip"), bigBox = el("div", "ctip bigview");
  tipBox.hidden = bigBox.hidden = true;
  document.body.append(tipBox, bigBox);
  // A popup row: the series' colour (a swatch) or a symbol, what it is, its value.
  const tipRow = ({ ico, color, label, value, cls, section, line }) => {
    if (section) return el("div", "tsec", section);
    const r = el("div", `tr${cls ? ` ${cls}` : ""}`), mark = el("span", "tm");
    if (color) { mark.classList.add("sw"); if (line) mark.classList.add("ln"); mark.style.background = color; } else if (ico) mark.append(icon(ico));
    r.append(mark, el("span", "tl", label), el("b", "tv", value));
    return r;
  };
  const SERIES = { "": "var(--ok)", cool: "#5dade2", bad: "#e67e22", acc: "var(--amethyst-soft)" };
  const slotWord = () => (range === 1 ? "This hour" : range ? "This day" : "This stretch");
  const showTip = (t, x, y) => {
    const held = tipBox.querySelector("#st-every");  // the status bot's select goes home before the card changes
    if (held) { held.hidden = true; $("t-system").append(held); }
    if (typeof t === "string") { const [head, ...rest] = t.split(" · "); tipBox.replaceChildren(el("b", "th", head), ...rest.map((x) => el("span", null, x))); }
    else tipBox.replaceChildren(el("b", t.cap ? "th cap" : "th", t.head), ...(t.note ? [el("span", "tn", t.note)] : []), ...(t.nodes || []), ...(t.rows || []).filter(Boolean).map(tipRow));
    tipBox.hidden = false;
    const w = tipBox.offsetWidth, h = tipBox.offsetHeight;
    tipBox.style.left = `${Math.min(innerWidth - w - 8, Math.max(8, x - w / 2))}px`;
    tipBox.style.top = `${y - h - 14 < 8 ? y + 18 : y - h - 14}px`;
  };
  // An element that explains itself: the chart popup instead of a native title; on hover, keyboard focus and tap.
  // t: a text ("head · line · line") or { head, note, rows }.
  // The card a click or tap pins open (a row with something to operate in it); it closes on a click outside or Escape.
  let pinned = null;
  const unpin = () => { pinned = null; tipBox.classList.remove("pin"); tipBox.hidden = true; };
  const hint = (node, t, pin) => {
    if (!t) return node;
    const tv = () => (typeof t === "function" ? t() : t);
    const at = () => { const r = node.getBoundingClientRect(); showTip(tv(), r.left + r.width / 2, r.top); };
    node.tabIndex = node.tabIndex < 0 ? 0 : node.tabIndex;
    const mine = () => !pinned || pinned === node;
    node.addEventListener("pointermove", (e) => { if (e.pointerType === "mouse" && !pinned && !e.target.closest?.("svg.spark")) showTip(tv(), e.clientX, e.clientY); });
    node.addEventListener("pointerleave", (e) => { if (!pinned && e.pointerType === "mouse") tipBox.hidden = true; });
    onTap(node, () => { if (mine()) at(); });
    node.addEventListener("focus", () => { if (!pinned) at(); });
    node.addEventListener("blur", () => { if (!pinned) tipBox.hidden = true; });
    node.addEventListener("click", () => { if (pin && !pinned) { at(); pinned = node; tipBox.classList.add("pin"); } });
    return node;
  };
  document.addEventListener("pointerdown", (e) => { if (pinned && !tipBox.contains(e.target) && !pinned.contains(e.target)) unpin(); }, true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pinned) unpin(); });
  // The server card's rows explain themselves in the hover card: CPU, memory and LAN with their history over the range
  // (the bars the detail cards drew), disk with its sizes, updates and services with what they hold.
  const serverSeries = (col) => {
    const s = srvNow || {}, now = Date.now();
    // One point a day for 7 and 30 days (and Total): its day number stands for its UTC midnight.
    const [pts, size, count] = hourView ? [s.history || [], 300000, 12] : range === 1 ? [s.history_day || [], 3600000, 24]
      : [(s.history_days || []).map((p) => [p[0] * 86400000, ...p.slice(1)]), 86400000, range === 7 ? 7 : 30];
    const first = Math.floor(now / size) - count + 1, sums = Array.from({ length: count }, () => [0, 0]);
    for (const p of pts) { const k = Math.floor(p[0] / size) - first; if (k >= 0 && k < count) { sums[k][0] += col(p); sums[k][1]++; } }
    return { values: sums.map(([t, n]) => (n ? t / n : 0)), names: sums.map((_, k) => (size === 86400000 ? dayName(first + k) : clock((first + k + 1) * size))) };
  };
  const histNode = (col, fmt) => {
    const { values, names } = serverSeries(col), g = sparkBars(values, "cool", null);
    return [g, tipRow({ label: `${names[0]} – ${names[names.length - 1]}`, value: fmt(values[values.length - 1]) })];
  };
  const serverHints = () => {
    const pct = (v) => `${Math.round(v)} %`;
    hint($("c-cpu"), () => { const s = srvNow || {}, c = s.cpu || {};
      return { cap: 1, head: "CPU", nodes: histNode((p) => p[1] ?? 0, pct), rows: [c.cores ? { label: "Cores", value: String(c.cores) } : null, c.load?.length ? { label: "Load", value: c.load.map((x) => x.toFixed(2)).join(" · ") } : null, s.uptime == null ? null : { label: "Up", value: span(s.uptime) }] }; });
    hint($("c-mem"), () => { const m = (srvNow || {}).memory, used = m ? m.total - m.available : 0;
      return { cap: 1, head: "Memory", nodes: histNode((p) => p[2] ?? 0, pct), rows: m ? [{ label: "Used", value: `${bytes(used)} of ${bytes(m.total)}` }, m.swap_total ? { label: "Swap", value: bytes(m.swap_total - m.swap_free) } : null] : [] }; });
    hint($("c-disk"), () => { const d = (srvNow || {}).disk, du = d ? d.used / d.total : 0;
      return { cap: 1, head: "Disk", nodes: d ? [shareBar(du, du > 0.9 ? "must" : du > 0.7 ? "update" : "cool")] : [], rows: d ? [{ label: "Used", value: bytes(d.used) }, { label: "Free", value: bytes(d.free) }, { label: "Size", value: bytes(d.total) }] : [{ label: "Disk", value: "unknown" }] }; });
    hint($("c-net"), () => { const n = (srvNow || {}).network || {};
      return { cap: 1, head: "LAN", nodes: histNode((p) => (p[4] || 0) + (p[5] || 0), (v) => `${bytes(v)}/s`), rows: [{ label: "Down", value: rate(n.rx_per_s) }, { label: "Up", value: rate(n.tx_per_s) }] }; });
    hint($("c-upd"), () => { const u = updData;
      if (!u) return { cap: 1, head: "Updates", rows: [{ label: "Collector", value: "unavailable" }] };
      const kernel = !!(u.installed_kernel && u.running_kernel && u.installed_kernel !== u.running_kernel), lts = !!(u.release?.result && !/no new release/i.test(u.release.result));
      const yn = (b) => (b ? "yes" : "no");
      return { cap: 1, head: "Updates", rows: [{ label: "Updates", value: u.apt.packages ? `${num(u.apt.packages)}${u.apt.security ? ` (${num(u.apt.security)} security)` : ""}` : "none" },
        { label: "Reboot needed", value: yn(u.reboot) }, { label: "Kernel update", value: yn(kernel) }, { label: "New Ubuntu LTS", value: yn(lts) }] }; });
    hint($("c-svc"), () => {
      const rows = [["hub", "Hub"], ["dev", "Dev"], ["tunnel", "Tunnel"], ["yogpt", "YoGPT"], ["stbot", "Status bot"]].map(([id, name]) => {
        const i = svcInfo[id], r = (k) => i?.rows?.find(([n]) => n === k)?.[1], up = r("Up"), row = el("div", "tr"), mark = el("span", "tm");
        mark.append(el("span", `dot ${!i ? "" : i.ok ? "ok" : "bad"}`));
        row.append(mark, el("span", "tl", name), el("b", "tv", !i ? "unknown" : i.ok ? (up && up !== "–" ? up : "running") : r("State") || "off"));
        return row;
      });
      const sel = $("st-every"), every = el("label", "hc-sel", "Status bot every");
      sel.hidden = false; every.append(sel);
      return { cap: 1, head: "Services", nodes: [...rows, every] };
    }, true);
  };
  document.addEventListener("pointerdown", (e) => { if (e.pointerType !== "mouse" && !pinned) tipBox.hidden = true; }, true);
  // On touch a card shows on a TAP only (a press that lifts without moving more than 8 px), never while a finger moves:
  // the page scrolls freely (touch-action: pan-y), and any scroll closes the cards. A mouse hovers as before.
  const onTap = (node, show) => {
    let down = null;
    node.addEventListener("pointerdown", (e) => { if (e.pointerType !== "mouse") down = [e.clientX, e.clientY]; });
    node.addEventListener("pointermove", (e) => { if (down && Math.hypot(e.clientX - down[0], e.clientY - down[1]) > 8) down = null; });
    node.addEventListener("pointercancel", () => { down = null; });
    node.addEventListener("pointerup", (e) => { if (down && e.pointerType !== "mouse") { down = null; show(e); } });
  };
  const hoverTip = (node, count, text, mark) => {
    const show = (e) => {
      const n = typeof count === "function" ? count() : count, r = node.getBoundingClientRect();
      if (!n || !r.width) return;
      const i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n))), t = text(i);
      if (!t) { tipBox.hidden = true; return; }
      showTip(t, e.clientX, e.clientY);
      mark?.(i);
    };
    node.addEventListener("pointermove", (e) => { if (e.pointerType === "mouse") show(e); });
    onTap(node, show);
    node.addEventListener("pointerleave", (e) => { if (e.pointerType === "mouse") { tipBox.hidden = true; mark?.(-1); } });
  };
  window.addEventListener("scroll", () => { if (!pinned) tipBox.hidden = true; if (!bigBox.hidden) bigBox.hidden = true; }, { passive: true, capture: true });
  // The painter of every running-total chart (the overview's two and the details'): bars (each slot's count, between
  // the line's points when there is one point more, else under them; up to half the height under a line), the line with
  // its dots over them, the first and last slot as the axis. Sized to its svg in pixels, so text stays text-sized.
  const paintCombo = (node, { line, bars, from, to, max, pick = -1, hover = -1 }) => {
    const W = node.clientWidth, H = node.clientHeight;
    if (!W || !H) return;
    const pad = 6, padTop = 14, B = H - 16, between = line && bars && bars.length === line.length - 1;
    const slots = bars ? bars.length : line.length, np = line ? line.length : 0, gap = (W - 2 * pad) / Math.max(1, slots);
    const px = (i) => pad + (between ? i : i + 0.5) * gap, out = [];
    if (bars) {
      const top = max || Math.max(1, ...bars.map((v) => v || 0)), room = (B - padTop) * (line ? 0.55 : 1);
      bars.forEach((v, i) => {
        const h = v ? Math.max(2, (v / top) * room) : 1.5;
        out.push(svg("rect", { x: (pad + i * gap + gap * 0.15).toFixed(1), width: (gap * 0.7).toFixed(1), y: (B - h).toFixed(1), height: h.toFixed(1), rx: 1.5,
          class: !v ? "cb zero" : i === pick || (pick < 0 && i === bars.length - 1) ? "cb now" : "cb" }));
      });
    }
    if (line) {
      const have = line.filter((v) => v != null), lo = Math.min(...have), hi = Math.max(...have);
      const y = (v) => padTop + (B - padTop - 3) * (hi === lo ? 0.5 : 1 - (v - lo) / (hi - lo));
      const pts = line.map((v, i) => (v == null ? null : `${px(i).toFixed(1)},${y(v).toFixed(1)}`)).filter(Boolean);
      if (pts.length > 1) out.push(svg("path", { class: "line", d: `M${pts.join(" L")}` }));
      const every = Math.max(1, Math.ceil(np / 16));
      line.forEach((v, i) => { if (v != null) out.push(svg("circle", { class: i % every && i !== np - 1 && i !== pick ? "dot hide" : "dot", cx: px(i).toFixed(1), cy: y(v).toFixed(1), r: 2.6 })); });
    }
    if (hover >= 0) { const hx = (bars ? pad + hover * gap + gap / 2 : px(hover)).toFixed(1); out.push(svg("line", { x1: hx, x2: hx, y1: padTop - 6, y2: B, class: "hmark" })); }
    const ax = (tx, text, anchor) => { const t = svg("text", { x: tx, y: H - 2, "text-anchor": anchor, class: "ax" }); t.textContent = text || ""; return t; };
    node.setAttribute("viewBox", `0 0 ${W} ${H}`);
    node.replaceChildren(...out, ax(pad, from, "start"), ax(W - pad, to, "end"));
  };
  const drawArea = (key) => {
    const d = areaData[key], node = $(`${key}-svg`);
    if (!d || !node) return;
    const W = node.clientWidth, H = node.clientHeight;
    if (!W || !H) return;
    // A second series: on its own scale (Active over Accounts) or on the same one (Media under Yos, their ratio).
    const o = d.overlay && areaData[d.overlay], same = o && SAME[key];
    // Room above the curve's top (14 px, its max there).
    const combo = COMBO[key], foot = combo ? 16 : 0;
    const both = same ? [...d.values, ...o.values] : d.values, lo = Math.min(...both), hi = Math.max(...both), pad = 6, padTop = 14, n = d.values.length, B = H - foot;
    const x = (i) => pad + (i * (W - 2 * pad)) / Math.max(1, n - 1), y = (v) => padTop + (B - padTop - 3) * (hi === lo ? 0.5 : 1 - (v - lo) / (hi - lo));
    const pts = d.values.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`);
    const fill = svg("path", { class: "fill", d: `M${x(0)},${B} L${pts.join(" L")} L${x(n - 1)},${B} Z` });
    const line = svg("path", { class: "line", d: `M${pts.join(" L")}` });
    const every = Math.max(1, Math.ceil(n / 16));
    const dots = d.values.map((v, i) => svg("circle", { class: i % every && i !== n - 1 ? "dot hide" : "dot", cx: x(i).toFixed(1), cy: y(v).toFixed(1), r: 2.6 }));
    // A second series on its own scale, drawn as a plain line (Active over Accounts).
    const extra = [];
    if (o) {
      // Per slot (one value per point after "before") it starts one point in; else across the width.
      const olo = same ? lo : Math.min(...o.values), ohi = same ? hi : Math.max(...o.values), on = o.values.length, shift = on === n - 1 ? 1 : 0;
      const op = o.values.map((v, i) => `${(shift ? x(i + 1) : pad + (i * (W - 2 * pad)) / Math.max(1, on - 1)).toFixed(1)},${(padTop + (B - padTop - 3) * (ohi === olo ? 0.5 : 1 - (v - olo) / (ohi - olo))).toFixed(1)}`);
      extra.push(svg("path", { class: "line overlay", d: `M${op.join(" L")}` }));
    }
    node.setAttribute("viewBox", `0 0 ${W} ${H}`);
    if (combo) {
      const hv = areaHover[key] ?? -1;
      paintCombo(node, { line: d.values, bars: d.incs, pick: hv, hover: hv, from: d.labels?.[0] === "1 hour ago" ? "1 hour ago" : range === 1 ? "24 hours ago" : d.names[0], to: range === 1 ? "now" : d.names[d.names.length - 1] });
      return;
    }
    // No numbers in the plot: the total is the title's, the time is on the axis under the bars.
    node.replaceChildren(fill, line, ...extra, ...dots);
  };
  // A second series drawn over a chart: Active over Accounts, Media over Yos (on the Yos' scale).
  const OVERLAY = {}, SAME = {}, COMBO = { yc: true, uc: true }, WORD = { yc: "Yos", uc: "accounts" };
  // Each curve's legend, under it: what its two lines are.
  const LEGEND = { yc: [["Total", "l-main"], ["Sent", "l-bar"]], uc: [["Total", "l-main"], ["Sign-ups", "l-bar"]] };
  const areaTips = {}, areaHover = {};
  let sentMode = (() => { try { return localStorage.getItem("sentMode") === "delivered" ? "delivered" : "sent"; } catch { return "sent"; } })();
  // The Sent card's pill: ↑ sent, ↓ delivered.
  const drawSentPill = () => {
    const head = $("t-yc").querySelector(".gh");
    head.querySelector(".pill-toggle")?.remove();
    head.children[1].textContent = sentMode === "sent" ? "Sent" : "Delivered";
    head.children[1].after(pillOf([["sent", "↑", "Sent"], ["delivered", "↓", "Delivered"]], sentMode, (v) => {
      sentMode = v; try { localStorage.setItem("sentMode", v); } catch {}
      if (last) render();
    }));
  };
  const areaChart = (key, total, perDay, hours, today, kind) => {
    set(`${key}-n`, num(total));
    if (!areaTips[key] && $(`${key}-svg`)) {
      areaTips[key] = true;
      hoverTip($(`${key}-svg`), () => areaData[key]?.incs?.length || 0, (i) => {
        const d = areaData[key], what = { yc: sentMode === "sent" ? "sent" : "delivered", uc: "signed up" }[key] || "";
        const c = key === "uc" ? "var(--amethyst-soft)" : "var(--ok)";
        return d ? { head: d.names[i], rows: [{ section: slotWord() }, { color: c, label: what[0].toUpperCase() + what.slice(1), value: num(d.incs[i]) },
          { section: "In all" }, { color: c, line: true, label: "Total", value: num(d.values[i + 1]) }] } : "";
      }, (i) => { areaHover[key] = i; drawArea(key); });
    }
    areaData[key] = { ...runningTotal(perDay, hours, total || 0, kind), overlay: OVERLAY[key] || null, word: WORD[key] };
    drawArea(key);
    // Under the chart, its activity as bars: per hour (1 day), per day (7, 30), per stretch of days (Total).
    const st = $(`${key}-strip`), inc = areaData[key].incs, top = Math.max(1, ...inc), sl = 140 / inc.length;
    if (!st) return;
    if (COMBO[key]) {
      st.style.display = "none";
      // The legend only on hover, over the chart's top left; today and the total are in the title.
      let legend = $(`${key}-area`).querySelector(".cfoot");
      if (!legend?.classList.contains("cfoot")) {
        legend = el("div", "cfoot");
        for (const [t, c] of LEGEND[key] || []) legend.append(el("span", `key ${c}`, t));
        $(`${key}-area`).append(legend);
      }
      set(`${key}-t`, num(today ?? 0));
      return;
    }
    const bars = inc.map((v, i) => {
      const h = v ? Math.max(2, (v / top) * 26) : 1.5;
      return svg("rect", { x: (i * sl + sl * 0.16).toFixed(2), width: (sl * 0.68).toFixed(2), y: (28 - h).toFixed(1), height: h.toFixed(1), rx: 1, class: v ? "b" : "b zero" });
    });
    st.replaceChildren(...bars);
    const names = areaData[key].names, what = { yc: "Yos", uc: "sign-ups" }[key] || "";
    // Two charts, each three parts: the curve with its legend under it, then the bars under a heading of their own
    // (what they count, today's count since UTC midnight on the right) with the first and last slot as their axis.
    let box = st.parentElement;
    if (!box.classList.contains("stripbox")) {
      box = el("div", "stripbox"); st.replaceWith(box); box.append(st, el("span", "sfrom"), el("span", "sto"));
      const legend = el("div", "cfoot");
      for (const [t, c] of LEGEND[key] || []) legend.append(el("span", `key ${c}`, t));
      $(`${key}-area`).after(legend);
      const head = el("div", "shead");
      head.append(el("span", null, { yc: "Sent", uc: "Sign-ups" }[key] || ""), el("b"));
      box.before(head);
    }
    box.previousElementSibling.querySelector("b").textContent = `${num(today ?? 0)} today`;
    box.querySelector(".sfrom").textContent = range === 1 ? clock((Math.floor(Date.now() / 3600000) - 23) * 3600000) : names[0] || "";
    box.querySelector(".sto").textContent = range === 1 ? "now" : names[names.length - 1] || "";
    for (const [k, d] of Object.entries(areaData)) if (d?.overlay === key) drawArea(k);
  };
  // Active: one value per day, drawn over Accounts.
  const seriesChart = (key, series, headline) => {
    areaData[key] = series;
    if (headline != null) set(`${key}-n`, headline);
    drawArea(key);
    for (const [k, d] of Object.entries(areaData)) if (d?.overlay === key) drawArea(k);
  };
  // Problems row: marked while tickets are open (unanswered support, open crash groups); its count is the graph's.
  let openTickets = 0, openSupport = 0, platData = null;
  // Problems: a red count of the support conversations still waiting for an answer, before its graph.
  const showOpen = () => {
    const row = $("c-pr");
    row.classList.toggle("has-open", openTickets > 0);
    let badge = row.querySelector(".pr-open");
    if (!badge) { badge = el("span", "pr-open"); row.querySelector(".cl")?.append(badge); }
    badge.textContent = openSupport ? String(openSupport) : "";
    badge.hidden = !openSupport;
  };
  const charts = ["yc", "uc"];
  if (window.ResizeObserver) { const ro = new ResizeObserver(() => charts.forEach(drawArea)); charts.forEach((k) => ro.observe($(`${k}-area`))); }
  const drawRange = () => {
    for (const b of $("range").querySelectorAll("button[data-r]")) { b.classList.toggle("on", +b.dataset.r === rangeKey); b.setAttribute("aria-pressed", String(+b.dataset.r === rangeKey)); }
    $("range").classList.toggle("hourly", hourView);
  };
  const last24 = (v) => (Array.isArray(v) ? v.slice(-24) : Array(24).fill(null));
  const last14 = (v) => (Array.isArray(v) ? v.slice(-14) : Array(14).fill(null));

  // ---- overview: the newest users, a name with small icons (platform coloured by its build, a lock when secured) --
  const ICONS = {
    ios: '<rect x="6.5" y="2.5" width="11" height="19" rx="2.5"/><path d="M11 18.5h2"/>',
    android: '<path d="M5 17V11a7 7 0 0 1 14 0v6z"/><path d="M8 5 6.5 3M16 5l1.5-2"/><path d="M9.5 10v.5M14.5 10v.5"/>',
    yogpt: '<rect x="4" y="7" width="16" height="12" rx="3"/><path d="M12 7V4"/><path d="M9 12.5v1M15 12.5v1"/>',
    cli: '<rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="m7 10 3 2.5L7 15"/><path d="M12.5 15h4"/>',
    lock: '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>',
  };
  const icon = (name, cls, title) => {
    const s = svg("svg", { viewBox: "0 0 24 24", class: `ico ${cls || ""}`.trim() });
    s.innerHTML = ICONS[name];
    if (title) { const t = svg("title"); t.textContent = title; s.prepend(t); }
    return s;
  };
  // ---- versions: current (green), update out (yellow), must update (red: below the hub's minimum), unknown (grey) --
  const VSTATES = { current: "current", update: "update", must: "must", unknown: "unknown" }, VKEYS = Object.keys(VSTATES);
  const platLine = (l, p) => `${l} ${num(p.all)}${VKEYS.filter((k) => p[k] && k !== "unknown").map((k) => ` · ${num(p[k])} ${k}`).join("")}`;
  // One stacked bar of shares, current at the bottom; an empty slot a faint dash.
  const vStack = (row, x, w, H, top) => {
    const all = row ? row.reduce((a, v) => a + v, 0) : 0;
    if (!all) return [svg("rect", { x: x.toFixed(1), width: w.toFixed(1), y: (H - 1.5).toFixed(1), height: 1.5, rx: 1, class: "b zero" })];
    let y = H;
    return VKEYS.map((k, i) => {
      const hh = (row[i] / all) * (H - top);
      y -= hh;
      return row[i] ? svg("rect", { x: x.toFixed(1), width: w.toFixed(1), y: y.toFixed(1), height: hh.toFixed(1), class: `v-${k}` }) : null;
    }).filter(Boolean);
  };
  // ---- overview ---------------------------------------------------------------------------------------------------
  // Active, a tile of the top grid like Sent and Accounts: who was here per day (bars) and the share of them who came
  // back (line); the number and its share of the month in the header, retention and the rest in the hover.
  let activeSpec = null, drawAccountStatsImpl = null, drawSentRowsImpl = null;
  const drawAccountStats = (h) => drawAccountStatsImpl?.(h);
  const drawActive = (h) => {
    const slot = $("ac-area");
    if (!slot || !h || h.error) return;
    const t = today0(), mau = h.active_month || 0, ds = Array.from({ length: 14 }, (_, i) => t - 13 + i), kept = h.active_days || {};
    const active = ds.map((d) => (d === t ? Math.max(kept[d] || 0, h.active_day || 0) : kept[d] || 0)), su = new Map(h.signup_days || []);
    const back = ds.map((d, i) => (active[i] ? (100 * Math.max(0, active[i] - (su.get(d) || 0))) / active[i] : null));
    const ret = Object.fromEntries((areaSub.yc || []).map(([l, v]) => [String(l).replace(/^Retention /, ""), v]));
    const stick = mau ? (100 * (h.active_day || 0)) / mau : null;
    set("ac-n", `DAU ${num(h.active_day)} · MAU ${num(mau)}`);
    set("ac-s", `${pctText(stick)} stickiness`);
    activeSpec = { ds, active, back, su, ret };
    // Bars: users seen per day (DAU). The line: the month's users (MAU, the last 30 days), one level: the collector keeps
    // each day's count, not who, so a rolling MAU per day cannot be told. Daily points on every range (no hourly counts).
    const level = ds.map(() => mau);
    slot.replaceChildren(liveChart({ bars: active, line: level, from: dayName(ds[0]), to: "today" }, "", null, (i) => { const s = activeSpec;
      return { head: dayName(s.ds[i]), rows: [{ section: "This day" }, { color: "var(--ok)", label: "DAU", value: num(s.active[i]) }, { ico: "user", label: "New", value: num(s.su.get(s.ds[i]) || 0) },
        { color: "var(--ok)", line: true, label: "MAU (30 days, now)", value: num(mau) }, { label: "Stickiness today", value: pctText(stick) }, { label: "Came back", value: pctText(s.back[i]) }, { section: "Retention" }, ...["D1", "D7", "D30"].map((k) => ({ label: k, value: s.ret[k] ?? "–" }))] }; }));
  };
  const reportKey = (r) => `${which}:r${r.id}`;
  let hubNow = null;
  const overview = (h) => {
    const ok = h && !h.error;
    hubNow = ok ? h : null;
    // Attention, in the chosen range (7, 30 or 365 days): totals, and bars per day (per month for a year).
    // Feedback: messages users wrote; Errors: crash and error reports (the hub keeps 14 days of them); Reports: new.
    const perDay = new Map();
    for (const r of h?.diagnostic_days || []) perDay.set(r.day, (perDay.get(r.day) || 0) + r.count);
    const reps = h?.reports || [], repDays = new Map();
    for (const r of reps) { const d = Math.floor(r.at / 86400000); repDays.set(d, (repDays.get(d) || 0) + 1); }
    const h0 = Math.floor(Date.now() / 3600000) - 23, repHours = Array(24).fill(0);
    for (const r of reps) { const i = Math.floor(r.at / 3600000) - h0; if (i >= 0 && i < 24) repHours[i]++; }
    // Problems: feedback, errors and reports together, per hour, day or week of the range.
    const problems = new Map();
    for (const m of [year(h?.support_days), perDay, repDays]) for (const [d, n] of m || []) problems.set(d, (problems.get(d) || 0) + n);
    const probHours = Array.from({ length: 24 }, (_, i) => (h?.support_hours?.slice(-24)[i] || 0) + repHours[i]);
    // Problems: how many were open at each point of the range (feedback not handled, errors not done, reports not
    // ticked), not how many came in; a bar an hour (1 day), a day (7, 30), up to 12 stretches (Total).
    const po = h?.problems_open, openRep = reps.filter((r) => !lastDone.has(reportKey(r)));
    const repAt = (t) => openRep.filter((r) => r.at <= t).length, nowMs = Date.now();
    if (po) {
      let names, values;
      if (range === 1) {
        names = Array.from({ length: 24 }, (_, i) => clock((h0 + i + 1) * 3600000));
        values = po.hours.map((v, i) => v + repAt(Math.min(nowMs, (h0 + i + 1) * 3600000)));
      } else {
        const dayAt = (k) => po.days[po.days.length - 1 - k] ?? 0, end = (k) => Math.min(nowMs, (today0() - k + 1) * 86400000);
        const first = po.days.findIndex((v) => v > 0), span = range || Math.max(7, first < 0 ? 7 : po.days.length - first);
        const step = range ? 1 : Math.ceil(span / 12), n = Math.ceil(span / step);
        const ks = Array.from({ length: n }, (_, i) => (n - 1 - i) * step);
        names = ks.map((k) => dayName(today0() - k)); values = ks.map((k) => dayAt(k) + repAt(end(k)));
      }
      spark("pr", values[values.length - 1], values, names, (i, v) => `${num(v)} open`, "Problems open: feedback, errors and reports");
      $("c-pr").classList.add("bad-up");
    } else rangeRow("pr", problems, "problems", "Problems: feedback, errors and reports", null, true, probHours);
    // The tile's bars: open reports per slot, errors (orange) with the crashes on top (red).
    if (h) {
      const node = $("pr-spark"), p = problemSplit(h), sb = sparkBars(p.er, "warm", null, p.cr, p.recorded);
      node.replaceChildren(...sb.childNodes);
      node.hidden = false;
      node._split = p;
      if (!node._h) {
        node._h = 1;
        hoverTip(node, () => node._split.cr.length, (i) => { const q = node._split; return { head: q.names[i], rows: problemRows(q, i) }; },
          (k) => [...node.children].forEach((r, j) => r.classList.toggle("on", j === k)));
      }
    }
    drawAccountStats(h);
    drawSentRowsImpl?.(h);
    showOpen();

    // Services: the queue at the bottom.
    const blobs = ok && h.blobs;
    set("q-n", ok ? num(h.messages) : "–");
    $("c-q").dataset.tip = ok ? `${num(h.messages)} messages waiting for their device${h.messages && h.oldest_waiting ? `, oldest ${rel(h.oldest_waiting)}` : ""} · ${blobs ? `${num(blobs[0])} images waiting: ${bytes(blobs[1])}` : "images unknown"}` : "";

    // Users: ten, by the chosen column (last seen first); a click opens the user in the Users tab.

    // Activity: active users and the sign-ups of the last 24 hours, each value on hover.
    if (ok) {
      // Usage, in the chosen range: totals and bars. Yos and Media count what went in (sent, uploaded; a bot's inbox
      // left out since the hub counts it apart); what went out (fetched) is in the hover.
      const ev = h.events, ym = (k) => year(ev?.[k]?.year), yh = (k) => ev?.[k]?.hours, yt = (k) => ev?.[k]?.total;
      // Media: the volume uploaded (the hub's media-in-bytes, per hour and day, kept for good); the count is the
      // second line of the Yos chart.
      rangeRow("md", ym("media-in-bytes"), "", "Media: photos and videos uploaded", null, false, yh("media-in-bytes"), null, yt("media-in-bytes"), "media-in-bytes");
      // Media's running total over Yos on its own scale; in the hover with its share of Yos.
      const mdTotal = yt("media-in") ?? 0, md = runningTotal(ym("media-in"), yh("media-in"), mdTotal);
      seriesChart("ym", { values: md.values, labels: md.labels, word: "media", share: true },
        null);
      rangeRow("gr", ym("group-create"), "made", "Groups: made", null, false, yh("group-create"), null, yt("group-create"), "group-create");
      part(() => healthRows(h, ym, yh));
      // Running totals: Yos into users' inboxes (a bot's inbox apart), accounts without bots.
      // Sent (into inboxes) or Delivered (acknowledged by a device): the pill in the header chooses, the choice is kept.
      const yk = sentMode === "sent" ? "yo-in" : "yo-out";
      drawSentPill();
      areaChart("yc", yt(yk) ?? 0, ym(yk), yh(yk), last24(yh(yk)).reduce((n, v) => n + (v || 0), 0), yk);
      const sud = new Map(h.signup_days || []);
      areaChart("uc", [...sud.values()].reduce((a, v) => a + v, 0), sud, h.signups_hours, last24(h.signups_hours).reduce((n, v) => n + (v || 0), 0));
      const dn14 = days14().map(dayName);
      // Health: secured (of each sign-up day's accounts, the share with a backup now), active (per UTC day), and the
      // app versions per platform as one stacked bar (current, update out, unknown).
      const sd = h.secured_days || Array(14).fill(null), pct = (d) => (d && d[0] ? Math.round((100 * d[1]) / d[0]) : null);
      const scAll = h.total ? Math.round((100 * (h.secured || 0)) / h.total) : null;
      spark("sc", null, sd.map((d) => pct(d) ?? 0), dn14, (i) => (sd[i] == null ? "not recorded" : `${pct(sd[i]) ?? 0} % (${num(sd[i][1])} of ${num(sd[i][0])})`),
        `Secured: ${scAll ?? "–"} % of users (a backup)`);
      set("sc-n", scAll == null ? "–" : `${scAll} %`);
      // Secured as one bar: the share of all accounts with a backup; the hover adds the last 7 days' sign-ups.
      $("sc-bar").firstChild.style.width = `${scAll ?? 0}%`;
      const wk = sd.slice(-7).filter(Boolean), made = wk.reduce((n, d) => n + d[0], 0), withKey = wk.reduce((n, d) => n + d[1], 0);
      $("c-sc").dataset.tip = `${scAll ?? "–"} % of ${num(h.total)} secured${made ? ` · new this week: ${Math.round((100 * withKey) / made)} %` : ""}`;
      const kept = h.active_days || {}, a14 = days14().map((d) => kept[d] ?? null);
      if (h.active_day != null) a14[13] = Math.max(a14[13] ?? 0, h.active_day);
      seriesChart("ac", { values: a14.map((v) => v || 0), labels: dn14, word: "active" }, null);
      // Platforms: one stacked bar, a share per platform; its versions (current, update out, must update: below the
      // hub's minimum, unknown) in the hover.
      const per = {};
      for (const [app, c] of h.apps || []) {
        const st0 = belowMin(app) ? "must" : appInfo(app)?.state || "unknown", st = st0 === "fresh" ? "current" : st0, l = appInfo(app)?.label || "unknown";
        (per[l] ||= { current: 0, update: 0, must: 0, unknown: 0, all: 0 })[st in VSTATES ? st : "unknown"] += c;
        per[l].all += c;
      }
      const devices = Object.values(per).reduce((n, p) => n + p.all, 0), order = ["iOS", "Android", "Bot", "CLI"];
      const plats = Object.entries(per).sort((x, y) => (order.indexOf(x[0]) + 1 || 9) - (order.indexOf(y[0]) + 1 || 9));
      set("pl-n", num(devices));
      $("pl-bar").replaceChildren(...plats.map(([l, p]) => {
        const x = el("span", `p-${l.toLowerCase()}`);
        x.style.flexGrow = p.all;
        x.dataset.tip = platLine(l, p);
        return x;
      }));
      $("c-pl").dataset.tip = plats.map(([l, p]) => `${l} ${num(p.all)}`).join(" · ");
      platData = { plats, min: h.min_app, devices };
      drawAccountStats(h);
    }
  };

  // ---- support chat (the Database widget, support_messages): the people who wrote on the left, "Open" or
  // "Answered"; the chosen one's conversation on the right, under one line of facts. Each message of theirs is ticked
  // handled by hand; a reply ticks the ones it answers. The to-dos are a table of their own (todos).
  let supportInstall = null, supportHub = null, supportName = "", supportSeen = null, supportBusy = false, supportThreads = [];
  let supportWant = null, listDrawn = "", supportMessages = [], userFilter = "open";
  const supportGet = async (install) => {
    const r = await fetch(`/support.json?${new URLSearchParams({ hub: which, ...(install ? { install } : {}) })}`, { cache: "no-store" });
    if (!r.ok) throw new Error("Could not load messages.");
    return r.json();
  };
  const reportsBy = (install) => (last?.[which]?.reports || []).filter((r) => String(r.reporter).toLowerCase() === install);
  const drawList = () => {
    const box = $("person-list"), lastLines = last?.[which]?.support_last || {};
    const rows = supportList({ threads: supportThreads, filter: userFilter });
    const drawn = JSON.stringify([which, supportInstall, userFilter, rows.map((p) => [p.id, p.open, p.at, lastLines[p.id], reportsBy(p.id).length])]);
    if (drawn === listDrawn && box.childElementCount) return;
    listDrawn = drawn;
    box.replaceChildren();
    for (const p of rows) {
      const b = el("button", `thread${p.open ? " unread" : ""}${supportInstall === p.id ? " on" : ""}`);
      b.type = "button"; b.title = `Install ${p.id}`;
      // One line: a dot while open, the install's short id, a flag if it reported someone, the time on the right.
      b.append(el("i", "odot"), el("b", "name mono", p.name));
      if (reportsBy(p.id).length) b.append(el("span", "flag", "⚑"));
      b.append(el("time", null, clock(p.at)));
      b.addEventListener("click", () => select(p.id, p.name));
      box.append(b);
    }
    if (!box.childElementCount) box.append(el("p", "empty", userFilter === "open" ? "Nothing open. 🎉" : "Nothing answered yet."));
  };
  const setFilter = (f) => {
    userFilter = f;
    for (const x of document.querySelectorAll("#people .seg [data-filter]")) x.classList.toggle("on", x.dataset.filter === f);
    listDrawn = "";
  };
  for (const b of document.querySelectorAll("#people .seg [data-filter]")) b.addEventListener("click", () => { setFilter(b.dataset.filter); part(drawList); });

  const supportDone = async (ids, done) => {
    const hub = which, user = supportInstall;
    try {
      await Promise.all(ids.map((id) => postJSON("/support/done", { hub, id, done })));
      if (which === hub && supportInstall === user) await supportOpen(user);
      await supportRefresh();
    } catch (e) { set("support-error", e.message); }
  };
  const drawThread = (messages) => {
    const box = $("support-history"), atEnd = box.scrollHeight - box.scrollTop - box.clientHeight < 60;
    box.replaceChildren();
    if (!messages.length) box.append(Object.assign(el("div", "empty-state"), { textContent: "No messages." }));
    let lastDay = "", lastFrom = null;
    for (const m of messages) {
      const dayKey = new Date(m.created_at).toDateString();
      if (dayKey !== lastDay) { box.append(el("div", "day", new Date(m.created_at).toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", year: "numeric" }))); lastDay = dayKey; lastFrom = null; }
      const from = m.from_team ? "team" : "user";
      const row = el("div", `msg ${from}${from === lastFrom ? " cont" : ""}${m.done_at != null ? " done" : ""}`);
      const bubble = el("div", "bubble", m.text), meta = el("span", "meta");
      const t = el("time", null, new Date(m.created_at).toLocaleTimeString(undefined, { timeStyle: "short" }));
      t.title = date(m.created_at) + (m.from_team ? (m.read_at ? " · read by the user" : " · not read yet") : "");
      meta.append(t);
      if (m.from_team) meta.append(el("span", `seen${m.read_at ? " on" : ""}`, m.read_at ? "✓✓" : "✓"));
      bubble.append(meta);
      row.append(bubble);
      if (!m.from_team) row.append(tickButton(m.done_at != null, "Message handled", (d) => supportDone([m.id], d)));
      box.append(row);
      lastFrom = from;
    }
    if (atEnd || box.dataset.user !== supportInstall) box.scrollTop = box.scrollHeight;
    box.dataset.user = supportInstall;
  };
  const growComposer = () => {
    const t = $("support-text");
    if (!t.offsetParent) { t.style.height = ""; return; }
    t.style.height = "auto"; t.style.height = `${Math.min(Math.max(t.scrollHeight, 22), 180)}px`;
  };
  // The chosen install in one line of facts: its short id (the full one on hover), its messages, first and last;
  // the people it reported folded below, each ticked handled here. Installs are random, no account behind them.
  const drawPerson = () => {
    const id = supportInstall, facts = $("p-facts");
    facts.replaceChildren();
    $("p-extra").hidden = !id;
    if (!id) return;
    const fact = (k, v, cls, title) => { const f = el("span", `fact ${cls || ""}`); f.append(el("small", null, k), el("b", null, v)); if (title) hint(f, title); facts.append(f); };
    const mine = supportHub === which ? supportMessages : [], theirs = mine.filter((m) => !m.from_team);
    fact("install", installShort(id), "mono", `Install ${id} (random, no account)`);
    fact("messages", num(theirs.length), "", "");
    if (theirs.length) fact("first", rel(theirs[0].created_at), "", date(theirs[0].created_at));
    const app = [...theirs].reverse().find((m) => m.app_version)?.app_version;
    if (app) fact("app", appInfo(app)?.label || app, "", app);
    const more = el("button", "fact more");
    more.type = "button";
    more.addEventListener("click", () => { $("p-extra").open = !$("p-extra").open; more.classList.toggle("open", $("p-extra").open); });
    more.classList.toggle("open", $("p-extra").open);
    facts.append(more);
    const reps = $("p-reports");
    reps.replaceChildren();
    const by = reportsBy(id);
    for (const r of by) {
      const done = lastDone.has(reportKey(r)), row = el("div", `drow${done ? " done" : ""}`), line = el("span");
      line.append(reasonTag(r.reason), document.createTextNode(` reported ${r.reported_name ?? r.reported.slice(0, 8)}`));
      if (r.id != null) row.append(tickButton(done, "Report handled", (d) => reportDone(r, d)));
      row.append(line, el("span", "muted", rel(r.at)));
      reps.append(row);
    }
    if (!reps.childElementCount) reps.append(el("p", "empty", "No reports."));
    const n = by.filter((r) => !lastDone.has(reportKey(r))).length;
    more.textContent = by.length ? `${by.length} report${by.length === 1 ? "" : "s"}${n ? ` (${n} open)` : ""}` : "no reports";
    more.classList.toggle("alert", n > 0);
  };
  const convoHead = (messages) => {
    $("support-reply").hidden = !supportInstall || !messages.length;
    $("support-text").placeholder = `Reply to ${supportName} (Enter sends, Shift+Enter new line)`;
    growComposer();
  };
  const select = (id, name) => {
    toInbox();
    supportOpen(id, name, true);
  };
  const supportOpen = async (user, title, chosen) => {
    const hub = which;
    if (chosen || supportWant === null) supportWant = user;
    if (chosen) {
      if (supportInstall !== user || supportHub !== hub) {
        $("support-text").value = ""; growComposer(); supportSeen = null; $("support-history").replaceChildren();
        $("support-reply").hidden = true;
      }
      supportInstall = user; supportHub = hub; supportName = title || user;
      part(drawPerson); part(drawList);
    }
    try {
      const data = await supportGet(user);
      if (which !== hub || supportWant !== user) return;
      supportInstall = user; supportHub = hub; supportName = title || supportName || user;
      supportMessages = data.messages;
      convoHead(data.messages);
      part(drawPerson);
      const seen = data.messages.map((m) => `${m.id}:${m.read_at ? 1 : 0}:${m.done_at}`).join(",");
      if (seen !== supportSeen) { supportSeen = seen; drawThread(data.messages); }
      if (chosen && !$("support-reply").hidden) $("support-text").focus({ preventScroll: true });
      set("support-error", "");
    } catch (e) { set("support-error", e.message); }
  };
  const supportRefresh = async () => {
    if (supportBusy) return;
    supportBusy = true;
    const hub = which;
    if (supportHub !== hub) {
      supportInstall = null; supportWant = null; supportSeen = null; $("support-reply").hidden = true;
      $("support-history").replaceChildren();
    }
    try {
      const threads = (await supportGet()).filter((t) => /^[0-9a-f]{32}$/i.test(t.install || ""));
      if (hub !== which) return;
      supportThreads = threads;
      // The page title's count: conversations still open, and open crash groups without the expected ones.
      const unanswered = threads.filter((t) => t.unread > 0).length;
      const crashes = new Set((last?.[which]?.diagnostics || []).filter((d) => !d.done && !diagInfo(d).expected).map(diagCause)).size;
      drawTitle();
      openTickets = unanswered + crashes; openSupport = unanswered;
      showOpen();
      part(drawList);
      part(drawPerson);
      // Nothing chosen yet: the newest open conversation, else the newest one.
      if (supportWant === null) {
        const t = [...threads].sort((a, b) => (b.unread > 0) - (a.unread > 0) || b.at - a.at)[0];
        if (t) supportOpen(t.install.toLowerCase(), installShort(t.install), true);
      }
      set("support-error", "");
    } catch (e) { set("support-error", e.message); }
    finally { supportBusy = false; }
  };
  const supportSend = async () => {
    const text = $("support-text").value;
    if (!supportInstall || supportHub !== which || !text.trim()) return;
    const button = $("support-reply").querySelector("button"), user = supportInstall, hub = supportHub;
    button.disabled = true;
    try {
      await postJSON("/support", { hub, install: user, action: "reply", text });
      // The reply answers the user's open messages in this thread: they are marked handled.
      const open = which !== hub ? [] : (await supportGet(user)).messages.filter((m) => !m.from_team && m.done_at == null);
      await Promise.all(open.map((m) => postJSON("/support/done", { hub, id: m.id, done: true })));
      if (which !== hub || supportInstall !== user) return;
      $("support-text").value = ""; growComposer();
      await supportRefresh();
      await supportOpen(user);
      part(drawPerson);
    } catch (e) { set("support-error", e.message); }
    finally { button.disabled = false; $("support-text").focus({ preventScroll: true }); }
  };
  $("support-reply").addEventListener("submit", (e) => { e.preventDefault(); supportSend(); });
  $("support-text").addEventListener("input", growComposer);
  $("support-text").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); supportSend(); }
  });
  setInterval(() => {
    if (document.visibilityState !== "visible") return;
    supportRefresh();
    if (supportInstall && supportHub === which) supportOpen(supportInstall);
  }, 4000);

  // ---- the team's to-dos (the Database widget, todos): add one, tick it done, delete it -------------------------------
  const todoChange = async (path, data) => {
    try { await postJSON(path, { hub: which, ...data }); await refresh(); } catch (e) { alert(e.message); }
  };
  const todoBar = () => {
    const form = el("form", "add todoadd"), input = el("input");
    input.placeholder = "A new to-do (Enter)"; input.maxLength = 4000; input.setAttribute("aria-label", "New to-do");
    const add = el("button", "pill", "Add"); add.type = "submit";
    form.append(input, add);
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const text = input.value.trim();
      if (!text) return;
      add.disabled = true;
      try {
        const r = await fetch("/todos", { method: "POST", body: new URLSearchParams({ hub: which, text }) });
        if (!r.ok) throw new Error("Could not add the to-do.");
        input.value = "";
        await refresh();
      } catch (x) { alert(x.message); } finally { add.disabled = false; }
    });
    return form;
  };

  // ---- users: every user in one sortable table (the Database widget, users) -------------------------------------------
  let allSort = { key: "created", dir: -1 };  // newest sign-ups first
  const allValue = (u, key) => key === "name" ? (u.name || "").toLowerCase() : key === "app" ? (parse(u.app)?.[1] ?? -1)
    : key === "update" ? (u.app ? (belowMin(u.app) ? 3 : appState(u.app, u.id) === "update" ? 2 : ["current", "fresh"].includes(appState(u.app, u.id)) ? 0 : 1) : -1)
    : key === "secured" || key === "push" ? Number(!!u[key]) : u[key] ?? 0;
  for (const th of document.querySelectorAll("#all-users-table th[data-sort]")) {
    th.tabIndex = 0;
    const go = () => {
      const key = th.dataset.sort;
      allSort = { key, dir: allSort.key === key ? -allSort.dir : key === "name" ? 1 : -1 };
      part(() => drawAllUsers(last?.[which]));
    };
    th.addEventListener("click", go);
    th.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
  }
  const drawAllUsers = (h) => {
    for (const th of document.querySelectorAll("#all-users-table th[data-sort]")) {
      th.classList.toggle("sorted", th.dataset.sort === allSort.key);
      th.dataset.dir = th.dataset.sort === allSort.key ? (allSort.dir > 0 ? "▲" : "▼") : "";
    }
    const users = h?.users || [];
    const list = [...users].sort((a, b) => {
      const x = allValue(a, allSort.key), y = allValue(b, allSort.key);
      return (x < y ? -1 : x > y ? 1 : 0) * allSort.dir || b.created - a.created;
    });
    const body = $("all-users-body");
    body.replaceChildren();
    for (const u of list) {
      // No click: a name opens nothing (the support chat is reached from its own table).
      const tr = body.insertRow();
      cell(tr, pii(u.name ?? "–"), "name", u.id);
      cell(tr, rel(u.created), null, date(u.created));
      cell(tr, seenDay(u.last_seen), null, u.last_seen ? new Date(u.last_seen).toLocaleDateString() : "never seen");
      // The platform's icon coloured by its build (green current, yellow an update out), the lock when secured.
      const platform = (u.app || "").split("/")[0], st = appState(u.app, u.id), app = el("span", "uicons");
      if (ICONS[platform] && platform !== "lock") app.append(icon(platform, st ? `app-${st}` : "", appTitle(u.app, u.id)));
      app.append(appTag(u.app, u.id));
      cell(tr, app);
      // The device's software: current, an update out (or below the hub's minimum), or unknown.
      const upd = u.app ? (belowMin(u.app) ? tag("must update", "bad") : st === "update" ? tag("update", "warn") : st === "current" ? tag("current", "ok") : st === "fresh" ? tag("fresh", "info") : tag("?", "muted")) : el("span", "muted", "–");
      cell(tr, upd);
      cell(tr, u.push ? tag("on", "ok") : tag("off", "muted"));
      cell(tr, u.secured ? icon("lock", "secured", u.backup_at ? `backup ${date(u.backup_at)}` : "secured") : el("span", "muted", "–"));
      cell(tr, num(u.devices), "num");
      cell(tr, u.queued ? num(u.queued) : "–", "num muted");
      // Signed requests to strangers in 24 h (spec 4.10): a flood by a stranger stands out; contacts' are never counted.
      cell(tr, u.requests_24h ? num(u.requests_24h) : "–", u.requests_24h >= 100 ? "num bad" : "num muted");
    }
    if (!list.length) cell(body.insertRow(), h && !h.error ? "No users yet." : (h?.error || "No data"), "empty").colSpan = 10;
  };

  // ---- reports: handled is kept by the admin page (done.json); ticked in a user's details
  const reportDone = async (r, done) => {
    await fetch("/feedback/done", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: `hub=${which}&id=r${r.id}&done=${done ? 1 : 0}` });
    await refresh();
  };
  const reasonTag = (reason) => tag(reason, reason === "abuse" ? "bad" : reason === "spam" ? "warn" : "muted");

  // ---- database: every table, grouped by topic; reserved names with their switch --------------------------------
  // Table groups as the README's endpoint groups ("What the hub knows per endpoint group"), in this order, each table
  // in its listed order; a name ending in "*" takes every table it starts. Each with a line icon (drawn like the
  // Overview's). The rest (Litestream's bookkeeping, settings) under Internal.
  const TABLE_GROUPS = [
    ["Account", "user", ["users", "devices", "handles", "gone", "reserved", "account_keys", "contact_keys", "prekeys", "profile_commitments", "storage"]],
    ["Messages", "mail", ["messages", "blobs", "blob_quota"]],
    ["Groups", "group", ["zk_groups"]],
    ["Backup", "disk", ["backups"]],
    ["Support and crashes", "chat", ["support_messages", "reports", "diagnostics", "diagnostic_days", "diagnostic_seen", "diagnostic_installs", "todos"]],
    ["Stats", "chart", ["deliveries_hourly*", "events", "request_hours"]],
  ];
  Object.assign(ICONS, {
    user: '<circle cx="12" cy="8" r="3.5"/><path d="M5 20a7 7 0 0 1 14 0"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="M11 12l8-8M16 7l2 2M14 9l2 2"/>',
    mail: '<rect x="3" y="5.5" width="18" height="13" rx="2"/><path d="m3.5 7 8.5 6 8.5-6"/>',
    group: '<circle cx="9" cy="8.5" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M15.5 5.2a3.5 3.5 0 0 1 0 6.6"/><path d="M18 14.3a6.5 6.5 0 0 1 3.5 5.7"/>',
    disk: '<ellipse cx="12" cy="6" rx="7.5" ry="2.8"/><path d="M4.5 6v12c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V6"/>',
    chat: '<path d="M5 5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H10l-4 3.5V16H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/>',
    warn: '<path d="M12 3.5 2.5 20h19z"/><path d="M12 10v4.5"/><path d="M12 17.2v.3"/>',
    chart: '<path d="M4 20V10M10 20V4M16 20v-7M21 20H3"/>',
    check: '<path d="M4.5 12.5l4.5 4.5L19.5 6.5"/>',
    speed: '<path d="M3 12h11"/><path d="m10 7.5 4.5 4.5-4.5 4.5"/><path d="M18 5v14"/>',
    clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
    image: '<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/>',
    lock: '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>',
    trash: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>',
    stay: '<path d="M4 12a8 8 0 1 0 2.3-5.6"/><path d="M4 4v4h4"/>',
    ratio: '<circle cx="12" cy="12" r="8.5"/><path d="M12 3.5V12h8.5"/>',
    send: '<path d="M21 3 10 14M21 3l-7 18-4-7-7-4 18-7z"/>',
    sum: '<path d="M17 5H7l6 7-6 7h10"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1"/>',
  });
  // The views over the tables, at the top of the sidebar: key, label, icon, what it is.
  const VIEWS = [
    ["people", "Users", "user", "Accounts with their devices, app, push and backup, put together"],
    ["v-support", "Support", "chat", "The support chat: who wrote, open or answered, and the conversation"],
    ["v-errors", "Errors", "warn", "Crash and error groups, open first, with Done and Reopen"],
    ["v-todos", "To-dos", "check", "The team's to-dos: add, tick, delete"],
  ];
  const VIEW_TABLE = { people: "users", "v-support": "support_messages", "v-errors": "diagnostics", "v-todos": "todos" };
  // What each table is for, from the hub's schema (hub/src/db.rs); "(?)" marks a guess.
  const TABLE_INFO = {
    users: "Accounts: id, account key, sign-up time",
    handles: "Usernames by skeleton, as their owners spell them",
    reserved: "Usernames nobody may claim, by skeleton",
    gone: "Replaced devices; their requests answer 410",
    account_keys: "Every account key a user ever had",
    devices: "Each user's devices: keys, push, app, last seen",
    contact_keys: "Checks of users' access keys, never the keys",
    profile_commitments: "Users' profile key commitments, for credentials",
    messages: "Encrypted envelopes waiting for their device",
    prekeys: "Devices' prekeys, to start encrypted sessions",
    blobs: "Photos and videos until their last fetch",
    blob_quota: "Uploads per user and day, kept two days",
    zk_groups: "Groups: encrypted state, revision (Signal Groups v2)",
    storage: "Each user's sealed storage, versioned writes",
    backups: "Each user's backup record; it secures the name",
    support_messages: "Support chat messages, by random install id (no account)",
    todos: "The team's to-dos",
    diagnostics: "Crash and error reports, one row per signature",
    diagnostic_seen: "Report ids already counted, never counted twice",
    diagnostic_days: "Crashes and errors per day, last 14 days",
    diagnostic_installs: "Which install sent a group on which day, kept 90 days",
    reports: "Users reported by installs (random id, no account), latest per pair",
    settings: "Hub switches, such as name reservation on/off",
    deliveries_hourly: "Deliveries per hour, a count only, 8 days",
    request_hours: "Signed requests (to strangers) per sender and hour, kept 25 hours",
    _litestream_lock: "Litestream replication bookkeeping (?)",
    _litestream_seq: "Litestream replication bookkeeping (?)",
  };
  const tableGroups = (list) => {
    const known = new Set();
    const groups = TABLE_GROUPS.map(([title, ico, names]) => {
      const ts = names.flatMap((n) => list.filter((t) => !known.has(t.name) && (n.endsWith("*") ? t.name.startsWith(n.slice(0, -1)) : t.name === n))
        .sort((x, y) => x.name.localeCompare(y.name)));
      ts.forEach((t) => known.add(t.name));
      return [title, ico, ts];
    });
    groups.push(["Internal", "gear", list.filter((t) => !known.has(t.name))]);
    return groups.filter(([, , ts]) => ts.length);
  };
  // Install id columns (help, spec 6.4) in their short form.
  const INSTALL_COL = /^(install|reporter)$/;
  // A timestamp column (…_at, ms since 1970) as a readable date, the raw value on hover.
  const isTime = (col, v) => /_at$/.test(col) && typeof v === "number" && v > 1e11 && v < 1e14;
  let tableOpen = null, tablesDrawn = "";
  const changeReserved = async (data) => {
    const r = await fetch("/reserved", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ hub: which, ...data }) });
    if (!r.ok) throw new Error(`Could not save (${r.status}).`);
    await tick(); await refresh();
  };
  const reservedBar = () => {
    const data = reservedData?.[which], wrap = el("div", "reserved-bar");
    const sw = el("button", `switch${data?.enabled ? " on" : ""}`);
    sw.type = "button"; sw.disabled = !data;
    sw.setAttribute("role", "switch"); sw.setAttribute("aria-checked", String(!!data?.enabled));
    sw.append(el("i"));
    const label = el("span", "state", data ? (data.enabled ? "Reservation ON: these names cannot be claimed" : "Reservation OFF: anyone may claim these names") : "Reservation state unavailable");
    const err = el("small", "error");
    sw.addEventListener("click", async () => { sw.disabled = true; try { await changeReserved({ action: "switch", enabled: !data.enabled }); } catch (e) { err.textContent = e.message; sw.disabled = false; } });
    const form = el("form", "add");
    const input = el("input"); input.placeholder = "Add names (comma separated)"; input.setAttribute("aria-label", "Names to reserve");
    const reason = el("input"); reason.placeholder = "Reason"; reason.maxLength = 200; reason.setAttribute("aria-label", "Reason for reserving them");
    const add = el("button", "pill", "Add"); add.type = "submit";
    form.append(input, reason, add);
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const note = reason.value.trim();
      const names = input.value.split(/[,\n]/).map((n) => n.trim()).filter(Boolean).map((name) => (note ? { name, note } : name));
      if (!names.length) return;
      try { await changeReserved({ action: "add", names }); input.value = ""; reason.value = ""; } catch (x) { err.textContent = x.message; }
    });
    wrap.classList.toggle("off", !!data && !data.enabled);
    wrap.append(sw, label, form, err);
    return wrap;
  };
  const drawTables = () => {
    const box = $("tables"), list = last?.[which]?.tables || [];
    const drawn = JSON.stringify([which, tableOpen, list.map((t) => [t.name, t.count]), tableOpen === "reserved" ? reservedData?.[which]?.enabled : 0,
      (list.find((t) => t.name === (VIEW_TABLE[tableOpen] || tableOpen)) || {}).rows?.length, tableOpen === "reserved" ? list.find((t) => t.name === "reserved")?.rows : 0]);
    if (drawn === tablesDrawn && box.childElementCount) return;
    tablesDrawn = drawn;
    // The views at the top, each over one table (Users put together from several); below, every table as it is, by name.
    const real = (n) => VIEW_TABLE[n] || n;
    const sel = list.find((x) => x.name === real(tableOpen));
    set("tables-note", `${list.length} tables · ${num(list.reduce((n, t) => n + (t.count || 0), 0))} rows${sel ? ` · ${sel.name}: ${num(sel.count)} rows${sel.count > sel.rows.length ? `, first ${num(sel.rows.length)} shown` : ""}` : ""}`);
    if (!tableOpen || (list.length && !list.some((t) => t.name === real(tableOpen)))) tableOpen = "people";
    const side = el("nav", "tside"), pane = el("div", "tpane");
    const count = (name) => list.find((x) => x.name === name)?.count || 0;
    // The views count what is open, not their table's rows: unanswered conversations, open error groups, to-dos left.
    const rowsOf = (name) => list.find((x) => x.name === name);
    const open = (name, column) => { const t = rowsOf(name); if (!t) return 0; const i = t.columns.indexOf(column);
      return i < 0 ? t.count : t.rows.filter((r) => !r[i]).length; };
    const viewCount = (key) => key === "v-support" ? openSupport
      : key === "v-errors" ? (last?.[which]?.diagnostics || []).filter((d) => !d.done).length || open("diagnostics", "done")
      : key === "v-todos" ? open("todos", "done_at") : count(VIEW_TABLE[key]);
    for (const [key, label, ico, tip] of VIEWS) {
      const b = el("button", `main view${tableOpen === key ? " on" : ""}`), name = el("span", null, label);
      b.type = "button";
      if (key === "v-errors") {
        // Groups (a row per build and line), causes (what they are about) and reports (the sum of counts), open ones.
        const g = (last?.[which]?.diagnostics || []).filter((d) => !d.done);
        hint(b, { cap: 1, head: "Open errors and crashes", rows: [{ label: "Groups", value: num(g.length) }, { label: "Causes", value: num(new Set(g.map(diagCause)).size) }, { label: "Reports", value: num(g.reduce((n, d) => n + (d.count || 0), 0)) }] });
      } else b.title = tip;
      name.prepend(icon(ico));
      b.append(name, el("small", null, num(viewCount(key))));
      b.addEventListener("click", () => { tableOpen = key; drawTables(); });
      side.append(b);
    }
    side.append(el("div", "tside-h", "Database"));
    for (const [kind, , ts] of tableGroups(list)) {
      side.append(el("div", "tside-k", kind));
      for (const t of ts) {
        const b = el("button", `sub raw${t.name === tableOpen ? " on" : ""}`);
        b.type = "button"; b.title = TABLE_INFO[t.name] || t.name;
        b.append(el("span", null, t.name), el("small", null, num(t.count)));
        b.addEventListener("click", () => { tableOpen = t.name; drawTables(); });
        side.append(b);
      }
    }
    // On a phone the list is a row of chips: the chosen one scrolled into view.
    requestAnimationFrame(() => { const on = side.querySelector(".on"); if (on && side.scrollWidth > side.clientWidth) side.scrollLeft = on.offsetLeft - side.offsetLeft - 12; });
    const t = list.find((x) => x.name === real(tableOpen));
    // Views: Support is the support chat (people on the left, the conversation and a reply box on the right; moved here,
    // never rebuilt), Users the enriched sortable users table; Errors and To-dos their tables with their tools.
    const chat = tableOpen === "v-support", usersView = tableOpen === "people";
    const view = chat ? "chat" : usersView ? "users" : "list";
    // Support and Users keep their pane (only the side list is new) while it holds them; a pane drawn before the data
    // came (a #db or #users link opens the Database before the first /users.json) is empty and drawn again.
    if (view !== "list" && box.dataset.name === tableOpen && box.dataset.view === view && box.firstChild
      && box.querySelector(view === "chat" ? ".tpane #people" : ".tpane #all-users")) { box.firstChild.replaceWith(side); return; }
    box.dataset.name = tableOpen; box.dataset.view = view;
    if (view !== "chat" && box.querySelector("#people")) $("chat-home").append($("people"));
    if (view !== "users" && box.querySelector("#all-users")) $("chat-home").append($("all-users"));
    if (t) {
      if (chat) pane.append($("people"));
      if (usersView) pane.append($("all-users"));
      if (t.name === "reserved") pane.append(reservedBar());
      if (tableOpen === "v-todos") pane.append(todoBar());
      if (view === "list") {
      const wrap = el("div", "scroll grid"), table = el("table", "list data");
      table.dataset.name = t.name;
      const hr = table.createTHead().insertRow();
      if (tableOpen === "v-todos") hr.append(el("th", "tick-col"));
      for (const c of t.columns) hr.append(el("th", null, c));
      if (t.name === "reserved") hr.append(el("th"));
      const body = table.createTBody();
      // Crash and error groups: open ones first, newest first; "Done" on each open one (done ones dimmed, "Reopen").
      const diag = tableOpen === "v-errors", todo = tableOpen === "v-todos", col = (n) => t.columns.indexOf(n);
      const rows = diag ? [...t.rows].sort((a, b) => (a[col("done")] ? 1 : 0) - (b[col("done")] ? 1 : 0) || b[col("last_seen")] - a[col("last_seen")]) : t.rows;
      if (diag || todo) hr.append(el("th"));
      for (const row of rows) {
        const tr = body.insertRow();
        if (diag && row[col("done")]) tr.className = "done";
        // A group's cause (all its builds and lines): reports, installs, and their share of the phones seen in 90 days.
        if (diag) {
          const g = (last?.[which]?.diagnostics || []).find((d) => d.id === row[col("id")]), seen90 = last?.[which]?.crash_free?.["0"]?.[1];
          if (g && g.cause_installs != null) hint(tr, { cap: 1, head: "This cause", rows: [{ label: "Reports", value: num(g.cause_reports) }, { label: "Installs", value: num(g.cause_installs) }, { label: "Of phones seen (90 days)", value: seen90 ? pctText((100 * g.cause_installs) / seen90) : "–" }] });
        }
        // A to-do: its tick first (done or open again), a delete at the end.
        if (todo) {
          const id = row[col("id")], done = row[col("done_at")] != null;
          if (done) tr.className = "done";
          tr.append(Object.assign(document.createElement("td"), { className: "tick-col" }));
          tr.lastChild.append(tickButton(done, "To-do done", (d) => todoChange("/todos/done", { id, done: d })));
        }
        row.forEach((v, i) => {
          // A reserved name's reason: edited in place, saved on Enter or leaving the field.
          if (t.name === "reserved" && t.columns[i] === "note") {
            const name = row[t.columns.indexOf("spelling")], note = el("input", "note");
            note.value = v ?? ""; note.maxLength = 200; note.placeholder = "Add a reason"; note.setAttribute("aria-label", `Reason for ${name}`);
            note.addEventListener("change", async () => {
              note.disabled = true;
              try { await changeReserved({ action: "add", names: [{ name, note: note.value.trim() }] }); } catch { note.disabled = false; note.classList.add("failed"); }
            });
            cell(tr, note);
            return;
          }
          if (diag && t.columns[i] === "signature") cell(tr, v == null ? "–" : String(v), "mono");
          else if (isTime(t.columns[i], v)) cell(tr, date(v), "mono", String(v));
          // A random install id (support, reports, diagnostic_installs): its first 6 hex, the full one on hover.
          else if (INSTALL_COL.test(t.columns[i]) && typeof v === "string" && /^[0-9a-f]{32}$/i.test(v)) cell(tr, installShort(v), "mono", v);
          else if (v != null && PII_COL.test(t.columns[i])) cell(tr, pii(String(v)), "mono", String(v).length > 40 ? String(v) : "");
          else cell(tr, v == null ? "–" : String(v), "mono", v != null && String(v).length > 40 ? String(v) : "");
          tr.lastChild.dataset.col = t.columns[i];
        });
        if (todo) {
          const x = el("button", "ghost-x", "Delete"); x.type = "button";
          x.addEventListener("click", () => { if (confirm("Delete this to-do?")) todoChange("/todos/delete", { id: row[col("id")] }); });
          cell(tr, x);
        }
        if (diag) {
          const id = row[col("id")], done = !!row[col("done")], hub = which, b = el("button", "pill", done ? "Reopen" : "Done");
          b.type = "button";
          b.addEventListener("click", async () => { b.disabled = true; try { await postJSON("/diagnostics/done", { hub, id, done: !done }); await refresh(); } catch { b.disabled = false; } });
          cell(tr, b);
        }
        if (t.name === "reserved") {
          const name = row[t.columns.indexOf("spelling")] ?? row[0];
          const x = el("button", "ghost-x", "Remove"); x.type = "button";
          x.addEventListener("click", async () => { x.disabled = true; try { await changeReserved({ action: "remove", name }); } catch { x.disabled = false; } });
          cell(tr, x);
        }
      }
      if (!t.rows.length) cell(body.insertRow(), "Empty.", "empty").colSpan = t.columns.length + (t.name === "reserved" || diag ? 1 : todo ? 2 : 0);
      wrap.append(table);
      pane.append(wrap);
      }
    }
    const keep = box.querySelector(".tpane .grid"), scroll = keep ? [keep.scrollLeft, keep.scrollTop, box.querySelector(".tpane").dataset.name] : null;
    box.replaceChildren(side, pane);
    if (usersView) part(() => drawAllUsers(last?.[which]));
    if (t) pane.dataset.name = t.name;
    if (scroll && t && scroll[2] === t.name) { const g = pane.querySelector(".grid"); g.scrollLeft = scroll[0]; g.scrollTop = scroll[1]; }
  };

  // ---- live Yos in the title bar: per minute for the last hour from the collector (bots left out, counts only), the
  // last ten minutes as bars. Each new Yo comes on the hub's stream (/live, Server-Sent Events, its kind only): a dot
  // leaves the "Yo" logo, arcs over the rate and rains down into the running minute's bar, which then takes it with a
  // tiny bump. Several at once leave one after another, but none waits longer than LIVE_WAIT: a Yo sent now is seen
  // now. Without the stream, new Yos come from the minutes' counts, spread over the refresh. Reduced motion or a
  // hidden page: no dots, the bar at once.
  // The stream is watched: a Yo or the proxy's ping (every 15 s) keeps it alive; silent for LIVE_DEAD it is taken as
  // cut (a connection Cloudflare or the network dropped without telling the browser) and opened again, refused it is
  // tried again sooner, then later (LIVE_RETRY, doubling up to LIVE_RETRY_MAX). While it is away the strip dims.
  const LIVE_SPREAD = 10000, LIVE_CAP = 30, LIVE_BARS = 10, LIVE_GAP = 170, LIVE_FLY = 1500, LIVE_WAIT = 400;
  const LIVE_DEAD = 35000, LIVE_RETRY = 2000, LIVE_RETRY_MAX = 30000, LIVE_DIM = 6000;
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  let liveSeen = null, liveTimers = [], liveData = null, liveAdd = 0, liveShown = null, liveNext = 0, liveFlying = 0;
  let liveSource = null, liveOpen = false, liveRetry = null, liveBack = LIVE_RETRY, liveHeard = 0, liveLost = 0;
  const liveStop = () => {
    liveTimers.forEach(clearTimeout);
    liveTimers = [];
    liveNext = 0;
    liveFlying = 0;
    $("live-lane").replaceChildren();
    if (liveData) paintLive();
  };
  const liveLater = (ms, f) => {
    const t = setTimeout(() => { liveTimers = liveTimers.filter((x) => x !== t); f(); }, ms);
    liveTimers.push(t);
  };
  // One dot from the logo's right edge into the last bar (rects read once, then transform and opacity only).
  const liveFly = (land) => {
    const lane = $("live-lane"), bar = $("live-spark").lastElementChild, logo = $("hub-word");
    if (!bar || document.visibilityState !== "visible") return land();
    const o = lane.getBoundingClientRect(), l = logo.getBoundingClientRect(), b = bar.getBoundingClientRect();
    const sx = l.right - o.left - 7, sy = l.top + l.height / 2 - o.top;
    const ex = b.left + b.width / 2 - o.left + (Math.random() - 0.5) * Math.min(3, b.width * 0.5), ey = b.top - o.top;
    // The arc: up from the logo over the rate, a little above the bars, to just over the bar; then the fall.
    const hy = -9 - Math.random() * 5, hx = ex - 2 + Math.random() * 4, cx = (sx + hx) / 2, cy = hy - 10;
    const frames = [], ARC = 0.62;
    for (let i = 0; i <= 8; i++) {
      const t = i / 8, u = 1 - t;
      frames.push({ offset: ARC * t, transform: `translate(${(u * u * sx + 2 * u * t * cx + t * t * hx).toFixed(1)}px, ${(u * u * sy + 2 * u * t * cy + t * t * hy).toFixed(1)}px) scale(${(0.5 + 0.5 * Math.min(1, t * 4)).toFixed(2)})`,
        opacity: Math.min(1, t * 5), easing: "linear" });
    }
    frames[frames.length - 1].easing = "cubic-bezier(.5, 0, .9, .6)";
    frames.push({ offset: 0.96, transform: `translate(${ex.toFixed(1)}px, ${(ey - 1).toFixed(1)}px) scale(.8, 1.15)`, opacity: 1, easing: "ease-out" });
    frames.push({ offset: 1, transform: `translate(${ex.toFixed(1)}px, ${(ey + 1).toFixed(1)}px) scale(.4)`, opacity: 0 });
    const s = el("i", "yo-spark");
    lane.append(s);
    s.animate(frames, { duration: LIVE_FLY * (0.92 + Math.random() * 0.16), easing: "cubic-bezier(.3, .1, .6, 1)", fill: "forwards" })
      .finished.then(() => { if (!s.isConnected) return; s.remove(); land(); }, () => {});
    if (!logo.getAnimations().length) logo.animate([{ transform: "scale(1)" }, { transform: "scale(1.045)" }, { transform: "scale(1)" }], { duration: 320, easing: "ease-out" });
  };
  // The running minute's bar takes a Yo: a tiny bump from its foot.
  const liveBump = () => {
    const bar = $("live-spark").lastElementChild;
    if (bar && !calm.matches) bar.animate([{ transform: "scaleY(1)" }, { transform: "scaleY(1.22)" }, { transform: "scaleY(1)" }], { duration: 260, easing: "ease-out" });
  };
  // A Yo to show: its dot leaves at the next free slot after the last, so a burst reads as rain; land paints it.
  const liveSpark = (land = () => {}) => {
    const lane = $("live-lane");
    if (calm.matches || document.visibilityState !== "visible" || lane.childElementCount + liveTimers.length > LIVE_CAP) return land();
    const now = performance.now(), at = Math.max(now, Math.min(liveNext, now + LIVE_WAIT));
    liveNext = at + LIVE_GAP * (0.8 + Math.random() * 0.4);
    liveLater(Math.round(at - now), () => liveFly(() => { land(); liveBump(); }));
  };
  // The strip from the last minutes and the Yos streamed since; the running minute never goes down within itself.
  const paintLive = () => {
    const box = $("live"), d = liveData;
    box.hidden = !d || !Array.isArray(d.minutes);
    if (box.hidden) return;
    const m = d.minutes.slice(), n = m.length, full = m.slice(-6, -1);
    m[n - 1] += liveAdd - liveFlying;
    const shown = `${which} ${d.minute}`;
    if (liveShown?.key === shown) m[n - 1] = Math.max(m[n - 1], liveShown.v);
    liveShown = { key: shown, v: m[n - 1] };
    // The rate: the last five whole minutes, one decimal below ten.
    const rate = full.reduce((a, b) => a + b, 0) / Math.max(1, full.length);
    set("live-n", rate >= 10 ? Math.round(rate).toLocaleString() : rate.toFixed(1).replace(/\.0$/, ""));
    const hour = m.reduce((a, b) => a + b, 0);
    if (!box._h) { box._h = 1; hint(box, () => box._tip); }
    box._tip = `${num(hour)} Yos in the last hour · ${num(m[n - 1])} this minute (bots left out)${box.dataset.state ? ` · ${box.dataset.state}` : ""}`;
    box.setAttribute("aria-label", `${rate.toFixed(1)} Yos per minute, ${hour} in the last hour`);
    // The last ten minutes as bars like Problems', the running minute (bright) at the right.
    $("live-spark").replaceChildren(...bars(m.slice(-LIVE_BARS), LIVE_BARS * 10, 24));
  };
  const drawLive = (h) => {
    const d = h?.live;
    liveData = d && Array.isArray(d.minutes) ? d : null;
    liveAdd = 0;
    paintLive();
    if (!liveData) { liveSeen = null; liveStop(); return; }
    const m = d.minutes, n = m.length;
    // New Yos by the minutes: what this refresh counts above the last one (the running minute only on the first look).
    // The stream, while open, shows them instead.
    const key = `${which}`, byMinute = new Map(m.map((v, i) => [d.minute - (n - 1) + i, v]));
    let fresh = 0;
    if (liveSeen?.key === key) for (const [k, v] of byMinute) fresh += Math.max(0, v - (liveSeen.m.get(k) ?? (k > liveSeen.minute ? 0 : v)));
    else { liveStop(); fresh = m[n - 1]; }
    liveSeen = { key, minute: d.minute, m: byMinute };
    if (liveOpen || calm.matches || document.visibilityState !== "visible") return;
    fresh = Math.min(LIVE_CAP, fresh);
    for (let i = 0; i < fresh; i++) liveLater(Math.round(((i + Math.random()) / fresh) * LIVE_SPREAD), () => liveSpark());
  };
  // The strip dims (a quiet "reconnecting" in its tip) while the stream of a visible page is away.
  const liveState = () => {
    const box = $("live"), away = document.visibilityState === "visible" && typeof EventSource !== "undefined" &&
      !liveOpen && liveLost && Date.now() - liveLost > LIVE_DIM;
    if (box.classList.contains("away") === !!away) return;
    box.classList.toggle("away", !!away);
    box.dataset.state = away ? "live feed reconnecting…" : "";
    if (liveData) paintLive();
  };
  const liveClose = () => {
    clearTimeout(liveRetry);
    liveRetry = null;
    liveSource?.close();
    liveSource = null;
    liveOpen = false;
  };
  const liveDown = () => {
    if (liveOpen || !liveLost) liveLost = Date.now();
    liveOpen = false;
  };
  // The stream of the hub shown, while the page is visible; the minutes' counts meanwhile.
  const liveConnect = () => {
    liveClose();
    if (document.visibilityState !== "visible" || typeof EventSource === "undefined") return;
    const hub = which, source = new EventSource(`/live?hub=${hub}`);
    liveSource = source;
    liveHeard = Date.now();
    const mine = () => hub === which && source === liveSource;
    source.onopen = () => {
      if (!mine()) return;
      liveOpen = true;
      liveLost = 0;
      liveBack = LIVE_RETRY;
      liveHeard = Date.now();
      liveState();
    };
    source.addEventListener("ping", () => { if (mine()) liveHeard = Date.now(); });
    source.onmessage = () => {
      if (!mine()) return;
      liveHeard = Date.now();
      liveOpen = true;
      liveAdd++;
      if (!liveData) return;
      // Counted on landing: the bar grows by one as each dot comes down.
      liveFlying++;
      liveSpark(() => { liveFlying = Math.max(0, liveFlying - 1); paintLive(); });
    };
    source.onerror = () => {
      if (!mine()) return;
      liveDown();
      // Refused (signed out, the hub away, a Cloudflare error page): the browser gives up, so try again, later each time.
      if (source.readyState === EventSource.CLOSED) {
        liveSource = null;
        liveRetry = setTimeout(liveConnect, liveBack);
        liveBack = Math.min(LIVE_RETRY_MAX, liveBack * 2);
      }
      liveState();
    };
  };
  // The watchdog: a stream silent too long (neither Yo nor ping) is cut and opened again.
  setInterval(() => {
    if (document.visibilityState !== "visible") return;
    if (liveSource && Date.now() - liveHeard > LIVE_DEAD) { liveDown(); liveConnect(); }
    else if (!liveSource && !liveRetry) liveConnect();
    liveState();
  }, 5000);
  // Back on the page (another tab, back from the cache, the network again): a fresh stream; the bars come from
  // refresh. Wall-clock times: a laptop's sleep counts as silence.
  const liveWake = () => {
    if (document.visibilityState !== "visible") { liveStop(); liveClose(); liveLost = 0; liveState(); return; }
    if (!liveOpen || Date.now() - liveHeard > LIVE_DEAD) { liveBack = LIVE_RETRY; liveConnect(); }
  };
  document.addEventListener("visibilitychange", liveWake);
  addEventListener("pageshow", (e) => { if (e.persisted) liveWake(); });
  addEventListener("online", () => { liveBack = LIVE_RETRY; liveConnect(); });
  calm.addEventListener?.("change", () => { if (calm.matches) liveStop(); });
  liveConnect();

  // ---- everything from the collector -------------------------------------------------------------------------------
  const render = () => {
    const h = last?.[which];
    part(() => drawLive(h));
    findNewest(h);
    part(() => overview(h));
    supportRefresh();
    part(drawPerson);
    part(() => drawAllUsers(h));
    part(drawTables);
    redrawDetail();
  };
  // The list (MBs on the live hub) comes again only when it changed: asked with its ETag, a 304 keeps the one shown
  // and only moves the collector's time on (Yo-At).
  let peopleBusy = false, usersTag = null;
  async function refresh() {
    if (document.visibilityState !== "visible" || peopleBusy) return;
    peopleBusy = true;
    const status = $("collection-status");
    if (!last) status.textContent = "Loading fresh data…";
    try {
      const r = await fetch("/users.json", { cache: "no-store", headers: last && usersTag ? { "If-None-Match": usersTag } : {} });
      let changed = r.status !== 304;
      if (!changed) {
        const at = Number(r.headers.get("Yo-At"));
        if (at && !last.stale) { last.at = at; if (last.updates) last.updates.at = at / 1000; }
      } else {
        if (!r.ok) throw new Error("collector unavailable");
        last = await r.json();
        usersTag = r.headers.get("ETag");
      }
      updates(last.updates);
      const doneResponse = await fetch("/done.json", { cache: "no-store" });
      const done = new Set(doneResponse.ok ? await doneResponse.json() : []);
      if (done.size !== lastDone.size || [...done].some((k) => !lastDone.has(k))) changed = true;
      lastDone = done;
      if (changed) render();
      status.textContent = last.stale ? "Stale data · waiting for the collector" : "";
    } catch {
      status.textContent = last ? "Stale data · refresh failed" : "Waiting for fresh data…";
    } finally { peopleBusy = false; }
  }

  // ---- status bot (skills/status): its last report in one line on the Overview, how often it sends right there -----
  const drawStatus = (d) => {
    const l = d.last, c = d.config || {}, at = l && l.at ? l.at * 1000 : null;
    svcInfo.stbot = { rows: [
      ["Last report", l ? `${clock(at)} · ${l.sent ? "sent" : "not sent"}` : "none yet", l && !l.sent ? "bad" : ""],
      ["Every", !c.every_minutes ? "off" : c.every_minutes === 1440 ? "day (09:00 Berlin)" : `${c.every_minutes} min`],
    ], ok: !!(c.every_minutes && l?.sent) };
    drawCli();
    redrawDetail(["svc", "server"]);
    if (document.activeElement !== $("st-every")) $("st-every").value = String(c.every_minutes ?? 0);
  };
  async function statusRefresh() {
    try {
      const r = await fetch("/status.json", { cache: "no-store" });
      if (r.ok) drawStatus(await r.json());
    } catch {}
  }
  $("st-every").addEventListener("change", async (e) => {
    try {
      drawStatus(await (await postJSON("/status", { every_minutes: Number(e.target.value) })).json());
    } catch { if (svcInfo.stbot) { svcInfo.stbot.rows.push(["", "not saved, try again", "bad"]); redrawDetail(["svc", "server"]); } }
  });

  // ---- one page of widgets; the Database widget is always open. Rows of Attention and Usage jump there: Feedback to
  // the support chat, Errors to diagnostics, Reports to reports, Sign-ups (and the Users card's title) to the users
  // table. Services is one row in Server, opened in place (on its own while something is down); the Status bot row
  // there shows its frequency select. Keys: 1 top, 2 chat, 3 diagnostics, 4 users, 5 database, 0 the hub.
  const showTable = (name) => { tableOpen = { users: "people", support_messages: "v-support", diagnostics: "v-errors", todos: "v-todos" }[name] || name; tablesDrawn = ""; part(drawTables); $("t-db").scrollIntoView({ behavior: "smooth", block: "start" }); };
  // The support chat shown, scrolled to only when it is out of sight (a click inside it never moves the page).
  const toInbox = () => {
    if (tableOpen !== "v-support") { tableOpen = "v-support"; tablesDrawn = ""; part(drawTables); }
    const r = $("people").getBoundingClientRect();
    if (r.bottom < 120 || r.top > innerHeight - 120) $("t-db").scrollIntoView({ behavior: "smooth", block: "start" });
  };
  // The Database widget's eye: names and ids frosted for a screenshot, remembered in this browser.
  let dbBlur = false;
  try { dbBlur = localStorage.getItem("db-blur") === "1"; } catch {}
  const drawDbBlur = () => { $("t-db").classList.toggle("blur", dbBlur); $("db-blur").setAttribute("aria-pressed", String(dbBlur)); };
  drawDbBlur();
  const toggleBlur = () => { dbBlur = !dbBlur; drawDbBlur(); try { localStorage.setItem("db-blur", dbBlur ? "1" : "0"); } catch {} };
  $("db-blur").addEventListener("click", toggleBlur);
  // A click on a name or id in the tables frosts them all (for a screenshot), a second click clears them.
  $("t-db").addEventListener("click", (e) => { if (e.target.closest(".tpane .pii")) toggleBlur(); });
  // ---- the detail: a click on a row, a service or a chart opens up to four cards about it between the widgets and the
  // Database, in the range above (the server's own last 24 hours); the same click again, × or Escape closes it.
  // Closed by default; the address remembers it (#detail-speed).
  const short = (v) => { const a = Math.abs(v); return a >= 1e9 ? `${(v / 1e9).toFixed(1)}G` : a >= 1e6 ? `${(v / 1e6).toFixed(a >= 1e7 ? 0 : 1)}M` : a >= 1e4 ? `${Math.round(v / 1e3)}k` : a >= 1e3 ? `${(v / 1e3).toFixed(1)}k` : `${Math.round(v)}`; };
  // Every chart in the detail is three parts: the title with its value, the chart (its first and last slot as the only
  // axis), and one quiet line under it: the legend or a short note. A click on a slot names it in that line instead;
  // a second click on it gives the legend back. The pick (by slot name) survives the redraws while the detail is open.
  const picks = {};
  const cfoot = (...home) => {
    const f = el("div", "cfoot"), kids = home.flat().filter((x) => x != null && x !== "").map((x) => (typeof x === "string" ? el("span", null, x) : x));
    f.home = () => f.replaceChildren(...kids);
    f.home();
    return f;
  };
  // A legend entry: a small swatch (its colour from the class or --c) and a word.
  const key = (text, cls, color) => { const k = el("span", `key ${cls || ""}`.trim(), text); if (color) k.style.setProperty("--c", color); return k; };
  const pickAt = (s, names, groups, foot, id, say) => {
    const n = names.length;
    const show = (i) => {
      s.classList.toggle("picking", i >= 0);
      groups.forEach((g, k) => g.classList.toggle("on", k === i));
      if (i >= 0) foot.replaceChildren(el("span", "pick", say(i))); else foot.home();
    };
    s.addEventListener("click", (e) => {
      const r = s.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n)));
      picks[id] = picks[id] === names[i] ? null : names[i];
      show(names.indexOf(picks[id]));
    });
    show(picks[id] == null ? -1 : names.indexOf(picks[id]));
  };
  const axis = (s, W, H, from, to) => {
    const label = (x, text, anchor) => { const t = svg("text", { x, y: H - 4, "text-anchor": anchor, class: "ax" }); t.textContent = text; return t; };
    s.append(label(0, from || "", "start"), label(W, to || "", "end"));
  };
  // Bars on a base line, no numbers in the plot; the first and last slot named below.
  const barChart = (d, cls, foot, id) => {
    const W = 264, H = 96, top = 4, bottom = 18, n = d.values.length, slot = W / Math.max(1, n);
    const max = d.scale || Math.max(1, ...d.values.map((v) => v || 0)), s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: `hc-chart ${cls || ""}` });
    s.append(svg("line", { x1: 0, x2: W, y1: H - bottom + .5, y2: H - bottom + .5, class: "base" }));
    const bars = d.values.map((v, i) => {
      const h = v ? Math.max(2, (v / max) * (H - top - bottom)) : 1.5, x = i * slot + slot * 0.14;
      return svg("rect", { x: x.toFixed(1), width: (slot * 0.72).toFixed(1), y: (H - bottom - h).toFixed(1), height: h.toFixed(1), rx: 2, class: v ? "b" : "b zero" });
    });
    s.prepend(...bars);
    axis(s, W, H, d.names[0], d.names[n - 1]);
    pickAt(s, d.names, bars, foot, id, (i) => `${d.names[i]} · ${d.values[i] == null ? "–" : d.tip ? d.tip(i, d.values[i]) : (d.fmt || num)(d.values[i])}`);
    return s;
  };
  // Rows with a bar: [label, value, share 0–1 or null for no bar].
  const barRows = (rows) => {
    const box = el("div", "hc-plats");
    for (const [l, v, f] of rows) {
      const row = el("div", "hc-plat"), bar = el("span", "hc-pbar");
      if (f != null) { const x = el("span", "current"); x.style.width = `${Math.max(0, Math.min(1, f)) * 100}%`; bar.append(x); } else bar.classList.add("none");
      row.append(el("b", null, l), bar, el("span", "hc-pn", v));
      box.append(row);
    }
    return box;
  };
  const kvRows = (rows) => {
    const dl = el("dl", "hc-kv");
    for (const [k, v, c] of rows) dl.append(el("dt", null, k), el("dd", c || null, v));
    return dl;
  };
  // Platforms: a share per platform in the row's colours (iOS amethyst, Android green, Bot grey, CLI yellow).
  const platCard = () => {
    // One bar of the split as parts of 100 %: iOS amethyst, Android green, Bot grey; each platform's count and share in its hover.
    const plats = platData?.plats || [], all = Math.max(1, plats.reduce((n, [, p]) => n + p.all, 0)), bar = el("span", "hc-pbar plats");
    for (const [l, p] of plats) {
      const x = el("span", `p-${l.toLowerCase()}`); x.style.width = `${(100 * p.all) / all}%`;
      hint(x, { cap: 1, head: l, rows: [{ label: "Devices", value: num(p.all) }, { label: "Share", value: `${Math.round((100 * p.all) / all)} %` }] });
      bar.append(x);
    }
    const box = el("div", "hc-plats");
    box.append(bar);
    return box;
  };
  const vLegend = () => {
    return cfoot(["current", "update", "must", "unknown"].map((k) => key(k, k)));
  };
  const VNAME = { current: "Current", update: "Update out", must: "Must update", unknown: "Unknown" }, VCLS = { current: "ok", update: "warn", must: "bad", unknown: "" };
  // One bar of a platform's versions: current, update out, must update, unknown.
  const verBar = (p) => { const b = el("span", "hc-pbar dbar"); for (const k of VKEYS) if (p[k]) { const x = el("span", k); x.style.flexGrow = p[k]; b.append(x); } return b; };
  // Releases over time: of the devices seen per slot (an hour of the last 24 for 1, else a day), each build's share as
  // its own band, 100 % stacked, the newest on top: the current build green, older ones greyer with age, builds below
  // the hub's minimum reddish. The six newest builds get bands, the rest are "older". A click on the chart names that
  // slot's shares in the line under it (the newest slot until then), the pick kept across redraws.
  let relPick = null;
  const RTOP = 6, REL_SHADES = ["#b98bd0", "#a48fb8", "#9690a0", "#86828e", "#77737d"], MUST_SHADES = ["#d0695e", "#b45d55", "#9a534c", "#844a45"];
  // iOS TestFlight builds first (newest first), then other platforms' dated builds, then dev builds and unknown.
  const buildKey = (app) => { const b = build(app); return /^\d{12}$/.test(b) ? `${String(app).startsWith("ios/") ? 2 : 1}${b}` : "0"; };
  const buildLabel = (app) => {
    const b = build(app), p = (app || "").split("/")[0], word = { ios: "", android: "Android ", yogpt: "Bot " }[p] ?? `${appInfo(app)?.label || p} `;
    if (!/^\d{12}$/.test(b)) return app === "unknown" ? "unknown" : `${word || "iOS "}${b}`.trim();
    const mon = new Date(Date.UTC(2000, +b.slice(4, 6) - 1, 1)).toLocaleDateString("en", { month: "short", timeZone: "UTC" });
    return `${word}${mon} ${+b.slice(6, 8)} ${b.slice(8, 10)}:${b.slice(10, 12)}`;
  };
  const REL_STATES = [["current", "Current", "var(--ok)"], ["fresh", "Fresh", "#a3e4b8"], ["update", "Update", "var(--warn)"], ["must", "Must update", "var(--bad)"], ["unknown", "Unknown", "var(--muted)"]];
  const relData = (h, plat) => {
    const t = today0(), kept = { ...(h.build_days || {}) };
    if (Array.isArray(h.apps_today) && h.apps_today.length) kept[t] = Object.fromEntries(h.apps_today.filter(([a]) => !String(a).startsWith("cli/")));
    let slots;
    if (range === 1) {
      const hrs = h.build_hours || {}, h1 = Math.floor(Date.now() / 3600000);
      const hourName = (x) => { const d = new Date(x * 3600000); return `${d.toDateString() === new Date().toDateString() ? "" : "yesterday "}${d.toLocaleTimeString(undefined, { timeStyle: "short" })}`; };
      slots = Array.from({ length: 24 }, (_, i) => h1 - 23 + i).map((x) => [x, x === h1 ? "now" : hourName(x), hrs[x] || null]);
    } else {
      const first = Object.keys(kept).map(Number).filter((d) => d <= t).sort((a, b) => a - b)[0] ?? t;
      const span = range || Math.max(7, t - first + 1), step = Math.max(1, Math.ceil(span / 60)), n = Math.ceil(span / step);
      slots = Array.from({ length: n }, (_, i) => t - (n - 1 - i) * step).map((e) => {
        let row = null;
        for (let d = e; d > e - step && !row; d--) row = kept[d] || null;
        return [e, e === t ? "today" : dayName(e), row];
      });
    }
    // The bands are the app tile's states, bottom to top: current, fresh, update, must update, unknown (dev builds and
    // the like). The builds stay in each slot's `raw` row, for the hover.
    const stateOf = (a) => (belowMin(a) ? "must" : appInfo(a)?.state === "current" ? "current" : appInfo(a)?.state === "fresh" ? "fresh" : appInfo(a)?.state === "update" ? "update" : "unknown");
    const mine = (a) => (plat === "all" ? /^(ios|android)\//.test(String(a)) : String(a).startsWith(`${plat}/`));
    // Update builds each get a band in yellow shades, the newest lightest (at most five, the rest together as "older");
    // the other states are one band each.
    const upd = new Map();
    for (const [, , row] of slots) for (const [ap, c] of Object.entries(row || {})) if (c > 0 && mine(ap) && stateOf(ap) === "update") upd.set(ap, (upd.get(ap) || 0) + c);
    const updApps = [...upd.keys()].sort((x, y) => buildKey(y).localeCompare(buildKey(x))), ownUpd = updApps.slice(0, 5), YELLOWS = ["#f6dc6b", "#e8c744", "#cdaa2c", "#ac8a21", "#8c6e1a"];
    const bands = [{ k: "current", name: "Current", color: "var(--ok)" }, { k: "fresh", name: "Fresh", color: "#a3e4b8" },
      ...ownUpd.map((ap, n) => ({ k: "update", name: buildLabel(ap), color: YELLOWS[n], app: ap })), ...(updApps.length > 5 ? [{ k: "update", name: "older", color: "#6f5714", older: true }] : []),
      { k: "must", name: "Must update", color: "var(--bad)" }, { k: "unknown", name: "Unknown", color: "var(--muted)" }];
    const bandOf = (ap) => { const s = stateOf(ap); return s === "update" ? bands.findIndex((b) => b.k === "update" && (b.app === ap || b.older && !ownUpd.includes(ap))) : bands.findIndex((b) => b.k === s); };
    const rows = slots.map(([, , row]) => {
      if (!row) return null;
      const r = bands.map(() => 0);
      for (const [ap, c] of Object.entries(row)) if (mine(ap)) r[bandOf(ap)] += c;
      return r.some((v) => v) ? r : null;
    });
    return { slots, bands, rows, raw: slots.map(([, , row]) => (row ? Object.entries(row).filter(([a, c]) => mine(a) && c > 0) : [])) };
  };
  const relChart = (h, plat, titleText) => {
    const d = relData(h, plat), n = d.slots.length, W = 264, H = 96, bottom = 18, B = H - bottom;
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: "hc-chart rchart" });
    const have = d.rows.map((r, i) => (r ? i : -1)).filter((i) => i >= 0), f0 = have[0] ?? 0, span = Math.max(1, n - 1 - f0);
    const xs = (i) => (n - 1 > f0 ? ((i - f0) * W) / span : W / 2);
    const label = (x, text, anchor) => { const t = svg("text", { x, y: H - 4, "text-anchor": anchor, class: "ax" }); t.textContent = text; return t; };
    s.append(svg("line", { x1: 0, x2: W, y1: B + 0.5, y2: B + 0.5, class: "base" }));
    if (!have.length) return dcard(titleText, null, note("No builds kept yet: counted per hour and day from now on."));
    // The bands' edges at each kept slot (a lone slot drawn as a thin column), bottom band first.
    const pts = have.length > 1 ? have : [have[0], have[0]];
    const x = (k, i) => (have.length > 1 ? xs(i) : xs(i) + (k ? 3 : -3));
    const tops = pts.map((i) => { const r = d.rows[i], all = r.reduce((a, v) => a + v, 0); let y = 0; return r.map((v) => (y += v / all)); });
    d.bands.forEach((b, j) => {
      if (!tops.some((c) => c[j] - (j ? c[j - 1] : 0) > 0)) return;
      const up = pts.map((i, k) => `${x(k, i).toFixed(1)},${(B - tops[k][j] * (B - 2)).toFixed(1)}`);
      const down = pts.map((i, k) => `${x(k, i).toFixed(1)},${(B - (j ? tops[k][j - 1] : 0) * (B - 2)).toFixed(1)}`).reverse();
      const p = svg("polygon", { points: [...up, ...down].join(" "), class: "rb" });
      p.style.fill = b.color;
      s.append(p);
    });
    const mark = svg("line", { y1: 0, y2: B, class: "rmark" });
    s.append(mark, label(0, d.slots[f0][1], "start"), label(W, d.slots[n - 1][1], "end"));
    // Under the chart the legend (the five newest builds); a click on the chart names that slot's shares instead, a
    // second click on the same slot gives the legend back.
    const pick = d.rows[have[have.length - 1]], pickAll = pick ? pick.reduce((a, v) => a + v, 0) : 0;
    const share = pickAll ? `${Math.round((100 * (pick[0] + pick[1])) / pickAll)} % current` : "";
    // No legend: the hover names each band's build with its share at that point.
    hoverTip(s, n - f0, (k) => {
      const i = f0 + k, r = d.rows[i], all = r ? r.reduce((x, v) => x + v, 0) : 0;
      return { head: d.slots[i][1], rows: r ? d.bands.map((b, j) => ({ color: b.color, label: b.name, value: `${Math.round((100 * r[j]) / all)} %` })).filter((x) => x.value !== "0 %").reverse() : [{ label: "Builds", value: "not recorded" }] };
    });
    const card = dcard(titleText, share, s);
    return card;
  };
  // A small pill of choices in a card's header (the chosen one lit); `change(value)` when another is chosen.
  const pillOf = (options, current, change) => {
    const p = el("span", "pill-toggle");
    p.setAttribute("role", "group");
    for (const [value, label, title] of options) {
      const b = el("button", value === current ? "on" : null, label);
      b.type = "button"; if (title) b.setAttribute("aria-label", title);
      b.setAttribute("aria-pressed", String(value === current));
      b.addEventListener("click", (e) => { e.stopPropagation(); change(value); });
      p.append(b);
    }
    return p;
  };
  const saved = (k, fallback, allowed) => { try { const v = localStorage.getItem(k); return allowed.includes(v) ? v : fallback; } catch { return fallback; } };
  const save = (k, v) => { try { localStorage.setItem(k, v); } catch {} };
  let relPlat = saved("relPlat", "all", ["all", "ios", "android"]);
  const relCard = (h) => {
    const card = relChart(h, relPlat, "App");
    card.querySelector("h3")?.children[0]?.after(pillOf([["all", "All"], ["ios", "iOS"], ["android", "Android"]], relPlat, (v) => { relPlat = v; save("relPlat", v); drawDetail(); }));
    const devices = platData?.devices, plats = platData?.plats || [];
    const rows = sparkCard("Devices", [{ ico: "ios", label: "Devices", value: devices == null ? "–" : num(devices),
      node: platCard().firstChild, title: { head: "Devices", rows: plats.map(([label, p]) => ({
        label, value: `${num(p.all)} · ${pctText(100 * p.all / Math.max(1, devices))}` })) } }]);
    card.append(rows.lastChild);
    return [card];
  };
  // Speed: the range's split in one bar with its four shares named.
  const latPct = (b, i) => { const t = b.reduce((a, v) => a + v, 0); return t ? (100 * b[i]) / t : 0; };
  const speedSplit = () => {
    const d = speedData, tot = d.all.reduce((a, v) => a + v, 0), bar = el("div", "hc-pbar sp-bar");
    d.all.forEach((v, i) => { if (v) { const x = el("span", `sp${i}`); x.style.flexGrow = v; bar.append(x); } });
    const legend = el("dl", "hc-kv sp-kv");
    LAT_NAME.forEach((name, i) => legend.append(el("dt", `sp${i}`, name), el("dd", null, pctText(latPct(d.all, i)))));
    return [el("div", "sp-big", `${pctText(latPct(d.all, 0))} within 1 s`), el("div", "hc-note", `${num(tot)} Yos · ${d.span}`), bar, legend];
  };
  // The same split per bar; a bar under the pointer names its time, its Yos and its share within 1 s.
  const speedBars = (foot) => {
    const d = speedData, n = d.lat.length, W = 264, H = 96, bottom = 18, B = H - bottom, slot = W / n;
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: "hc-chart vchart" });
    const groups = d.lat.map((b, i) => {
      const g = svg("g", { class: "vb" }), t = b.reduce((a, v) => a + v, 0), x = (i * slot + slot * 0.14).toFixed(1), w = (slot * 0.72).toFixed(1);
      if (!t) g.append(svg("rect", { x, width: w, y: B - 1.5, height: 1.5, rx: 1, class: "b zero" }));
      let y = B;
      b.forEach((v, k) => { if (!v) return; const hh = (v / t) * (B - 2); y -= hh; g.append(svg("rect", { x, width: w, y: y.toFixed(1), height: hh.toFixed(1), class: `sp${k}` })); });
      return g;
    });
    s.append(...groups);
    axis(s, W, H, d.names[0], d.names[n - 1]);
    pickAt(s, d.names, groups, foot, "split", (i) => { const t = d.lat[i].reduce((a, v) => a + v, 0); return `${d.names[i]} · ${t ? `${num(t)} Yos · ${pctText(latPct(d.lat[i], 0))} within 1 s` : "no Yos"}`; });
    return s;
  };

  // A detail card: a title, its value on the right, then charts, rows and a note.
  const dcard = (title, value, ...kids) => {
    const c = el("section", "dcard"), h = el("h3");
    h.append(el("span", null, title));
    if (value instanceof Node) h.append(value); else if (value != null && value !== "") h.append(el("b", "dval", value));
    c.append(h, ...kids.flat().filter(Boolean));
    return c;
  };
  const note = (text) => (text ? el("div", "hc-lines", text) : null);
  // A chart in a detail card, drawn by the overview's painter at the card's size (again on every resize): bars and/or a
  // line, the first and last slot as the axis. A click on a slot names it in the card's line under the chart; the same
  // click again gives the line back.
  const COLORS = { "": "c-green", cool: "c-blue", bad: "c-orange", acc: "c-acc" };
  const liveChart = (spec, cls, foot, say) => {
    const box = el("div", `area dchart ${COLORS[cls || ""] || "c-green"}`), node = svg("svg", { "aria-hidden": "true" }), n = (spec.bars || spec.line).length;
    box.append(node);
    let pick = -1, seen = false;
    const ro = new ResizeObserver(() => { if (!box.isConnected) { if (seen) ro.disconnect(); return; } seen = true; paintCombo(node, { ...spec, pick }); });
    ro.observe(box);
    if (say) hoverTip(node, n, say, (i) => paintCombo(node, { ...spec, pick: i >= 0 ? i : pick, hover: i }));
    if (foot && say) node.addEventListener("click", (e) => {
      const r = node.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n)));
      pick = pick === i ? -1 : i;
      paintCombo(node, { ...spec, pick });
      const t = say(i);
      if (pick < 0) foot.home(); else foot.replaceChildren(el("span", "pick", typeof t === "string" ? t : [t.head, ...t.rows.filter(Boolean).map((r) => `${r.label} ${r.value}`)].join(" · ")));
    });
    return box;
  };
  const axisOf = (names) => (range === 1 && names.length === 24 ? { from: "24 hours ago", to: "now" } : { from: names[0], to: names[names.length - 1] });
  const sayOf = (d) => (i) => `${d.names[i]} · ${d.values[i] == null ? "–" : d.tip ? d.tip(i, d.values[i]) : (d.fmt || num)(d.values[i])}`;
  // The total large, under it what came today (green), as on the overview's tiles.
  const dnums = (big, sub, cls = "") => { const v = el("span", "dnums"); v.append(el("b", "dval", big)); if (sub) v.append(el("small", cls || null, sub)); return v; };
  const todayOf = (vals) => (range === 1 ? vals.slice(-(new Date().getUTCHours() + 1)) : vals.slice(-1)).reduce((a, v) => a + (v || 0), 0);
  // Levels (open problems, CPU, shares): bars only.
  const chartCard = (title, value, d, cls, ...more) => {
    if (!d?.values?.length) return dcard(title, value, note("No data yet."), ...more);
    const foot = cfoot();
    const say = (i) => ({ head: d.names[i], rows: [{ color: SERIES[cls] || SERIES[""], label: title, value: d.values[i] == null ? "–" : d.tip ? d.tip(i, d.values[i]) : (d.fmt || num)(d.values[i]) }] });
    return dcard(title, value, liveChart({ bars: d.values.map((v) => v || 0), max: d.scale, ...axisOf(d.names) }, cls, foot, say), foot,
      ...more.filter((x) => x && !x.classList?.contains("hc-lines")));
  };
  // Counts: each slot's as a bar under the running total (to `total` at the end), like the overview's tiles.
  // lineVals/lineFmt: the line of another measure (Media: the size over the count).
  const comboCard = (title, d, { total, cls = "", fmt = num, lineVals, lineFmt, sub, subCls, more, moreAll, unit = "" } = {}) => {
    if (!d?.values?.length) return dcard(title, null, note("No data yet."));
    const inc = (lineVals || d.values).map((v) => v || 0), added = inc.reduce((a, v) => a + v, 0), end = total ?? added, lf = lineFmt || fmt;
    let run = end - added;
    const line = [run, ...inc.map((v) => (run += v))], foot = cfoot();
    const c = SERIES[cls] || SERIES[""];
    const say = (i) => ({ head: d.names[i], rows: [
      { section: slotWord() },
      { color: c, label: unit ? unit[0].toUpperCase() + unit.slice(1) : title, value: fmt(d.values[i] || 0) },
      ...(lineVals ? [{ ico: "disk", label: "Size", value: lf(lineVals[i] || 0) }] : []),
      ...(more ? more(i) : []),
      { section: "In all" },
      { color: c, line: true, label: "Total", value: lf(line[i + 1]) },
      ...(moreAll ? moreAll(i) : []),
    ] });
    return dcard(title, dnums(lf(end), sub !== undefined ? sub : d.day24 != null ? `+${fmt(d.day24)} in 24 h` : `+${fmt(todayOf(d.values))} today`, subCls),
      liveChart({ line, bars: d.values.map((v) => v || 0), ...axisOf(d.names) }, cls, foot, say), foot);
  };
  // The queue: Yos waiting for offline phones (the line) and the size of the photos and videos waiting (the bars), as
  // the hub keeps them (users.py from its levels: per hour for 48 hours, per day for good, now live).
  const queueSlots = (h) => {
    if (range === 1) {
      const h1 = Math.floor(Date.now() / 3600000), q = h.queue_hours || {}, hrs = Array.from({ length: 24 }, (_, i) => h1 - 23 + i);
      return { names: hrs.map((x) => clock(x * 3600000)), rows: hrs.map((x) => q[x] || null) };
    }
    const t = today0(), n = range || 30, q = h.queue_days || {}, ds = Array.from({ length: n }, (_, i) => t - n + 1 + i);
    return { names: ds.map(dayName), rows: ds.map((d) => q[d] || null) };
  };
  // A sparkline like the overview's rows: a bar per slot (the last one full), no axis.
  // top: a second list stacked on the first (the first from the bottom in cls, this one above it in red).
  const sparkBars = (values, cls, tip, top2, recorded) => {
    const n = Math.max(1, values.length), sl = 140 / n, tot = values.map((v, i) => (v || 0) + (top2?.[i] || 0)), top = Math.max(1e-9, ...tot);
    const s = svg("svg", { class: "spark", viewBox: "0 0 140 28", preserveAspectRatio: "none", "aria-hidden": "true" });
    const rect = (i, y, h, c) => svg("rect", { x: (i * sl + sl * 0.16).toFixed(2), width: (sl * 0.68).toFixed(2), y: y.toFixed(1), height: h.toFixed(1), rx: 1, class: c });
    values.forEach((v, i) => {
      if (!top2) { const h = v ? Math.max(2, (v / top) * 26) : 1.5; s.append(rect(i, 28 - h, h, v ? `b ${cls || ""}`.trim() : "b zero")); return; }
      const g = svg("g", {}), h = tot[i] ? Math.max(2, (tot[i] / top) * 26) : 1.5, hb = tot[i] ? (h * (v || 0)) / tot[i] : 0;
      if (recorded && !recorded[i]) g.setAttribute("opacity", "0.65");
      if (!tot[i]) g.append(rect(i, 28 - h, h, "b zero"));
      if (hb) g.append(rect(i, 28 - hb, hb, `b ${cls || ""}`.trim()));
      if (h - hb > 0 && tot[i]) g.append(rect(i, 28 - h, h - hb, "b hot"));
      s.append(g);
    });
    if (tip) hoverTip(s, values.length, tip, (k) => [...s.children].forEach((r, j) => r.classList.toggle("on", j === k)));
    return s;
  };
  // A sparkline of stacked bars: per slot the series (each 0 to 100) one on the other from the bottom.
  const stackBars = (series, colors, tip) => {
    const n = Math.max(1, series[0]?.length || 1), W = 140, H = 28, sl = W / n, s = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" });
    for (let i = 0; i < n; i++) {
      const g = svg("g", {}); let y = H;
      series.forEach((v, k) => { const hh = ((v[i] || 0) / 100) * H; if (hh <= 0) return; y -= hh; g.append(svg("rect", { x: (i * sl + sl * 0.12).toFixed(1), width: (sl * 0.76).toFixed(1), y: y.toFixed(1), height: hh.toFixed(1), class: "b", style: `fill:${colors[k]}` })); });
      if (y === H) g.append(svg("rect", { x: (i * sl + sl * 0.12).toFixed(1), width: (sl * 0.76).toFixed(1), y: H - 1.5, height: 1.5, class: "b zero" }));
      s.append(g);
    }
    if (tip) hoverTip(s, n, tip, (k) => [...s.children].forEach((r, j) => r.classList.toggle("on", j === k)));
    return s;
  };
  // A card of rows like the overview's: an icon, the label, its value and a sparkline (each slot's popup on hover);
  // a rule before a row marked sep. Rows: { ico, label, value, cls, values, scls, tip, sep }.
  const sparkCard = (title, rows) => {
    const box = el("div", "srows");
    for (const r of rows) {
      if (r.heading) { box.append(el("div", "srow-head", r.heading)); continue; }
      const row = el("div", `srow${r.values || r.bar != null || r.stack || r.node ? "" : " wide"}${r.go ? " go" : ""}`);
      if (r.go) row.addEventListener("click", r.go);
      hint(row, r.title || r.goTitle);
      row.append(icon(r.ico || "chart"), el("span", "sl", r.label), el("b", `sv ${r.cls || ""}`.trim(), r.value ?? "–"));
      if (r.values) row.append(sparkBars(r.values, r.scls, r.tip, r.top, r.recorded));
      else if (r.node) row.append(r.node);
      else if (r.stack) row.append(stackBars(r.stack, r.stackColors, r.tip));
      else if (r.bar != null) { const b = shareBar(r.bar, "current"); b.classList.add("srow-bar"); row.append(b); }
      box.append(row);
    }
    return dcard(title, null, box);
  };
  // The delivery rows (Speed, Queue, Push, Media queue): the first group of the Problems card.
  const perfRows = (h) => {
    const d = speedData, tot = (b) => b.reduce((a, v) => a + v, 0), { names, rows } = queueSlots(h);
    return ([
      { ico: "speed", label: "Speed", value: d && tot(d.all) ? `${pctText(latPct(d.all, 0))} in 1 s` : "–", values: d ? d.lat.map((b) => (tot(b) ? latPct(b, 0) : 0)) : [],
        title: d && tot(d.all) ? (() => {
          // The whole range: each slot's split into the four buckets as stacked bars, the share within 1 s as the line over them.
          const W = 240, H = 70, n = d.lat.length, g = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" }), colors = ["var(--ok)", "#a3e4b8", "var(--warn)", "var(--muted)"];
          d.lat.forEach((b, i) => { let y = H; if (!tot(b)) return; LAT.forEach((_, k) => { const hh = (latPct(b, k) / 100) * H; y -= hh; if (hh > 0) g.append(svg("rect", { x: ((i + 0.12) * W) / n, width: (0.76 * W) / n, y, height: hh, fill: colors[k], opacity: ".85" })); }); });
          g.append(svg("polyline", { fill: "none", stroke: "var(--text)", "stroke-width": 1.5, "vector-effect": "non-scaling-stroke", "stroke-linejoin": "round",
            points: d.lat.map((b, i) => (tot(b) ? `${(((i + 0.5) * W) / n).toFixed(1)},${(H - (latPct(b, 0) / 100) * H).toFixed(1)}` : null)).filter(Boolean).join(" ") }));
          return { cap: 1, head: `${pctText(latPct(d.all, 0))} in 1 s · ${pctText(latPct(d.all, 0) + latPct(d.all, 1))} in 5 s`, nodes: [g],
            rows: [...LAT_NAME.map((nm, k) => ({ color: colors[k], label: nm, value: pctText(latPct(d.all, k)) })), { ico: "send", label: "Yos", value: num(tot(d.all)) }] };
        })() : null,
        tip: (i) => ({ head: d.names[i], rows: tot(d.lat[i]) ? [...LAT_NAME.map((n, k) => ({ color: ["var(--ok)", "#a3e4b8", "var(--warn)", "var(--muted)"][k], label: n, value: pctText(latPct(d.lat[i], k)) })),
          { ico: "send", label: "Yos", value: num(tot(d.lat[i])) }] : [{ ico: "send", label: "Yos", value: "none" }] }) },
      { ico: "clock", label: "Queue", value: `${num(h.messages)} Yos`, values: rows.map((r) => r?.[0] || 0), scls: "cool",
        tip: (i) => ({ head: names[i], rows: [{ color: "#5dade2", label: "Yos waiting", value: rows[i] ? num(rows[i][0]) : "not kept" }] }) },
      ...(pushData ? [(() => { const p = pushData, counts = healthData.ps, sum = (v) => v.reduce((x, y) => x + y, 0), at = (i) => ({ head: p.names[i], rows: [{ ico: "send", label: "Pushes sent", value: num(p.sent[i]) },
          { label: "Taken", value: p.sent[i] ? pctText(100 * counts[i][0] / p.sent[i]) : "–" }, { label: "Failed", value: num(counts[i][1]) }, { label: "Folded, 1 s window", value: num(p.folded[i]) }, { label: "Folded, backlog", value: num(p.paced[i]) },
          { label: "In backlog pacing now", value: h.push_backlog_devices == null ? "–" : num(h.push_backlog_devices) }] });
        const sent = sum(p.sent), [ok, failed] = healthData.psAll;
        const taken = evSlots("push-ok"), failures = evSlots("push-fail");
        const bars = taken.values.map((v, i) => (v || 0) + (failures.values[i] || 0));
        let running = 0;
        const line = [0, ...bars.map(v => (running += v))];
        const chart = liveChart({ bars, line, ...axisOf(taken.names) }, "cool");
        return { ico: "send", label: "Push", title: { head: "Push", nodes: [chart],
          rows: [{ label: "Sent", value: num(sent) }, { label: "Taken", value: sent ? pctText(100 * ok / sent) : "–" },
            { label: "Failed", value: num(failed) }] }, value: num(sent), values: p.sent, scls: "cool", tip: at }; })()] : []),
      { ico: "image", label: "Media queue", value: h.blobs ? bytes(h.blobs[1]) : "–", values: rows.map((r) => r?.[1] || 0), scls: "cool",
        tip: (i) => ({ head: names[i], rows: [{ ico: "disk", label: "Media waiting", value: rows[i] ? bytes(rows[i][1]) : "not kept" }] }) },
    ]);
  };
  // Push pacing (spec 6.7): pushes sent, folded by the 1 s window, folded by the backlog pacing, devices paced now.
  const pacingCard = (h) => {
    const d = pushData, tot = (v) => v.reduce((a, x) => a + x, 0);
    if (!d) return null;
    const bars = (label, values, text) => ({ ico: "send", label, value: num(tot(values)), values, scls: "cool", tip: (i) => ({ head: d.names[i], rows: [{ ico: "send", label: text, value: num(values[i]) }] }) });
    return sparkCard("Push pacing", [
      bars("Pushes", d.sent, "Pushes sent"),
      bars("Folded, 1 s window", d.folded, "Folded by the window"),
      bars("Folded, backlog", d.paced, "Folded by the backlog pacing"),
      { ico: "clock", label: "In backlog pacing now", value: h.push_backlog_devices == null ? "–" : num(h.push_backlog_devices) },
    ]);
  };
  const queueCard = (h) => {
    const { names, rows } = queueSlots(h);
    const value = dnums(`${num(h.messages)} Yos`, h.blobs ? `${bytes(h.blobs[1])} · ${num(h.blobs[0])} photos and videos` : null, "muted");
    if (!rows.some(Boolean)) return dcard("Queue", value, note("Kept from now on."));
    const foot = cfoot([key("Yos waiting", null, "#5dade2"), key("Media size", "dim", "#5dade2")]);
    return dcard("Queue", value, liveChart({ line: rows.map((r) => r?.[0] ?? null), bars: rows.map((r) => r?.[1] || 0), ...axisOf(names) }, "cool", foot,
      (i) => `${names[i]} · ${rows[i] ? `${num(rows[i][0])} Yos · ${bytes(rows[i][1])}` : "–"}`), foot);
  };
  // Speed: the share of Yos on the friend's phone within 1 s, per slot as a line; the range's share in the title.
  const speedCard = () => {
    const d = speedData, tot = (b) => b.reduce((a, v) => a + v, 0);
    if (!d || !tot(d.all)) return null;
    const vals = d.lat.map((b) => (tot(b) ? latPct(b, 0) : null)), foot = cfoot();
    return dcard("Speed", dnums(`${pctText(latPct(d.all, 0))}`, "within 1 s", "muted"), liveChart({ line: vals, ...axisOf(d.names) }, "", foot,
      (i) => `${d.names[i]} · ${vals[i] == null ? "no Yos" : `${pctText(vals[i])} within 1 s · ${num(tot(d.lat[i]))} Yos`}`), foot);
  };
  // Yos per active user per day (the days admin.py kept the active count): 14 days, 30 for 30 days and Total.
  const perUserChart = (h) => {
    const kept = h.active_days || {}, yo = year(h.events?.["yo-in"]?.year), t = today0(), len = range === 30 || range === 0 ? 30 : 14;
    const ds = Array.from({ length: len }, (_, i) => t - len + 1 + i);
    const vals = ds.map((d) => { const a = d === t ? Math.max(kept[d] || 0, h.active_day || 0) : kept[d] || 0; return a ? (yo.get(d) || 0) / a : null; });
    if (vals.filter((v) => v != null).length < 2) return null;
    const foot = cfoot();
    return [liveChart({ line: vals, from: dayName(ds[0]), to: "today" }, "", foot, (i) => `${dayName(ds[i])} · ${vals[i] == null ? "–" : vals[i].toFixed(1)}`), foot];
  };
  const shareBar = (f, cls) => { const b = el("span", "hc-pbar dbar"), x = el("span", cls || "current"); x.style.width = `${Math.max(0, Math.min(1, f || 0)) * 100}%`; b.append(x); return b; };
  const spanWord = () => (hourView ? "last hour (where counted per minute)" : range === 1 ? "last 24 hours" : range ? `last ${range} days` : "since the start");
  // A count kept per day (and per hour for the last 24) over the range: an hour each (1), a day each (7, 30), up to 12
  // stretches (Total).
  const perSlot = (perDay, hours) => {
    if (range === 1) return { names: hourNames(), values: last24(hours).map((v) => v || 0) };
    const slots = rangeSlots(perDay);
    return { names: slots.map(([n]) => n), values: slots.map((s) => slotSum(s, perDay, hours)) };
  };
  // The last 24 hours of an event kind, rolling (not since UTC midnight): the "in 24 h" sub-lines.
  const last24sum = (k) => last24(hubNow?.events?.[k]?.hours).reduce((a, v) => a + (v || 0), 0);
  const evSlots = (k) => ({ ...(minuteSlots(k) || perSlot(year(hubNow?.events?.[k]?.year), hubNow?.events?.[k]?.hours)), day24: last24sum(k) });
  const sumOf = (d) => d.values.reduce((a, v) => a + (v || 0), 0);
  // Users seen in the range: today (1), 7 days (7), 30 days (30, Total).
  const activeIn = () => (range === 1 ? hubNow?.active_day : range === 7 ? hubNow?.active_users : hubNow?.active_month) ?? 0;
  const activeWord = () => (range === 1 ? "today" : range === 7 ? "in 7 days" : "in 30 days");
  // Users seen per day over a 7 or 30 day range (kept by admin.py), today's at least the hub's own count.
  const activeDays = () => {
    if (range !== 7 && range !== 30) return null;
    const kept = hubNow.active_days || {}, ds = Array.from({ length: range }, (_, i) => today0() - range + 1 + i);
    return { days: ds, names: ds.map(dayName), values: ds.map((d) => (d === today0() ? Math.max(kept[d] || 0, hubNow.active_day || 0) : kept[d] || 0)) };
  };
  // Crash reports in the range, per slot: per hour for the last 24 hours (the hub's "crash" events), per UTC day
  // otherwise (diagnostic_days, 14 days kept; older days from the events). A hub that does not count crash events
  // yet: the last 7 days, today's count as the number.
  const crashSlots = (h) => {
    const ev = h.events?.crash, t0 = today0(), perDay = (d) => (h.diagnostic_days || []).filter((r) => r.kind === "crash" && r.day === d).reduce((a, r) => a + r.count, 0);
    if (range === 1 && ev?.total > 0) {
      const h1 = Math.floor(Date.now() / 3600000), values = last24(ev.hours).map((v) => v || 0);
      return { values, names: values.map((_, i) => clock((h1 - 23 + i) * 3600000)), total: values.reduce((a, v) => a + v, 0), what: "last 24 hours" };
    }
    const evDays = year(ev?.year), kept = new Set((h.diagnostic_days || []).map((r) => r.day));
    const day = (d) => (kept.has(d) ? perDay(d) : evDays.get(d) || 0);
    const len = range === 1 ? 7 : range || 30, ds = Array.from({ length: len }, (_, i) => t0 - len + 1 + i), values = ds.map(day);
    const total = range === 1 ? perDay(t0) : range ? values.reduce((a, v) => a + v, 0)
      : Math.max((h.diagnostic_days || []).filter((r) => r.kind === "crash").reduce((a, r) => a + r.count, 0), ev?.total || 0);
    return { values, names: ds.map(dayName), total, what: range === 1 ? "today (UTC), per day: the hub counts hours after its next deploy" : range ? `${range} days` : "since the start" };
  };
  // The counts of the range's problems: all reports (sum of counts), the installs behind them, their share of the phones seen.
  const problemRows = (q, i) => {
    const t = problemTotals;
    return [{ label: "Open report counts", value: q.recorded[i] ? "recorded" : "estimated" }, { color: "#e67e22", label: "Errors reports", value: num(q.er[i]) }, { color: "var(--bad)", label: "Crash reports", value: num(q.cr[i]) },
      { label: "Errors open (causes)", value: num(q.ec[i]) }, { label: "Crashes open (causes)", value: num(q.cc[i]) }, { label: "Groups", value: num(q.gr[i]) },
      ...(t ? [{ label: "Installs affected (range)", value: t.installs == null ? "–" : num(t.installs) }, { label: "Of phones seen", value: t.installs == null || !t.seen ? "–" : pctText((100 * t.installs) / t.seen) }] : [])];
  };
  // Before recording began, estimate reports from the currently open groups.
  const problemSplit = (h) => {
    const now = Date.now(), open = (h.diagnostics || []).filter((d) => !d.done && !diagInfo(d).expected);
    const t0 = open.length ? Math.min(...open.map((d) => d.first_seen)) : now;
    // Each open group's reports (its count) arrive at an even point of its kind between the oldest group and now.
    const groups = (kind) => open.filter((d) => d.kind === kind).sort((x, y) => x.first_seen - y.first_seen)
      .map((d, i, l) => ({ at: t0 + ((i + 1) / l.length) * (now - t0), n: d.count || 0, cause: diagCause(d) }));
    const eg = groups("error"), cg = groups("crash");
    const upTo = (l, t) => l.filter((x) => x.at <= t), sum = (l) => l.reduce((n, x) => n + x.n, 0);
    const ends = range === 1 ? Array.from({ length: 24 }, (_, i) => Math.min(now, (Math.floor(now / 3600000) - 22 + i) * 3600000))
      : Array.from({ length: range || 30 }, (_, i) => Math.min(now, (today0() - (range || 30) + 2 + i) * 86400000));
    const names = range === 1 ? ends.map((t, i) => clock((Math.floor(now / 3600000) - 23 + i) * 3600000)) : ends.map((t, i) => dayName(today0() - (range || 30) + 1 + i));
    const at = (t) => { const e = upTo(eg, t), c = upTo(cg, t); return { er: sum(e), cr: sum(c), ec: new Set(e.map((x) => x.cause)).size, cc: new Set(c.map((x) => x.cause)).size, gr: e.length + c.length }; };
    const points = h.open_report_history || [];
    let next = 0;
    const cols = ends.map((end, i) => {
      // Closed slots exclude the next slot's midnight/hour boundary.
      const until = i === ends.length - 1 ? end : end - 1;
      while (next < points.length && points[next].t <= until) next++;
      const col = at(end), point = points[next - 1];
      return point ? { ...col, er: point.errors_open, cr: point.crashes_open, recorded: true } : col;
    });
    return { names, recorded: cols.map((x) => !!x.recorded), er: cols.map((x) => x.er), cr: cols.map((x) => x.cr), ec: cols.map((x) => x.ec), cc: cols.map((x) => x.cc), gr: cols.map((x) => x.gr) };
  };
  // Problems that came in over the range, by kind (errors per day only: today for the last 24 hours).
  const problemKinds = (h) => {
    const from = range === 1 ? Date.now() - 86400000 : range ? (today0() - range + 1) * 86400000 : 0, d0 = range === 1 ? today0() : Math.floor(from / 86400000);
    const fb = range === 1 ? last24(h.support_hours).reduce((a, v) => a + (v || 0), 0) : [...year(h.support_days)].filter(([d]) => d >= d0).reduce((a, [, v]) => a + (v || 0), 0);
    const diag = (kind) => (h.diagnostic_days || []).filter((r) => r.kind === kind && r.day >= d0).reduce((a, r) => a + r.count, 0);
    return [["Feedback", fb], ["Crashes", crashSlots(h).total], ["Errors", diag("error")], ["Reports", (h.reports || []).filter((r) => r.at >= from).length]];
  };
  const CRASH_FREE_NOTE = "1 − installs with a crash ÷ phones seen (iOS, Android; no macOS or bots), since the range's first UTC day"
    + " (24 hours: since yesterday). Estimate: the hub has no app start per install, so the phones seen stand for the installs in use.";
  const DETAIL = {
    yc: () => {
      const h = hubNow, yo = evSlots("yo-in"), md = evSlots("media-in"), n = sumOf(yo), act = activeIn(), ad = activeDays();
      const per = ad && { names: ad.names, values: ad.values.map((a, i) => (a ? yo.values[i] / a : 0)), tip: (i, v) => `${v.toFixed(1)} Yos per active user` };
      const perNote = note(`${num(n)} Yos, ${num(act)} users seen ${activeWord()}`);
      return [
        dcard("Retention", null, barRows(areaSub.yc || [])),
        per ? chartCard("Yos per active user", act ? (n / act).toFixed(1) : "–", per, "", perNote) : dcard("Yos per active user", act ? (n / act).toFixed(1) : "–", perNote),
      ];
    },
    uc: () => {
      const h = hubNow, hd = healthData || {}, a = areaData.uc, ad = activeDays(), mau = h.active_month || 0, f = (v) => (mau && v != null ? v / mau : null);
      const su = perSlot(new Map(h.signup_days || []), h.signups_hours), gone = evSlots("account-gone");
      const rows = barRows([["Today", num(h.active_day), f(h.active_day)], ["7 days", num(h.active_users), f(h.active_users)], ["30 days", num(mau), f(mau)]]);
      return [
        chartCard("Accounts", num(a?.values[a.values.length - 1]), a && { values: a.values, names: a.labels, tip: (i, v) => `${num(v)} accounts` }, "acc"),
        ad ? chartCard("Active", `${pctText(hd.stick)} DAU / MAU`, { ...ad, tip: (i, v) => `${num(v)} active` }, "cool", rows) : dcard("Active", `${pctText(hd.stick)} DAU / MAU`, rows, note("Users seen today, in 7 and in 30 days.")),
        chartCard("Sign-ups", num(sumOf(su)), { ...su, tip: (i, v) => `${num(v)} sign-ups` }, "acc"),
        chartCard("Deleted", num(sumOf(gone)), { ...gone, tip: (i, v) => `${num(v)} deleted` }, "bad"),
      ];
    },
    md: () => {
      const h = hubNow, by = evSlots("media-in-bytes"), n = evSlots("media-in"), out = evSlots("media-out"), b = h.blobs, nb = sumOf(by), nn = sumOf(n);
      return [
        comboCard("Media", n, { cls: "cool", lineVals: by.values, lineFmt: bytes, total: h.events?.["media-in-bytes"]?.total ?? nb }),
        comboCard("Fetched", out, { cls: "cool", total: h.events?.["media-out"]?.total }),
        dcard("Waiting", b ? num(b[0]) : "–", kvRows([["Photos and videos", b ? num(b[0]) : "–"], ["Size", b ? bytes(b[1]) : "–"]])),
      ];
    },
    gr: () => {
      const h = hubNow, g = evSlots("group-create");
      return [comboCard("Groups", g, { total: h.groups })];
    },
    sc: () => {
      const h = hubNow, all = h.total ? (100 * (h.secured || 0)) / h.total : null, wk = (h.secured_days || []).slice(-7).filter(Boolean);
      const made = wk.reduce((n, d) => n + d[0], 0), withKey = wk.reduce((n, d) => n + d[1], 0);
      return [
        dcard("Secured", pctText(all), barRows([["All", pctText(all), (all || 0) / 100], ["This week", made ? pctText((100 * withKey) / made) : "–", made ? withKey / made : 0]])),
        chartCard("Secured by sign-up day", null, sparkData.sc, ""),
      ];
    },
    pl: () => [dcard("Devices", num(platData?.devices), platCard())],
    vr: () => {
      const h = hubNow;
      const apps = [...(h.apps || [])].sort((a, b) => b[1] - a[1]).slice(0, 8);
      return [
        ...relCard(h),
        dcard("Builds", null, kvRows([["Hub's minimum", h.min_app || "none"], ...apps.map(([app, c]) => [`${appInfo(app)?.label || "?"} ${build(app)}`, num(c), belowMin(app) ? "bad" : VCLS[appState(app)] || ""])])),
      ];
    },
    pr: () => {
      const h = hubNow, hd = healthData || {}, d = sparkData.pr, kinds = problemKinds(h), top = Math.max(1, ...kinds.map(([, v]) => v));
      const go = el("button", "hc-hint", "Open the support chat →");
      go.type = "button"; go.addEventListener("click", () => showTable("support_messages"));
      const he = hd.he, rate = ([e, n]) => (n ? (100 * e) / n : 0);
      return [
        chartCard("Open", d ? num(d.values[d.values.length - 1]) : "–", d, "bad", go),
        dcard("Came in", num(kinds.reduce((a, [, v]) => a + v, 0)), barRows(kinds.map(([l, v]) => [l, num(v), v / top]))),
        // Crash-free installs (est.): installs with a crash in the range against the phones seen in it.
        dcard("Crash-free installs (est.)", pctText(hd.free), kvRows([
          ["Installs with a crash", hd.crashInstalls == null ? "– (hub too old)" : num(hd.crashInstalls)],
          ["Phones seen (iOS, Android)", num(hd.seen)],
          ["Crash reports", num(crashSlots(h).total)]]),
          note(CRASH_FREE_NOTE)),
        he && hd.heAll[1] ? chartCard("Hub errors", `${num(hd.heAll[0])} 5xx`, { names: hd.names, values: he.map(([e]) => e),
          tip: (i) => (he[i][1] ? `${num(he[i][0])} 5xx (${pctText(rate(he[i]))}) · ${num(he[i][2])} 4xx · ${num(he[i][3])} 429 of ${num(he[i][1])}` : "no answers") }, "bad",
        note(`${pctText(hd.heRate)} of ${num(hd.heAll[1])} answers`)) : dcard("Hub errors", null, note("None counted yet in this range.")),
      ];
    },
    dl: () => {
      const h = hubNow, d = speedData, tot = (b) => b.reduce((a, v) => a + v, 0), foot = cfoot(LAT_NAME.map((name, i) => key(`${name} ${d ? pctText(latPct(d.all, i)) : ""}`.trim(), `sp${i}`)));
      const waiting = dcard("Waiting for offline phones", num(h.messages), kvRows([["Yos", num(h.messages)], ["Oldest", h.messages && h.oldest_waiting ? rel(h.oldest_waiting) : "–"],
        ["Photos and videos", h.blobs ? num(h.blobs[0]) : "–"], ["Size", h.blobs ? bytes(h.blobs[1]) : "–"]]));
      if (!d || !tot(d.all)) return [dcard("Delivery", null, note(sparkData.dl?.empty || "No data yet.")), waiting];
      return [
        dcard("Delivery", `${pctText(latPct(d.all, 0))} within 1 s`, speedBars(foot), foot),
        comboCard("Yos delivered", { names: d.names, values: d.lat.map(tot) }),
        waiting,
      ];
    },
    ps: () => {
      const hd = healthData;
      if (!hd || !(hd.psAll[0] + hd.psAll[1])) return [dcard("Push", null, note(sparkData.ps?.empty || "No data yet."))];
      const [ok, bad] = hd.psAll, all = ok + bad;
      return [
        dcard("Push taken", pctText((100 * ok) / all), shareBar(ok / all), kvRows([["Taken", num(ok), "ok"], ["Failed", num(bad), bad ? "bad" : ""], ["Sent", num(all)]])),
        comboCard("Pushes", { names: hd.names, values: hd.ps.map(([a, b]) => a + b) }, { cls: "cool" }),
      ];
    },
    upd: () => {
      const u = updData, at = (x) => (x ? new Date(x * 1000).toLocaleString() : null);
      if (!u) return [dcard("Updates", null, note("Collector data unavailable."))];
      return [
        dcard("Packages", u.summary, kvRows([["Packages / security", `${u.apt.packages ?? "?"} / ${u.apt.security ?? "?"}`], ["Security first seen", u.apt.security ? at(u.apt.security_since) : "none"],
          ["APT checked", at(u.apt.checked) || "never"], ["Last automatic upgrade", u.last_upgrade_trigger || "unknown"], ["Checked", at(u.at)]]), u.stale ? note("Older than 30 minutes: the collector may be stuck.") : null),
        dcard("Reboot", u.reboot ? "needed" : "not needed", kvRows([["Needed since", u.reboot ? at(u.reboot_since) : "–"], ["For", (u.reboot_packages || []).join(", ") || "–"],
          ["Running kernel", u.running_kernel || "?"], ["Installed kernel", u.installed_kernel || "?"], ["Next window (only if needed)", u.next_reboot || "unknown"]])),
        dcard("Release", null, kvRows([["Ubuntu", u.current_release || "?"], ["LTS release check", u.release?.error || u.release?.result || "unknown"],
          ["Errors", [...(u.errors || []), u.apt.error].filter(Boolean).join("\n") || "none"]])),
        dcard("Unattended upgrades", null, el("pre", "dlog", u.unattended || "Unknown")),
      ];
    },
    svc: () => [["hub", "Hub"], ["dev", "Dev"], ["tunnel", "Tunnel"], ["cli", "CLI"]].map(([id, name]) => {
      const info = svcInfo[id], body = [], chip = $(`t-${id}`);
      if (info?.parts) for (const [n, p] of info.parts) body.push(el("div", `hc-sub ${p?.ok ? "ok" : "bad"}`, n), kvRows(p?.rows || [["State", "unknown"]]));
      else body.push(kvRows(info?.rows || [["State", "unknown"]]));
      if (info?.select) { const sel = $("st-every"); sel.hidden = false; const l = el("label", "hc-sel", "Report every"); l.append(sel); body.push(l); }
      const c = dcard(name, null, body);
      c.querySelector("h3").prepend(el("span", `dot ${chip?.classList.contains("ok") ? "ok" : chip?.classList.contains("bad") ? "bad" : ""}`));
      return c;
    }),
  };
  // Rows of a topic: CPU, Memory, Disk and LAN share the server's; the services one; the hidden Queue and Hub errors
  // rows belong to Speed and Problems.
  const TOPIC = (k) => (["cpu", "mem", "disk", "net"].includes(k) ? "server" : k.startsWith("svc:") ? "svc" : { q: "dl", he: "pr" }[k] || k);
  // What each tile opens, put together from the single topics above (old links to a topic still open it alone).
  // The operator's bots (YO_BOT_ACCOUNTS), counted apart by the hub and out of every number above: photos each sent
  // today, the Yos of the server's own services as a series of their own. Only in this detail, never on the card.
  // Bots: all bot traffic as bars stacked by kind (Bot Yos, then each bot's photos) in blue shades; the numbers per kind
  // and the note in the hover. Bots are not in Sent, Media or Accounts.
  const botCards = (h) => {
    const ev = h.events || {}, today = (k) => last24(ev[k]?.hours).reduce((n, v) => n + (v || 0), 0);
    const media = Object.keys(ev).filter((k) => k.startsWith("bot-media:")).sort();
    if (!media.length && !ev["bot-yo"]?.total && !today("bot-yo")) return [];
    const kinds = [["bot-yo", "Bot Yos"], ...media.map((k) => [k, `${k.slice(10)} photos`])], shades = ["#5dade2", "#3d8fc4", "#2f6f9c", "#7fc0ea", "#1f4f75"];
    const series = kinds.map(([k, name], n) => ({ name, color: shades[n % shades.length], slots: evSlots(k), today: today(k) }));
    const len = series[0].slots.values.length, names = series[0].slots.names, totalAt = (i) => series.reduce((s, x) => s + (x.slots.values[i] || 0), 0);
    const top = Math.max(1, ...Array.from({ length: len }, (_, i) => totalAt(i))), sum24 = series.reduce((n, s) => n + s.today, 0);
    const pct = series.map((s) => Array.from({ length: len }, (_, i) => ((s.slots.values[i] || 0) / top) * 100)), colors = series.map((s) => s.color);
    const W = 240, H = 70, g = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" });
    for (let i = 0; i < len; i++) { let y = H; series.forEach((s, k) => { const hh = (pct[k][i] / 100) * H; if (hh <= 0) return; y -= hh; g.append(svg("rect", { x: ((i + 0.15) * W) / len, width: (0.7 * W) / len, y, height: hh, fill: s.color })); }); }
    return [{ ico: "cli", label: "Bots", value: `${num(sum24)} in 24 h`, stack: pct, stackColors: colors,
      tip: (i) => ({ head: names[i], rows: series.map((s) => ({ color: s.color, label: s.name, value: num(s.slots.values[i] || 0) })) }),
      title: { cap: 1, head: "Bots", note: "Not in the counts of Sent, Media or Accounts.", nodes: [g], rows: [...series.map((s) => ({ color: s.color, label: s.name, value: `${num(s.today)} in 24 h` })), { label: "All bots", value: `${num(sum24)} in 24 h` }] } }];
  };

  {
    // Yo: beside the tile's Yos: delivered, delivery performance (speed, the queues), media sent, groups.
    DETAIL.yc = () => {
      const h = hubNow, d = speedData;
      return [
      ];
    };
    // Accounts: activity (who is active, how many stay, how much they send) and the accounts' properties (secured,
    // deleted), as the overview's rows: a label, its value, a sparkline where there is a history, a popup per slot.
    // The Accounts stats card: account metrics as the overview's rows (secured bar, new accounts secured, deleted,
    // Yos per user), each with its history in the hover.
    const statsRows = (h) => {
      const hd = healthData || {}, mau = h.active_month || 0, dau = h.active_today ?? 0, act = dau, yoN = last24sum("yo-in"), t = today0();
      const ds = Array.from({ length: 14 }, (_, i) => t - 13 + i), kept = h.active_days || {}, yo = year(h.events?.["yo-in"]?.year);
      const active = ds.map((d) => (d === t ? Math.max(kept[d] || 0, h.active_day || 0) : kept[d] || 0));
      const per = ds.map((d, i) => (active[i] ? (yo.get(d) || 0) / active[i] : 0));
      const ret = Object.fromEntries((areaSub.yc || []).map(([l, v]) => [String(l).replace(/^Retention /, ""), v]));
      const sc = sparkData.sc, gone = evSlots("account-gone");
      const wk = (h.secured_days || []).slice(-7).filter(Boolean), made = wk.reduce((n, d) => n + d[0], 0), withKey = wk.reduce((n, d) => n + d[1], 0);
      return [
          { ico: "chart", label: "Active", value: pctText(mau ? (100 * dau) / mau : null), bar: mau ? dau / mau : 0,
            title: { cap: 1, head: "Active users", nodes: [sparkBars(active, "", null)], note: "Bars: users seen per day, the last 14 days", rows: [{ label: "DAU (last 24 h)", value: num(dau) }, { label: "MAU (last 30 days)", value: num(mau) }, { label: "Stickiness DAU / MAU", value: pctText(mau ? (100 * dau) / mau : null) },
              { section: "Retention" }, ...["D1", "D7", "D30"].map((k) => ({ label: k, value: ret[k] ?? "–" }))] } },
          (() => {
            // Secured: accounts with a backup as a share of all accounts (the bar); its line over the last 30 days in the hover.
            const us = h.users || [], dd = Array.from({ length: 30 }, (_, i) => t - 29 + i), end = (d) => (d + 1) * 86400000;
            const share = dd.map((d) => { const made = us.filter((u) => u.created < end(d)); return made.length ? (100 * made.filter((u) => u.backup_at != null && u.backup_at < end(d)).length) / made.length : null; });
            const now = h.total ? (100 * (h.secured || 0)) / h.total : null, have = share.filter((v) => v != null), lo = Math.min(...have), hi = Math.max(...have);
            const g = svg("svg", { class: "spark", viewBox: "0 0 240 56", preserveAspectRatio: "none", "aria-hidden": "true" });
            if (have.length > 1) g.append(svg("polyline", { fill: "none", stroke: "var(--ok)", "stroke-width": 2, "vector-effect": "non-scaling-stroke", "stroke-linejoin": "round",
              points: share.map((v, k) => (v == null ? null : `${((k * 240) / (share.length - 1)).toFixed(1)},${(50 - (hi === lo ? 0.5 : (v - lo) / (hi - lo)) * 44).toFixed(1)}`)).filter(Boolean).join(" ") }));
            return { ico: "lock", label: "Secured", value: pctText(now), bar: (now || 0) / 100,
              title: { cap: 1, head: "Secured, all accounts", nodes: [g, ...(sc?.values?.length ? [sparkBars(sc.values.map((v) => v || 0), "", null)] : [])], rows: [{ label: `${dayName(dd[0])} – today`, value: pctText(now) }, { section: "New accounts, the bars above" },
                { label: "Secured, last 7 days", value: made ? pctText((100 * withKey) / made) : "–" }] } };
          })(),
          { ico: "trash", label: "Deleted", value: num(sumOf(gone)), cls: sumOf(gone) ? "warn" : "", values: gone.values.map((v) => v || 0), scls: "warm",
            tip: (i) => ({ head: gone.names[i], rows: [{ color: "#e67e22", label: "Deleted", value: num(gone.values[i] || 0) }] }) },
          { ico: "send", label: "Yos per user", value: act ? (yoN / act).toFixed(1) : "–", values: per,
            tip: (i) => ({ head: dayName(ds[i]), rows: [{ color: "var(--ok)", label: "Yos per active user", value: per[i] ? per[i].toFixed(1) : "–" }, { ico: "user", label: "Active", value: num(active[i]) }] }) },
      ];
    };
    // The Yos card: Media, Groups and Bots as rows (value, sparkline; the bars and the totals in the hover).
    drawSentRowsImpl = (h) => {
      const slot = $("yc-rows");
      if (!slot || !h || h.error) return;
      const by = evSlots("media-in-bytes"), yo = evSlots("yo-in");
      const row = (ico, label, k, total, size) => {
        const s = evSlots(k);
        return { ico, label, value: `${num(total)} · +${num(s.day24)} in 24 h`, values: s.values.map((v) => v || 0), scls: "cool",
          tip: (i) => ({ head: s.names[i], rows: [{ color: "#5dade2", label: label, value: num(s.values[i] || 0) }, ...(size ? [{ ico: "disk", label: "Size", value: bytes(by.values[i] || 0) },
            { ico: "send", label: "Share of Yos", value: yo.values[i] ? pctText((100 * (s.values[i] || 0)) / yo.values[i]) : "–" }] : []),
            { section: "In all" }, { color: "#5dade2", line: true, label: "Total", value: num(total - s.values.slice(i + 1).reduce((n, v) => n + (v || 0), 0)) }] }),
          title: { cap: 1, head: label, nodes: [sparkBars(s.values.map((v) => v || 0), "cool", null)], rows: [{ label: "Total", value: num(total) }, { label: "In 24 h", value: num(s.day24) },
            ...(size ? [{ label: "Size, all media", value: h.events?.["media-in-bytes"]?.total == null ? "–" : bytes(h.events["media-in-bytes"].total) }] : [])] } };
      };
      slot.replaceChildren(sparkCard("Sent", [row("image", "Media", "media-in", h.events?.["media-in"]?.total ?? 0, true), row("user", "Groups", "group-create", h.groups ?? 0, false), ...botCards(h)]).querySelector(".srows"));
    };
    drawAccountStatsImpl = (h) => {
      const slot = $("uc-rows");
      if (!slot || !h || h.error) return;
      slot.replaceChildren(sparkCard("Stats", statsRows(h)).querySelector(".srows"));
    };
    // Secured: accounts with a backup, this week's new ones, and the share per sign-up day.
    const securedCard = () => {
      const h = hubNow, all = h.total ? (100 * (h.secured || 0)) / h.total : null, wk = (h.secured_days || []).slice(-7).filter(Boolean);
      const made = wk.reduce((n, d) => n + d[0], 0), withKey = wk.reduce((n, d) => n + d[1], 0), sc = sparkData.sc, sf = cfoot();
      return dcard("Secured", dnums(pctText(all), made ? `${pctText((100 * withKey) / made)} this week` : null),
        sc?.values?.length ? [liveChart({ bars: sc.values.map((v) => v || 0), max: sc.scale, ...axisOf(sc.names) }, "", sf, sayOf(sc)), sf] : null);
    };
    // Problems: what users sent and what crashed (feedback, reports, crashes, crash-free installs (est.); the way to
    // the chat), delivery and speed. The app's errors are in the Errors view.
    DETAIL.pr = () => {
      const h = hubNow, hd = healthData || {}, kinds = Object.fromEntries(problemKinds(h));
      // Feedback: what users sent, as the overview's rows: feedback and reports per slot, crashes and crash-free (est.).
      const h1 = Math.floor(Date.now() / 3600000), t0 = today0(), span = range === 1 ? 24 : range || 30;
      const slots = Array.from({ length: span }, (_, i) => (range === 1 ? h1 - span + 1 + i : t0 - span + 1 + i));
      const slotName = (x) => (range === 1 ? clock(x * 3600000) : dayName(x)), unitMs = range === 1 ? 3600000 : 86400000;
      const fbDays = year(h.support_days), fbHours = last24(h.support_hours);
      const fb = slots.map((x, i) => (range === 1 ? fbHours[i] || 0 : fbDays.get(x) || 0));
      const rp = slots.map((x) => (h.reports || []).filter((r) => Math.floor(r.at / unitMs) === x).length);
      const userRows = [
        { ico: "chat", label: "Feedback", value: num(kinds.Feedback), values: fb, go: () => showTable("support_messages"), goTitle: "Open the support chat", tip: (i) => ({ head: slotName(slots[i]), rows: [{ color: "var(--ok)", label: "Feedback", value: num(fb[i]) }] }) },
        { ico: "warn", label: "Reports", value: num(kinds.Reports), cls: kinds.Reports ? "warn" : "", values: rp, scls: "warm",
          tip: (i) => ({ head: slotName(slots[i]), rows: [{ color: "#e67e22", label: "Reports", value: num(rp[i]) }] }) },
        (() => { const p = problemSplit(h), sum = (v) => v.reduce((x, y) => x + y, 0);
          // Errors (orange) and crash reports (red) per slot, stacked.
          return { ico: "warn", label: "Problems", title: { cap: 1, head: "How problems are counted", rows: [{ label: "1  Report", value: "from a phone" }, { label: "2  Fingerprint", value: "kind · code · line · build" }, { label: "3  Group", value: "same fingerprint, count goes up" }, { label: "4  Problem", value: "the cause: its groups together" }], note: "Ticked stays closed. The same bug in a new build reopens as a new group." }, value: `${p.ec.at(-1) + p.cc.at(-1)}${matchMedia("(max-width: 430px)").matches ? "" : p.ec.at(-1) + p.cc.at(-1) === 1 ? " cause" : " causes"} · ${num(p.er.at(-1) + p.cr.at(-1))} · ${problemTotals?.installs != null && problemTotals.seen ? pctText((100 * problemTotals.installs) / problemTotals.seen) : "–"}`, cls: p.ec.at(-1) + p.cc.at(-1) ? "warn" : "", values: p.er, top: p.cr, recorded: p.recorded, scls: "warm",
            tip: (i) => ({ head: p.names[i], rows: problemRows(p, i) }) }; })(),
        (() => {
          // Phones as one bar: fine (green), error only (orange), crash (red); the value is the share fine. Hover: the three
          // shares, crash-free (est.) with its explanation, iOS and Android.
          const pt = problemTotals, pc = (n, d) => (d ? pctText((100 * n) / d) : "–");
          if (pt?.installs == null || !pt.seen) return { ico: "ios", label: "Phones", value: "–" };
          const mk = (seen, any, crash) => { const b = el("span", "hc-pbar dbar srow-bar"); for (const [k, v] of [["current", Math.max(0, seen - any)], ["warm", Math.max(0, any - crash)], ["must", crash]]) if (v) { const x = el("span", k); x.style.flexGrow = v; b.append(x); } return b; };
          const three = (seen, any, crash) => [{ color: "var(--ok)", label: "Fine", value: pc(Math.max(0, seen - any), seen) }, { color: "#e67e22", label: "With an error", value: pc(Math.max(0, any - crash), seen) }, { color: "var(--bad)", label: "With a crash", value: pc(crash, seen) }];
          const crash = pt.crashInstalls || 0;
          return { ico: "ios", label: "Phones", value: `${pc(Math.max(0, pt.seen - pt.installs), pt.seen)} fine`, node: mk(pt.seen, pt.installs, crash),
            title: { cap: 1, head: "Phones seen in the range", note: "Crash-free (est.): 1 − installs with a crash ÷ phones seen (iOS, Android), since the range's first UTC day. The hub has no app start per install, so the phones seen stand for the installs in use.",
              rows: [...three(pt.seen, pt.installs, crash), { label: "Crash-free (est.)", value: hd.free == null ? "–" : pctText(hd.free) },
                ...pt.platforms.filter(([, , n]) => n).flatMap(([l, hit, n, c]) => [{ section: l }, ...three(n, hit, c)])] } };
        })(),
      ];
      // Yo to Yo speed: from the hub taking a Yo to the friend's phone having it, the share within 1 s per slot as the
      // line over the Yos delivered as bars; within 1 s and within 5 s in the title, every bucket in the popup.
      const sd = speedData, stot = (b) => b.reduce((a, v) => a + v, 0);
      const fast = sd && stot(sd.all) ? (() => {
        const line = sd.lat.map((b) => (stot(b) ? latPct(b, 0) : null)), within5 = (b) => latPct(b, 0) + latPct(b, 1);
        return dcard("Yo to Yo speed", dnums(`${pctText(latPct(sd.all, 0))} in 1 s`, `${pctText(within5(sd.all))} in 5 s`, "muted"),
          liveChart({ line, bars: sd.lat.map(stot), ...axisOf(sd.names) }, "", null, (i) => ({ head: sd.names[i], rows: stot(sd.lat[i]) ? [
            ...LAT_NAME.map((n, k) => ({ color: ["var(--ok)", "#a3e4b8", "var(--warn)", "var(--muted)"][k], label: n, value: pctText(latPct(sd.lat[i], k)) })),
            { ico: "send", label: "Yos", value: num(stot(sd.lat[i])) }] : [{ ico: "send", label: "Yos", value: "none" }] })));
      })() : dcard("Yo to Yo speed", null, note("No Yos delivered in this range."));
      // Speed two columns wide: the four columns filled without the Errors card (errors are in the Errors view).
      fast.classList.add("wide");
      return [sparkCard("Problems", [{ heading: "Delivery" }, ...perfRows(h), { heading: "Feedback & problems" }, ...userRows])];
    };

  }
  const TITLE = { yc: "Yos", uc: "Accounts", md: "Media", gr: "Groups", sc: "Secured", pl: "Platforms", vr: "App", pr: "Problems", dl: "Speed", ps: "Push", server: "Server", upd: "Updates", svc: "Services" };
  const SLUG = { yc: "yos", uc: "accounts", md: "media", gr: "groups", sc: "secured", pl: "platforms", vr: "app", pr: "problems", dl: "speed", ps: "push",
    cpu: "cpu", mem: "memory", disk: "disk", net: "lan", upd: "updates", "svc:hub": "hub", "svc:dev": "dev", "svc:tunnel": "tunnel", "svc:cli": "cli" };
  const detailBox = $("detail");
  let detailPending = false;
  // The status bot's select lives in the server card's hover while it shows, else hidden at home.
  const parkSelect = () => { const sel = $("st-every"); if (sel && !$("t-system").contains(sel) && !tipBox.contains(sel)) { sel.hidden = true; $("t-system").append(sel); } };
  const drawDetail = () => {
    if (!hubNow) return;
    parkSelect();
    $("grp-vr").replaceChildren(...relCard(hubNow));
    $("problem-rows").replaceChildren(DETAIL.pr()[0].querySelector(".srows"));
  };
  // Redrawn with fresh data, unless the pointer is on a card (its captions) or a select is open; then once it leaves.
  function redrawDetail() {
    if (detailBox.querySelector(".dcard:hover") || document.activeElement?.closest?.("#detail select")) { detailPending = true; return; }
    drawDetail();
  }
  detailBox.addEventListener("pointerleave", () => { if (detailPending) { detailPending = false; drawDetail(); } });
  const openDetail = (key) => {
    const target = { yc: "t-yc", uc: "t-uc", vr: "grp-vr", pr: "grp-pr", ps: "grp-pr", dl: "grp-pr" }[TOPIC(key)];
    if (target) $(target).scrollIntoView({ behavior: "smooth", block: "start" });
  };
  serverHints();
  for (const n of document.querySelectorAll("[data-card]")) {
    if (n.tagName !== "BUTTON") { n.setAttribute("role", "button"); n.tabIndex = 0; }
    n.setAttribute("aria-controls", "detail");
    n.addEventListener("click", (e) => { if (!e.target.closest("select, #hub-word")) openDetail(n.dataset.card); });
    n.addEventListener("keydown", (e) => { if (e.target === n && n.tagName !== "BUTTON" && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); openDetail(n.dataset.card); } });
  }
  // Links, old ones too: #support/#feedback at the chat, #crashes/#diagnostics at diagnostics, #reserved, #users.
  const TABLES = { users: "users", todos: "todos", inbox: "support_messages", support: "support_messages", feedback: "support_messages", crashes: "diagnostics", diagnostics: "diagnostics", reports: "reports", db: null, reserved: "reserved" };
  const landing = () => {
    const h = location.hash.slice(1), d = h.startsWith("detail-") && Object.keys(SLUG).find((k) => SLUG[k] === h.slice(7));
    if (d) openDetail(d); else if (h in TABLES) showTable(TABLES[h] || tableOpen);
  };
  window.addEventListener("hashchange", landing);
  const typing = (t) => t instanceof Element && (t.isContentEditable || t.closest("textarea, select, [contenteditable]")
    || (t.closest("input") && !["checkbox", "radio", "button", "submit"].includes(t.closest("input").type)));
  // Keys: 0 switches the hub; 1, 2, 3, 4 the range (1 day, 7 days, 30 days, since the start).
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || typing(e.target)) return;
    const n = /^Digit[0-5]$/.test(e.code) ? Number(e.code.slice(5)) : /^[0-5]$/.test(e.key) ? Number(e.key) : null;
    if (n == null) return;
    e.preventDefault();
    if (n === 0) { switchHub(); return; }
    setRange(RANGES[n - 1]);
  });
  landing();
  drawRange();
  statusRefresh();
  tick();
  setInterval(tick, 3000);
  refresh();
  setInterval(refresh, 10000);
  setInterval(() => { if (document.visibilityState === "visible") statusRefresh(); }, 30000);
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") { tick(); refresh(); } });
}
