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
// <report-groups> The Reports view's grouping, pure (admin/test_reports.py runs it in node): one row per REPORTED user
// (the hub's per-pair reports: who is reported, by how many installs, why), most reports first. Reporters are random
// install ids and stay that: never matched to an account. "open" = a report of theirs is not ticked handled yet.
const REASON_LABELS = { spam: "Spam", abuse: "Abuse", other: "Other" };
const reasonLabel = (r) => REASON_LABELS[r] || `Reason ${r}`;
const reportGroups = ({ reports, users = [], isDone = () => false }) => {
  const byId = new Map(users.map((u) => [u.id, u])), groups = new Map();
  for (const r of reports) {
    let g = groups.get(r.reported);
    if (!g) groups.set(r.reported, g = { id: r.reported, rows: [], reasons: new Map(), reporters: new Set(), first: r.at, last: r.at, open: false });
    g.rows.push(r);
    g.reasons.set(r.reason, (g.reasons.get(r.reason) || 0) + 1);
    g.reporters.add(r.reporter);
    g.first = Math.min(g.first, r.at); g.last = Math.max(g.last, r.at);
    g.open ||= !isDone(r);
  }
  return [...groups.values()].map((g) => {
    const u = byId.get(g.id), name = g.rows.find((r) => r.reported_name)?.reported_name || u?.name || null;
    return { id: g.id, name: name || g.id.slice(0, 8), named: !!name, count: g.rows.length, reporters: g.reporters.size,
      reasons: [...g.reasons].sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0]))),
      first: g.first, last: g.last, open: g.open, rows: g.rows.slice().sort((a, b) => b.at - a.at),
      user: u ? { created: u.created, last_seen: u.last_seen, devices: u.devices } : null };
  }).sort((a, b) => b.count - a.count || b.last - a.last);
};
// </report-groups>
// A crash or error group's title and text from its payload; "expected" ones (known to happen) count but stay hidden.
const diagPayload = (d) => {
  try { return JSON.parse(d.payload) ?? {}; } catch { return d.payload ?? {}; }
};
const diagInfo = (d) => {
  const p = diagPayload(d), domain = p.domain || "", code = String(p.code ?? "");
  let name = p.message || domain || "Error";
  if (/AuthenticationServices|ASAuthorizationError/i.test(domain) && code === "1004") name = "Passkey: request failed";
  if (domain === "NSURLErrorDomain" && code === "-1009") name = "Offline";
  if (domain === "SharedAccess" && code === "35") name = "Shared state busy";
  if (/^HTTP$/i.test(domain)) name = ({409: "Request conflict", 410: "Resource gone", 429: "Too many requests"})[code]
    || (/^5\d\d$/.test(code) ? "Server error" : "HTTP request failed");
  const title = d.kind === "crash" ? [p.exception, p.termination].filter(Boolean).join(" · ") || "Crash"
    : code ? `${name} (${code})` : name;
  return { title, expected: p.severity === "expected" };
};
// Match the collector's cause key across builds and source line changes.
const diagCause = (d) => {
  const p = diagPayload(d);
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
  // Summarize the four service indicators in the header.
  const SVCS = [["t-hub", "Hub"], ["t-dev", "Dev"], ["t-tunnel", "Tunnel"], ["t-cli", "CLI"]];
  const svcSum = () => {
    const n = $("svc-n");
    if (!n) return;
    const down = SVCS.filter(([id]) => $(id)?.classList.contains("bad")).map(([, name]) => name);
    const known = SVCS.every(([id]) => $(id)?.classList.contains("ok"));
    const failed = srvNow?.failed_units;
    const reboot = srvNow?.reboot?.needed ?? (!updData?.stale && updData?.reboot);
    const level = down.length || failed > 0 ? "bad" : reboot || updData?.level ? "warn" : null;
    const healthy = known && failed === 0 && updData && !updData.unavailable;
    n.textContent = level ? "!" : healthy ? "✓" : "–";
    state("c-svc", level);
    $("c-svc").setAttribute("aria-label", ["Status", down.length ? "Down: " + down.join(", ") : known ? "Services running" : "Services unknown", updData?.summary || "Updates unknown", failed > 0 && `${failed} failed units`, reboot && "Reboot needed"].filter(Boolean).join(" · "));
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
    if (r === range) return;
    range = r;
    try { localStorage.setItem("range", String(range)); } catch {}
    drawRange();
    if (last) render();
  };
  $("range").addEventListener("click", (e) => { const b = e.target.closest("button[data-r]"); if (b) setRange(+b.dataset.r); });

  // ---- server: every three seconds --------------------------------------------------------------------------------
  const serverPercent = (key, percent) => {
    const value = $(`${key}-n`);
    value.textContent = Number.isFinite(percent) ? Math.round(percent) : "–";
    if (Number.isFinite(percent)) value.append(el("small", "", "%"));
    const dial = $(`c-${key}`);
    dial.classList.toggle("filled", percent > 0);
    dial.style.setProperty("--fill", Number.isFinite(percent) ? Math.max(0, Math.min(100, percent)) : 0);
    state(dial.id, percent > 90 ? "bad" : percent > 70 ? "warn" : null);
  };
  // A service chip: its dot; state, uptime, memory, restarts and version in its card.
  const svcInfo = {};
  let srvNow = null, updData = null;
  // The tab's title counts only what does not run (a service down), never reports or crash groups.
  const svcDown = {};
  const drawTitle = () => { const n = Object.values(svcDown).filter(Boolean).length; document.title = n ? `(${n} down) Yo admin` : "Yo admin"; };
  const service = (id, u) => {
    u = u || {};
    const ok = u.state ? u.state === "active" : null;
    svcDown[id] = ok === false; drawTitle();
    if ($(`t-${id}`)) state(`t-${id}`, ok == null ? null : ok ? "ok" : "bad");
    svcInfo[id] = { ok, rows: [
      ["State", ok ? "running" : u.state || "unknown", ok == null ? "" : ok ? "ok" : "bad"],
      ["Up", ok && u.since ? span(Date.now() / 1000 - u.since) : "–"],
      ["Memory", u.memory != null ? bytes(u.memory) : "–"],
      ["Restarts", num(u.restarts || 0), u.restarts ? "warn" : ""],
      ...(u.version ? [["Version", u.version.split(" ")[0]]] : []),
    ] };

  };
  // CLI: YoGPT and the status bot together, green only while both units run.
  function drawCli() {
    const y = svcInfo.yogpt, b = svcInfo.stbot;
    state("t-cli", y?.ok === false || b?.ok === false ? "bad" : y?.ok && b?.ok ? "ok" : null);
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
    reservedData = s.reserved;
    serverPercent("cpu", s.cpu?.percent);
    const m = s.memory, d = s.disk;
    serverPercent("mem", m?.total ? 100 * (m.total - m.available) / m.total : null);
    serverPercent("disk", d?.total ? 100 * d.used / d.total : null);
    part(() => {
      const n = s.network || {};
      // Below 100 KB/s in KB/s ("79K"), from there in Mbit/s with one decimal ("1.4M").
      const k = v => {
        if (v == null) return "–";
        if (v < 100000) return Math.round(v / 1000) + "K";
        let speed = v * 8 / 1e6;
        for (const suffix of ["M", "G", "T"]) {
          if (speed < 999.5) return speed.toFixed(speed < 9.95 ? 1 : 0) + suffix;
          speed /= 1000;
        }
        return "999T+";
      };
      const line = (arrow, value) => {
        const row = el("span");
        row.append(el("small", "", arrow), k(value));
        return row;
      };
      $("net-n").replaceChildren(line("↓", n.rx_per_s), line("↑", n.tx_per_s));
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
      service("stbot", s.stbot);
      drawCli();
      const people = last?.live?.yogpt_contacts;
      if (s.yogpt?.state === "active" && people != null) svcInfo.yogpt.rows.push(["Knows", `${num(people)} people`]);
    });
    if (last) part(drawTables);
    redrawDetail(["server", "svc"]);
  }

  // Updates: a dot in the row (green: all fine, no package updates); a click opens its detail.
  const updates = (u) => {
    const stale = !u || Date.now() / 1000 - u.at > 1800;
    const summary = stale ? "Update data unavailable" : u.apt.error || u.apt.packages == null ? "Package check unavailable"
      : u.apt.packages ? `${u.apt.packages} updates (${u.apt.security} security)` : "Up to date";
    const unavailable = stale || u.apt.error || u.apt.packages == null;
    const level = !unavailable && u.apt.security > 0 ? "warn" : null;
    updData = u ? { ...u, summary, stale, unavailable, level } : null;
    svcSum();
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
  const RANGES = [1, 7, 30, 0];
  let range = 7;
  try {
    const saved = localStorage.getItem("range");
    if (saved === "-1") range = 1;
    else if (saved !== null && RANGES.includes(+saved)) range = +saved;
  } catch {}
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
  const weekStarts = () => Array.from({ length: 28 }, (_, i) => hubNow.week.start + i * 6);
  const weekNames = () => weekStarts().map(h => new Date(h * 3600000).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric" }));
  const weekEvent = key => hubNow?.week?.events && (hubNow.week.events[key] || Array(28).fill(0));
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
    const all = (key) => ym(key), yo = ym("yo-out"), weekly = range === 7 && h.week?.events, names24 = hourNames(), slots = range === 1 ? names24.map((n, i) => [n, null, [i]]) : rangeSlots(yo), names = weekly ? weekNames() : slots.map(([n]) => n);
    const sums = (keys) => weekly ? names.map((_, i) => keys.map(k => h.week.events[k]?.[i] || 0)) : slots.map((s) => keys.map((k) => slotSum(s, all(k), yh(k))));
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
      `Push: taken by APNs and FCM · ${pctText(share(psAll))} · ${num(psAll[1])} failed (in Problems) · ${span}`, 100);
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
    const cb = h.current_build_installs || null;
    problemTotals = { current: cb ? [[cb[0], cb[1]], [cb[2], cb[3]], [cb[4], cb[5]]] : null, reports: reports(), installs: pi ? pi[1] : null, seen, errInstalls: pi ? pi[0] : null, crashInstalls, platforms: pi ? [["iOS", pi[2], pi[3], pi[4]], ["Android", pi[5], pi[6], pi[7]]] : [] };
    healthData = { span, ps, psAll, he, heAll, names, crashInstalls, seen, free: crashFree(crashInstalls, seen), gone, dau, mau, stick, heRate: rate(heAll) };
  };
  // The two chart widgets: a running total over the range (a point an hour, a day, or for Total up to 60 points),
  // ending at the total since the start; a click opens their detail.
  const areaData = {};
  const runningTotal = (perDay, hours, total, week) => {
    let names, incs, hourStarts, hourEnds;
    const nowH = Math.floor(Date.now() / 3600000);
    if (range === 1) {
      const h0 = nowH - 23;
      hourStarts = Array.from({ length: 24 }, (_, i) => h0 + i);
      hourEnds = hourStarts.map(h => h + 1);
      names = Array.from({ length: 24 }, (_, i) => clock((h0 + i + 1) * 3600000));
      incs = Array.isArray(hours) ? hours.slice(-24).map((v) => v || 0) : Array(24).fill(0);
    } else if (range === 7 && week) {
      incs = week;
      names = weekNames();
      hourStarts = weekStarts();
      hourEnds = hourStarts.map(h => Math.min(h + 6, nowH));
    } else {
      const first = range ? today0() - range + 1 : Math.min(today0() - 6, ...[...(perDay?.keys() || [])].filter((d) => perDay.get(d) > 0));
      const span = today0() - first + 1, step = Math.max(1, Math.ceil(span / 60)), n = Math.ceil(span / step);
      const ends = Array.from({ length: n }, (_, i) => today0() - (n - 1 - i) * step);
      names = ends.map(dayName);
      hourStarts = ends.map(e => (e + 1 - step) * 24);
      hourEnds = ends.map((e) => Math.min((e + 1) * 24, nowH));
      incs = ends.map((e) => { let t = 0; for (let d = e - step + 1; d <= e; d++) t += perDay?.get(d) || 0; return t; });
    }
    const values = []; let v = total;
    for (let i = incs.length - 1; i >= 0; i--) { values[i] = v; v -= incs[i]; }
    return { names, incs, hourStarts, hourEnds, values: [Math.max(0, v), ...values], labels: [range === 1 ? "24 hours ago" : "before", ...names] };
  };
  // The hover popup of every chart: the slot under the pointer with its values (text "slot · value · value"; the first
  // line bold), beside the pointer; mark(i) highlights that slot in the chart (-1 when the pointer leaves).
  const tipBox = el("div", "ctip"), closeTip = el("button", "tip-close", "×");
  closeTip.type = "button";
  closeTip.setAttribute("aria-label", "Close card");
  tipBox.hidden = true;
  document.body.append(tipBox);
  // A popup row: the series' colour (a swatch) or a symbol, what it is, its value.
  const tipRow = ({ ico, color, label, sub, value, cls, section, line }) => {
    if (section) return el("div", "tsec", section);
    const r = el("div", `tr${cls ? ` ${cls}` : ""}`), mark = el("span", "tm"), l = el("span", "tl", label);
    if (color) { mark.classList.add("sw"); if (line) mark.classList.add("ln"); mark.style.background = color; } else if (ico) mark.append(icon(ico));
    if (sub) l.append(el("small", "tsub", sub));
    r.append(mark, l, el("b", "tv", value));
    return r;
  };
  const SERIES = { "": "var(--ok)", cool: "#5dade2", bad: "#e67e22", acc: "var(--amethyst-soft)" };
  const slotWord = () => (range === 1 ? "This hour" : range ? "This day" : "This stretch");
  let tipAnchor, tipX, tipY;
  const showTip = (t, x, y, node) => {
    const held = tipBox.querySelector("#st-every");  // the status bot's select goes home before the card changes
    if (held) { held.hidden = true; $("t-system").append(held); }
    if (typeof t === "string") {
      const [head, ...rest] = t.split(" · ");
      t = { head, note: rest.join(" · ") };
    }
    tipBox.replaceChildren(el("b", "th", t.head), ...(t.nodes || []).filter(Boolean), ...(t.rows || []).filter(Boolean).map(tipRow), ...(t.note ? [el("span", "tn", t.note)] : []));
    if (t.action) tipBox.append(t.action);
    tipBox.append(closeTip);
    tipBox.hidden = false;
    tipAnchor = node;
    tipX = x;
    tipY = y;
    fitTip();
  };
  // An element that explains itself: the chart popup instead of a native title; on hover, keyboard focus and tap.
  // t: a text ("head · line · line") or { head, note, rows }.
  // The card a click or tap pins open (a row with something to operate in it); it closes on a click outside or Escape.
  let pinned = null, clearMark;
  const cardTriggers = new WeakSet();
  let preview;
  const cancelPreview = () => {
    if (!preview) return;
    preview = null;
    tipBox.hidden = true;
  };
  const bindPreview = (node, show, leave) => {
    cardTriggers.add(node);
    const schedule = (point, source) => {
      if (pinned) return;
      preview = { node, source };
      show(point);
    };
    const mouse = e => {
      if (pinned || e.pointerType !== "mouse" || e.composedPath().find(el => cardTriggers.has(el)) !== node) return;
      schedule(e, "mouse");
    };
    node.addEventListener("pointerenter", mouse);
    node.addEventListener("pointermove", mouse);
    node.addEventListener("pointerleave", e => {
      if (pinned || e.pointerType !== "mouse") return;
      if (preview?.node === node && preview.source === "mouse") cancelPreview();
      leave?.(-1);
    });
    node.addEventListener("focus", () => {
      if (!node.matches(":focus-visible")) return;
      const r = node.getBoundingClientRect();
      schedule({ clientX: r.left + r.width / 2, clientY: r.top }, "focus");
    });
    node.addEventListener("blur", () => {
      if (preview?.node === node && preview.source === "focus") { cancelPreview(); leave?.(-1); }
    });
  };
  const unpin = () => {
    pinned = null;
    clearMark?.(-1);
    clearMark = null;
    tipBox.classList.remove("pin");
    tipBox.hidden = true;
    const sel = tipBox.querySelector("#st-every");
    if (sel) { sel.hidden = true; $("t-system").append(sel); }
  };
  closeTip.addEventListener("click", unpin);
  const fitTip = () => {
    if (tipBox.hidden) return;
    const r = tipAnchor.getBoundingClientRect(), w = tipBox.offsetWidth, h = tipBox.offsetHeight;
    const clamp = (v, max) => Math.max(8, Math.min(v, max - 8));
    let x = clamp(tipX - w / 2, innerWidth - w), y;
    if (r.top >= h + 16) y = r.top - h - 8;
    else if (innerHeight - r.bottom >= h + 16) y = r.bottom + 8;
    else {
      y = clamp(tipY - h / 2, innerHeight - h);
      if (innerWidth - r.right >= w + 16) x = r.right + 8;
      else if (r.left >= w + 16) x = r.left - w - 8;
      else y = clamp(r.top > innerHeight - r.bottom ? r.top - h - 8 : r.bottom + 8, innerHeight - h);
    }
    tipBox.style.left = x + "px";
    tipBox.style.top = clamp(y, innerHeight - h) + "px";
  };
  new ResizeObserver(fitTip).observe(tipBox);
  window.addEventListener("resize", fitTip);
  const pinCard = (node, show, mark) => {
    if (node.tabIndex < 0 || node.tabIndex == null) node.tabIndex = 0;
    if (!node.matches("button, a[href], input, select")) node.addEventListener("keydown", event => {
      if (event.target !== node || !["Enter", " "].includes(event.key)) return;
      event.preventDefault();
      const r = node.getBoundingClientRect();
      node.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: r.left + r.width / 2, clientY: r.top }));
    });
    node.addEventListener("click", (event) => {
      if (pinned) return;
      cancelPreview();
      tipBox.classList.add("pin");
      if (show(event) === false) { unpin(); return; }
      pinned = node;
      clearMark = mark;
    });
  };
  const hint = (node, t) => {
    if (!t) return node;
    const tv = () => (typeof t === "function" ? t() : t);
    const at = () => { const r = node.getBoundingClientRect(); showTip(tv(), r.left + r.width / 2, r.top, node); };
    bindPreview(node, e => showTip(tv(), e.clientX, e.clientY, node));
    pinCard(node, at);
    return node;
  };
  document.addEventListener("click", (e) => { if (pinned && !tipBox.contains(e.target) && !pinned.contains(e.target)) unpin(); }, true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pinned) unpin(); });
  // The header gauges show server details: CPU, memory and LAN with their history over the range
  // (the bars the detail cards drew), disk with its sizes, updates and services with what they hold.
  const serverSeries = (col) => {
    const s = srvNow || {}, now = Date.now();
    // One point a day for 7 and 30 days (and Total): its day number stands for its UTC midnight.
    const [pts, size, count] = range === 1 ? [s.history_day || [], 3600000, 24]
      : [(s.history_days || []).map((p) => [p[0] * 86400000, ...p.slice(1)]), 86400000, range || Math.max(1, today0() - (s.history_days?.[0]?.[0] ?? today0()) + 1)];
    const first = Math.floor(now / size) - count + 1, sums = Array.from({ length: count }, () => [0, 0]);
    for (const p of pts) { const k = Math.floor(p[0] / size) - first; if (k >= 0 && k < count) { sums[k][0] += col(p); sums[k][1]++; } }
    return { values: sums.map(([t, n]) => (n ? t / n : null)), names: sums.map((_, k) => (size === 86400000 ? dayName(first + k) : clock((first + k + 1) * size))) };
  };
  const plotWithAxis = (chart, from, to) => {
    const plot = el("div", "tip-plot"), axis = el("div", "chart-axis");
    axis.append(el("span", null, from), el("span", null, to));
    plot.append(chart, axis);
    return plot;
  };
  const historyPlot = (values, names, cls, line = false) => {
    const first = values.findIndex(v => v != null);
    if (first < 0) return el("span", "tn", "No recorded samples in range");
    if (values.filter(v => v != null).length < 3) return null;
    const data = values.slice(first), from = (first ? "since " : "") + names[first];
    if (line) return liveChart({ line: data, from, to: names.at(-1), level: true }, cls);
    return plotWithAxis(sparkBars(data, cls), from, names.at(-1));
  };
  const histNode = (col, fmt) => {
    const { values, names } = serverSeries(col), g = historyPlot(values, names, "cool");
    const latest = values.findLast(v => v != null);
    return [g, tipRow({ label: "Latest sample", value: latest == null ? "–" : fmt(latest) })];
  };
  let problemCard;
  const serverHints = () => {
    hint($("problem-dial"), () => problemCard || { head: "Problems", note: "Waiting for data" });
    const pct = (v) => `${Math.round(v)} %`;
    hint($("c-cpu"), () => { const s = srvNow || {}, c = s.cpu || {};
      return { cap: 1, head: "CPU", nodes: histNode((p) => p[1] ?? 0, pct), rows: [c.cores ? { label: "Cores", value: String(c.cores) } : null, c.load?.length ? { label: "Load", value: c.load.map((x) => x.toFixed(2)).join(" · ") } : null, s.uptime == null ? null : { label: "Up", value: span(s.uptime) }] }; });
    hint($("c-mem"), () => { const m = (srvNow || {}).memory, used = m ? m.total - m.available : 0;
      return { cap: 1, head: "Memory", nodes: histNode((p) => p[2] ?? 0, pct), rows: m ? [{ label: "Used", value: `${bytes(used)} of ${bytes(m.total)}` }, m.swap_total ? { label: "Swap", value: bytes(m.swap_total - m.swap_free) } : null] : [] }; });
    hint($("c-disk"), () => { const d = (srvNow || {}).disk;
      return { cap: 1, head: "Disk", rows: d ? [{ label: "Used", value: bytes(d.used) }, { label: "Free", value: bytes(d.free) }, { label: "Size", value: bytes(d.total) }] : [{ label: "Disk", value: "unknown" }] }; });
    hint($("c-net"), () => {
      const n = srvNow?.network || {}, recent = srvNow?.history || [];
      const throughput = (p) => (p[4] || 0) + (p[5] || 0);
      const nodes = histNode(throughput, rate);
      if (recent.length >= 3) {
        const chart = sparkBars(recent.map(throughput), "cool", null);
        const caption = el("span", "tn", "Last hour");
        const readSample = (event) => {
          const rect = chart.getBoundingClientRect();
          const i = Math.floor((event.clientX - rect.left) * recent.length / rect.width);
          const sample = recent[Math.max(0, Math.min(i, recent.length - 1))];
          caption.textContent = `${clock(sample[0])} · ↓${rate(sample[4])} ↑${rate(sample[5])}`;
        };
        chart.addEventListener("pointermove", readSample);
        chart.addEventListener("click", readSample);
        nodes.push(plotWithAxis(chart, clock(recent[0][0]), clock(recent.at(-1)[0])), caption);
      }
      return { cap: 1, head: "LAN", note: "K = KB/s · M = Mbit/s", nodes,
        rows: [{ label: "Down", value: rate(n.rx_per_s) }, { label: "Up", value: rate(n.tx_per_s) }] };
    });
    const updateDetails = () => {
      const u = updData;
      if (!u) return { cap: 1, head: "Updates", rows: [{ label: "Collector", value: "unavailable" }] };
      const kernel = !!(u.installed_kernel && u.running_kernel && u.installed_kernel !== u.running_kernel), lts = !!(u.release?.result && !/no new release/i.test(u.release.result));
      const yn = (b) => (b ? "yes" : "no");
      return { cap: 1, head: "Updates", note: u.unavailable ? u.summary : null, rows: [{ label: "Updates", value: u.stale || u.apt.error || u.apt.packages == null ? "unknown" : u.apt.packages ? `${num(u.apt.packages)}${u.apt.security ? ` (${num(u.apt.security)} security)` : ""}` : "none" },
        { label: "Reboot needed", value: yn(srvNow?.reboot?.needed ?? u.reboot) }, u.reboot_packages?.length ? { label: "Packages", value: u.reboot_packages.join(", ") } : null, { label: "Kernel update", value: yn(kernel) }, { label: "New Ubuntu LTS", value: yn(lts) }] };
    };
    hint($("c-svc"), () => {
      const rows = [["hub", "Hub"], ["dev", "Dev"], ["tunnel", "Tunnel"], ["yogpt", "YoGPT"], ["stbot", "Status bot"]].map(([id, name]) => {
        const i = svcInfo[id], r = (k) => i?.rows?.find(([n]) => n === k)?.[1], up = r("Up"), row = el("div", "tr"), mark = el("span", "tm");
        mark.append(el("span", `dot ${i?.ok == null ? "" : i.ok ? "ok" : "bad"}`));
        row.append(mark, el("span", "tl", name), el("b", "tv", i?.ok == null ? "unknown" : i.ok ? (up && up !== "–" ? up : "running") : r("State") || "off"));
        if (!i?.rows?.length) return row;
        const detail = el("details", "tip-details"), summary = el("summary");
        summary.append(row);
        detail.append(summary, ...i.rows.concat(id === "stbot" ? botReportRows : []).map(([label, value, cls]) => tipRow({ label, value, cls })));
        return detail;
      });
      const sel = $("st-every"), every = el("label", "hc-sel", "Status bot schedule");
      sel.hidden = false; every.append(sel);
      const u = updateDetails(), extra = el("details", "tip-details");
      extra.append(el("summary", null, "Package and release details"));
      extra.addEventListener("toggle", () => {
        if (extra.open && extra.childElementCount === 1) extra.append(...DETAIL.upd());
      });
      return { head: "Status", nodes: [tipRow({ section: "Updates" }), ...u.rows.filter(Boolean).map(tipRow),
        ...(u.note ? [el("span", "tn", u.note)] : []), extra, tipRow({ section: "Services" }), ...rows, tipRow({ label: "Failed units", value: num(srvNow?.failed_units) }), every] };
    });
  };
  document.addEventListener("pointerdown", (e) => { if (e.pointerType !== "mouse" && !pinned) cancelPreview(); }, true);
  const hoverTip = (node, count, text, mark) => {
    const slots = () => typeof count === "function" ? count() : count;
    const slotAt = (e, n) => {
      const r = node.getBoundingClientRect();
      return n && r.width ? Math.min(n - 1, Math.max(0, Math.floor((e.clientX - r.left) * n / r.width))) : -1;
    };
    const show = (e) => {
      const n = slots(), i = slotAt(e, n), t = i >= 0 && text(i);
      if (!t) { tipBox.hidden = true; mark?.(-1); return false; }
      mark?.(i);
      const card = typeof t === "string" ? { head: t } : t;
      if (Number(node.dataset.samples ?? n) < 3) {
        showTip(card, e.clientX, e.clientY, node);
        return;
      }
      const chart = node.cloneNode(true);
      chart.removeAttribute("id");
      chart.removeAttribute("hidden");
      chart.querySelectorAll("[id]").forEach(el => el.removeAttribute("id"));
      const first = Number(node.dataset.first || 0);
      if (first > 0 && first < n && chart.classList.contains("spark")) {
        const offset = 140 * first / n;
        chart.setAttribute("viewBox", `${offset} 0 ${140 - offset} 28`);
      }
      chart.querySelectorAll("text.ax").forEach(label => label.remove());
      const plot = plotWithAxis(chart, (first > 0 || node.dataset.since ? "since " : "") + (text(first)?.head || ""), text(n - 1)?.head || "");
      if (node.closest(".area")) plot.classList.add("area", node.closest("#t-uc") ? "c-acc" : "c-green");
      showTip({ ...card, nodes: [plot, ...(card.nodes || [])] }, e.clientX, e.clientY, node);
    };
    bindPreview(node, show, mark);
    pinCard(node, show, mark);
  };
  window.addEventListener("scroll", () => { if (!pinned) cancelPreview(); }, { passive: true, capture: true });
  // The painter of every running-total chart (the overview's two and the details'): bars (each slot's count, between
  // the line's points when there is one point more, else under them; up to half the height under a line), the line with
  // its dots over them, the first and last slot as the axis. Sized to its svg in pixels, so text stays text-sized.
  const paintCombo = (node, { line, line2, bars, from, to, max, level = false, pick = -1, hover = -1 }) => {
    const W = node.clientWidth, H = node.clientHeight;
    if (!W || !H) return;
    const pad = 6, padTop = 14, B = H - 16, between = line && bars && bars.length === line.length - 1;
    const slots = bars ? bars.length : line.length, np = line ? line.length : 0, gap = (W - 2 * pad) / Math.max(1, slots);
    const px = (i) => pad + (between ? i : i + 0.5) * gap, out = [];
    if (bars) {
      const top = max || Math.max(1, ...bars.map((v) => v || 0)), room = (B - padTop) * (line ? 0.55 : 1);
      bars.forEach((v, i) => {
        const h = v ? Math.max(2, (v / top) * room) : 1.5;
        out.push(svg("rect", { x: (pad + i * gap + 0.5).toFixed(1), width: Math.max(0.5, gap - 1).toFixed(1), y: (B - h).toFixed(1), height: h.toFixed(1), rx: 1.5,
          class: `cb${v ? "" : " zero"}${i === pick || (pick < 0 && i === bars.length - 1) ? " now" : ""}` }));
      });
    }
    if (line) {
      const have = line.filter((v) => v != null), lo = level ? 0 : Math.min(...have), hi = Math.max(level ? 1 : -Infinity, ...have);
      const y = (v) => padTop + (B - padTop - 3) * (hi === lo ? 0.5 : 1 - (v - lo) / (hi - lo));
      const pts = line.map((v, i) => (v == null ? null : `${px(i).toFixed(1)},${y(v).toFixed(1)}`)).filter(Boolean);
      if (level && pts.length > 1) out.push(svg("polygon", { class: "level-fill", points: [`${px(0)},${B}`, ...pts, `${px(np - 1)},${B}`].join(" ") }));
      if (pts.length > 1) out.push(svg("path", { class: "line", d: `M${pts.join(" L")}` }));
      const every = Math.max(1, Math.ceil(np / 16));
      line.forEach((v, i) => { if (v != null) out.push(svg("circle", { class: i % every && i !== np - 1 && i !== pick ? "dot hide" : "dot", cx: px(i).toFixed(1), cy: y(v).toFixed(1), r: 2.6 })); });
    }
    // Active share uses the full 0–100 % range and leaves unsampled slots empty.
    if (line2) {
      const have = line2.filter((v) => v != null);
      if (have.length) {
        const y = (v) => padTop + (B - padTop - 3) * (1 - v / 100);
        const pts = [], dots = [];
        line2.forEach((v, i) => {
          if (v == null) return;
          const cx = px(i).toFixed(1), cy = y(v).toFixed(1);
          pts.push(`${line2[i - 1] == null ? "M" : "L"}${cx},${cy}`);
          const visible = line2[i - 1] == null || line2[i + 1] == null || i === pick;
          dots.push(svg("circle", { class: visible ? "dot act" : "dot act hide", cx, cy, r: 2.6 }));
        });
        if (pts.length > 1) out.push(svg("path", { class: "line act", d: pts.join(" ") }));
        out.push(...dots);
      }
    }
    if (hover >= 0) { const hx = (bars ? pad + hover * gap + gap / 2 : px(hover)).toFixed(1); out.push(svg("line", { x1: hx, x2: hx, y1: padTop - 6, y2: B, class: "hmark" })); }
    const ax = (tx, text, anchor) => { const t = svg("text", { x: tx, y: H - 2, "text-anchor": anchor, class: "ax" }); t.textContent = text || ""; return t; };
    node.setAttribute("viewBox", `0 0 ${W} ${H}`);
    node.replaceChildren(svg("line", { x1: pad, x2: W - pad, y1: B, y2: B, stroke: "var(--line)" }), ...out, ax(pad, from, "start"), ax(W - pad, to, "end"));
  };
  // Accounts: the active accounts (the hub's active-48h, hourly, then daily) at the end of each slot; none before sampling began.
  const activeSlots = (d, samples) => d.hourEnds?.map((end, i) => {
    const total = d.values[i + 1];
    let count = null;
    for (const [hour, n] of samples) {
      if (hour > end) break;
      if (hour > d.hourStarts[i]) count = n;
    }
    return count != null && total > 0 ? { count, total, percent: 100 * count / total } : null;
  });
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
      paintCombo(node, { line: d.values, line2: d.active && [null, ...d.active.map(slot => slot?.percent)], bars: d.incs, pick: hv, hover: hv, from: range === 1 ? "24 hours ago" : d.names[0], to: range === 1 ? "now" : d.names[d.names.length - 1] });
      return;
    }
    // No numbers in the plot: the total is the title's, the time is on the axis under the bars.
    node.replaceChildren(fill, line, ...extra, ...dots);
  };
  // A second series drawn over a chart: Active over Accounts, Media over Yos (on the Yos' scale).
  const OVERLAY = {}, SAME = {}, COMBO = { yc: true, uc: true }, WORD = { yc: "Yos", uc: "accounts" };
  // Each curve's legend, under it: what its two lines are.
  const LEGEND = { yc: [["Total", "l-main"], ["Sent", "l-bar"]], uc: [["Total", "l-main"], ["Sign-ups", "l-bar"], ["Active (48 h)", "l-act"]] };
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
  const areaChart = (key, total, perDay, hours, today, week) => {
    set(`${key}-n`, num(total));
    if (!areaTips[key] && $(`${key}-svg`)) {
      areaTips[key] = true;
      hoverTip($(`${key}-svg`), () => areaData[key]?.incs?.length || 0, (i) => {
        const d = areaData[key], what = { yc: sentMode === "sent" ? "sent" : "delivered", uc: "signed up" }[key] || "";
        const active = d?.active?.[i];
        const c = key === "uc" ? "var(--amethyst-soft)" : "var(--ok)";
        return d ? { head: d.names[i], rows: [{ section: range === 7 && d.incs.length === 28 ? "These six hours" : slotWord() }, { color: c, label: what[0].toUpperCase() + what.slice(1), value: num(d.incs[i]) },
          { section: "In all" }, { color: c, line: true, label: "Total", value: num(d.values[i + 1]) },
          ...(key === "yc" && sentMode !== "sent" ? [{ section: "One per device; before 2026-11-21 incl. bots" }] : []),
          ...(active ? [{ color: "var(--ok)", line: true, label: "Active", value: `${Math.round(active.percent)} % (${num(active.count)} of ${num(active.total)})` }] : [])] } : "";
      }, (i) => { areaHover[key] = i; drawArea(key); });
    }
    areaData[key] = { ...runningTotal(perDay, hours, total || 0, week), overlay: OVERLAY[key] || null, word: WORD[key] };
    if (key === "uc") areaData[key].active = activeSlots(areaData[key], hubNow?.active48 || []);
    drawArea(key);
    // Under the chart, its activity as bars: per hour (1 day), per six hours (7), per day (30), per stretch of days (Total).
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
      // The growth over the range is the big number (a day: the last 24 hours), the total small under it.
      const grow = range === 1 || !range ? (today ?? 0) : inc.reduce((a, v) => a + v, 0);
      set(`${key}-t`, num(grow)); set(`${key}-w`, range === 7 ? "in 7 d" : range === 30 ? "in 30 d" : "in 24 h");
      return;
    }
    const bars = inc.map((v, i) => {
      const h = fixedMax ? (v / top) * 26 : v ? Math.max(2, (v / top) * 26) : 1.5;
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
  let platData = null;
  const charts = ["yc", "uc"];
  if (window.ResizeObserver) { const ro = new ResizeObserver(() => charts.forEach(drawArea)); charts.forEach((k) => ro.observe($(`${k}-area`))); }
  const drawRange = () => {
    for (const b of $("range").querySelectorAll("button[data-r]")) { b.classList.toggle("on", +b.dataset.r === range); b.setAttribute("aria-pressed", String(+b.dataset.r === range)); }
  };
  const last24 = (v) => (Array.isArray(v) ? v.slice(-24) : Array(24).fill(null));
  const last14 = (v) => (Array.isArray(v) ? v.slice(-14) : Array(14).fill(null));

  // ---- overview: the newest users, a name with small icons (platform coloured by its build, a lock when secured) --
  const ICONS = {
    apple: '<path d="M12 7.5c-1.3-1.2-4.2-1.4-5.600.8C5 10.600 6 15.500 8 18c1 1.300 2 1.500 4 .8 2 .7 3 .5 4-.8 1.200-1.500 2-3.200 2.200-4.200-2-.8-2.700-3.300-1-5-1.300-1.500-3.500-1.500-5.200-.3z"/><path d="M12 7.5c0-2 1-3.500 3-4.500"/>',
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
  // One stacked bar of shares, current at the top; an empty slot a faint dash.
  const vStack = (row, x, w, H, top) => {
    const all = row ? row.reduce((a, v) => a + v, 0) : 0;
    if (!all) return [svg("rect", { x: x.toFixed(1), width: w.toFixed(1), y: (H - 1.5).toFixed(1), height: 1.5, rx: 1, class: "b zero" })];
    let y = top;
    return VKEYS.map((k, i) => {
      const hh = (row[i] / all) * (H - top);
      const bandTop = y;
      y += hh;
      return row[i] ? svg("rect", { x: x.toFixed(1), width: w.toFixed(1), y: bandTop.toFixed(1), height: hh.toFixed(1), class: `v-${k}` }) : null;
    }).filter(Boolean);
  };
  // ---- overview ---------------------------------------------------------------------------------------------------
  // Active, a tile of the top grid like Sent and Accounts: who was here per day (bars) and the share of them who came
  // back (line); the number and its share of the month in the header, retention and the rest in the hover.
  let activeSpec = null;
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
    const attest = $("attestation");
    attest.hidden = !hubNow?.attestation;
    if (!attest.hidden) {
      const rows = Object.entries(h.attestation);
      const platform = p => p === "ios" ? "iOS" : "Android";
      attest.textContent = "Attested / failed / unknown · " + rows.map(([p, r]) =>
        `${platform(p)} ${num(r.attested)} / ${num(r.failed)} / ${num(r.unknown)}`).join(" · ");
      attest.title = rows.map(([p, r]) => {
        const details = [...Object.entries(r.reasons), ...Object.entries(r.levels)].map(([k, n]) => `${k} ${num(n)}`);
        details.push(r.last ? "Last " + new Date(r.last).toLocaleString() : "No attestation yet");
        return [platform(p), ...details].join(" · ");
      }).concat("Unknown counts phone accounts seen within 48 hours.").join("\n");
    }
    if (!hubNow) {
      set("problem-n", "–");
      state("problem-dial", null);
      $("problem-trend").replaceChildren();
      problemCard = null;
    }
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
      rangeRow("md", ym("media-in-bytes"), "", "Media: photos and videos uploaded", null, false, yh("media-in-bytes"), null, yt("media-in-bytes"));
      // Media's running total over Yos on its own scale; in the hover with its share of Yos.
      const mdTotal = yt("media-in") ?? 0, md = runningTotal(ym("media-in"), yh("media-in"), mdTotal, weekEvent("media-in"));
      seriesChart("ym", { values: md.values, labels: md.labels, word: "media", share: true },
        null);
      rangeRow("gr", ym("group-create"), "made", "Groups: made", null, false, yh("group-create"), null, yt("group-create"));
      part(() => healthRows(h, ym, yh));
      // Running totals: Yos into users' inboxes (a bot's inbox apart), accounts without bots.
      // Sent (into inboxes) or Delivered (acknowledged by a device): the pill in the header chooses, the choice is kept.
      const yk = sentMode === "sent" ? "yo-in" : "yo-out";
      drawSentPill();
      areaChart("yc", yt(yk) ?? 0, ym(yk), yh(yk), last24(yh(yk)).reduce((n, v) => n + (v || 0), 0), weekEvent(yk));
      const sud = new Map(h.signup_days || []);
      areaChart("uc", [...sud.values()].reduce((a, v) => a + v, 0), sud, h.signups_hours, last24(h.signups_hours).reduce((n, v) => n + (v || 0), 0), h.week?.signups);
      const signups = range ? sumOf(perSlot(sud, h.signups_hours)) : [...sud.values()].reduce((a, v) => a + v, 0);
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
  let showDone = false;
  try { showDone = localStorage.getItem("todos-show-done") === "1"; } catch {}
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
  const allValue = (u, key, releases) => key === "name" ? (u.name || "").toLowerCase() : key === "app" ? (parse(u.app)?.[1] ?? -1)
    : key === "update" ? (releases.get(u.app)?.behind ?? -1)
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
    const releases = releaseBands(h || {});
    const list = [...users].sort((a, b) => {
      const x = allValue(a, allSort.key, releases), y = allValue(b, allSort.key, releases);
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
      const band = releases.get(u.app);
      const upd = tag(band ? (band.behind ? `−${band.behind}` : "current") : "?", band ? "release" : "muted");
      if (band) upd.style.setProperty("--release-color", band.color);
      upd.title = appTitle(u.app, u.id) || "Unknown build";
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
    ["Support, Problems and Reports", "chat", ["support_messages", "reports", "diagnostics", "diagnostic_days", "diagnostic_seen", "diagnostic_installs", "todos"]],
    ["Stats", "chart", ["deliveries_hourly*", "events", "request_hours"]],
  ];
  Object.assign(ICONS, {
    user: '<circle cx="12" cy="8" r="3.5"/><path d="M5 20a7 7 0 0 1 14 0"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="M11 12l8-8M16 7l2 2M14 9l2 2"/>',
    mail: '<rect x="3" y="5.5" width="18" height="13" rx="2"/><path d="m3.5 7 8.5 6 8.5-6"/>',
    group: '<circle cx="9" cy="8.5" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M15.5 5.2a3.5 3.5 0 0 1 0 6.6"/><path d="M18 14.3a6.5 6.5 0 0 1 3.5 5.7"/>',
    disk: '<ellipse cx="12" cy="6" rx="7.5" ry="2.8"/><path d="M4.5 6v12c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V6"/>',
    chat: '<path d="M5 5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H10l-4 3.5V16H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/>',
    flag: '<path d="M5 21V4c5-4 9 4 15 0v10c-6 4-10-4-15 0"/>',
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
    ["v-errors", "Problems", "warn", "App problems grouped by cause, open first, with Done and Reopen"],
    ["v-reports", "Reports", "flag", "Reported users: how many reports, from how many installs, why; the raw table is under Database"],
    ["v-todos", "To-dos", "check", "The team's to-dos: add, tick, delete"],
  ];
  const VIEW_TABLE = { people: "users", "v-support": "support_messages", "v-errors": "diagnostics", "v-reports": "reports", "v-todos": "todos" };
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
    diagnostics: "App problems, one row per signature",
    diagnostic_seen: "Report ids already counted, never counted twice",
    diagnostic_days: "App problems per day, last 14 days",
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
  const diagnosticBuild = (d, payload) => {
    const app = d.app || payload.app || "";
    const rc = [payload.release, payload.version, app].join(" ").match(/\bRC(?:[ -]?\d+)?\b/i);
    const stamp = build(app), dated = /^\d{12}$/.test(stamp);
    const dateLabel = dated ? buildDate(app) : "";
    const label = dated && versionOf(app) ? buildLabel(app) : rc && dated ? `${rc[0]} (${dateLabel})` : rc?.[0] || dateLabel || app;
    return { key: app || label, label, oldest: dated ? dateLabel.replace(/ \d\d:\d\d$/, "") : label,
      full: dated ? `${label} · ${app} UTC` : app || label,
      time: dated ? Date.UTC(+stamp.slice(0, 4), +stamp.slice(4, 6) - 1, +stamp.slice(6, 8),
        +stamp.slice(8, 10), +stamp.slice(10)) : d.first_seen };
  };
  const diagnosticList = (rows, previous) => {
    const list = el("div", "diag-list"), groups = new Map(), hub = which;
    const expanded = new Set([...previous.querySelectorAll("details[data-cause][open]")].map(d => d.dataset.cause));
    const done = el("details", "diag-done"), doneList = el("div", "diag-list");
    done.open = !!previous.querySelector(".diag-done[open]");
    for (const d of rows) {
      const key = diagCause(d);
      if (!groups.has(key)) groups.set(key, { key, rows: [], open: false, expected: true, recent: 0, recentApps: new Set(), first: d.first_seen, last: d.last_seen });
      const g = groups.get(key);
      g.rows.push(d);
      g.open ||= !d.done && !diagInfo(d).expected;
      g.expected &&= diagInfo(d).expected;
      // Its reports in the last 24 hours and the builds they came from (the hub's diagnostic_hours and _builds).
      g.recent += (d.hours_24h || []).reduce((a, n) => a + n, 0);
      for (const [app, , seen] of d.builds || []) if (seen > Date.now() - 864e5) g.recentApps.add(app);
      g.first = Math.min(g.first, d.first_seen);
      g.last = Math.max(g.last, d.last_seen);
    }
    const still = el("div", "diag-list diag-still"), expectedList = el("div", "diag-list");
    const expected = el("details", "diag-done diag-expected");
    expected.open = !!previous.querySelector(".diag-expected[open]");
    let expectedRecent = 0;
    for (const g of [...groups.values()].sort((a, b) => b.open - a.open || b.recent - a.recent || b.last - a.last)) {
      const details = el("details"), summary = el("summary"), item = el("article", "diag-item");
      details.dataset.cause = g.key;
      details.open = expanded.has(g.key);
      const places = new Map(), builds = new Map();
      for (const d of g.rows) {
        const p = diagPayload(d);
        const source = [p.file, p.line].filter(Boolean).join(":");
        const place = [source, p.place || p.function].filter(Boolean).join(" ") || p.endpoint
          || String(p.frames || "").split("\n").find(line => /\bYo\b/.test(line));
        if (place) places.set(place, (places.get(place) || 0) + d.count);
        const version = diagnosticBuild(d, p);
        if (version.key && !builds.has(version.key)) builds.set(version.key, version);
      }
      const locations = [...places.keys()].sort((a, b) => places.get(b) - places.get(a));
      const versions = [...builds.values()].sort((a, b) => b.time - a.time);
      const first = g.rows[0], reports = g.rows.reduce((sum, d) => sum + d.count, 0);
      const title = el("strong", "diag-title", diagInfo(first).title);
      title.dataset.status = g.open ? "Open" : "Done";
      const stats = el("div", "diag-stats"), phones = first.cause_installs;
      const seen = last?.[hub]?.crash_free?.["0"]?.[1], age = rel(g.last);
      const share = el("span", null, pctText(seen && phones != null ? 100 * phones / seen : null));
      share.title = "Share of phones seen in 90 days";
      const recent = el("span", null, age === "just now" || !g.last ? age : age + " ago");
      recent.title = "Last seen " + date(g.last);
      stats.append(el("span", null, `${num(reports)} occurrence${reports === 1 ? "" : "s"}`),
        el("span", null, `${num(phones)} phone${phones === 1 ? "" : "s"}`), share, recent);
      if (g.recent) stats.append(el("span", "diag-recent", `${num(g.recent)} in 24 h`));
      const placeLine = el("div", "diag-place");
      const main = locations[0]?.replace(/^\S*\/([^/\s]+\.swift:\d+)/, "$1").replace(/^\d+\s+Yo\s+/, "");
      placeLine.append(el("span", null, main || "Location unknown"));
      if (locations.length > 1) placeLine.append(el("small", null, `+${locations.length - 1} more`));
      summary.textContent = versions.length > 1
        ? `${versions.length} builds · newest ${versions[0].label} · oldest ${versions.at(-1).oldest}`
        : versions.length ? "1 build · " + versions[0].label : "Build unknown";
      const facts = el("dl", "diag-facts");
      for (const [name, values] of [["Builds", versions.map(v => v.full)], ["Places", locations], ["First seen", [date(g.first)]]]) {
        facts.append(el("dt", null, name));
        for (const value of values.length ? values : ["Unknown"]) facts.append(el("dd", null, value));
      }
      const raw = el("pre", "pii");
      const showRaw = () => {
        if (details.open && !raw.textContent) raw.textContent = JSON.stringify(g.rows.map(d => ({ ...d, payload: diagPayload(d) })), null, 2);
      };
      details.addEventListener("toggle", showRaw);
      showRaw();
      details.append(summary, facts, raw);
      const button = el("button", "pill", g.open ? "Mark done" : "Reopen");
      button.type = "button";
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          for (const d of g.rows.filter(d => !!d.done !== g.open))
            await postJSON("/diagnostics/done", { hub, id: d.id, done: g.open });
        } catch (error) { item.append(el("span", "error", error.message)); }
        finally { await refresh(); button.disabled = false; }
      });
      item.append(title, stats, button, placeLine);
      // Done and still coming in: from builds it was marked done in (a newer build opens it again, spec 6.4).
      const stillIn = !g.open && !g.expected && g.recent > 0;
      if (stillIn) {
        const newest = [...g.recentApps].sort((a, b) => build(b).localeCompare(build(a)));
        const names = ["ios", "android"].map(p => newest.find(a => a.startsWith(`${p}/`))).filter(Boolean)
          .map(a => `≤ ${/^\d{12}$/.test(build(a)) && versionOf(a) ? `${a.startsWith("android/") ? "Android " : ""}${versionOf(a)}` : buildDate(a)}`);
        item.append(el("div", "diag-place diag-still-note", `Still reported: ${num(g.recent)} in 24 h, only on old builds${names.length ? ` (${names.join(", ")})` : ""}`));
      }
      item.append(details);
      if (g.expected) expectedRecent += g.recent;
      (g.open ? list : stillIn ? still : g.expected ? expectedList : doneList).append(item);
    }
    if (still.childElementCount) list.append(el("h4", "diag-head", `Acknowledged, still coming in (${still.childElementCount})`), still);
    if (expectedList.childElementCount) {
      expected.append(el("summary", null, `Expected (${expectedList.childElementCount}) · ${num(expectedRecent)} in 24 h`), expectedList);
      list.append(expected);
    }
    if (doneList.childElementCount) {
      done.append(el("summary", null, `Done (${doneList.childElementCount})`), doneList);
      list.append(done);
    }
    if (!groups.size) list.append(el("p", "empty", "No diagnostics."));
    return list;
  };
  const openSupportCount = h => h?.support_unread_days?.at(-1) ?? 0;
  const reportList = (h, previous) => {
    const list = el("div", "diag-list"), done = el("details", "diag-done"), doneList = el("div", "diag-list");
    const expanded = new Set([...previous.querySelectorAll("details[data-reported][open]")].map(d => d.dataset.reported));
    done.open = !!previous.querySelector(".diag-done[open]");
    const groups = reportGroups({ reports: h?.reports || [], users: h?.users || [], isDone: (r) => lastDone.has(reportKey(r)) });
    for (const g of groups) {
      const item = el("article", "diag-item"), details = el("details"), title = el("strong", "diag-title");
      title.dataset.status = g.open ? "Open" : "Done";
      title.append(g.named ? pii(g.name) : el("span", "mono", g.name));
      const stats = el("div", "diag-stats");
      stats.append(el("span", null, `${num(g.count)} report${g.count === 1 ? "" : "s"}`),
        el("span", null, `${num(g.reporters)} reporter${g.reporters === 1 ? "" : "s"}`),
        el("span", null, g.reasons.map(([r, n]) => `${reasonLabel(r)} ${num(n)}`).join(" · ")));
      const lastSpan = el("span", null, rel(g.last) + (rel(g.last) === "just now" ? "" : " ago"));
      stats.append(lastSpan);
      hint(stats, { cap: 1, head: g.name, rows: [{ label: "Reports", value: num(g.count) }, { label: "Distinct reporters", value: num(g.reporters) },
        ...g.reasons.map(([r, n]) => ({ label: reasonLabel(r), value: num(n) })),
        { label: "First report", value: date(g.first) }, { label: "Last report", value: date(g.last) },
        { label: "Signed up", value: g.user ? date(g.user.created) : "unknown" }, { label: "Last seen", value: g.user ? date(g.user.last_seen) : "unknown" },
        { label: "Devices", value: g.user ? num(g.user.devices) : "unknown" }] });
      details.dataset.reported = g.id; details.open = expanded.has(g.id);
      const status = g.user ? `signed up ${date(g.user.created)} · last seen ${g.user.last_seen ? date(g.user.last_seen) : "never"} · ${g.user.devices} device${g.user.devices === 1 ? "" : "s"}` : "account not in the user list";
      details.append(el("summary", null, `${status} · ${g.rows.length} individual report${g.rows.length === 1 ? "" : "s"}`));
      for (const r of g.rows) {
        const row = el("div", "drow"), line = el("span");
        line.append(reasonTag(r.reason), document.createTextNode(" by "), el("span", "mono", installShort(r.reporter)));
        line.title = `Reporter install ${r.reporter}`;
        row.append(line, el("span", "muted", date(r.at)));
        details.append(row);
      }
      const button = el("button", "pill", g.open ? "Mark handled" : "Reopen");
      button.type = "button";
      button.addEventListener("click", async () => {
        button.disabled = true;
        try { for (const r of g.rows.filter((r) => lastDone.has(reportKey(r)) === g.open)) await reportDone(r, g.open); }
        catch (error) { item.append(el("span", "error", error.message)); }
        finally { await refresh(); button.disabled = false; }
      });
      item.append(title, stats, button, details);
      (g.open ? list : doneList).append(item);
    }
    if (doneList.childElementCount) { done.append(el("summary", null, `Handled (${doneList.childElementCount})`), doneList); list.append(done); }
    if (!groups.length) list.append(el("p", "empty", "No reports."));
    return list;
  };
  const openReportCount = h => reportGroups({ reports: h?.reports || [], isDone: (r) => lastDone.has(reportKey(r)) }).filter(g => g.open).length;
  const openProblemGroups = h => (h?.diagnostics || []).filter(d => !d.done && !diagInfo(d).expected);
  // Done, no expected one, and still reported in the last 24 hours (on the builds it was marked done in).
  const stillProblemGroups = h => (h?.diagnostics || []).filter(d => d.done && !diagInfo(d).expected
    && (d.hours_24h || []).some(n => n > 0));
  const causeCount = groups => new Set(groups.map(diagCause)).size;
  $("tables").addEventListener("scroll", ({ target }) => {
    if (target.matches(".scroll")) target.classList.toggle("scrolled-x", target.scrollLeft > 0);
  }, { capture: true, passive: true });
  const drawTables = () => {
    const box = $("tables"), list = last?.[which]?.tables || [];
    const openGroups = openProblemGroups(last?.[which]), openCauses = causeCount(openGroups);
    const stillCauses = causeCount(stillProblemGroups(last?.[which]).filter(d => !openGroups.some(o => diagCause(o) === diagCause(d))));
    const openSupport = openSupportCount(last?.[which]), openReports = openReportCount(last?.[which]);
    const occurrences = openGroups.reduce((n, d) => n + (d.count || 0), 0);
    const drawn = JSON.stringify([which, tableOpen, openSupport, openReports, openCauses, openGroups.length, occurrences, tableOpen === "v-errors" && last?.[which]?.diagnostics?.map(d => [d.id, d.last_seen, d.count, d.done, d.cause_installs]), tableOpen === "v-reports" && [(last?.[which]?.reports || []).map(r => [r.id, r.at, lastDone.has(reportKey(r))]), (last?.[which]?.users || []).length], list.map((t) => [t.name, t.count]), tableOpen === "reserved" ? reservedData?.[which]?.enabled : 0,
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
      : key === "v-reports" ? openReports
      : key === "v-errors" ? (stillCauses ? `${num(openCauses)} · ${num(stillCauses)} still` : openCauses)
      : key === "v-todos" ? open("todos", "done_at") : count(VIEW_TABLE[key]);
    for (const [key, label, ico, tip] of VIEWS) {
      const b = el("button", `main view${tableOpen === key ? " on" : ""}`), name = el("span", null, label);
      b.type = "button";
      if (key === "v-errors") {
        // Groups (a row per build and line), causes (what they are about) and reports (the sum of counts), open ones.
        hint(b, { cap: 1, head: "Problems", rows: [{ label: "Groups", value: num(openGroups.length) }, { label: "Causes", value: num(openCauses) }, { label: "Occurrences", value: num(occurrences) }, { label: "Done, still reported in 24 h", value: num(stillCauses) }] });
      } else b.title = tip;
      name.prepend(icon(ico));
      const n = viewCount(key);
      b.append(name, el("small", null, typeof n === "string" ? n : num(n)));
      b.addEventListener("click", () => { unpin(); tableOpen = key; drawTables(); });
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
      if (tableOpen === "v-errors") pane.append(diagnosticList(last?.[which]?.diagnostics || [], box));
      else if (tableOpen === "v-reports") pane.append(reportList(last?.[which], box));
      else if (view === "list") {
      const wrap = el("div", "scroll grid"), table = el("table", "list data");
      table.dataset.name = t.name;
      const hr = table.createTHead().insertRow();
      if (tableOpen === "v-todos") hr.append(el("th", "tick-col"));
      for (const c of t.columns) hr.append(el("th", null, c));
      if (t.name === "reserved") hr.append(el("th"));
      const body = table.createTBody();
      const todo = tableOpen === "v-todos", col = (n) => t.columns.indexOf(n);
      let rows = t.rows;
      if (todo) {
        hr.append(el("th"));
        const doneAt = col("done_at");
        const openRows = rows.filter((row) => row[doneAt] == null);
        const label = el("label", "todo-filter"), checkbox = el("input");
        checkbox.type = "checkbox";
        checkbox.checked = showDone;
        label.append(checkbox, `Show done (${num(rows.length - openRows.length)})`);
        checkbox.addEventListener("change", () => {
          showDone = checkbox.checked;
          try { localStorage.setItem("todos-show-done", showDone ? "1" : "0"); } catch {}
          tablesDrawn = "";
          part(drawTables);
        });
        pane.append(label);
        if (!showDone) rows = openRows;
      }
      for (const row of rows) {
        const tr = body.insertRow();
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
        if (t.name === "reserved") {
          const name = row[t.columns.indexOf("spelling")] ?? row[0];
          const x = el("button", "ghost-x", "Remove"); x.type = "button";
          x.addEventListener("click", async () => { x.disabled = true; try { await changeReserved({ action: "remove", name }); } catch { x.disabled = false; } });
          cell(tr, x);
        }
      }
      if (!rows.length) cell(body.insertRow(), "Empty.", "empty").colSpan = t.columns.length + (t.name === "reserved" ? 1 : todo ? 2 : 0);
      wrap.append(table);
      pane.append(wrap);
      }
    }
    const keep = box.querySelector(".tpane .grid"), scroll = keep ? [keep.scrollLeft, keep.scrollTop, box.querySelector(".tpane").dataset.name] : null;
    box.replaceChildren(side, pane);
    if (usersView) part(() => drawAllUsers(last?.[which]));
    if (t) pane.dataset.name = t.name;
    if (scroll && t && scroll[2] === t.name) { const g = pane.querySelector(".grid"); if (g) { g.scrollLeft = scroll[0]; g.scrollTop = scroll[1]; } }
  };

  // ---- live Yos in the header: collector minute buckets, updated immediately by the hub stream -------------------
  const LIVE_SPREAD = 10000, LIVE_CAP = 30, LIVE_BARS = 10, LIVE_FLY = 540;
  const LIVE_DEAD = 35000, LIVE_RETRY = 2000, LIVE_RETRY_MAX = 30000, LIVE_DIM = 6000;
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  let liveSeen = null, liveTimers = [], liveData = null;
  let liveSource = null, liveOpen = false, liveRetry = null, liveBack = LIVE_RETRY, liveHeard = 0, liveLost = 0;
  const liveStop = () => {
    liveTimers.forEach(clearTimeout);
    liveTimers = [];
    $("live-lane").replaceChildren();
  };
  const liveLater = (ms, f) => {
    const t = setTimeout(() => { liveTimers = liveTimers.filter((x) => x !== t); f(); }, ms);
    liveTimers.push(t);
  };
  // One dot from the logo's right edge into the last bar (rects read once, then transform and opacity only).
  const liveFly = (bot) => {
    const lane = $("live-lane"), bar = $("live-spark").lastElementChild, logo = $("hub-word");
    if (!bar || document.visibilityState !== "visible") return;
    const o = lane.getBoundingClientRect(), l = logo.getBoundingClientRect(), b = bar.getBoundingClientRect();
    const sx = l.right - o.left - 7, sy = l.top + l.height / 2 - o.top;
    const ex = b.left + b.width / 2 - o.left + (Math.random() - 0.5) * Math.min(3, b.width * 0.5), ey = b.top - o.top;
    // The arc: up from the logo over the rate, a little above the bars, to just over the bar; then the fall.
    const hy = Math.min(sy, ey) - 9 - Math.random() * 5, hx = ex - 2 + Math.random() * 4, cx = (sx + hx) / 2, cy = hy - 10;
    const frames = [], ARC = 0.62;
    for (let i = 0; i <= 8; i++) {
      const t = i / 8, u = 1 - t;
      frames.push({ offset: ARC * t, transform: `translate(${(u * u * sx + 2 * u * t * cx + t * t * hx).toFixed(1)}px, ${(u * u * sy + 2 * u * t * cy + t * t * hy).toFixed(1)}px) scale(${(0.5 + 0.5 * Math.min(1, t * 4)).toFixed(2)})`,
        opacity: 1, easing: "linear" });
    }
    frames[frames.length - 1].easing = "cubic-bezier(.5, 0, .9, .6)";
    frames.push({ offset: 0.96, transform: `translate(${ex.toFixed(1)}px, ${(ey - 1).toFixed(1)}px) scale(.8, 1.15)`, opacity: 1, easing: "ease-out" });
    frames.push({ offset: 1, transform: `translate(${ex.toFixed(1)}px, ${(ey + 1).toFixed(1)}px) scale(.4)`, opacity: 0 });
    const s = el("i", bot ? "yo-spark bot" : "yo-spark");
    lane.append(s);
    s.animate(frames, { duration: LIVE_FLY * (0.92 + Math.random() * 0.16), easing: "cubic-bezier(.3, .1, .6, 1)", fill: "forwards" })
      .finished.then(() => s.remove(), () => s.remove());
    if (!logo.getAnimations().length) logo.animate([{ transform: "scale(1)" }, { transform: "scale(1.045)" }, { transform: "scale(1)" }], { duration: 320, easing: "ease-out" });
  };
  // The running minute's bar takes a Yo: a tiny bump from its foot.
  const liveBump = () => {
    const bar = $("live-spark").lastElementChild;
    if (bar && !calm.matches) bar.animate([{ transform: "scaleY(1)" }, { transform: "scaleY(1.22)" }, { transform: "scaleY(1)" }], { duration: 260, easing: "ease-out" });
  };
  const liveSpark = (bot) => {
    if (calm.matches || document.visibilityState !== "visible") return;
    liveFly(bot);
    liveBump();
  };
  const paintLive = () => {
    const box = $("live"), d = liveData;
    box.hidden = !d;
    if (!d) return;
    const m = d.minutes, rate = m.at(-1);
    set("live-n", num(rate));
    const hour = m.reduce((a, b) => a + b, 0);
    if (!box._h) {
      box._h = 1;
      hint(box, () => {
        const minutes = liveData.minutes, full = minutes.slice(-6, -1);
        const avg = full.reduce((a, b) => a + b, 0) / Math.max(1, full.length);
        return { head: "Yos per minute", nodes: [plotWithAxis(sparkBars(minutes, ""), "Last hour", "now")],
          rows: [{ label: "This minute", value: num(minutes.at(-1)) }, { label: "Per minute (5 min average)", value: avg.toFixed(1) },
            { label: "Last hour", value: num(minutes.reduce((a, b) => a + b, 0)) },
            { label: "Of them bots' (blue)", value: num((liveData.bots || []).reduce((a, b) => a + b, 0)) }], note: box.dataset.state || "" };
      });
    }
    box.setAttribute("aria-label", `${num(rate)} Yos this minute, ${num(hour)} in the last hour`);
    // The last ten minutes as bars like Problems', the running minute (bright) at the right.
    $("live-spark").replaceChildren(...bars(m.slice(-LIVE_BARS), LIVE_BARS * 10, 24));
  };
  const drawLive = (h) => {
    const d = h?.live;
    if (liveData?.hub !== which) { liveStop(); liveData = null; }
    if (!d?.minutes?.length) { paintLive(); return; }
    // A refresh may lag the stream. Merge overlapping buckets by minute to avoid losing or double counting Yos.
    const minute = Math.max(d.minute, liveData?.minute ?? d.minute);
    const minutes = d.minutes.map((_, i) => {
      const k = minute - d.minutes.length + 1 + i;
      const snapshot = d.minutes[d.minutes.length - 1 + k - d.minute] ?? 0;
      const streamed = liveData?.minutes[liveData.minutes.length - 1 + k - liveData.minute] ?? 0;
      return Math.max(snapshot, streamed);
    });
    const bots = d.minutes.map((_, i) => {
      const k = minute - d.minutes.length + 1 + i;
      const snapshot = d.bots?.[d.minutes.length - 1 + k - d.minute] ?? 0;
      const streamed = liveData?.bots?.[liveData.minutes.length - 1 + k - liveData.minute] ?? 0;
      return Math.max(snapshot, streamed);
    });
    liveData = { hub: which, minute, minutes, bots };
    paintLive();
    const m = d.minutes, n = m.length;
    // New Yos by the minutes: what this refresh counts above the last one (the running minute only on the first look).
    // The stream, while open, shows them instead.
    const key = `${which}`, byMinute = new Map(m.map((v, i) => [d.minute - (n - 1) + i, v]));
    let fresh = 0;
    if (liveSeen?.key === key) for (const [k, v] of byMinute) fresh += Math.max(0, v - (liveSeen.m.get(k) ?? (k > liveSeen.minute ? 0 : v)));
    else fresh = m[n - 1];
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
      liveStop();
      liveBack = LIVE_RETRY;
      liveHeard = Date.now();
      liveState();
    };
    source.addEventListener("ping", () => { if (mine()) liveHeard = Date.now(); });
    source.onmessage = (e) => {
      if (!mine()) return;
      let bot = false;
      try { bot = JSON.parse(e.data).k === "bot"; } catch { /* an old hub's event: a person's Yo */ }
      liveHeard = Date.now();
      liveOpen = true;
      const minute = Math.floor(Date.now() / 60000);
      if (liveData?.hub !== hub) liveData = { hub, minute, minutes: Array(60).fill(0), bots: Array(60).fill(0) };
      liveData.bots ||= Array(liveData.minutes.length).fill(0);
      const elapsed = Math.min(liveData.minutes.length, minute - liveData.minute);
      if (elapsed > 0) {
        liveData.minutes.splice(0, elapsed);
        liveData.minutes.push(...Array(elapsed).fill(0));
        liveData.bots.splice(0, elapsed);
        liveData.bots.push(...Array(elapsed).fill(0));
        liveData.minute = minute;
      }
      liveData.minutes[liveData.minutes.length - 1]++;
      if (bot) liveData.bots[liveData.bots.length - 1]++;
      paintLive();
      liveSpark(bot);
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
  let botReportRows = [];
  const drawStatus = (d) => {
    const l = d.last, c = d.config || {}, at = l && l.at ? l.at * 1000 : null;
    botReportRows = [
      ["Last report", l ? `${clock(at)} · ${l.sent ? "sent" : "not sent"}` : "none yet", l && !l.sent ? "bad" : ""],
      ["Schedule", !c.every_minutes ? "on demand" : c.every_minutes === 1440 ? "day (09:00 Berlin)" : `${c.every_minutes} min`],
    ];
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
    } catch { if (svcInfo.stbot) { botReportRows.push(["", "not saved, try again", "bad"]); redrawDetail(["svc", "server"]); } }
  });

  // ---- one page of widgets; the Database widget is always open. Rows of Attention and Usage jump there: Feedback to
  // the support chat, Errors to diagnostics, Reports to reports, Sign-ups (and the Users card's title) to the users
  // table. Services is one row in Server, opened in place (on its own while something is down); the Status bot row
  // there shows its frequency select. Keys: 1 top, 2 chat, 3 diagnostics, 4 users, 5 database, 0 the hub.
  const showTable = (name) => { tableOpen = { users: "people", support_messages: "v-support", diagnostics: "v-errors", reports: "v-reports", todos: "v-todos" }[name] || name; tablesDrawn = ""; part(drawTables); $("t-db").scrollIntoView({ behavior: "smooth", block: "start" }); };
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
  // Releases over time, stacked by build with the newest at the top.
  // @build-label
  // A release's build by its version and build ("1.0.0-RC36 (202611241344)", "Android 1.0.0-RC36 (…)"): the version
  // the app sent (Yo-App, spec 6.3) or users.py's RELEASES; any other dated build by its UTC minute ("Nov 24 07:54").
  const buildDate = (app) => {
    const b = build(app), p = (app || "").split("/")[0], word = { ios: "", android: "Android ", yogpt: "Bot " }[p] ?? `${appInfo(app)?.label || p} `;
    if (!/^\d{12}$/.test(b)) return app === "unknown" ? "unknown" : `${word || "iOS "}${b}`.trim();
    const mon = new Date(Date.UTC(2000, +b.slice(4, 6) - 1, 1)).toLocaleDateString("en", { month: "short", timeZone: "UTC" });
    return `${word}${mon} ${+b.slice(6, 8)} ${b.slice(8, 10)}:${b.slice(10, 12)}`;
  };
  const versionOf = (app) => last?.app_versions?.[app] || null;
  const buildLabel = (app) => {
    const b = build(app), v = /^\d{12}$/.test(b) && versionOf(app);
    return v ? `${(app || "").startsWith("android/") ? "Android " : ""}${v} (${b})` : buildDate(app);
  };
  // @end
  // @release-color
  // By support: of a platform's builds the hub still takes (YO_MIN_APP), the newest green through the oldest yellow
  // (rank `i` of `n`, 0 the newest); below the minimum (refused with 426, must update) red.
  const buildAgeH = (b) => (Date.now() - Date.UTC(+b.slice(0, 4), +b.slice(4, 6) - 1, +b.slice(6, 8), +b.slice(8, 10), +b.slice(10, 12))) / 36e5;
  const releaseColor = (i, n, below) => below ? "hsl(4 75% 50%)"
    : `hsl(${Math.round(135 - 85 * (n > 1 ? i / (n - 1) : 0))} 70% 42%)`;
  // @end
  const releaseBands = (h) => {
    const apps = new Set((h.users || []).map(u => u.app));
    for (const [app] of [...(h.apps || []), ...(h.apps_today || [])]) apps.add(app);
    for (const history of [h.build_days, h.build_hours, h.build_levels]) {
      for (const row of Object.values(history || {})) {
        for (const [app, count] of Object.entries(row || {})) if (count > 0) apps.add(app);
      }
    }
    const bands = new Map();
    for (const platform of ["ios", "android"]) {
      const dated = [...apps].filter(app => app?.startsWith(platform + "/") && /^\d{12}$/.test(app.slice(platform.length + 1))).sort().reverse();
      // Colour by the build's age (builds are dated ids); behind = rank, newest first, for sorting.
      const allowed = dated.filter(app => !belowMin(app));
      dated.forEach((app, behind) => {
        const ageH = Math.max(0, buildAgeH(app.slice(platform.length + 1)));
        bands.set(app, { behind, ageH, color: releaseColor(allowed.indexOf(app), allowed.length, belowMin(app)) });
      });
    }
    return bands;
  };
  // @release-groups
  // The App card's bands, newest first, the same in the chart and its hover: one per release (its version; in All iOS
  // and Android together); the builds of main as one "Dev builds" at the newest one's place; builds without a date
  // "Unknown"; then "Below minimum" (YO_MIN_APP: refused, must update), always last. Each band its own shade: the
  // builds the hub takes go green (newest) to yellow (oldest), below the minimum red.
  const releaseGroups = (apps, versionOf, below) => {
    const stamp = (app) => (app || "").split("/").pop(), dated = (app) => /^\d{12}$/.test(stamp(app));
    const groups = [], byVersion = new Map(), add = (name, kind) => { const g = { name, kind, apps: [] }; groups.push(g); return g; };
    let dev = null, unknown = null;
    const low = { name: "Below minimum", kind: "below", apps: [] };
    const sorted = apps.filter(dated).sort((a, b) => stamp(b).localeCompare(stamp(a)) || a.localeCompare(b));
    for (const app of sorted) {
      const v = versionOf(app);
      if (below(app)) low.apps.push(app);
      else if (v) { if (!byVersion.has(v)) byVersion.set(v, add(v, "release")); byVersion.get(v).apps.push(app); }
      else (dev ||= add("Dev builds", "dev")).apps.push(app);
    }
    for (const app of apps.filter(a => !dated(a)).sort()) (unknown ||= add("Unknown", "unknown")).apps.push(app);
    groups.push(low);
    const taken = groups.filter(g => g.kind !== "below" && g.kind !== "unknown");
    for (const g of groups) g.color = g.kind === "below" ? releaseColor(0, 1, true) : g.kind === "unknown" ? "var(--muted)"
      : releaseColor(taken.indexOf(g), taken.length);
    return groups;
  };
  // A hover's rows ({kind, …}, newest first): past `most` the oldest the hub still takes fold into one "Older" (their
  // sum, the oldest's colour); "Below minimum" stays last.
  const foldRows = (rows, most = 8) => {
    const below = rows.filter(r => r.kind === "below"), rest = rows.filter(r => r.kind !== "below");
    if (rest.length <= most) return [...rest, ...below];
    const old = rest.slice(most - 1);
    return [...rest.slice(0, most - 1), { kind: "older", name: "Older", color: old[old.length - 1].color,
      n: old.reduce((a, r) => a + r.n, 0), ios: old.reduce((a, r) => a + (r.ios || 0), 0),
      android: old.reduce((a, r) => a + (r.android || 0), 0) }, ...below];
  };
  // @end
  const relData = (h, plat) => {
    // Points by the hour: the hub's hourly `build:` levels first (devices seen within 48 hours, 24 before 2026-11-24; 30 days by the hour,
    // then the day's last), else the admin's own hours (48) and days (the day's at its next midnight, today's now).
    const t = today0(), h1 = Math.floor(Date.now() / 3600000), at = new Map();
    const days = { ...(h.build_days || {}) };
    if (Array.isArray(h.apps_today) && h.apps_today.length) days[t] = Object.fromEntries(h.apps_today.filter(([a]) => !String(a).startsWith("cli/")));
    for (const [d, row] of Object.entries(days)) if (+d <= t) at.set(Math.min((+d + 1) * 24, h1), row);
    for (const [x, row] of Object.entries(h.build_hours || {})) at.set(+x, row);
    for (const [x, row] of Object.entries(h.build_levels || {})) at.set(+x, row);
    // Each slot ends at its hour and takes the latest point within it: 1D and 7D by the hour, 30D every 4 hours,
    // Total by the day (at most 60 slots).
    const time = (x) => new Date(x * 3600000).toLocaleTimeString(undefined, { timeStyle: "short" });
    let stepH, n, ends, name;
    if (range === 0) {
      const first = Math.min(h1, ...at.keys()), span = Math.max(7, t - Math.floor((first - 1) / 24) + 1), step = Math.max(1, Math.ceil(span / 60));
      stepH = 24 * step; n = Math.ceil(span / step);
      ends = (i) => Math.min(h1, (t + 1) * 24 - (n - 1 - i) * stepH);
      name = (x) => { const d = Math.floor((x - 1) / 24); return x === h1 ? "today" : dayName(d); };
    } else {
      stepH = range === 30 ? 4 : 1; n = (range === 1 ? 24 : range * 24) / stepH;
      ends = (i) => h1 - (n - 1 - i) * stepH;
      name = range === 1
        ? (x) => `${new Date(x * 3600000).toDateString() === new Date().toDateString() ? "" : "yesterday "}${time(x)}`
        : (x) => `${dayName(Math.floor(x / 24))} ${time(x)}`;
    }
    const slots = Array.from({ length: n }, (_, i) => ends(i)).map((e) => {
      let row = null;
      for (let x = e; x > e - stepH && !row; x--) row = at.get(x) || null;
      return [e, e === h1 ? "now" : name(e), row];
    });
    if (range === 0) slots[n - 1][1] = "today";
    const mine = (a) => plat === "all" ? a === "unknown" || /^(ios|android)\//.test(a) : a.startsWith(`${plat}/`);
    const apps = new Set();
    for (const [, , row] of slots) {
      for (const [app, count] of Object.entries(row || {})) if (count > 0 && mine(app)) apps.add(app);
    }
    const releases = releaseBands(h);
    const bands = releaseGroups([...apps], versionOf, belowMin);
    const rows = slots.map(([, , row]) => {
      const counts = bands.map(b => b.apps.reduce((sum, app) => sum + (row?.[app] || 0), 0));
      return counts.some(v => v > 0) ? counts : null;
    });
    // Per slot the devices on a build the hub still takes and under 24 hours old: the card's "current".
    const fresh = slots.map(([, , row]) => Object.entries(row || {}).reduce((sum, [app, count]) =>
      sum + (mine(app) && !belowMin(app) && releases.get(app)?.ageH < 24 ? count : 0), 0));
    return { slots, bands, rows, fresh };
  };
  const relChart = (h, plat, titleText) => {
    const d = relData(h, plat), n = d.slots.length, W = 264, B = 96;
    const s = svg("svg", { viewBox: `0 0 ${W} ${B + 1}`, preserveAspectRatio: "none", class: "hc-chart rchart" });
    const have = d.rows.map((r, i) => (r ? i : -1)).filter((i) => i >= 0), f0 = have[0] ?? 0, span = Math.max(1, n - 1 - f0);
    const xs = (i) => (n - 1 > f0 ? ((i - f0) * W) / span : W / 2);
    s.append(svg("line", { x1: 0, x2: W, y1: B + 0.5, y2: B + 0.5, class: "base" }));
    if (!have.length) return dcard(titleText, null, note("No builds kept yet: counted per hour and day from now on."));
    // The bands' edges at each kept slot (a lone slot drawn as a thin column), top band first.
    const pts = have.length > 1 ? have : [have[0], have[0]];
    const x = (k, i) => (have.length > 1 ? xs(i) : xs(i) + (k ? 3 : -3));
    const polys = [];
    const tops = pts.map((i) => { const r = d.rows[i], all = r.reduce((a, v) => a + v, 0); let y = 0; return r.map((v) => (y += v / all)); });
    d.bands.forEach((b, j) => {
      if (!tops.some((c) => c[j] - (j ? c[j - 1] : 0) > 0)) return;
      const up = pts.map((i, k) => `${x(k, i).toFixed(1)},${(2 + tops[k][j] * (B - 2)).toFixed(1)}`);
      const down = pts.map((i, k) => `${x(k, i).toFixed(1)},${(2 + (j ? tops[k][j - 1] : 0) * (B - 2)).toFixed(1)}`).reverse();
      const p = svg("polygon", { points: [...up, ...down].join(" "), class: "rb" });
      p.setAttribute("fill", b.color);
      polys[j] = p;
      s.append(p);
    });
    const mark = svg("line", { y1: 0, y2: B, class: "rmark" });
    s.append(mark);
    const axis = el("div", "chart-axis");
    axis.append(el("span", null, (f0 ? "since " : "") + d.slots[f0][1]), el("span", null, d.slots[n - 1][1]));
    const pick = d.rows[have[have.length - 1]], pickAll = pick.reduce((sum, count) => sum + count, 0);
    const current = d.fresh[have[have.length - 1]];
    const share = `${Math.round(100 * current / pickAll)} % current`;
    if (f0) s.dataset.since = "true";
    // One hover only, the grouped list (no copy of the chart in it), of the nearest slot with builds kept; the band
    // under the pointer lit in the chart and in the list.
    s.dataset.samples = "0";
    let pointerY = null, lit = -1;
    s.addEventListener("pointermove", (e) => { const r = s.getBoundingClientRect(); pointerY = r.height ? (e.clientY - r.top) / r.height : null; });
    const light = (j) => { lit = j; polys.forEach((p, k) => p?.classList.toggle("dim", j >= 0 && k !== j)); };
    s.addEventListener("pointerleave", () => light(-1));
    hoverTip(s, n - f0, (k) => {
      const i = have.reduce((a, b) => Math.abs(b - f0 - k) < Math.abs(a - f0 - k) ? b : a, have[0]);
      const r = d.rows[i], all = r.reduce((x, v) => x + v, 0), c = tops[Math.max(0, have.indexOf(i))];
      const y = pointerY == null ? -1 : ((pointerY * (B + 1)) - 2) / (B - 2);
      light(y < 0 ? -1 : c.findIndex((top, j) => r[j] > 0 && y <= top + 1e-9));
      const row = d.slots[i][2] || {}, pct = (v) => v ? `${Math.round(100 * v / all) || "<1"} %` : "0 %";
      const on = (b, p) => b.apps.reduce((sum, app) => sum + (app.startsWith(`${p}/`) ? row[app] || 0 : 0), 0);
      const rows = foldRows(d.bands.flatMap((b, j) => r[j] > 0 || b.kind === "below"
        ? [{ kind: b.kind, name: b.name, color: b.color, n: r[j], ios: on(b, "ios"), android: on(b, "android"), lit: j === lit }] : []));
      // In All, each row's share per platform beneath it.
      const split = (x) => plat !== "all" ? null : [["iOS", x.ios], ["Android", x.android]].filter(([, v]) => v > 0)
        .map(([p, v]) => `${p} ${pct(v)}`).join(" · ") || null;
      return { head: d.slots[i][1], rows: rows.map((x) => ({ color: x.color, label: x.name, sub: split(x), value: pct(x.n), cls: x.lit ? "on" : "" })) };
    }, (i) => { if (!(i >= 0)) light(-1); });
    const card = dcard(titleText, share, s, axis);
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
    return [card];
  };
  const deviceRow = () => {
    const devices = platData?.devices, plats = platData?.plats || [];
    return { ico: "ios", label: "Devices", value: devices == null ? "–" : num(devices),
      node: platCard().firstChild, title: { head: "Devices", rows: plats.map(([label, p]) => ({
        label, value: `${num(p.all)} · ${pctText(100 * p.all / Math.max(1, devices))}` })) } };
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
  const dnums = (big, sub, cls = "", bigCls = "") => { const v = el("span", "dnums"); v.append(el("b", bigCls ? `dval ${bigCls}` : "dval", big)); if (sub) v.append(el("small", cls || null, sub)); return v; };
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
    // The growth is the big green number (the range's, a day: the last 24 hours), the total small and muted under it.
    const grown = d.values.reduce((a, v) => a + (v || 0), 0), word = range === 7 || range === 30 ? `in ${range} d` : d.day24 != null ? "in 24 h" : "today";
    const big = sub !== undefined ? null : `+${fmt(range === 7 || range === 30 ? grown : d.day24 ?? todayOf(d.values))} ${word}`;
    return dcard(title, big ? dnums(big, `${lf(end)} total`, "muted", "grow") : dnums(lf(end), sub, subCls),
      liveChart({ line, bars: d.values.map((v) => v || 0), ...axisOf(d.names) }, cls, foot, say), foot);
  };
  // The queue: Yos waiting for offline phones (the line) and the size of the photos and videos waiting (the bars), as
  // the hub keeps them (users.py from its levels: per hour for the week, then per day, now live).
  const queueSlots = (h) => {
    if (range === 1) {
      const h1 = Math.floor(Date.now() / 3600000), q = h.queue_hours || {}, hrs = Array.from({ length: 24 }, (_, i) => h1 - 23 + i);
      return { names: hrs.map((x) => clock(x * 3600000)), rows: hrs.map((x) => q[x] || null) };
    }
    if (range === 7 && h.week && h.queue_week) {
      const rows = weekStarts().map(start => {
        for (let hour = start + 5; hour >= start; hour--) if (h.queue_week[hour]) return h.queue_week[hour];
        return null;
      });
      return { names: weekNames(), rows };
    }
    const t = today0(), n = range || 30, q = h.queue_days || {}, ds = Array.from({ length: n }, (_, i) => t - n + 1 + i);
    return { names: ds.map(dayName), rows: ds.map((d) => q[d] || null) };
  };
  // A sparkline like the overview's rows: a bar per slot (the last one full), no axis.
  // top: a second list stacked on the first (the first from the bottom in cls, this one above it in red).
  const sparkBars = (values, cls, tip, top2, recorded, fixedMax) => {
    const n = Math.max(1, values.length), sl = 140 / n, tot = values.map((v, i) => (v || 0) + (top2?.[i] || 0)), top = fixedMax ?? Math.max(1e-9, ...tot);
    const s = svg("svg", { class: "spark", viewBox: "0 0 140 28", preserveAspectRatio: "none", "aria-hidden": "true" });
    const rect = (i, y, h, c) => svg("rect", { x: (i * sl).toFixed(2), width: sl.toFixed(2), y: y.toFixed(1), height: h.toFixed(1), rx: 1, class: c });
    values.forEach((v, i) => {
      if (v == null && !top2) { s.append(svg("g", {})); return; }
      if (!top2) { const h = fixedMax ? (v / top) * 26 : v ? Math.max(2, (v / top) * 26) : 1.5; s.append(rect(i, 28 - h, h, v ? `b ${cls || ""}`.trim() : "b zero")); return; }
      const g = svg("g", {}), h = tot[i] ? Math.max(2, (tot[i] / top) * 26) : 1.5, hb = tot[i] ? (h * (v || 0)) / tot[i] : 0;
      if (recorded && !recorded[i]) { s.append(g); return; }
      if (!tot[i]) g.append(rect(i, 28 - h, h, "b zero"));
      if (hb) g.append(rect(i, 28 - hb, hb, `b ${cls || ""}`.trim()));
      if (h - hb > 0 && tot[i]) g.append(rect(i, 28 - h, h - hb, "b hot"));
      s.append(g);
    });
    s.dataset.samples = values.filter((v, i) => recorded ? recorded[i] : v != null).length;
    s.dataset.first = Math.max(0, values.findIndex((v, i) => recorded ? recorded[i] : v != null));
    if (tip) hoverTip(s, values.length, tip, (k) => [...s.children].forEach((r, j) => r.classList.toggle("on", j === k)));
    return s;
  };
  // A sparkline of stacked bars: per slot the series (each 0 to 100) one on the other from the bottom.
  const stackBars = (series, colors, tip) => {
    const n = Math.max(1, series[0]?.length || 1), W = 140, H = 28, sl = W / n, s = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" });
    for (let i = 0; i < n; i++) {
      const g = svg("g", {}); let y = H;
      series.forEach((v, k) => { const hh = ((v[i] || 0) / 100) * H; if (hh <= 0) return; y -= hh; g.append(svg("rect", { x: (i * sl).toFixed(1), width: sl.toFixed(1), y: y.toFixed(1), height: hh.toFixed(1), class: "segment", fill: colors[k] })); });
      if (y === H) g.append(svg("rect", { x: (i * sl).toFixed(1), width: sl.toFixed(1), y: H - 1.5, height: 1.5, class: "b zero" }));
      s.append(g);
    }
    if (tip) hoverTip(s, n, tip, (k) => [...s.children].forEach((r, j) => r.classList.toggle("on", j === k)));
    return s;
  };
  // A card of rows like the overview's: an icon, the label, its value and a sparkline (each slot's popup on hover);
  // a rule before a row marked sep. Rows: { ico, label, value, cls, values, scls, tip, sep }.
  const sparkRows = (rows, heading = false) => {
    const box = el("div", "srows");
    for (const r of rows) {
      const first = heading && !box.childElementCount;
      const row = el(first ? "h2" : "div", `srow${first ? " gh" : ""}${r.keepValueColumn || r.values || r.bar != null || r.stack || r.node ? "" : " wide"}${r.go ? " go" : ""}`);
      let cachedCard;
      if (r.title || r.values?.length) hint(row, () => {
        if (cachedCard) return cachedCard;
        const card = typeof r.title === "string" ? { head: r.label, note: r.title, rows: [{ label: r.label, value: r.value }] }
          : r.title || { head: r.label, rows: [{ label: r.label, value: r.value }] };
        const nodes = [...(card.nodes || [])];
        if (!nodes.length && r.values?.length && r.tip) {
          const levels = r.values.map((v, i) => v == null || (r.recorded && !r.recorded[i]) ? null : v + (r.top?.[i] || 0));
          nodes.push(historyPlot(levels, levels.map((_, i) => r.tip(i)?.head || ""), r.scls));
        }
        cachedCard = { ...card, nodes };
        if (!r.go) return cachedCard;
        const action = el("button", "hc-hint", r.goTitle);
        action.type = "button";
        action.onclick = () => { unpin(); r.go(); };
        cachedCard.action = action;
        return cachedCard;
      });
      row.append(icon(r.ico || "chart"), el("span", "sl", r.label), el("b", `sv ${r.cls || ""}`.trim(), r.value ?? "–"));
      if (r.values) row.append(sparkBars(r.values, r.scls, r.tip, r.top, r.recorded));
      else if (r.node) row.append(r.node);
      else if (r.stack) row.append(stackBars(r.stack, r.stackColors, r.tip));
      else if (r.bar != null) { const b = shareBar(r.bar, "current"); b.classList.add("srow-bar"); row.append(b); }
      box.append(row);
    }
    return box;
  };
  const sparkCard = (title, rows) => dcard(title, null, sparkRows(rows));
  const pushRow = (h) => {
    const p = pushData, counts = healthData.ps, sum = (v) => v.reduce((x, y) => x + y, 0), at = (i) => ({ head: p.names[i], rows: [{ ico: "send", label: "Pushes sent", value: num(p.sent[i]) },
      { label: "Taken", value: p.sent[i] ? pctText(100 * counts[i][0] / p.sent[i]) : "–" }, { label: "Failed", value: num(counts[i][1]) }, { label: "Folded, 1 s window", value: num(p.folded[i]) }, { label: "Folded, backlog", value: num(p.paced[i]) },
      { label: "In backlog pacing now", value: h.push_backlog_devices == null ? "–" : num(h.push_backlog_devices) }] });
    const sent = sum(p.sent), [ok, failed] = healthData.psAll;
    const taken = evSlots("push-ok"), failures = evSlots("push-fail");
    const bars = taken.values.map((v, i) => (v || 0) + (failures.values[i] || 0));
    let running = 0;
    const line = [0, ...bars.map(v => (running += v))];
    const chart = liveChart({ bars, line, ...axisOf(taken.names) }, "cool");
    return { ico: "send", label: "Push", title: { head: "Push", nodes: bars.length >= 3 ? [chart] : [],
      rows: [{ label: "Sent", value: num(sent) }, { label: "Taken", value: sent ? pctText(100 * ok / sent) : "–" },
        { label: "Failed", value: num(failed) }] }, value: num(sent), values: p.sent, scls: "cool", tip: at };
  };
  const perfRows = (h) => {
    const d = speedData, tot = (b) => b.reduce((a, v) => a + v, 0), { names, rows } = queueSlots(h);
    return ([
      { ico: "speed", label: "Speed", value: d && tot(d.all) ? pctText(latPct(d.all, 0)) : "–", values: d ? d.lat.map((b) => (tot(b) ? latPct(b, 0) : 0)) : [],
        title: d && tot(d.all) ? (() => {
          // The whole range: each slot's split into the four buckets as stacked bars, the share within 1 s as the line over them.
          const first = d.lat.findIndex(b => tot(b) > 0), samples = d.lat.slice(first);
          const W = 240, H = 70, n = samples.length, g = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" }), colors = ["var(--ok)", "#a3e4b8", "var(--warn)", "var(--muted)"];
          samples.forEach((b, i) => { let y = H; if (!tot(b)) return; LAT.forEach((_, k) => { const hh = (latPct(b, k) / 100) * H; y -= hh; if (hh > 0) g.append(svg("rect", { x: ((i + 0.12) * W) / n, width: (0.76 * W) / n, y, height: hh, fill: colors[k], opacity: ".85" })); }); });
          g.append(svg("path", { fill: "none", stroke: "var(--text)", "stroke-width": 1.5, "vector-effect": "non-scaling-stroke", "stroke-linejoin": "round",
            d: samples.map((b, i) => (tot(b) ? `${i && tot(samples[i - 1]) ? "L" : "M"}${(((i + 0.5) * W) / n).toFixed(1)},${(H - (latPct(b, 0) / 100) * H).toFixed(1)}` : null)).filter(Boolean).join(" ") }));
          return { cap: 1, head: "Delivery speed", nodes: samples.filter(b => tot(b)).length < 3 ? [] : [plotWithAxis(g, (first ? "since " : "") + d.names[first], d.names.at(-1))],
            rows: [{ label: "Within 1 s", value: pctText(latPct(d.all, 0)) }, { label: "Within 5 s", value: pctText(latPct(d.all, 0) + latPct(d.all, 1)) }, { section: "Delivery times" }, ...LAT_NAME.map((nm, k) => ({ color: colors[k], label: nm, value: pctText(latPct(d.all, k)) })), { ico: "send", label: "Yos", value: num(tot(d.all)) }] };
        })() : null,
        tip: (i) => ({ head: d.names[i], rows: tot(d.lat[i]) ? [...LAT_NAME.map((n, k) => ({ color: ["var(--ok)", "#a3e4b8", "var(--warn)", "var(--muted)"][k], label: n, value: pctText(latPct(d.lat[i], k)) })),
          { ico: "send", label: "Yos", value: num(tot(d.lat[i])) }] : [{ ico: "send", label: "Yos", value: "none" }] }) },
      { ico: "clock", label: "Queue", value: num(h.messages), title: { head: "Yos waiting", rows: [{ label: "Queue", value: num(h.messages) }] }, values: rows.map((r) => r?.[0] ?? null), scls: "cool",
        tip: (i) => ({ head: names[i], rows: [{ color: "#5dade2", label: "Yos waiting", value: rows[i] ? num(rows[i][0]) : "not kept" }] }) },
      { ico: "image", label: "Media queue", value: h.blobs ? bytes(h.blobs[1]) : "–", values: rows.map((r) => r?.[1] ?? null), scls: "cool",
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
  const spanWord = () => (range === 1 ? "last 24 hours" : range ? `last ${range} days` : "since the start");
  // A count kept per day (and per hour for the last 24) over the range: an hour each (1), a day each (7, 30), up to 12
  // stretches (Total).
  const perSlot = (perDay, hours) => {
    if (range === 1) return { names: hourNames(), values: last24(hours).map((v) => v || 0) };
    const slots = rangeSlots(perDay);
    return { names: slots.map(([n]) => n), values: slots.map((s) => slotSum(s, perDay, hours)) };
  };
  // The last 24 hours of an event kind, rolling (not since UTC midnight): the "in 24 h" sub-lines.
  const last24sum = (k) => last24(hubNow?.events?.[k]?.hours).reduce((a, v) => a + (v || 0), 0);
  const evSlots = (k) => ({ ...(range === 7 && hubNow?.week ? { values: weekEvent(k), names: weekNames() } : perSlot(year(hubNow?.events?.[k]?.year), hubNow?.events?.[k]?.hours)), day24: last24sum(k) });
  const rangeCount = (key, slots = evSlots(key)) => range ? sumOf(slots) : hubNow?.events?.[key]?.total ?? sumOf(slots);
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
  const problemTip = (p, i) => p.recorded[i] ? { head: p.names[i], rows: [
    { color: "#e67e22", label: "Problems without crashes", value: num(p.er[i]) },
    { color: "var(--bad)", label: "Crashes", value: num(p.cr[i]) },
  ] } : null;
  const problemSplit = (h) => {
    const hourly = range === 1, weekly = range === 7 && h.week, size = hourly ? 3600000 : weekly ? 21600000 : 86400000, count = hourly ? 24 : weekly ? 28 : range || Math.max(1, today0() - Math.floor((h.open_report_history?.[0]?.t ?? Date.now()) / size) + 1);
    const first = weekly ? h.week.start * 3600000 : (Math.floor(Date.now() / size) - count + 1) * size;
    const slots = Array(count).fill(null);
    for (const point of h.open_report_history || []) {
      const i = Math.floor((point.t - first) / size);
      if (i >= 0 && i < count) slots[i] = point;
    }
    return { names: weekly ? weekNames() : slots.map((_, i) => hourly ? clock(first + i * size) : dayName(first / size + i)),
      recorded: slots.map(Boolean), er: slots.map((p) => p?.errors_open), cr: slots.map((p) => p?.crashes_open) };
  };
  // Problems that came in over the range, by kind (errors per day only: today for the last 24 hours).
  const problemKinds = (h) => {
    const from = range === 1 ? Date.now() - 86400000 : range ? (today0() - range + 1) * 86400000 : 0, d0 = range === 1 ? today0() : Math.floor(from / 86400000);
    const fb = range === 1 ? last24(h.support_hours).reduce((a, v) => a + (v || 0), 0) : [...year(h.support_days)].filter(([d]) => d >= d0).reduce((a, [, v]) => a + (v || 0), 0);
    const diag = (kind) => (h.diagnostic_days || []).filter((r) => r.kind === kind && r.day >= d0).reduce((a, r) => a + r.count, 0);
    return [["Support", fb], ["Crashes", crashSlots(h).total], ["Problems without crashes", diag("error")], ["Reports", (h.reports || []).filter((r) => r.at >= from).length]];
  };
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
    const sent = rangeCount("bot-yo"), traffic = sent + rangeCount("yo-in") + rangeCount("yo-in-bot");
    const kinds = [["bot-yo", "Bot Yos"], ...media.map((k) => [k, `${k.slice(10)} photos`])], shades = ["#5dade2", "#3d8fc4", "#2f6f9c", "#7fc0ea", "#1f4f75"];
    const series = kinds.map(([k, name], n) => ({ name, color: shades[n % shades.length], slots: evSlots(k), today: today(k) }));
    const len = series[0].slots.values.length, names = series[0].slots.names, totalAt = (i) => series.reduce((s, x) => s + (x.slots.values[i] || 0), 0);
    const top = Math.max(1, ...Array.from({ length: len }, (_, i) => totalAt(i))), sum24 = series.reduce((n, s) => n + s.today, 0);
    const pct = series.map((s) => Array.from({ length: len }, (_, i) => ((s.slots.values[i] || 0) / top) * 100)), colors = series.map((s) => s.color);
    const W = 240, H = 70, g = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" });
    for (let i = 0; i < len; i++) { let y = H; series.forEach((s, k) => { const hh = (pct[k][i] / 100) * H; if (hh <= 0) return; y -= hh; g.append(svg("rect", { x: ((i + 0.15) * W) / len, width: (0.7 * W) / len, y, height: hh, fill: s.color })); }); }
    return [{ ico: "cli", label: "Bots", value: traffic ? pctText(100 * sent / traffic) : "–", stack: pct, stackColors: colors,
      tip: (i) => ({ head: names[i], rows: series.map((s) => ({ color: s.color, label: s.name, value: num(s.slots.values[i] || 0) })) }),
      title: { cap: 1, head: "Bots", note: "Not in the counts of Sent, Media or Accounts.", nodes: names.length < 3 ? [] : [plotWithAxis(g, names[0], names.at(-1))], rows: [{ label: "Yos sent by bots", value: num(sent) }, { label: "Yos acknowledged by bots", value: num(rangeCount("yo-out-bot")) }, { label: "All Yos in range", value: num(traffic) }, ...series.map((s) => ({ color: s.color, label: s.name, value: `${num(s.today)} in 24 h` })), { label: "All bots", value: `${num(sum24)} in 24 h` }] } }];
  };

  let drawUsageRows;
  {
    // Yo: beside the tile's Yos: delivered, delivery performance (speed, the queues), media sent, groups.
    DETAIL.yc = () => {
      const h = hubNow, d = speedData;
      return [
      ];
    };
    // Accounts: activity (who is active, how many stay, how much they send) and the accounts' properties (secured,
    // deleted), as the overview's rows: a label, its value, a sparkline where there is a history, a popup per slot.
    // The Accounts stats card: account metrics as the overview's rows (secured bar, new accounts secured,
    // Yos per user), each with its history in the hover.
    const statsRows = (h) => {
      const mau = h.active_month || 0, dau = h.active_today ?? 0, t = today0();
      const days = range || 30, ds = Array.from({ length: days }, (_, i) => t - days + 1 + i), kept = h.active_days || {}, yo = year(h.events?.["yo-in"]?.year);
      const activeOn = d => d === t ? Math.max(kept[d] || 0, h.active_day || 0) : kept[d] || 0;
      const yoN = h.events ? rangeCount("yo-in") : null;
      const usersInRange = ({ 1: h.active_today, 7: h.active_users, 30: h.active_month })[range];
      // Rolling activity levels cannot give distinct users for hourly, six-hour or multi-day slots.
      const daily = range === 30 || (range === 7 && !h.week);
      const per = daily ? ds.map(d => activeOn(d) && yo.get(d) != null ? yo.get(d) / activeOn(d) : null) : [];
      const mean = (yos, users) => yos != null && users ? (yos / users).toFixed(1) : "–";
      const meanTip = (head, yos, users) => ({ head, note: "Accepted Yos per active account.",
        rows: [{ label: "Mean", value: mean(yos, users) }, { label: "Median", value: "Unavailable" },
          { color: "var(--ok)", label: "Accepted Yos", value: num(yos) }, { ico: "user", label: "Active accounts", value: num(users) }] });
      const ret = Object.fromEntries((areaSub.yc || []).map(([l, v]) => [String(l).replace(/^Retention /, ""), v]));
      const sc = sparkData.sc, gone = evSlots("account-gone");
      const wk = (h.secured_days || []).slice(-7).filter(Boolean), made = wk.reduce((n, d) => n + d[0], 0), withKey = wk.reduce((n, d) => n + d[1], 0);
      return [
          { ico: "chart", label: "Active", value: pctText(mau ? (100 * dau) / mau : null), bar: mau ? dau / mau : 0,
            title: { cap: 1, head: "Active users", nodes: [historyPlot(days14().map(d => d === t || kept[d] != null ? activeOn(d) : null), days14().map(dayName), "")], note: "Bars: users seen per day, the last 14 days", rows: [{ label: "DAU (last 24 h)", value: num(dau) }, { label: "MAU (last 30 days)", value: num(mau) }, { label: "Stickiness DAU / MAU", value: pctText(mau ? (100 * dau) / mau : null) },
              { label: "Deleted in range", value: num(rangeCount("account-gone", gone)) }, { section: "Retention" }, ...["D1", "D7", "D30"].map((k) => ({ label: k, value: ret[k] ?? "–" }))] } },
          (() => {
            // Secured: accounts with a backup as a share of all accounts (the bar); its line over the last 30 days in the hover.
            const us = h.users || [], dd = Array.from({ length: 30 }, (_, i) => t - 29 + i), end = (d) => (d + 1) * 86400000;
            const share = dd.map((d) => { const made = us.filter((u) => u.created < end(d)); return made.length ? (100 * made.filter((u) => u.backup_at != null && u.backup_at < end(d)).length) / made.length : null; });
            const now = h.total ? (100 * (h.secured || 0)) / h.total : null;
            const g = historyPlot(share, dd.map(dayName), "", true);
            return { ico: "lock", label: "Secured", value: pctText(now), bar: (now || 0) / 100,
              title: { cap: 1, head: "Secured, all accounts", nodes: [g, ...(sc?.values?.length ? [historyPlot(sc.values.map((v, i) => h.secured_days?.[i]?.[0] ? v : null), sc.names, "")] : [])], rows: [{ label: `${dayName(dd[0])} – today`, value: pctText(now) }, { section: "New accounts, the bars above" },
                { label: "Secured, last 7 days", value: made ? pctText((100 * withKey) / made) : "–" }] } };
          })(),
          { ico: "user", label: "Yos / user", keepValueColumn: true, value: mean(yoN, usersInRange), values: per.some(v => v != null) ? per : null,
            title: { ...meanTip(spanWord(), yoN, usersInRange), note: !range ? "The hub has no all-time count of distinct active accounts." :
              daily ? "Accepted Yos divided by distinct active accounts in the range. Each bar uses that day’s counts." :
              "Accepted Yos divided by distinct active accounts in the range. Account activity is unavailable for individual slots." },
            tip: (i) => meanTip(dayName(ds[i]), yo.get(ds[i]), activeOn(ds[i]) || null) },
      ];
    };
    drawUsageRows = (h) => {
      const by = evSlots("media-in-bytes"), yo = evSlots("yo-in");
      const row = (ico, label, k, total, size) => {
        const s = evSlots(k), count = rangeCount(k, s), yos = rangeCount("yo-in", yo);
        return { ico, label, value: size ? (yos ? pctText(100 * count / yos) : "–") : `+${num(count)}`, values: s.values.map((v) => v || 0), scls: "cool",
          tip: (i) => ({ head: s.names[i], rows: [{ color: "#5dade2", label: label, value: num(s.values[i] || 0) }, ...(size ? [{ ico: "disk", label: "Size", value: bytes(by.values[i] || 0) },
            { ico: "send", label: "Share of Yos", value: yo.values[i] ? pctText((100 * (s.values[i] || 0)) / yo.values[i]) : "–" }] : []),
            { section: "In all" }, { color: "#5dade2", line: true, label: "Total", value: num(total - s.values.slice(i + 1).reduce((n, v) => n + (v || 0), 0)) }] }),
          title: { cap: 1, head: label, nodes: [historyPlot(s.values, s.names, "cool")], rows: [{ label: "In range", value: num(count) }, ...(size ? [{ label: "Yos in range", value: num(yos) }] : []), { label: "Total", value: num(total) }, { label: "In 24 h", value: num(s.day24) },
            ...(size ? [{ label: "Size, all media", value: h.events?.["media-in-bytes"]?.total == null ? "–" : bytes(h.events["media-in-bytes"].total) }] : [])] } };
      };
      const media = row("image", "Media", "media-in", h.events?.["media-in"]?.total ?? 0, true);
      const groups = row("user", "Groups", "group-create", h.groups ?? 0, false);
      $("uc-rows").replaceChildren(sparkRows([...statsRows(h), deviceRow(), groups, ...(pushData ? [pushRow(h)] : [])], true));
      $("delivery-rows").replaceChildren(sparkRows([...perfRows(h), media, ...botCards(h)], true));
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
      const h = hubNow, kinds = Object.fromEntries(problemKinds(h));
      // Feedback: what users sent, as the overview's rows: feedback and reports per slot, crashes and crash-free (est.).
      const h1 = Math.floor(Date.now() / 3600000), t0 = today0(), span = range === 1 ? 24 : range || 30;
      const slots = Array.from({ length: span }, (_, i) => (range === 1 ? h1 - span + 1 + i : t0 - span + 1 + i));
      const slotName = (x) => (range === 1 ? clock(x * 3600000) : dayName(x)), unitMs = range === 1 ? 3600000 : 86400000;
      const fbDays = year(h.support_days), fbHours = last24(h.support_hours);
      const weekly = range === 7 && h.week?.feedback;
      const names = weekly ? weekNames() : slots.map(slotName);
      const fb = weekly || slots.map((x, i) => (range === 1 ? fbHours[i] || 0 : fbDays.get(x) || 0));
      const rp = weekly ? weekStarts().map(h0 => (h.reports || []).filter(r => r.at >= h0 * 3600000 && r.at < (h0 + 6) * 3600000).length) : slots.map((x) => (h.reports || []).filter((r) => Math.floor(r.at / unitMs) === x).length);
      const supportCount = openSupportCount(h);
      const openReports = openReportCount(h);
      const period = range === 1 ? "in 24 h" : range ? `in ${range} d` : "in total";
      const summary = (head, open, incoming) => ({ head, rows: [{ label: "Open", value: num(open) }, { label: `Came in ${period}`, value: num(incoming) }] });
      const userRows = [
        (() => { const p = problemSplit(h), open = openProblemGroups(h);
          const openCount = causeCount(open), reports = open.reduce((n, d) => n + (d.count || 0), 0);
          const levels = p.er.map((v, i) => p.recorded[i] ? v + p.cr[i] : null);
          const names = p.names;
          const recorded = levels.filter(v => v != null);
          const rise = recorded.length ? recorded.at(-1) - recorded[0] : 0;
          const fast = recorded.length > 1 && rise >= Math.max(3, Math.ceil(recorded[0] * 0.5));
          state("problem-dial", fast || open.some(d => d.kind === "crash") || p.cr.at(-1) > 0 ? "bad" : rise > 0 ? "warn" : null);
          set("problem-n", openCount >= 1000 ? short(openCount) : num(openCount));
          const trend = historyPlot(levels, names, "warm");
          const peak = Math.max(1, ...recorded);
          let path = "";
          levels.forEach((v, i) => {
            if (v == null) return;
            const x = 1 + 26 * i / Math.max(1, levels.length - 1);
            path += `${levels[i - 1] == null ? "M" : "L"}${x} ${7 - 6 * v / peak}h0`;
          });
          $("problem-trend").replaceChildren(svg("path", { d: path }));
          const stillCount = causeCount(stillProblemGroups(h).filter(d => !open.some(o => diagCause(o) === diagCause(d))));
          problemCard = { head: "Problems", nodes: [trend], rows: [{ label: "Open causes", value: num(openCount) }, { label: "Done, still reported in 24 h", value: num(stillCount) }],
            note: "Occurrences of open problems, including crashes." };
          return { ico: "warn", label: "Problems", title: { cap: 1, head: "Problems", rows: [{ label: "Open causes", value: num(openCount) }, { label: "Done, still reported in 24 h", value: num(stillCount) }, { label: "Occurrences", value: num(reports) }, { label: "Phones affected in range", value: problemTotals?.installs != null && problemTotals.seen ? pctText((100 * problemTotals.installs) / problemTotals.seen) : "–" }, { label: "1  Occurrence", value: "from a phone" }, { label: "2  Fingerprint", value: "kind · code · line" }, { label: "3  Group", value: "same fingerprint, count goes up" }, { label: "4  Problem", value: "the cause: its groups together" }], note: "Ticked stays closed for the builds it was ticked in; a newer build reporting it opens it again." }, value: num(openCount), cls: openCount ? "warn" : "", values: p.er, top: p.cr, recorded: p.recorded, scls: "warm",
            tip: (i) => problemTip(p, i) }; })(),
        { ico: "chat", label: "Support", value: num(supportCount), cls: supportCount ? "warn" : "", title: summary("Support", supportCount, weekly ? fb.reduce((a, b) => a + b, 0) : kinds.Support), values: fb, go: () => showTable("support_messages"), goTitle: "Open the support chat", tip: (i) => ({ head: names[i], rows: [{ color: "var(--ok)", label: "Support", value: num(fb[i]) }] }) },
        { ico: "flag", label: "Reports", value: num(openReports), cls: openReports ? "warn" : "", title: summary("Reports", openReports, weekly ? rp.reduce((a, b) => a + b, 0) : kinds.Reports), values: rp, scls: "warm",
          tip: (i) => ({ head: names[i], rows: [{ color: "#e67e22", label: "Reports", value: num(rp[i]) }] }) },
        ...(() => {
          const pt = problemTotals, pc = (n, d) => (d ? pctText((100 * n) / d) : "–");
          const explanation = "Share of the phones seen in the 24 hours before each point that sent no error or crash in those 24 hours (the network's own errors left out). The value is the latest point.";
          // Every point counts only the phones seen in the 24 hours before it, and of them those without a report in the same
          // 24 hours (the hub's hourly levels, per platform): as many bars as the other graphs, an empty one where there is no level.
          const efl = h.error_free_levels || {}, hourly = range === 1;
          const share = (l) => (l && l[0] ? 100 * Math.max(0, l[0] - l[1]) / l[0] : null);
          const upTo = (hi, lo, at) => { for (let hh = Math.min(hi, h1); hh > lo; hh--) if (efl[hh]?.[at] != null) return efl[hh].slice(at, at + 2); return null; };
          const mk = (seen, any, crash) => { const b = el("span", "hc-pbar error-free-split"); for (const [k, v] of [["current", Math.max(0, seen - any)], ["warm", Math.max(0, any - crash)], ["must", crash]]) if (v) { const x = el("span", k); x.style.flexGrow = v; b.append(x); } return b; };
          const three = (seen, any, crash) => [{ color: "var(--ok)", label: "Error-free", value: pc(Math.max(0, seen - any), seen) }, { color: "#e67e22", label: "With a problem, without a crash", value: pc(Math.max(0, any - crash), seen) }, { color: "var(--bad)", label: "With a crash", value: pc(crash, seen) }];
          // at: where the platform's pair starts in a level (2 iOS, 4 Android; 0 all phones).
          const row = (ico, label, at, plat) => {
            const values = weekly ? weekStarts().map((hs) => share(upTo(hs + 6, hs, at)))
              : slots.map((x) => share(hourly ? efl[x]?.[at] != null ? efl[x].slice(at, at + 2) : null : upTo(x * 24 + 24, x * 24, at)));
            const tip = (i) => ({ head: names[i], rows: [{ label: "Error-free, 24 h before", value: values[i] == null ? "–" : pctText(values[i]) }] });
            // A slot without data draws as an empty bar, so the graph keeps its full bar count.
            const node = sparkBars(values.map((v) => v ?? 0), "", tip, null, null, 100);
            const latest = values.findLast((v) => v != null), head = latest == null ? "–" : pctText(latest);
            const current = pt?.current?.[at / 2], [, hit, n, c] = plat || [];
            return { ico, label, node, value: head,
              title: { cap: 1, head: label, note: explanation, nodes: n ? [mk(n, hit, c)] : [],
                rows: [{ label: "Error-free, last 24 h", value: head },
                  { label: "On current builds, last 24 h", value: current?.[0] ? pc(Math.max(0, current[0] - current[1]), current[0]) : "–" },
                  ...(n ? [{ section: "In the range" }, ...three(n, hit, c)] : [])] } };
          };
          const [ios, android] = pt?.platforms?.length ? pt.platforms.map(([, hit, n, c]) => [null, hit, n, c]) : [null, null];
          return [row("apple", "Error-free", 2, ios), row("android", "Error-free", 4, android)];
        })(),
      ];
      return [sparkRows(userRows, true)];
    };

  }
  const TITLE = { yc: "Yos", uc: "Accounts", md: "Media", gr: "Groups", sc: "Secured", pl: "Platforms", vr: "App", pr: "Problems", dl: "Speed", ps: "Push", server: "Server", upd: "Updates", svc: "Services" };
  const SLUG = { yc: "yos", uc: "accounts", md: "media", gr: "groups", sc: "secured", pl: "platforms", vr: "app", pr: "problems", dl: "speed", ps: "push",
    cpu: "cpu", mem: "memory", disk: "disk", net: "lan", upd: "updates", "svc:hub": "hub", "svc:dev": "dev", "svc:tunnel": "tunnel", "svc:cli": "cli" };
  const detailHosts = [$("grp-vr"), $("delivery-rows"), $("grp-pr"), $("uc-rows")];
  let detailPending = false;
  // The status bot's select lives in the Services popover while it shows, else hidden at home.
  const parkSelect = () => { const sel = $("st-every"); if (sel && !$("t-system").contains(sel) && !tipBox.contains(sel)) { sel.hidden = true; $("t-system").append(sel); } };
  const drawDetail = () => {
    if (!hubNow) return;
    parkSelect();
    $("grp-vr").replaceChildren(...relCard(hubNow));
    drawUsageRows(hubNow);
    $("grp-pr").replaceChildren(DETAIL.pr()[0]);
  };
  // Redrawn with fresh data, unless the pointer is on a card (its captions) or a select is open; then once it leaves.
  function redrawDetail() {
    if (matchMedia("(any-hover: hover)").matches && detailHosts.some((host) => host.matches(":hover"))) { detailPending = true; return; }
    drawDetail();
  }
  for (const host of detailHosts) host.addEventListener("pointerleave", () => {
    if (detailPending) { detailPending = false; drawDetail(); }
  });
  const openDetail = (key) => {
    const target = { yc: "t-yc", uc: "t-uc", vr: "grp-vr", pr: "grp-pr", ps: "uc-rows", dl: "delivery-rows" }[TOPIC(key)];
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
