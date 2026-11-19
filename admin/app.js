// Login: a word on why the last try did not work. Dashboard: the server every three seconds, the hubs every ten.
const $ = (id) => document.getElementById(id);

// <support-list> The support chat's list, pure (admin/test_support.py runs it in node): the people who wrote to the
// team, "open" (something of theirs not handled yet, the default) or "answered" (nothing open); newest first.
const supportList = ({ users, threads, filter = "open" }) => {
  const up = (id) => String(id || "").toUpperCase();
  const byId = new Map(users.map((u) => [up(u.id), u]));
  return threads
    .filter((t) => t.user_id !== "team" && (filter === "open" ? t.unread > 0 : !(t.unread > 0)))
    .map((t) => { const id = up(t.user_id), user = byId.get(id) || null;
      return { id, user, name: user ? user.name || "(no name)" : t.name || "Deleted account", thread: t, open: t.unread || 0, at: t.at || 0 }; })
    .sort((a, b) => b.at - a.at);
};
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
// </support-list>

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
    if (title) td.title = title;
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
    if (r === range) return;
    range = r;
    try { localStorage.setItem("range", String(range)); } catch {}
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
  const service = (id, u) => {
    u = u || {};
    const ok = u.state === "active";
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
    redrawCard();
  }

  // Updates live in the System title: a dot (green: all fine, no package updates), details on hover and on a click.
  const popup = (fields, text, mono) => {
    const d = $("pop"), dl = d.querySelector("dl"), pre = d.querySelector("pre");
    dl.replaceChildren();
    for (const [k, v] of fields) dl.append(el("dt", null, k), el("dd", null, v ?? "–"));
    pre.hidden = !text; pre.textContent = text || ""; pre.classList.toggle("mono", !!mono);
    d.showModal();
  };
  const updates = (u) => {
    const stale = !u || Date.now() / 1000 - u.at > 1800;
    state("t-system", stale ? "warn" : u.status);
    const summary = stale ? "Update data unavailable" : u.apt.error || u.apt.packages == null ? "Package check unavailable"
      : u.apt.packages ? `${u.apt.packages} updates (${u.apt.security} security)` : "Up to date";
    set("upd-n", stale ? "unknown" : u.apt.packages ? `${u.apt.packages} (${u.apt.security} security)${u.reboot ? " · reboot" : ""}` : u.reboot ? "reboot needed" : "up to date");
    state("c-upd", stale ? "warn" : u.status);
    $("c-upd").dataset.tip = [u?.release?.available ? `${u.release.available} available` : "", u?.reboot ? "reboot needed" : "", "click Server for details"].filter(Boolean).join(" · ");
    const open = () => popup(u ? [
      ["Checked", new Date(u.at * 1000).toLocaleString()],
      ["Packages / security", `${u.apt.packages ?? "?"} / ${u.apt.security ?? "?"}`],
      ["APT checked", u.apt.checked ? new Date(u.apt.checked * 1000).toLocaleString() : "Never"],
      ["Security first observed", u.apt.security ? new Date(u.apt.security_since * 1000).toLocaleString() : "None"],
      ["Reboot needed since", u.reboot ? new Date(u.reboot_since * 1000).toLocaleString() : "No"],
      ["Reboot packages", (u.reboot_packages || []).join(", ")],
      ["Running / installed kernel", `${u.running_kernel || "?"} / ${u.installed_kernel || "?"}`],
      ["Last automatic upgrade", u.last_upgrade_trigger || "Unknown"],
      ["Unattended upgrades", u.unattended || "Unknown"],
      ["Next reboot window (only if needed)", u.next_reboot || "Unknown"],
      ["Ubuntu", u.current_release],
      ["LTS release check", u.release?.error || u.release?.result || "Unknown"],
      ["Errors", [...(u.errors || []), u.apt.error].filter(Boolean).join("\n") || "None"],
    ] : [["Updates", "Collector data unavailable"]]);
    // The server details open from the Updates row.
    const upd = $("c-upd");
    upd.classList.add("opens"); upd.setAttribute("role", "button"); upd.tabIndex = 0;
    upd.onclick = open;
    upd.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } };
  };

  // ---- app builds: the platform word ("iOS", "Android", "Bot"), green when current, yellow when an update is out,
  // plain when unknown; build and newest build only in the tooltip. Worked out by the collector: current is the highest
  // iOS build reported outside the internal testers (YO_ADMIN_INTERNAL_USERS), who are never shown yellow.
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
  // "current", "update" or null (unknown, or no app).
  const internal = (uid) => !!uid && (last?.internal_users || []).includes(uid);
  const appState = (app, uid) => {
    const s = belowMin(app) ? "update" : appInfo(app)?.state || null;
    return internal(uid) && s === "update" ? null : s;
  };
  const appTitle = (app, uid) => {
    const i = appInfo(app);
    return i ? `${internal(uid) ? "internal tester · " : ""}${i.title}${belowMin(app) ? " · below the hub's minimum: refused, must update" : ""}` : "";
  };
  const appTag = (app, uid) => {
    const i = appInfo(app), a = appState(app, uid);
    const t = el("span", a ? `app-${a}` : i ? null : "muted", i ? i.label : "–");
    t.title = appTitle(app, uid);
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
  const sparkData = {}, FMT = { md: bytes }, CUM = { md: true };
  const spark = (key, big, values, names, tip, title, scale) => {
    if (CUM[key]) { let t = 0; values = values.map((v) => (v == null ? v : (t += v))); }
    if (big !== null) set(`${key}-n`, big === undefined ? "–" : (FMT[key] || num)(big));
    if (title !== undefined) $(`c-${key}`).dataset.tip = title || "";
    const node = $(`${key}-spark`), W = 140, H = 28, n = values.length, slot = W / n, top = scale || Math.max(1, ...values.map((v) => v || 0));
    node.replaceChildren(...values.map((v, i) => {
      const h = v ? Math.max(2, (v / top) * (H - 2)) : 1.5;
      return svg("rect", { x: (i * slot + slot * 0.16).toFixed(1), width: (slot * 0.68).toFixed(1), y: (H - h).toFixed(1), height: h.toFixed(1), rx: 1.2, class: v ? "b" : "b zero" });
    }));
    sparkData[key] = { values, names, tip, scale, fmt: FMT[key] };
  };
  // The one range of Usage and Attention (1, 7 or 30 days, or 0: since the start), kept in this browser; the switch
  // next to the logo.
  const RANGES = [1, 7, 30, 0];
  let range = 7;
  try { const r = localStorage.getItem("range"); if (r !== null && RANGES.includes(+r)) range = +r; } catch {}
  const today0 = () => Math.floor(Date.now() / 86400000);
  // A list of the last days' values (today last) as a map by UTC day.
  const year = (arr) => (Array.isArray(arr) ? new Map(arr.map((v, i) => [today0() - arr.length + 1 + i, v])) : new Map());
  // A row in the range: the total with its trend against the range before, and bars (a day each; a month each for a
  // year). out: a second count for the hover. bad: more is worse (Attention: red bars, a rise in red).
  // hours: the last 24 hours (the current one last) for the 1-day range: a bar per four hours, the total of those 24 hours;
  // a row without hours (the hub keeps errors per day only) shows today's count and no bars.
  const rangeRow = (key, perDay, word, tip, out, bad, hours, outHours, total) => {
    // No bars where there is nothing to draw (a row kept per day only, over the last 24 hours).
    $(`${key}-spark`).toggleAttribute("hidden", range === 1 && !Array.isArray(hours));
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
  // retention D1/D7/D30 under Yos, crash-free users with Problems.
  // Speed: how long from the hub taking a Yo to the friend's phone having it, in four buckets; the last one is mostly a
  // phone that was offline (the Yo waited at the hub), not a slow hub.
  const LAT = ["latency-1s", "latency-5s", "latency-60s", "latency-slow"];
  const LAT_NAME = ["Within 1 s", "1–5 s", "5–60 s", "Later (phone offline)"];
  let speedData = null;
  const areaSub = {};
  const healthRows = (h, ym, yh) => {
    const span = range === 1 ? "last 24 hours" : range ? `${range} days` : "since the start";
    const all = (key) => ym(key), yo = ym("yo-out"), slots = rangeSlots(yo), names = slots.map(([n]) => n);
    const sums = (keys) => slots.map((s) => keys.map((k) => slotSum(s, all(k), yh(k))));
    const whole = (rows) => rows.reduce((a, r) => a.map((v, i) => v + r[i]), rows[0].map(() => 0));
    const none = (key, what) => { sparkData[key].empty = `No data yet: the hub has counted no ${what} in this range`; $(`c-${key}`).dataset.tip = "no data yet"; };
    // Speed: per bar the share of Yos on the friend's phone within 1 s (higher is better); the hover splits them all.
    const lat = sums(LAT), latAll = whole(lat), latN = (b) => b.reduce((a, v) => a + v, 0);
    const fast = (b) => (latN(b) ? (100 * b[0]) / latN(b) : 0);
    spark("dl", null, lat.map(fast), names, (i) => (latN(lat[i]) ? `${num(latN(lat[i]))} Yos · ${pctText(fast(lat[i]))} within 1 s` : "no Yos"),
      latN(latAll) ? `${pctText(fast(latAll))} within 1 s` : "", 100);
    speedData = { lat, all: latAll, names, span };
    if (!latN(latAll)) none("dl", "delivered Yos");
    // Push: the share the services took.
    const ps = sums(["push-ok", "push-fail"]), psAll = whole(ps), share = ([ok, bad]) => (ok + bad ? (100 * ok) / (ok + bad) : null);
    spark("ps", null, ps.map((p) => share(p) ?? 0), names,
      (i) => (ps[i][0] + ps[i][1] ? `${pctText(share(ps[i]))} taken · ${num(ps[i][1])} failed of ${num(ps[i][0] + ps[i][1])}` : "no pushes"),
      `Push: taken by APNs and FCM · ${pctText(share(psAll))} · ${num(psAll[1])} failed (in Errors) · ${span}`, 100);
    if (!(psAll[0] + psAll[1])) none("ps", "pushes");
    // Hub errors: 5xx answers per bar (a bar's height its count); their share, 4xx and 429 in the hover.
    const he = sums(["http-5xx", "http", "http-4xx", "http-429"]), heAll = whole(he), rate = ([e, n]) => (n ? (100 * e) / n : 0);
    spark("he", null, he.map(([e]) => e), names,
      (i) => (he[i][1] ? `${num(he[i][0])} 5xx (${pctText(rate(he[i]))}) · ${num(he[i][2])} 4xx · ${num(he[i][3])} 429 of ${num(he[i][1])}` : "no answers"),
      `Hub errors: answers that were 5xx · ${num(heAll[0])} (${pctText(rate(heAll))}) of ${num(heAll[1])} · ${span}`);
    if (!heAll[1]) none("he", "answers");
    $("c-he").classList.add("bad-up");
    // Accounts' hover: DAU/MAU (users seen today of those seen in 30 days) and the accounts deleted in the range.
    const gone = sums(["account-gone"]).reduce((a, [n]) => a + n, 0);
    const dau = h.active_day ?? 0, mau = h.active_month ?? 0, stick = mau ? (100 * dau) / mau : null;
    areaSub.uc = [["DAU / MAU", `${pctText(stick)}`, stick == null ? null : stick / 100], [`Deleted, ${span}`, num(gone), null]];
    // Yos' hover: retention, of the accounts made 1 (7, 30) to 90 days ago those seen that many days after signing up.
    const rt = h.retention || {};
    areaSub.yc = ["d1", "d7", "d30"].map((k) => [`Retention ${k.toUpperCase()}`, rt[k]?.[1] ? pctText((100 * rt[k][0]) / rt[k][1]) : "–", rt[k]?.[1] ? rt[k][0] / rt[k][1] : null]);
    // Problems' hover: crash-free users, the last 7 days' crash reports against the users seen in them (a crash one user).
    const crashes = (h.diagnostic_days || []).filter((r) => r.kind === "crash" && r.day > today0() - 7).reduce((n, r) => n + r.count, 0);
    const wau = h.active_users ?? 0, free = wau ? Math.max(0, 100 - (100 * crashes) / wau) : null;
    // Hub errors belong to Problems: the 5xx answers in its hover, next to crash-free users.
    $("c-pr").dataset.more = `Crash-free users ${pctText(free)} · ${num(crashes)} crashes, ${num(wau)} users in 7 days · hub errors ${heAll[1] ? `${num(heAll[0])} 5xx (${pctText(rate(heAll))})` : "none counted yet"}`;
  };
  // The two chart widgets: a running total over the range (a point an hour, a day, or for Total up to 60 points),
  // ending at the total since the start; the hover shows the point's time and value.
  const areaData = {};
  // A chart's hover line: the point's values, and under them a small line of its own (areaSub: retention, DAU/MAU).
  const say = (key, text) => {
    const tip = $(`${key}-tip`);
    if (!text) { tip.replaceChildren(); return; }
    tip.replaceChildren(el("span", null, text));
  };
  const runningTotal = (perDay, hours, total) => {
    let names, incs;
    if (range === 1) {
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
    return { names, incs, values: [Math.max(0, v), ...values], labels: [range === 1 ? "24 hours ago" : "before", ...names] };
  };
  const drawArea = (key) => {
    const d = areaData[key], node = $(`${key}-svg`);
    if (!d || !node) return;
    const W = node.clientWidth, H = node.clientHeight;
    if (!W || !H) return;
    // A second series: on its own scale (Active over Accounts) or on the same one (Media under Yos, their ratio).
    const o = d.overlay && areaData[d.overlay], same = o && SAME[key];
    // Room above the curve's top (14 px, its max there).
    const both = same ? [...d.values, ...o.values] : d.values, lo = Math.min(...both), hi = Math.max(...both), pad = 6, padTop = 14, foot = 0, n = d.values.length, B = H - foot;
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
    // A vertical guide at the hovered point.
    const guide = svg("line", { class: "guide", y1: 0, y2: B, x1: -10, x2: -10 });
    // One tiny label: the scale's lowest value at the bottom left, where the curve starts (the total is the card's number).
    const lab = (tx, ty, text, anchor) => { const t = svg("text", { x: tx, y: ty, "text-anchor": anchor, class: "lab" }); t.textContent = text; return t; };
    const labels = [lab(pad, B - 9, num(lo), "start")];
    node.replaceChildren(fill, guide, line, ...extra, ...dots, ...labels);
    node.onpointermove = (e) => {
      const r = node.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.round(((e.clientX - r.left - pad) / (r.width - 2 * pad)) * (n - 1))));
      dots.forEach((c, k) => c.classList.toggle("on", k === i));
      guide.setAttribute("x1", x(i).toFixed(1)); guide.setAttribute("x2", x(i).toFixed(1)); guide.classList.add("on");
      // The second line's value too where both run over the same points (Media over Yos).
      const j = !o ? -1 : o.values.length === n ? i : o.values.length === n - 1 ? i - 1 : (o.labels || []).indexOf(d.labels[i]);
      const pct = o?.share && j >= 0 && d.values[i] ? ` (${Math.round((100 * o.values[j]) / d.values[i])} %)` : "";
      const ov = j >= 0 ? ` · ${num(o.values[j])} ${o.word || ""}`.trimEnd() + pct : "";
      // The same hover card as the rows (Platforms): the point's time, its values, and the chart's own line.
      const head = el("div", "hc-head");
      head.append(el("b", null, d.labels[i]), el("span", "hc-val", ""));
      // Rows like Platforms: a label, a bar of its share (where it has one), its value.
      const cap = (w) => (w ? w[0].toUpperCase() + w.slice(1) : "");
      const rows = [[cap(d.word), num(d.values[i]), 1]];
      if (j >= 0) rows.push([cap(o.word), `${num(o.values[j])}${pct}`, d.values[i] ? Math.min(1, o.values[j] / Math.max(...[d.values[i], o.values[j]])) : 0]);
      hc.replaceChildren(head, barRows([...rows, ...(areaSub[key] || [])]));
      hc.hidden = false;
      const w = hc.offsetWidth, h = hc.offsetHeight;
      hc.style.left = `${Math.round(Math.min(Math.max(8, e.clientX + 14), innerWidth - w - 8))}px`;
      hc.style.top = `${Math.round(e.clientY + h + 20 > innerHeight ? e.clientY - h - 12 : e.clientY + 16)}px`;
      say(key, "");
    };
    node.onpointerleave = () => { dots.forEach((c) => c.classList.remove("on")); guide.classList.remove("on"); say(key, ""); if (!hcFor) hc.hidden = true; };
  };
  // A second series drawn over a chart: Active over Accounts, Media over Yos (on the Yos' scale).
  const OVERLAY = { uc: "ac", yc: "ym" }, SAME = { yc: true }, WORD = { yc: "Yos", uc: "accounts" };
  // A small legend over each chart: what its two lines are.
  const LEGEND = { yc: [["Yos", "l-main"], ["Media", "l-over"]], uc: [["Accounts", "l-main"], ["Active", "l-over"]] };
  for (const [k, items] of Object.entries(LEGEND)) {
    const area = $(`${k}-area`);
    if (area && !area.previousElementSibling?.classList.contains("clegend")) {
      const lg = el("div", "clegend");
      for (const [t, c] of items) lg.append(el("span", c, t));
      area.before(lg);
    }
  }
  const areaChart = (key, total, perDay, hours, today) => {
    set(`${key}-n`, num(total));
    areaData[key] = { ...runningTotal(perDay, hours, total || 0), overlay: OVERLAY[key] || null, word: WORD[key] };
    drawArea(key);
    // Under the chart, its activity as bars: per hour (1 day), per day (7, 30), per stretch of days (Total).
    const st = $(`${key}-strip`), inc = areaData[key].incs, top = Math.max(1, ...inc), sl = 140 / inc.length;
    if (!st) return;
    const bars = inc.map((v, i) => {
      const h = v ? Math.max(2, (v / top) * 26) : 1.5;
      return svg("rect", { x: (i * sl + sl * 0.16).toFixed(2), width: (sl * 0.68).toFixed(2), y: (28 - h).toFixed(1), height: h.toFixed(1), rx: 1, class: v ? "b" : "b zero" });
    });
    st.replaceChildren(...bars);
    const names = areaData[key].names, what = { yc: "Yos", uc: "sign-ups" }[key] || "";
    // Its tiny labels: today's count top right (Yos, sign-ups since UTC midnight), the first and last slot under it.
    let box = st.parentElement;
    if (!box.classList.contains("stripbox")) {
      box = el("div", "stripbox"); st.replaceWith(box); box.append(st, el("span", "smax"), el("span", "sfrom"), el("span", "sto"));
    }
    const tb = el("b", null, num(today ?? 0));
    box.querySelector(".smax").replaceChildren(el("span", "stoday", "today"), tb);
    box.querySelector(".smax").title = `${num(today ?? 0)} ${what} today (UTC)`;
    box.querySelector(".sfrom").textContent = range === 1 ? clock((Math.floor(Date.now() / 3600000) - 23) * 3600000) : names[0] || "";
    box.querySelector(".sto").textContent = range === 1 ? "now" : names[names.length - 1] || "";
    // A bar under the pointer: its hour or day and count in the chart's value line.
    st.onpointermove = (e) => {
      const r = st.getBoundingClientRect(), i = Math.min(inc.length - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * inc.length)));
      bars.forEach((b, k) => b.classList.toggle("on", k === i));
      say(key, `${range === 1 ? `${clock((Math.floor(Date.now() / 3600000) - 23 + i) * 3600000)} – ${names[i]}` : names[i]} · ${num(inc[i])} ${what}`);
    };
    st.onpointerleave = () => { bars.forEach((b) => b.classList.remove("on")); say(key, ""); };
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
    for (const b of $("range").querySelectorAll("button[data-r]")) { b.classList.toggle("on", +b.dataset.r === range); b.setAttribute("aria-pressed", String(+b.dataset.r === range)); }
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
  // Per slot of the range: [current, update, must, unknown] of the devices seen that day (the collector's daily
  // snapshot, kept since it began), null where none was kept. 1: today; 7, 30: per day; Total: since the first.
  let verData = null;
  const vPct = (row, i = 0) => { const all = row ? row.reduce((a, v) => a + v, 0) : 0; return all ? Math.round((100 * row[i]) / all) : null; };
  const versionsRow = (h) => {
    const kept = { ...(h.version_days || {}) }, t = today0();
    if (h.versions_day) kept[t] = VKEYS.map((k) => h.versions_day[k] || 0);
    const days = Object.keys(kept).map(Number).filter((d) => d <= t).sort((a, b) => a - b), first = days[0] ?? t;
    if (range === 1) {
      // The last 24 hours, an hour each (kept hourly by admin.py), empty slots where none was kept yet.
      const hrs = h.version_hours || {}, h1 = Math.floor(Date.now() / 3600000);
      const hk = Array.from({ length: 24 }, (_, i) => h1 - 23 + i);
      verData = { rows: hk.map((x) => hrs[x] || null), names: hk.map((x) => clock(x * 3600000)), first, today: kept[t] || null };
    } else {
    const span = range === 1 ? 1 : range || Math.max(7, t - first + 1), step = Math.max(1, Math.ceil(span / 60)), n = Math.ceil(span / step);
    const ends = Array.from({ length: n }, (_, i) => t - (n - 1 - i) * step);
    // A stretch of days (Total, long): its last kept day.
    const rows = ends.map((e) => { for (let d = e; d > e - step; d--) if (kept[d]) return kept[d]; return null; });
    verData = { rows, names: ends.map((e) => (e === t ? "today" : dayName(e))), first, today: kept[t] || null };
    }
    const rows = verData.rows;
    // The row: a bar per slot like Problems, each full height: current (green) from the bottom, the rest above it, so
    // the green is seen growing after a release.
    const host = $("vr-bar"), W = 140, H = 28, sl = W / rows.length, top = 0;
    const s = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none" });
    rows.forEach((r, i) => vStack(r, i * sl + sl * 0.16, sl * 0.68, H, top).forEach((x) => s.append(x)));
    host.replaceChildren(s);
    const p = vPct(verData.today);
    $("c-vr").dataset.tip = p == null ? "" : `today: ${p} % current`;
  };
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
  const reportKey = (r) => `${which}:r${r.id}`;
  const overview = (h) => {
    const ok = h && !h.error;
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
    showOpen();

    // Services: the queue at the bottom.
    const blobs = ok && h.blobs;
    set("q-n", ok ? num(h.messages) : "–");
    // The queue in the Speed row's hover: what still waits for its device.
    $("c-dl").dataset.more = ok ? `Waiting for offline phones: ${num(h.messages)} Yos${h.messages && h.oldest_waiting ? ` (oldest ${rel(h.oldest_waiting)})` : ""}${blobs ? ` · ${num(blobs[0])} photos (${bytes(blobs[1])})` : ""}` : "";
    $("c-q").dataset.tip = ok ? `${num(h.messages)} messages waiting for their device${h.messages && h.oldest_waiting ? `, oldest ${rel(h.oldest_waiting)}` : ""} · ${blobs ? `${num(blobs[0])} images waiting: ${bytes(blobs[1])}` : "images unknown"}` : "";

    // Users: ten, by the chosen column (last seen first); a click opens the user in the Users tab.

    // Activity: active users and the sign-ups of the last 24 hours, each value on hover.
    if (ok) {
      // Usage, in the chosen range: totals and bars. Yos and Media count what went in (sent, uploaded; a bot's inbox
      // left out since the hub counts it apart); what went out (fetched) is in the hover.
      const ev = h.events, ym = (k) => year(ev?.[k]?.year), yh = (k) => ev?.[k]?.hours, yt = (k) => ev?.[k]?.total;
      // Media: the volume uploaded (the hub's media-in-bytes, per hour and day, kept for good); the count is the
      // second line of the Yos chart.
      rangeRow("md", ym("media-in-bytes"), "", "Media: photos and videos uploaded", null, false, yh("media-in-bytes"), null, yt("media-in-bytes"));
      // Media's running total over Yos on its own scale; in the hover with its share of Yos.
      const mdTotal = yt("media-in") ?? 0, md = runningTotal(ym("media-in"), yh("media-in"), mdTotal);
      seriesChart("ym", { values: md.values, labels: md.labels, word: "media", share: true },
        null);
      rangeRow("gr", ym("group-create"), "made", "Groups: made", null, false, yh("group-create"), null, yt("group-create"));
      part(() => healthRows(h, ym, yh));
      // Running totals: Yos into users' inboxes (a bot's inbox apart), accounts without bots.
      areaChart("yc", yt("yo-in") ?? 0, ym("yo-in"), yh("yo-in"), ev?.["yo-in"]?.today ?? 0);
      const sud = new Map(h.signup_days || []);
      areaChart("uc", [...sud.values()].reduce((a, v) => a + v, 0), sud, h.signups_hours, sud.get(today0()) ?? 0);
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
        const st = belowMin(app) ? "must" : appInfo(app)?.state || "unknown", l = appInfo(app)?.label || "unknown";
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
      platData = { plats, min: h.min_app };
      versionsRow(h);
    }
  };

  // ---- support chat (the Database widget, support_messages): the people who wrote on the left, "Open" or
  // "Answered"; the chosen one's conversation on the right, under one line of facts. Each message of theirs is ticked
  // handled by hand; a reply ticks the ones it answers. The to-dos are a table of their own (todos).
  let supportUser = null, supportHub = null, supportName = "", supportSeen = null, supportBusy = false, supportThreads = [];
  let supportWant = null, listDrawn = "", supportMessages = [], userFilter = "open";
  const supportGet = async (user) => {
    const r = await fetch(`/support.json?${new URLSearchParams({ hub: which, ...(user ? { user } : {}) })}`, { cache: "no-store" });
    if (!r.ok) throw new Error("Could not load messages.");
    return r.json();
  };
  const up = (id) => String(id || "").toUpperCase();
  const userById = (id) => (last?.[which]?.users || []).find((u) => up(u.id) === up(id));
  const reportsAbout = (id) => (last?.[which]?.reports || []).filter((r) => up(r.reported) === up(id));
  const openReports = (id) => reportsAbout(id).filter((r) => !lastDone.has(reportKey(r))).length;
  const drawList = () => {
    const box = $("person-list"), lastLines = last?.[which]?.support_last || {};
    const rows = supportList({ users: last?.[which]?.users || [], threads: supportThreads, filter: userFilter });
    const drawn = JSON.stringify([which, supportUser, userFilter, rows.map((p) => [p.id, p.name, p.open, p.at, lastLines[p.id], openReports(p.id)])]);
    if (drawn === listDrawn && box.childElementCount) return;
    listDrawn = drawn;
    box.replaceChildren();
    for (const p of rows) {
      const b = el("button", `thread${p.open ? " unread" : ""}${up(supportUser) === p.id ? " on" : ""}`);
      b.type = "button";
      // One line: a dot while open, the name, a flag for open reports, the time on the right.
      const rep = openReports(p.id);
      b.append(el("i", "odot"), el("b", "name", p.name));
      if (rep) b.append(el("span", "flag", "⚑"));
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
    const hub = which, user = supportUser;
    try {
      await Promise.all(ids.map((id) => postJSON("/support/done", { hub, id, done })));
      if (which === hub && supportUser === user) await supportOpen(user);
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
    if (atEnd || box.dataset.user !== supportUser) box.scrollTop = box.scrollHeight;
    box.dataset.user = supportUser;
  };
  const growComposer = () => {
    const t = $("support-text");
    if (!t.offsetParent) { t.style.height = ""; return; }
    t.style.height = "auto"; t.style.height = `${Math.min(Math.max(t.scrollHeight, 22), 180)}px`;
  };
  // The chosen person in one line of facts (the name is on the left); devices and reports folded below.
  const drawPerson = () => {
    const u = supportUser ? userById(supportUser) : null, facts = $("p-facts");
    facts.replaceChildren();
    $("p-extra").hidden = !u;
    if (!u) return;
    const a = appState(u.app, u.id);
    const fact = (k, v, cls, title) => { const f = el("span", `fact ${cls || ""}`); f.append(el("small", null, k), el("b", null, v)); if (title) f.title = title; facts.append(f); };
    fact("signed up", rel(u.created), "", date(u.created));
    fact("seen", seenDay(u.last_seen), "", u.last_seen ? new Date(u.last_seen).toLocaleDateString() : "");
    const ai = appInfo(u.app);
    fact("app", ai ? ai.label : "–", a ? `app-${a}` : "", ai ? `${u.app} · ${appTitle(u.app, u.id)}` : "");
    fact("secured", u.secured ? "yes" : "no", u.secured ? "ok" : "", u.backup_at ? `backup ${date(u.backup_at)}` : "");
    fact("push", u.push ? "on" : "off", "", "");
    if (u.queued) fact("waiting", num(u.queued), "", "messages waiting for this user's devices");
    const more = el("button", "fact more");
    more.type = "button";
    more.addEventListener("click", () => { $("p-extra").open = !$("p-extra").open; more.classList.toggle("open", $("p-extra").open); });
    more.classList.toggle("open", $("p-extra").open);
    facts.append(more);
    const dev = $("p-devices");
    dev.replaceChildren();
    for (const d of u.device_list || []) {
      const r = el("div", "drow");
      r.append(Object.assign(el("b", null, d.app ? appInfo(d.app).label : "unknown app"), { title: appTitle(d.app, u.id) }), el("span", "muted", [d.push || "no push", `added ${rel(d.created)}`, `seen ${seenDay(d.last_seen)}`].join(" · ")));
      r.title = d.app || "";
      dev.append(r);
    }
    if (!dev.childElementCount) dev.append(el("p", "empty", `${u.devices || 0} device${u.devices === 1 ? "" : "s"}`));
    const reps = $("p-reports");
    reps.replaceChildren();
    const about = reportsAbout(u.id), by = (last?.[which]?.reports || []).filter((r) => up(r.reporter) === up(u.id));
    for (const [r, dir] of [...about.map((r) => [r, "about"]), ...by.map((r) => [r, "by"])]) {
      const done = lastDone.has(reportKey(r)), row = el("div", `drow${done ? " done" : ""}`), line = el("span");
      line.append(reasonTag(r.reason), document.createTextNode(dir === "about" ? ` by ${r.reporter_name ?? r.reporter.slice(0, 8)}` : ` reported ${r.reported_name ?? r.reported.slice(0, 8)}`));
      if (dir === "about" && r.id != null) row.append(tickButton(done, "Report handled", (d) => reportDone(r, d)));
      row.append(line, el("span", "muted", rel(r.at)));
      reps.append(row);
    }
    if (!reps.childElementCount) reps.append(el("p", "empty", "No reports."));
    const n = openReports(u.id);
    more.textContent = `${num(u.devices)} device${u.devices === 1 ? "" : "s"} · ${about.length ? `${about.length} report${about.length === 1 ? "" : "s"}${n ? ` (${n} open)` : ""}` : "no reports"}`;
    more.classList.toggle("alert", n > 0);
  };
  const convoHead = (messages) => {
    $("support-reply").hidden = !supportUser || !messages.length;
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
      if (supportUser !== user || supportHub !== hub) {
        $("support-text").value = ""; growComposer(); supportSeen = null; $("support-history").replaceChildren();
        $("support-reply").hidden = true;
      }
      supportUser = user; supportHub = hub; supportName = title || user;
      part(drawPerson); part(drawList);
    }
    try {
      const data = await supportGet(user);
      if (which !== hub || supportWant !== user) return;
      supportUser = user; supportHub = hub; supportName = title || supportName || user;
      supportMessages = data.messages;
      convoHead(data.messages);
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
      supportUser = null; supportWant = null; supportSeen = null; $("support-reply").hidden = true;
      $("support-history").replaceChildren();
    }
    try {
      const threads = (await supportGet()).filter((t) => t.user_id !== "team");
      if (hub !== which) return;
      supportThreads = threads;
      // The page title's count: conversations still open, and open crash groups without the expected ones.
      const unanswered = threads.filter((t) => t.unread > 0).length;
      const crashes = (last?.[which]?.diagnostics || []).filter((d) => !d.done && !diagInfo(d).expected).length;
      document.title = unanswered + crashes ? `(${unanswered + crashes}) Yo admin` : "Yo admin";
      openTickets = unanswered + crashes; openSupport = unanswered;
      showOpen();
      part(drawList);
      part(drawPerson);
      // Nothing chosen yet: the newest open conversation, else the newest one.
      if (supportWant === null) {
        const user = [...threads].sort((a, b) => (b.unread > 0) - (a.unread > 0) || b.at - a.at)[0];
        if (user) supportOpen(up(user.user_id), userById(user.user_id)?.name || user.name || "Deleted account", true);
      }
      set("support-error", "");
    } catch (e) { set("support-error", e.message); }
    finally { supportBusy = false; }
  };
  const supportSend = async () => {
    const text = $("support-text").value;
    if (!supportUser || supportHub !== which || !text.trim()) return;
    const button = $("support-reply").querySelector("button"), user = supportUser, hub = supportHub;
    button.disabled = true;
    try {
      await postJSON("/support", { hub, user, action: "reply", text });
      // The reply answers the user's open messages in this thread: they are marked handled.
      const open = which !== hub ? [] : (await supportGet(user)).messages.filter((m) => !m.from_team && m.done_at == null);
      await Promise.all(open.map((m) => postJSON("/support/done", { hub, id: m.id, done: true })));
      if (which !== hub || supportUser !== user) return;
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
    if (supportUser && supportHub === which) supportOpen(supportUser);
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
    : key === "update" ? (u.app ? (belowMin(u.app) ? 3 : appState(u.app, u.id) === "update" ? 2 : appState(u.app, u.id) === "current" ? 0 : 1) : -1)
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
      const upd = u.app ? (belowMin(u.app) ? tag("must update", "bad") : st === "update" ? tag("update", "warn") : st === "current" ? tag("current", "ok") : tag("?", "muted")) : el("span", "muted", "–");
      cell(tr, upd);
      cell(tr, u.push ? tag("on", "ok") : tag("off", "muted"));
      cell(tr, u.secured ? icon("lock", "secured", u.backup_at ? `backup ${date(u.backup_at)}` : "secured") : el("span", "muted", "–"));
      cell(tr, num(u.devices), "num");
      cell(tr, u.queued ? num(u.queued) : "–", "num muted");
    }
    if (!list.length) cell(body.insertRow(), h && !h.error ? "No users yet." : (h?.error || "No data"), "empty").colSpan = 9;
  };

  // ---- reports: handled is kept by the admin page (done.json); ticked in a user's details
  const reportDone = async (r, done) => {
    await fetch("/feedback/done", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: `hub=${which}&id=r${r.id}&done=${done ? 1 : 0}` });
    await refresh();
  };
  const reasonTag = (reason) => tag(reason, reason === "abuse" ? "bad" : reason === "spam" ? "warn" : "muted");

  // ---- database: every table, grouped by topic; reserved names with their switch --------------------------------
  // Table groups, the main table first, each with a line icon (drawn like the Overview's).
  const TABLE_GROUPS = [
    ["Accounts", "user", ["users", "handles", "reserved", "gone", "devices"]],
    ["Messages", "mail", ["messages", "blobs", "blob_quota"]],
    ["Support", "chat", ["support_messages", "todos", "reports", "feedback"]],
    ["Errors", "warn", ["diagnostics", "diagnostic_days", "diagnostic_seen"]],
    ["Keys", "key", ["account_keys", "contact_keys", "prekeys", "profile_commitments"]],
    ["Groups", "group", ["zk_groups"]],
    ["Account data", "disk", ["backups", "storage"]],
    ["Stats", "chart", ["events", "deliveries_hourly"]],
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
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1"/>',
  });
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
    support_messages: "Support chat messages",
    todos: "The team's to-dos",
    diagnostics: "Crash and error reports, one row per signature",
    diagnostic_seen: "Report ids already counted, never counted twice",
    diagnostic_days: "Crashes and errors per day, last 14 days",
    reports: "Users reported by users, latest per pair",
    feedback: "Old app feedback, before the support chat",
    settings: "Hub switches, such as name reservation on/off",
    deliveries_hourly: "Deliveries per hour, a count only, 8 days",
    _litestream_lock: "Litestream replication bookkeeping (?)",
    _litestream_seq: "Litestream replication bookkeeping (?)",
  };
  const tableGroups = (list) => {
    const byName = new Map(list.map((t) => [t.name, t])), known = new Set();
    const groups = TABLE_GROUPS.map(([title, ico, names]) => {
      names.forEach((n) => known.add(n));
      return [title, ico, names.filter((n) => byName.has(n)).map((n) => byName.get(n))];
    });
    groups.push(["Internal", "gear", list.filter((t) => !known.has(t.name))]);
    return groups.filter(([, , ts]) => ts.length);
  };
  // A timestamp column (…_at, ms since 1970) as a readable date, the raw value on hover.
  const isTime = (col, v) => /_at$/.test(col) && typeof v === "number" && v > 1e11 && v < 1e14;
  let tableOpen = null, tablesDrawn = "", dbView = "chat";
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
    const drawn = JSON.stringify([which, tableOpen, dbView, list.map((t) => [t.name, t.count]), tableOpen === "reserved" ? reservedData?.[which]?.enabled : 0,
      (list.find((t) => t.name === tableOpen) || {}).rows?.length, tableOpen === "reserved" ? list.find((t) => t.name === "reserved")?.rows : 0]);
    if (drawn === tablesDrawn && box.childElementCount) return;
    tablesDrawn = drawn;
    // "people": the users put together from several tables (the default); "users" alone is the raw table.
    const real = (n) => (n === "people" ? "users" : n);
    const sel = list.find((x) => x.name === real(tableOpen));
    set("tables-note", `${list.length} tables · ${num(list.reduce((n, t) => n + (t.count || 0), 0))} rows${sel ? ` · ${sel.name}: ${num(sel.count)} rows${sel.count > sel.rows.length ? `, first ${num(sel.rows.length)} shown` : ""}` : ""}`);
    const groups = tableGroups(list);
    if (list.length && tableOpen !== "people" && !list.some((t) => t.name === tableOpen)) tableOpen = "people";
    const side = el("nav", "tside"), pane = el("div", "tpane");
    // At the top, apart: Users, put together from several tables. Then "Database": every table as it is, each group's
    // heading its main table (icon, name, count), the others indented below it.
    const people = el("button", `main people${tableOpen === "people" ? " on" : ""}`), pname = el("span", null, "Users");
    people.type = "button"; people.title = "Users: accounts with their devices, app, push and backup, put together";
    pname.prepend(icon("user"));
    people.append(pname, el("small", null, num(list.find((x) => x.name === "users")?.count || 0)));
    people.addEventListener("click", () => { tableOpen = "people"; drawTables(); });
    side.append(people, el("div", "tside-h", "Database"));
    for (const [title, ico, ts] of groups) {
      for (const [k, t] of ts.entries()) {
        const b = el("button", `${k ? "sub" : "main"}${t.name === tableOpen ? " on" : ""}`);
        b.type = "button"; b.title = `${title}: ${TABLE_INFO[t.name] || t.name}`;
        const name = el("span", null, t.name);
        if (!k) name.prepend(icon(ico));
        b.append(name, el("small", null, num(t.count)));
        b.addEventListener("click", () => { tableOpen = t.name; drawTables(); });
        side.append(b);
      }
    }
    // On a phone the list is a row of chips: the chosen one scrolled into view.
    requestAnimationFrame(() => { const on = side.querySelector(".on"); if (on && side.scrollWidth > side.clientWidth) side.scrollLeft = on.offsetLeft - side.offsetLeft - 12; });
    const t = list.find((x) => x.name === real(tableOpen));
    // support_messages: "Chat" (the default) shows the support chat (people on the left, the conversation and a reply
    // box on the right, the to-dos as one entry); "List" the plain rows. The chat is moved here, never rebuilt.
    // users: the enriched, sortable users table (name, app, push, secured, signed up, last seen, devices) instead.
    const chatOk = t?.name === "support_messages", chat = chatOk && dbView === "chat", usersView = tableOpen === "people";
    const view = chat ? "chat" : usersView ? "users" : "list";
    if (view !== "list" && box.dataset.name === t.name && box.dataset.view === view && box.firstChild) { box.firstChild.replaceWith(side); return; }
    box.dataset.name = t?.name || ""; box.dataset.view = view;
    if (view !== "chat" && box.querySelector("#people")) $("chat-home").append($("people"));
    if (view !== "users" && box.querySelector("#all-users")) $("chat-home").append($("all-users"));
    if (t) {
      if (chatOk) {
        const seg = el("div", "seg");
        for (const [v, label] of [["list", "List"], ["chat", "Chat"]]) {
          const b = el("button", v === dbView ? "on" : "", label);
          b.type = "button";
          b.addEventListener("click", () => { dbView = v; tablesDrawn = ""; drawTables(); if (v === "chat") growComposer(); });
          seg.append(b);
        }
        seg.classList.add("tview");
        pane.append(seg);
      }
      if (chat) pane.append($("people"));
      if (usersView) pane.append($("all-users"));
      if (t.name === "reserved") pane.append(reservedBar());
      if (t.name === "todos") pane.append(todoBar());
      if (view === "list") {
      const wrap = el("div", "scroll grid"), table = el("table", "list data");
      table.dataset.name = t.name;
      const hr = table.createTHead().insertRow();
      if (t.name === "todos") hr.append(el("th", "tick-col"));
      for (const c of t.columns) hr.append(el("th", null, c));
      if (t.name === "reserved") hr.append(el("th"));
      const body = table.createTBody();
      // Crash and error groups: open ones first, newest first; "Done" on each open one (done ones dimmed, "Reopen").
      const diag = t.name === "diagnostics", todo = t.name === "todos", col = (n) => t.columns.indexOf(n);
      const rows = diag ? [...t.rows].sort((a, b) => (a[col("done")] ? 1 : 0) - (b[col("done")] ? 1 : 0) || b[col("last_seen")] - a[col("last_seen")]) : t.rows;
      if (diag || todo) hr.append(el("th"));
      for (const row of rows) {
        const tr = body.insertRow();
        if (diag && row[col("done")]) tr.className = "done";
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
          if (isTime(t.columns[i], v)) cell(tr, date(v), "mono", String(v));
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

  // ---- everything from the collector -------------------------------------------------------------------------------
  const render = () => {
    const h = last?.[which];
    findNewest(h);
    part(() => overview(h));
    supportRefresh();
    part(drawPerson);
    part(() => drawAllUsers(h));
    part(drawTables);
  };
  let peopleBusy = false;
  async function refresh() {
    if (document.visibilityState !== "visible" || peopleBusy) return;
    peopleBusy = true;
    const status = $("collection-status");
    if (!last) status.textContent = "Loading fresh data…";
    try {
      const r = await fetch("/users.json", { cache: "no-store" });
      if (!r.ok) throw new Error("collector unavailable");
      last = await r.json();
      updates(last.updates);
      const doneResponse = await fetch("/done.json", { cache: "no-store" });
      lastDone = new Set(doneResponse.ok ? await doneResponse.json() : []);
      render();
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
    redrawCard();
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
    } catch { if (svcInfo.stbot) { svcInfo.stbot.rows.push(["", "not saved, try again", "bad"]); redrawCard(); } }
  });

  // ---- one page of widgets; the Database widget is always open. Rows of Attention and Usage jump there: Feedback to
  // the support chat, Errors to diagnostics, Reports to reports, Sign-ups (and the Users card's title) to the users
  // table. Services is one row in Server, opened in place (on its own while something is down); the Status bot row
  // there shows its frequency select. Keys: 1 top, 2 chat, 3 diagnostics, 4 users, 5 database, 0 the hub.
  const showTable = (name) => { tableOpen = name === "users" ? "people" : name; if (name === "support_messages") dbView = "chat"; tablesDrawn = ""; part(drawTables); $("t-db").scrollIntoView({ behavior: "smooth", block: "start" }); };
  // The support chat shown, scrolled to only when it is out of sight (a click inside it never moves the page).
  const toInbox = () => {
    if (tableOpen !== "support_messages" || dbView !== "chat") { tableOpen = "support_messages"; dbView = "chat"; tablesDrawn = ""; part(drawTables); }
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
  // ---- the hover card: one small card for a row or chip under the pointer (or tapped): a chart of its bars with
  // values and axis labels, the platforms per build, a service's details; a click pins it, outside or Escape closes.
  const hc = el("div", "hcard");
  hc.hidden = true; hc.setAttribute("role", "tooltip");
  document.body.append(hc);
  let hcFor = null, hcPinned = false, hcTimer = 0;
  const short = (v) => { const a = Math.abs(v); return a >= 1e9 ? `${(v / 1e9).toFixed(1)}G` : a >= 1e6 ? `${(v / 1e6).toFixed(a >= 1e7 ? 0 : 1)}M` : a >= 1e4 ? `${Math.round(v / 1e3)}k` : a >= 1e3 ? `${(v / 1e3).toFixed(1)}k` : `${Math.round(v)}`; };
  // Bars with their values (on top while they fit, else on hover), the first and last slot named below.
  const barChart = (d, cls, cap) => {
    const W = 264, H = 104, top = 14, bottom = 18, n = d.values.length, slot = W / Math.max(1, n);
    const max = d.scale || Math.max(1, ...d.values.map((v) => v || 0)), s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: `hc-chart ${cls || ""}` });
    const say = (i) => { cap.textContent = `${d.names[i]} · ${d.values[i] == null ? "–" : d.tip ? d.tip(i, d.values[i]) : (d.fmt || num)(d.values[i])}`; };
    const label = (x, y, text, anchor, c) => { const t = svg("text", { x, y, "text-anchor": anchor, class: c }); t.textContent = text; return t; };
    s.append(svg("line", { x1: 0, x2: W, y1: H - bottom + .5, y2: H - bottom + .5, class: "base" }));
    const bars = d.values.map((v, i) => {
      const h = v ? Math.max(2, (v / max) * (H - top - bottom)) : 1.5, x = i * slot + slot * 0.14;
      const r = svg("rect", { x: x.toFixed(1), width: (slot * 0.72).toFixed(1), y: (H - bottom - h).toFixed(1), height: h.toFixed(1), rx: 2, class: v ? "b" : "b zero" });
      if (n <= 14 && v) s.append(label(x + slot * 0.36, H - bottom - h - 4, d.fmt === bytes ? bytes(v).replace(/ ?([KMGT])?B$/, "$1") : d.scale === 100 ? `${Math.round(v)}` : short(v), "middle", "v"));
      return r;
    });
    s.prepend(...bars);
    if (n > 14) s.append(label(0, 9, `max ${d.fmt === bytes ? bytes(max) : d.scale === 100 ? "100 %" : num(max)}`, "start", "ax"));
    s.append(label(0, H - 4, d.names[0] || "", "start", "ax"), label(W, H - 4, d.names[n - 1] || "", "end", "ax"));
    s.onpointermove = (e) => {
      const r = s.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n)));
      bars.forEach((b, k) => b.classList.toggle("on", k === i)); say(i);
    };
    s.onpointerleave = () => { bars.forEach((b) => b.classList.remove("on")); say(n - 1); };
    say(n - 1);
    return s;
  };
  // Hover rows with a bar: [label, value, share 0–1 or null for no bar].
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
  // Platforms: a share per platform in the row's colours (iOS amethyst, Android green, Bot grey, CLI yellow); the
  // versions of each are in the Versions row.
  const platCard = () => {
    const box = el("div", "hc-plats"), plats = platData?.plats || [], all = Math.max(1, plats.reduce((n, [, p]) => n + p.all, 0));
    for (const [l, p] of plats) {
      const row = el("div", "hc-plat"), bar = el("span", "hc-pbar plats");
      const x = el("span", `p-${l.toLowerCase()}`); x.style.width = `${(100 * p.all) / all}%`; bar.append(x);
      row.append(el("b", null, l), bar, el("span", "hc-pn", `${num(p.all)} · ${Math.round((100 * p.all) / all)} %`));
      box.append(row);
    }
    return box;
  };
  const vLegend = () => {
    const legend = el("div", "hc-legend");
    legend.append(el("span", "current", "current"), el("span", "update", "update"), el("span", "must", "must"), el("span", "unknown", "unknown"));
    return legend;
  };
  // Versions: the range's stacked bars, bigger; a bar under the pointer names its day and shares.
  const verCard = (cap) => {
    const d = verData, W = 264, H = 104, top = 4, bottom = 18, n = d?.rows.length || 1, slot = W / n, B = H - bottom;
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: "hc-chart vchart" });
    const groups = (d?.rows || []).map((row, i) => { const g = svg("g", { class: "vb" }); g.append(...vStack(row, i * slot + slot * 0.14, slot * 0.72, B, top)); return g; });
    s.append(...groups);
    const label = (x, y, text, anchor) => { const t = svg("text", { x, y, "text-anchor": anchor, class: "ax" }); t.textContent = text; return t; };
    s.append(label(0, H - 4, d?.names[0] || "", "start"), label(W, H - 4, d?.names[n - 1] || "", "end"));
    const say = (i) => {
      const row = d?.rows[i], all = row ? row.reduce((a, v) => a + v, 0) : 0;
      cap.textContent = `${d?.names[i] || ""} · ${all ? VKEYS.slice(0, 3).map((k, j) => (row[j] ? `${vPct(row, j)} % ${k}` : "")).filter(Boolean).join(" · ") + ` (${num(all)})` : "not recorded"}`;
    };
    s.onpointermove = (e) => {
      const r = s.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n)));
      groups.forEach((g, k) => g.classList.toggle("on", k === i)); say(i);
    };
    s.onpointerleave = () => { groups.forEach((g) => g.classList.remove("on")); say(n - 1); };
    say(n - 1);
    const out = [s, cap, vLegend()];
    // History starts with the collector's first snapshot: say so while the range reaches before it.
    if (d && d.rows.some((r) => !r)) out.push(el("div", "hc-lines", `Kept daily since ${dayName(d.first)}.`));
    return out;
  };
  // Speed: the range's split in one bar with its four shares named, then per bar the same, smaller; a bar under the
  // pointer names its time, its Yos and its share within 1 s.
  const speedCard = (cap) => {
    const d = speedData, n = d.lat.length, W = 264, H = 64, bottom = 16, B = H - bottom, slot = W / n;
    const tot = d.all.reduce((a, v) => a + v, 0), pct = (b, i) => { const t = b.reduce((a, v) => a + v, 0); return t ? (100 * b[i]) / t : 0; };
    const bar = el("div", "hc-pbar sp-bar");
    d.all.forEach((v, i) => { if (v) { const x = el("span", `sp${i}`); x.style.flexGrow = v; bar.append(x); } });
    const legend = el("dl", "hc-kv sp-kv");
    LAT_NAME.forEach((name, i) => legend.append(el("dt", `sp${i}`, name), el("dd", null, pctText(pct(d.all, i)))));
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, class: "hc-chart vchart" });
    const groups = d.lat.map((b, i) => {
      const g = svg("g", { class: "vb" }), t = b.reduce((a, v) => a + v, 0), x = (i * slot + slot * 0.14).toFixed(1), w = (slot * 0.72).toFixed(1);
      if (!t) g.append(svg("rect", { x, width: w, y: B - 1.5, height: 1.5, rx: 1, class: "b zero" }));
      let y = B;
      b.forEach((v, k) => { if (!v) return; const hh = (v / t) * (B - 2); y -= hh; g.append(svg("rect", { x, width: w, y: y.toFixed(1), height: hh.toFixed(1), class: `sp${k}` })); });
      return g;
    });
    s.append(...groups);
    const label = (x, text, anchor) => { const t = svg("text", { x, y: H - 3, "text-anchor": anchor, class: "ax" }); t.textContent = text; return t; };
    s.append(label(0, d.names[0] || "", "start"), label(W, d.names[n - 1] || "", "end"));
    const say = (i) => { const t = d.lat[i].reduce((a, v) => a + v, 0); cap.textContent = `${d.names[i]} · ${t ? `${num(t)} Yos · ${pctText(pct(d.lat[i], 0))} within 1 s` : "no Yos"}`; };
    s.onpointermove = (e) => {
      const r = s.getBoundingClientRect(), i = Math.min(n - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * n)));
      groups.forEach((g, k) => g.classList.toggle("on", k === i)); say(i);
    };
    s.onpointerleave = () => { groups.forEach((g) => g.classList.remove("on")); say(n - 1); };
    say(n - 1);
    return [el("div", "sp-big", `${pctText(pct(d.all, 0))} within 1 s`), el("div", "hc-note", `${num(tot)} Yos · ${d.span}`), bar, legend, s, cap,
      el("div", "hc-lines", "Time from sending a Yo to the friend's phone having it. Later = the friend's phone was offline, so the Yo waited until it came back.")];
  };
  const buildCard = (node) => {
    const key = node.dataset.card, head = el("div", "hc-head"), body = [];
    if (key.startsWith("svc:")) {
      const id = key.slice(4), info = svcInfo[id];
      head.append(el("span", `dot ${node.classList.contains("ok") ? "ok" : node.classList.contains("bad") ? "bad" : ""}`), el("b", null, node.textContent.trim()));
      if (info?.parts) for (const [name, p] of info.parts) { body.push(el("div", `hc-sub ${p?.ok ? "ok" : "bad"}`, name), kvRows(p?.rows || [["State", "unknown"]])); }
      else body.push(kvRows(info?.rows || [["State", "unknown"]]));
      if (info?.select) { const sel = $("st-every"); sel.hidden = false; const l = el("label", "hc-sel", "Report every"); l.append(sel); body.push(l); }
      return [head, ...body];
    }
    const row = node, label = row.querySelector(".cl")?.textContent || "", value = row.querySelector(".cn")?.textContent || "";
    head.append(el("b", null, label), el("span", "hc-val", value && value !== "–" ? value : ""));
    const d = sparkData[key], cap = el("div", "hc-cap");
    if (key === "pl") body.push(platCard());
    else if (key === "vr") {
      const p = vPct(verData?.today);
      head.querySelector(".hc-val").textContent = p == null ? "" : `today: ${p} % current`;
      body.push(...verCard(cap));
    }
    else if (d?.empty) body.push(el("div", "hc-lines", d.empty));
    else if (key === "dl" && speedData) body.push(...speedCard(cap));
    else if (d && d.values.length > 1) {
      const cls = row.classList.contains("bad-up") ? "bad" : d.cls || (row.closest("#t-system") ? "cool" : "");
      body.push(barChart(d, cls, cap), cap);
    }
    // Few words: a row's note only where there is no chart to read (Queue, Updates).
    const lines = key === "pl" || key === "vr" || body.length ? [] : (row.dataset.tip || "").split(" · ").filter(Boolean).filter((x) => x !== value);
    if (lines.length) body.push(el("div", "hc-lines", lines.join(" · ")));
    // A line more under the chart (Problems: crash-free users).
    if (row.dataset.more) body.push(el("div", "hc-lines", row.dataset.more));
    if (row.dataset.open) {
      const go = el("button", "hc-hint", row.dataset.open.startsWith("table:") ? "Open the table →" : "Details →");
      go.type = "button";
      go.addEventListener("click", () => { hideCard(); row.click(); });
      body.push(go);
    }
    return [head, ...body];
  };
  const placeCard = () => {
    if (!hcFor || hc.hidden) return;
    const r = hcFor.getBoundingClientRect(), w = hc.offsetWidth, h = hc.offsetHeight;
    let y = r.bottom + 6;
    if (y + h > innerHeight - 8 && r.top - h - 6 > 8) y = r.top - h - 6;
    hc.style.left = `${Math.round(Math.min(Math.max(8, r.right - w), innerWidth - w - 8))}px`;
    hc.style.top = `${Math.round(y)}px`;
  };
  function redrawCard() {
    if (!hcFor || hc.hidden || hc.contains(document.activeElement)) return;
    hc.replaceChildren(...buildCard(hcFor));
    placeCard();
  }
  const showCard = (node, pin) => {
    clearTimeout(hcTimer);
    if (hcFor && hcFor !== node) hcFor.classList.remove("carded");
    hcFor = node; hcPinned = !!pin;
    node.classList.add("carded");
    hc.hidden = false;
    hc.replaceChildren(...buildCard(node));
    placeCard();
  };
  const hideCard = () => {
    clearTimeout(hcTimer);
    if (hc.contains(document.activeElement)) document.activeElement.blur();
    if (hcFor) hcFor.classList.remove("carded");
    hc.hidden = true; hcFor = null; hcPinned = false;
  };
  const softHide = () => { if (!hcPinned) { clearTimeout(hcTimer); hcTimer = setTimeout(hideCard, 160); } };
  for (const r of document.querySelectorAll(".crow[id^='c-']:not(.chips)")) if (!r.dataset.card) r.dataset.card = r.id.slice(2);
  let lastPointer = "mouse";
  document.addEventListener("pointerdown", (e) => { lastPointer = e.pointerType; }, true);
  for (const n of document.querySelectorAll("[data-card]")) {
    n.addEventListener("pointerenter", (e) => { if (e.pointerType === "mouse" && !(hcPinned && hcFor !== n)) showCard(n, hcPinned && hcFor === n); });
    n.addEventListener("pointerleave", (e) => { if (e.pointerType === "mouse") softHide(); });
    n.addEventListener("click", (e) => {
      if (e.target.closest("select")) return;
      // A finger: the first tap shows the card (its button opens the row), the row's own action only from there.
      if (n.dataset.open && lastPointer === "touch" && e.isTrusted && hcFor !== n) { e.stopImmediatePropagation(); showCard(n, true); return; }
      if (n.dataset.open) { hideCard(); return; }
      if (hcFor === n && hcPinned) hideCard(); else showCard(n, true);
    });
  }
  hc.addEventListener("pointerenter", () => clearTimeout(hcTimer));
  hc.addEventListener("pointerleave", softHide);
  document.addEventListener("pointerdown", (e) => { if (!hc.hidden && !hc.contains(e.target) && !hcFor?.contains(e.target)) hideCard(); });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !hc.hidden) hideCard(); });
  window.addEventListener("scroll", placeCard, { passive: true, capture: true });
  window.addEventListener("resize", placeCard);
  const actions = {};
  for (const r of document.querySelectorAll(".crow[data-open]")) {
    const open = () => (r.dataset.open.startsWith("table:") ? showTable(r.dataset.open.slice(6)) : actions[r.dataset.open]?.());
    r.addEventListener("click", (e) => { if (!e.target.closest("select")) open(); });
    r.addEventListener("keydown", (e) => { if (e.target === r && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); open(); } });
  }
  // Links, old ones too: #support/#feedback at the chat, #crashes/#diagnostics at diagnostics, #reserved, #users.
  const TABLES = { users: "users", todos: "todos", inbox: "support_messages", support: "support_messages", feedback: "support_messages", crashes: "diagnostics", diagnostics: "diagnostics", reports: "reports", db: null, reserved: "reserved" };
  const landing = () => { const h = location.hash.slice(1); if (h in TABLES) showTable(TABLES[h] || tableOpen); };
  window.addEventListener("hashchange", landing);
  const typing = (t) => t instanceof Element && (t.isContentEditable || t.closest("textarea, select, [contenteditable]")
    || (t.closest("input") && !["checkbox", "radio", "button", "submit"].includes(t.closest("input").type)));
  // Keys: 0 switches the hub; 1, 2, 3, 4 the range (1 day, 7 days, 30 days, since the start).
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || typing(e.target) || $("pop").open) return;
    const n = /^Digit[0-4]$/.test(e.code) ? Number(e.code.slice(5)) : /^[0-4]$/.test(e.key) ? Number(e.key) : null;
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
