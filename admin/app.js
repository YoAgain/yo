// Login: a word on why the last try did not work. Dashboard: the server every three seconds, the hubs every ten.
const $ = (id) => document.getElementById(id);

// <support-list> The Support tab's list, pure (admin/test_support.py runs it in node). One "To-dos" row holds every
// to-do; then the people who wrote to the team ("chats", the default), the ones with something open ("open"), the
// starred ("star") or every user ("all"). Starred on top, then newest first.
const supportList = ({ users, threads, todos, todoAbout, starred, query = "", filter = "chats" }) => {
  const up = (id) => String(id || "").toUpperCase();
  const ids = new Set(users.map((u) => up(u.id)));
  const threadOf = new Map(threads.filter((t) => t.user_id !== "team").map((t) => [up(t.user_id), t]));
  const person = (id, user, name) => {
    const thread = threadOf.get(id), mine = todos.filter((t) => todoAbout(t.id) === id);
    const own = mine.filter((t) => t.done_at == null).length;
    return { kind: "user", id, key: `u:${id}`, user, name, thread, todos: own, open: (thread?.unread || 0) + own,
      at: Math.max(thread?.at || 0, ...mine.map((t) => t.created_at || 0)), seen: user?.last_seen || 0 };
  };
  let rows = users.map((u) => person(up(u.id), u, u.name || "(no name)"));
  for (const [id, t] of threadOf) if (!ids.has(id)) rows.push(person(id, null, t.name || "Deleted account"));
  if (filter === "chats") rows = rows.filter((r) => r.thread);
  const open = todos.filter((t) => t.done_at == null);
  rows.push({ kind: "todo", id: "team", key: "t:all", name: open.length ? `To-dos · ${open.length}` : "To-dos",
    todos: open.length, open: open.length, last: open.sort((a, b) => b.created_at - a.created_at)[0] || null,
    at: Math.max(0, ...todos.map((t) => t.created_at || 0)), seen: 0 });
  for (const r of rows) r.star = starred(r.key);
  if (query) rows = rows.filter((r) => r.name.toLowerCase().includes(query) || r.id.toLowerCase().includes(query));
  if (filter === "open") rows = rows.filter((r) => r.open);
  if (filter === "star") rows = rows.filter((r) => r.star);
  return rows.sort((a, b) => b.star - a.star || b.at - a.at || b.seen - a.seen || (b.user?.created || 0) - (a.user?.created || 0));
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
  const set = (id, text) => { $(id).textContent = text; };
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
  let which = "live", last = null, lastDone = new Set(), currentTab = "overview", reservedData = null;
  try { if (localStorage.getItem("hub") === "dev") which = "dev"; } catch {}
  const switchHub = () => {
    which = which === "live" ? "dev" : "live";
    try { localStorage.setItem("hub", which); } catch {}
    set("hub-word", which);
    document.body.classList.toggle("dev", which === "dev");
    if (last) render();
  };
  set("hub-word", which);
  document.body.classList.toggle("dev", which === "dev");
  $("hub-word").addEventListener("click", switchHub);

  // ---- server: every three seconds --------------------------------------------------------------------------------
  const bar = (id, frac) => {
    const p = Math.min(100, Math.max(0, frac * 100));
    $(`r-${id}`).style.setProperty("--p", p.toFixed(1));
    $(`r-${id}`).classList.toggle("hot", p > 90);
  };
  const service = (id, u) => {
    u = u || {};
    const ok = u.state === "active";
    state(`t-${id}`, ok ? "ok" : "bad");
    set(`${id}-cap`, ok ? (u.since ? span(Date.now() / 1000 - u.since) : "–") : (u.state || "not running"));
    const restarts = u.restarts ? ` · ${u.restarts} restarts` : "";
    $(`t-${id}`).title = ok ? `running${u.memory != null ? ` · ${bytes(u.memory)}` : ""}${restarts}` : "not running";
  };
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
    reservedData = s.reserved;
    part(() => {
      const c = s.cpu || {};
      set("cpu", c.percent == null ? "–" : `${Math.round(c.percent)} %`);
      if (c.percent != null) bar("cpu", c.percent / 100);
      $("r-cpu").title = [c.cores ? `${c.cores} cores` : "", c.load?.length ? `load ${c.load.map((x) => x.toFixed(2)).join(" · ")}` : ""].filter(Boolean).join(" · ");
    });
    part(() => {
      const m = s.memory;
      if (!m) { set("mem", "–"); return; }
      const used = m.total - m.available;
      set("mem", bytes(used)); bar("mem", used / m.total);
      $("r-mem").title = `of ${bytes(m.total)} · ${Math.round((100 * used) / m.total)} %${m.swap_total ? ` · swap ${bytes(m.swap_total - m.swap_free)}` : ""}`;
    });
    part(() => {
      const d = s.disk;
      if (!d) { set("disk", "–"); return; }
      set("disk", bytes(d.used)); bar("disk", d.used / d.total);
      $("r-disk").title = `of ${bytes(d.total)} · ${bytes(d.free)} free`;
    });
    part(() => {
      const n = s.network || {};
      set("rx", rate(n.rx_per_s)); set("tx", rate(n.tx_per_s));
      bar("rx", (n.rx_per_s || 0) / 1e6); bar("tx", (n.tx_per_s || 0) / 1e6);  // bars up to 1 MB/s
      $("r-rx").title = `${bytes(n.rx)} since boot`; $("r-tx").title = `${bytes(n.tx)} since boot`;
    });
    part(() => {
      $("t-services").title = s.uptime == null ? "" : `server running ${span(s.uptime)}`;
      const r = s.reboot;
      set("reboot-text", r && r.needed ? "reboot needed" : "");
      $("reboot-text").title = r && r.needed && r.packages?.length ? r.packages.join(", ") : "";
    });
    part(() => {
      const version = s.hub && s.hub.version;
      service("hub", s.hub);
      if (version) $("t-hub").title = `${version} · ${$("t-hub").title}`;
      set("foot-hub", version ? ` · hub ${version.split(" ")[0]}` : "");
    });
    part(() => service("dev", s.hub_dev));
    part(() => service("tunnel", s.tunnel));
    part(() => {
      service("yogpt", s.yogpt);
      const people = last?.live?.yogpt_contacts;
      if (s.yogpt?.state === "active" && people != null) $("t-yogpt").title += ` · knows ${people}`;
    });
    if (currentTab === "db" && last) part(drawTables);
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
    set("sys-updates", stale || u.status !== "ok" ? (u?.reboot ? "reboot needed" : summary) : "");
    $("sys-head").title = [u ? `${u.current_release}${u.release?.available ? ` · ${u.release.available} available` : ""}` : "", summary,
      u?.reboot ? `Reboot needed (kernel ${u.running_kernel || "?"} → ${u.installed_kernel || "?"})` : "", "click for details"].filter(Boolean).join("\n");
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
    $("sys-head").onclick = open;
    $("sys-head").onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } };
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
  let minApp = null;
  const findNewest = (h) => { minApp = parse(h?.min_app); };
  const appInfo = (app) => last?.app_builds?.[app] || (app ? { label: build(app), state: null, title: app } : null);
  const belowMin = (app) => { const v = parse(app); return !!(minApp && v && minApp[0] === v[0] && v[1] < minApp[1]); };
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
  // Lines on one scale; a hover title per day.
  const lines = (node, series, titles) => {
    const n = 14, top = Math.max(1, ...series.flatMap(([v]) => v)), w = 140, hgt = 40, step = w / (n - 1);
    const y = (v) => (hgt - 2 - (v / top) * (hgt - 6)).toFixed(1);
    node.replaceChildren(
      ...[0.5, 0.25].map((f) => svg("line", { x1: 0, x2: w, y1: (hgt * f).toFixed(1), y2: (hgt * f).toFixed(1), class: "grid" })),
      ...series.map(([values, cls]) => svg("path", { d: values.map((v, i) => `${i ? "L" : "M"}${(i * step).toFixed(1)},${y(v)}`).join(" "), class: `line ${cls}` })),
      ...titles.map((t, i) => { const r = svg("rect", { x: Math.max(0, i * step - step / 2), width: step, y: 0, height: hgt }); r.append(svg("title")); r.lastChild.textContent = t; return r; }));
  };

  // ---- overview: the users widget, sortable by every column ---------------------------------------------------------
  let ovSort = { key: "created", dir: -1 };
  const ovValue = (u, key) => key === "name" ? (u.name || "").toLowerCase() : key === "app" ? (parse(u.app)?.[1] ?? -1)
    : key === "secured" ? Number(!!u.secured) : u[key] ?? 0;
  for (const th of document.querySelectorAll("#ov-users th[data-sort]")) {
    th.tabIndex = 0;
    const go = () => {
      const key = th.dataset.sort;
      ovSort = { key, dir: ovSort.key === key ? -ovSort.dir : key === "name" ? 1 : -1 };
      part(() => drawOvUsers(last?.[which]));
    };
    th.addEventListener("click", go);
    th.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
  }
  const drawOvUsers = (h) => {
    for (const th of document.querySelectorAll("#ov-users th[data-sort]")) {
      th.classList.toggle("sorted", th.dataset.sort === ovSort.key);
      th.dataset.dir = th.dataset.sort === ovSort.key ? (ovSort.dir > 0 ? "▲" : "▼") : "";
    }
    const body = $("newusers-body");
    body.replaceChildren();
    const list = [...(h?.users || [])].sort((a, b) => {
      const x = ovValue(a, ovSort.key), y = ovValue(b, ovSort.key);
      return (x < y ? -1 : x > y ? 1 : 0) * ovSort.dir || b.created - a.created;
    }).slice(0, 10);
    for (const u of list) {
      const tr = body.insertRow();
      tr.className = "opens";
      const nm = el("span", null, u.name ?? "–");
      const seen = el("small", "muted", ` · ${seenDay(u.last_seen)}`);
      seen.title = u.last_seen ? `last seen ${new Date(u.last_seen).toLocaleDateString()}` : "never seen";
      nm.append(seen);
      cell(tr, nm, "name", u.id);
      cell(tr, rel(u.created), null, date(u.created));
      cell(tr, u.secured ? tag("yes", "ok") : tag("no", "muted"));
      cell(tr, appTag(u.app, u.id));
      tr.addEventListener("click", () => select(up(u.id), u.name || u.id));
    }
    if (!list.length) cell(body.insertRow(), h && !h.error ? "No users yet." : (h?.error || "No data"), "empty").colSpan = 4;
  };

  // ---- overview ---------------------------------------------------------------------------------------------------
  const reportKey = (r) => `${which}:r${r.id}`;
  const overview = (h) => {
    const ok = h && !h.error;
    // Open: feedback (open user messages at each day's end), crashes (open, by the day last seen), reports (open, by day).
    const days = days14(), fb = h?.support_unread_days || [];
    const f = days.map((_, i) => fb[fb.length - 14 + i] ?? 0);
    const openDiag = (h?.diagnostics || []).filter((d) => !d.done);
    const cr = days.map((d) => openDiag.filter((x) => Math.floor(x.last_seen / 86400000) === d).length);
    const openRep = (h?.reports || []).filter((r) => !lastDone.has(reportKey(r)));
    const rp = days.map((d) => openRep.filter((r) => Math.floor(r.at / 86400000) === d).length);
    lines($("issues-graph"), [[f, "fb"], [cr, "cr"], [rp, "rp"]], days.map((d, i) => `${dayName(d)}: ${f[i]} feedback open · ${cr[i]} crashes · ${rp[i]} reports`));
    if (!supportThreads.length) set("feedback-unread", fb.length ? fb[fb.length - 1] : "–");
    set("diagnostics-open", h?.diagnostics_open ?? openDiag.length);
    set("reports-open", openRep.length);

    // Services: the queue at the bottom.
    const blobs = ok && h.blobs;
    set("q-line", ok ? `${num(h.messages)} in queue · ${blobs ? num(blobs[0]) : "–"} images in queue` : "–");
    $("q-line").title = blobs ? `images waiting: ${bytes(blobs[1])}` : "";

    // Users: ten, by the chosen column (last seen first); a click opens the user in the Users tab.
    drawOvUsers(h);

    // Activity: sign-ups per day as bars, messages per day as a line (own scale), 14 days.
    if (ok) {
      const total = h.total || 0, today = h.active_today || 0, week = Math.max(today, h.active_users || 0);
      set("ug-today", num(today));
      set("ug-total", `of ${num(total)} today`);
      $("ug-today").parentElement.title = `${week} active this week · ${h.secured ?? "?"} secured · ${h.devices ?? "?"} devices`;
      set("st-msgs", num(h.deliveries?.day));
      const su = (h.signups || []).slice(-14), md = h.deliveries?.days || [];
      const ms = days.map((_, i) => md[md.length - 14 + i] ?? null);
      const W = 280, H = 90, slot = W / 14, sTop = Math.max(1, ...su), mTop = Math.max(1, ...ms.filter((v) => v != null));
      const chart = $("activity-chart"), kids = [];
      su.forEach((v, i) => kids.push(svg("rect", { x: (i * slot + slot * 0.18).toFixed(1), width: (slot * 0.64).toFixed(1),
        y: (H - Math.max(1.5, (v / sTop) * (H - 14))).toFixed(1), height: Math.max(1.5, (v / sTop) * (H - 14)).toFixed(1), rx: 2, class: v ? "su" : "su zero" })));
      const pts = ms.map((v, i) => (v == null ? null : [i * slot + slot / 2, H - 4 - (v / mTop) * (H - 14)]));
      let d = "", pen = false;
      for (const p of pts) { if (!p) { pen = false; continue; } d += `${pen ? "L" : "M"}${p[0].toFixed(1)},${p[1].toFixed(1)} `; pen = true; }
      kids.push(svg("path", { d: d.trim(), class: "ms" }));
      pts.forEach((p) => { if (p) kids.push(svg("circle", { cx: p[0].toFixed(1), cy: p[1].toFixed(1), r: 2.2, class: "ms" })); });
      days.forEach((day, i) => {
        const r = svg("rect", { x: (i * slot).toFixed(1), width: slot.toFixed(1), y: 0, height: H, class: "hit" });
        const t = svg("title"); t.textContent = `${dayName(day)}: ${su[i] ?? 0} sign-ups · ${ms[i] == null ? "messages not kept" : `${num(ms[i])} messages`}`;
        r.append(t); kids.push(r);
      });
      chart.replaceChildren(...kids);

      // App versions: devices current, with an update out, unknown; per platform in the caption.
      let fresh = 0, old = 0, other = 0;
      const per = {};
      for (const [app, c] of h.apps || []) {
        const a = appState(app), l = appInfo(app)?.label || "unknown";
        if (a === "current") fresh += c; else if (a === "update") old += c; else other += c;
        const p = per[l] ||= {};
        p[a || "unknown"] = (p[a || "unknown"] || 0) + c;
      }
      set("ug-fresh", `${fresh}/${fresh + old + other}`);
      set("ug-apps-cap", [...Object.entries(per).map(([l, p]) => `${l}: ${["current", "update", "unknown"].filter((w) => p[w]).map((w) => `${p[w]} ${w}`).join(", ")}`),
        `min ${h.min_app ? build(h.min_app) : "not pinned"}`].join(" · "));
      $("ug-apps-cap").title = "current: the highest iOS build users report, internal testers left out (bot: its deploy) · update: an older one · unknown: dev, Android, CLI";
      $("ug-apps-bar").replaceChildren(...[[fresh, "new", "current"], [old, "old", "update available"], [other, "a0", "unknown (dev, Android, CLI)"]]
        .filter(([v]) => v > 0).map(([v, cls, t]) => { const s = el("span", cls); s.style.flexGrow = v; s.title = `${v} ${t}`; return s; }));
    }
  };

  // ---- support: conversations and to-dos on the left, the chosen one on the right (who, to-dos, reports, chat) ------
  // One list: every user (with their conversation, if any), and each open team to-do not about a user. Starred rows
  // and rows with something open float to the top on their own; a quiet count says how much is open. Stars and which
  // user a to-do is about are kept by this page (marks.json); the hub keeps messages, to-dos and their done state.
  let supportUser = null, supportHub = null, supportName = "", supportSeen = null, supportBusy = false, supportThreads = [];
  let supportWant = null, listDrawn = "", supportMessages = [], teamTodos = null, composeMode = "reply";
  let marks = { stars: [], todo_user: {} };
  let userQuery = "", userFilter = "chats";
  const supportGet = async (user) => {
    const r = await fetch(`/support.json?${new URLSearchParams({ hub: which, ...(user ? { user } : {}) })}`, { cache: "no-store" });
    if (!r.ok) throw new Error("Could not load messages.");
    return r.json();
  };
  const isTeam = (u) => u === "team";
  const up = (id) => String(id || "").toUpperCase();
  const userById = (id) => (last?.[which]?.users || []).find((u) => up(u.id) === up(id));
  const threadOf = (id) => supportThreads.find((t) => up(t.user_id) === up(id));
  const reportsAbout = (id) => (last?.[which]?.reports || []).filter((r) => up(r.reported) === up(id));
  const openReports = (id) => reportsAbout(id).filter((r) => !lastDone.has(reportKey(r))).length;
  const starred = (key) => marks.stars.includes(`${which}:${key}`);
  const todoAbout = (id) => marks.todo_user[`${which}:${id}`] || null;
  // The team's to-dos: from the hub when loaded, else from the collector's snapshot.
  const todos = () => teamTodos || (last?.[which]?.todos || []).map((t) => ({ id: t.id, text: t.text, created_at: t.at, done_at: t.done_at }));
  const todosOf = (id) => todos().filter((t) => todoAbout(t.id) === up(id));
  const setMark = async (data) => {
    const r = await fetch("/marks", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ hub: which, ...data }) });
    if (!r.ok) throw new Error("Could not save. Try again.");
    marks = await r.json();
    listDrawn = "";
    part(drawList); part(drawPerson); part(drawStar);
    if (isTeam(supportUser)) part(() => drawThread(supportMessages));
  };
  const starButton = (key, label) => {
    const on = starred(key), b = el("button", `star${on ? " on" : ""}`, "★");
    b.type = "button";
    b.setAttribute("aria-pressed", String(on));
    b.setAttribute("aria-label", `Star ${label}`);
    b.title = on ? "starred (click to unstar)" : "star: keep on top";
    b.addEventListener("click", (e) => { e.stopPropagation(); setMark({ star: key, on: !on }).catch((x) => set("support-error", x.message)); });
    return b;
  };
  const drawList = () => {
    const box = $("person-list"), lastLines = last?.[which]?.support_last || {};
    const rows = supportList({ users: last?.[which]?.users || [], threads: supportThreads, todos: todos(), todoAbout,
      starred, query: userQuery, filter: userFilter });
    const openTodos = todos().filter((t) => t.done_at == null).length;
    set("new-todo-count", openTodos ? ` · ${openTodos}` : "");
    const drawn = JSON.stringify([which, supportUser, userQuery, userFilter, marks, rows.map((p) => [p.id, p.name, p.thread, p.open, p.at, p.last?.text, p.user?.last_seen, p.user?.app, lastLines[p.id], openReports(p.id)])]);
    if (drawn === listDrawn && box.childElementCount) return;
    listDrawn = drawn;
    box.replaceChildren();
    for (const p of rows) {
      const wrap = el("div", "trow");
      const b = el("button", `thread${p.open ? " unread" : ""}${p.kind === "todo" ? " todo-row" : ""}${(p.kind === "todo" ? isTeam(supportUser) : up(supportUser) === p.id) ? " on" : ""}`);
      b.type = "button";
      const body = el("span", "body"), top = el("span", "top"), bottom = el("span", "bottom");
      top.append(el("b", "name", p.name));
      let line, time, title = "";
      if (p.kind === "todo") {
        line = p.last ? p.last.text : "Nothing open"; time = p.at ? clock(p.at) : ""; title = p.at ? `newest to-do ${date(p.at)}` : "";
      } else {
        const l = lastLines[p.id], u = p.user, rep = openReports(p.id);
        if (rep) top.append(Object.assign(el("span", "flag", "⚑"), { title: `${rep} open report${rep === 1 ? "" : "s"} about this user` }));
        line = p.todos ? `${p.todos} to-do${p.todos === 1 ? "" : "s"}${l ? ` · ${l.team ? "You: " : ""}${l.text}` : ""}`
          : l ? `${l.team ? "You: " : ""}${l.text}` : u ? [u.app ? build(u.app) : "", `seen ${seenDay(u.last_seen)}`].filter(Boolean).join(" · ") : "";
        time = p.thread ? clock(p.thread.at) : u ? clock(u.last_seen) : "";
        title = p.thread ? `last message ${date(p.thread.at)}` : "";
      }
      top.append(el("time", null, time));
      bottom.append(el("span", "line", line));
      if (p.open && p.kind === "user") bottom.append(Object.assign(el("span", "count", String(p.open)), { title: `${p.thread?.unread || 0} open message(s) · ${p.todos} open to-do(s)` }));
      body.append(top, bottom);
      const av = avatar(el("span", "avatar"), p.name, p.kind === "todo");
      if (p.name === "Deleted account") av.classList.add("gone");
      b.append(av, body);
      if (title) b.title = title;
      b.addEventListener("click", () => select(p.kind === "todo" ? "team" : p.id, p.kind === "todo" ? "To-dos" : p.name));
      wrap.append(b, starButton(p.key, p.kind === "todo" ? "the to-dos" : p.name));
      if (p.star) wrap.classList.add("starred");
      box.append(wrap);
    }
    if (!box.childElementCount) box.append(el("p", "empty", userQuery ? "Nothing matches." : userFilter === "open" ? "Nothing open." : userFilter === "star" ? "Nothing starred yet. Star a conversation to keep it on top." : "No users yet."));
  };
  $("user-search").addEventListener("input", (e) => { userQuery = e.target.value.trim().toLowerCase(); part(drawList); });
  for (const b of document.querySelectorAll(".seg [data-filter]")) b.addEventListener("click", () => {
    userFilter = b.dataset.filter;
    for (const x of document.querySelectorAll(".seg [data-filter]")) x.classList.toggle("on", x === b);
    part(drawList);
  });
  $("new-todo").addEventListener("click", () => select("team", "To-dos"));

  const supportDone = async (ids, done) => {
    const hub = which, user = supportUser;
    try {
      await Promise.all(ids.map((id) => postJSON("/support/done", { hub, id, done })));
      if (which === hub && supportUser === user) await supportOpen(user);
      await supportRefresh();
    } catch (e) { set("support-error", e.message); }
  };
  // A to-do line: tick, star, text, who it is about (in the team list), when.
  const todoRow = (m, showWho) => {
    const row = el("div", `todo${m.done_at != null ? " done" : ""}${starred(`t:${m.id}`) ? " starred" : ""}`);
    row.append(tickButton(m.done_at != null, "To-do done", (d) => supportDone([m.id], d)), el("span", "text", m.text));
    const about = todoAbout(m.id);
    if (showWho && about) {
      const name = userById(about)?.name || threadOf(about)?.name || about.slice(0, 8).toLowerCase();
      const who = el("button", "link about", `about ${name}`);
      who.type = "button"; who.title = "open this user";
      who.addEventListener("click", () => select(about, name));
      row.append(who);
    }
    const t = el("time", null, clock(m.created_at)); t.title = date(m.created_at) + (m.done_at ? ` · done ${date(m.done_at)}` : "");
    row.append(t, starButton(`t:${m.id}`, "to-do"));
    return row;
  };
  const todoOrder = (a, b) => (a.done_at != null) - (b.done_at != null) || starred(`t:${b.id}`) - starred(`t:${a.id}`) || a.created_at - b.created_at;
  const drawThread = (messages) => {
    const box = $("support-history"), atEnd = box.scrollHeight - box.scrollTop - box.clientHeight < 60;
    const team = isTeam(supportUser);
    box.replaceChildren();
    box.classList.toggle("todos", team);
    if (!messages.length) {
      const e = el("div", "empty-state");
      e.append(el("b", null, team ? "No to-dos yet" : "No messages"), el("span", null, team ? "Add the first one below." : "This user has not written to the team. Replies are possible once they do; a to-do about them works now."));
      box.append(e);
    }
    if (team) {
      // The to-dos as a list: open ones first (starred, then oldest first), done ones after, dimmed.
      for (const m of [...messages].sort(todoOrder)) box.append(todoRow(m, true));
    } else {
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
    }
    if (atEnd || box.dataset.user !== supportUser) box.scrollTop = team ? 0 : box.scrollHeight;
    box.dataset.user = supportUser;
  };
  const growComposer = () => {
    const t = $("support-text");
    if (!t.offsetParent) { t.style.height = ""; return; }
    t.style.height = "auto"; t.style.height = `${Math.min(Math.max(t.scrollHeight, 22), 180)}px`;
  };
  const drawStar = () => {
    const s = $("convo-star"), team = isTeam(supportUser);
    s.hidden = !supportUser || team;
    if (s.hidden) return;
    const on = starred(`u:${up(supportUser)}`);
    s.classList.toggle("on", on); s.setAttribute("aria-pressed", String(on));
    s.title = on ? "starred (click to unstar)" : "star: keep on top";
  };
  $("convo-star").addEventListener("click", () => {
    if (!supportUser || isTeam(supportUser)) return;
    const key = `u:${up(supportUser)}`;
    setMark({ star: key, on: !starred(key) }).catch((x) => set("support-error", x.message));
  });
  // Who the chosen person is: facts in a row, their to-dos, devices and reports folded below.
  const drawPerson = () => {
    const team = isTeam(supportUser), u = team ? null : userById(supportUser);
    const facts = $("p-facts");
    facts.replaceChildren();
    $("p-id").hidden = team || !supportUser;
    if (!team && supportUser) { $("p-id").textContent = supportUser.toLowerCase(); $("p-id").dataset.id = supportUser.toLowerCase(); }
    const own = team || !supportUser ? [] : todosOf(supportUser).sort(todoOrder);
    const ut = $("user-todos");
    ut.hidden = !own.length;
    ut.replaceChildren(...own.map((m) => todoRow(m, false)));
    $("p-extra").hidden = team || !u;
    if (team || !u) return;
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
  $("p-id").addEventListener("click", () => {
    navigator.clipboard?.writeText($("p-id").dataset.id).then(() => { $("p-id").dataset.copied = "1"; setTimeout(() => delete $("p-id").dataset.copied, 1200); }).catch(() => {});
  });
  // The composer: in a user's conversation, a reply (once they wrote) or a to-do about them; in To-dos, a to-do.
  const setMode = (mode) => {
    composeMode = mode;
    for (const b of document.querySelectorAll("#compose-mode [data-mode]")) b.classList.toggle("on", b.dataset.mode === mode);
    convoHead(supportMessages);
  };
  for (const b of document.querySelectorAll("#compose-mode [data-mode]")) b.addEventListener("click", () => { if (!b.disabled) { setMode(b.dataset.mode); $("support-text").focus(); } });
  const convoHead = (messages) => {
    const team = isTeam(supportUser), open = messages.filter((m) => (team || !m.from_team) && m.done_at == null).length;
    $("convo-done").hidden = team || !open;
    const canReply = !team && messages.length > 0;
    if (!team && !canReply) composeMode = "todo";
    const todo = team || composeMode === "todo";
    $("compose-mode").hidden = team || !supportUser;
    $("compose-mode").querySelector('[data-mode="reply"]').disabled = !canReply;
    for (const b of document.querySelectorAll("#compose-mode [data-mode]")) b.classList.toggle("on", b.dataset.mode === (todo ? "todo" : "reply"));
    $("support-reply").hidden = !supportUser;
    $("support-reply").classList.toggle("team", todo);
    $("support-text").maxLength = todo ? 4000 : 2000;
    $("support-text").placeholder = team ? "Add a to-do (Enter)" : todo ? `A to-do about ${supportName} (Enter)` : `Reply to ${supportName} (Enter sends, Shift+Enter new line)`;
    growComposer();
  };
  const select = (id, name) => {
    if (location.hash !== "#support") history.replaceState(null, "", "#support");
    if (currentTab !== "support") tab();
    supportOpen(id, name, true);
  };
  const supportOpen = async (user, title, chosen) => {
    const hub = which;
    if (chosen || supportWant === null) supportWant = user;
    if (chosen) {
      if (supportUser !== user || supportHub !== hub) {
        $("support-text").value = ""; growComposer(); supportSeen = null; $("support-history").replaceChildren();
        $("support-reply").hidden = true; $("convo-done").hidden = true; composeMode = "reply";
      }
      supportUser = user; supportHub = hub; supportName = title || user;
      avatar($("convo-avatar"), supportName, isTeam(user)); set("support-title", supportName);
      part(drawPerson); part(drawList); part(drawStar);
    }
    try {
      const data = await supportGet(user);
      if (which !== hub || supportWant !== user) return;
      supportUser = user; supportHub = hub; supportName = title || supportName || user;
      supportMessages = data.messages;
      if (isTeam(user)) teamTodos = data.messages;
      convoHead(data.messages);
      const seen = data.messages.map((m) => `${m.id}:${m.read_at ? 1 : 0}:${m.done_at}`).join(",") + JSON.stringify(marks);
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
      supportUser = null; supportWant = null; supportSeen = null; teamTodos = null; $("support-reply").hidden = true;
      $("support-history").replaceChildren(); set("support-title", "Loading…");
    }
    try {
      const [threads, team] = await Promise.all([supportGet(), supportGet("team").catch(() => null)]);
      if (hub !== which) return;
      supportThreads = threads;
      if (team) teamTodos = team.messages;
      const open = threads.reduce((n, t) => n + (isTeam(t.user_id) ? 0 : t.unread), 0);
      set("feedback-unread", open);
      // The Support tab's quiet count: conversations whose last word is the user's and still open, plus open to-dos.
      const lastLines = last?.[which]?.support_last || {};
      const unanswered = threads.filter((t) => !isTeam(t.user_id) && t.unread > 0 && !lastLines[up(t.user_id)]?.team).length;
      const openTodos = todos().filter((t) => t.done_at == null).length;
      set("tab-support", unanswered + openTodos ? `${unanswered + openTodos}` : "");
      $("tab-support").title = `${unanswered} waiting for a reply · ${openTodos} open to-dos`;
      part(drawList);
      if (currentTab === "support") part(drawPerson);
      // Nothing chosen yet: the first conversation with something open (else the newest, else the To-dos).
      if (supportWant === null && currentTab === "support") {
        const user = threads.filter((t) => !isTeam(t.user_id)).sort((a, b) => (b.unread > 0) - (a.unread > 0) || b.at - a.at)[0];
        if (user) supportOpen(up(user.user_id), userById(user.user_id)?.name || user.name || "Deleted account", true);
        else if (threads.length || teamTodos?.length) supportOpen("team", "To-dos", true);
      }
      set("support-error", "");
    } catch (e) { set("support-error", e.message); }
    finally { supportBusy = false; }
  };
  const supportSend = async () => {
    const text = $("support-text").value;
    if (!supportUser || supportHub !== which || !text.trim()) return;
    const button = $("support-reply").querySelector("button"), user = supportUser, hub = supportHub;
    const todo = isTeam(user) || composeMode === "todo";
    button.disabled = true;
    try {
      if (todo) {
        const r = await fetch("/todos", { method: "POST", body: new URLSearchParams({ hub, text }) });
        if (!r.ok) throw new Error("Could not add the to-do.");
        const made = await r.json();
        if (!isTeam(user) && made?.id) await setMark({ todo: made.id, user: up(user) });
      } else {
        await postJSON("/support", { hub, user, action: "reply", text });
        // The reply answers the user's open messages in this thread: they are marked handled.
        const open = which !== hub ? [] : (await supportGet(user)).messages.filter((m) => !m.from_team && m.done_at == null);
        await Promise.all(open.map((m) => postJSON("/support/done", { hub, id: m.id, done: true })));
      }
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
  $("convo-done").addEventListener("click", () => supportDone(supportMessages.filter((m) => !m.from_team && m.done_at == null).map((m) => m.id), true));
  setInterval(() => {
    if (document.visibilityState !== "visible" || currentTab !== "support") return;
    supportRefresh();
    if (supportUser && supportHub === which) supportOpen(supportUser);
  }, 4000);

  // ---- users tab: today's figures, sign-ups and active users per day (30 days), every user sortable ---------------
  const days30 = () => Array.from({ length: 30 }, (_, i) => Math.floor(Date.now() / 86400000) - 29 + i);
  // Bars on one scale, a hover title per day; a null value is a day without data (a short grey stub).
  const bars = (node, values, titles) => {
    const W = 300, H = 90, slot = W / values.length, top = Math.max(1, ...values.filter((v) => v != null)), kids = [];
    values.forEach((v, i) => {
      const h = v ? Math.max(1.5, (v / top) * (H - 12)) : 1.5;
      kids.push(svg("rect", { x: (i * slot + slot * 0.16).toFixed(1), width: (slot * 0.68).toFixed(1), y: (H - h).toFixed(1), height: h.toFixed(1), rx: 1.5, class: v ? "su" : "su zero" }));
    });
    values.forEach((_, i) => {
      const r = svg("rect", { x: (i * slot).toFixed(1), width: slot.toFixed(1), y: 0, height: H, class: "hit" });
      const t = svg("title"); t.textContent = titles[i]; r.append(t); kids.push(r);
    });
    node.replaceChildren(...kids);
  };
  let allSort = { key: "created", dir: -1 }, allQuery = "";
  const allValue = (u, key) => key === "name" ? (u.name || "").toLowerCase() : key === "app" ? (parse(u.app)?.[1] ?? -1)
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
  $("all-users-search").addEventListener("input", (e) => { allQuery = e.target.value.trim().toLowerCase(); part(() => drawAllUsers(last?.[which])); });
  const drawAllUsers = (h) => {
    for (const th of document.querySelectorAll("#all-users-table th[data-sort]")) {
      th.classList.toggle("sorted", th.dataset.sort === allSort.key);
      th.dataset.dir = th.dataset.sort === allSort.key ? (allSort.dir > 0 ? "▲" : "▼") : "";
    }
    const users = h?.users || [];
    const list = users.filter((u) => !allQuery || (u.name || "").toLowerCase().includes(allQuery) || u.id.includes(allQuery)).sort((a, b) => {
      const x = allValue(a, allSort.key), y = allValue(b, allSort.key);
      return (x < y ? -1 : x > y ? 1 : 0) * allSort.dir || b.created - a.created;
    });
    set("all-users-note", `${num(list.length)}${list.length !== users.length ? ` of ${num(users.length)}` : ""}${h?.total > users.length ? ` · the ${num(users.length)} seen last of ${num(h.total)}` : ""}`);
    const body = $("all-users-body");
    body.replaceChildren();
    for (const u of list) {
      const tr = body.insertRow();
      tr.className = "opens";
      tr.title = "open the conversation";
      cell(tr, u.name ?? "–", "name", u.id);
      cell(tr, rel(u.created), null, date(u.created));
      cell(tr, seenDay(u.last_seen), null, u.last_seen ? new Date(u.last_seen).toLocaleDateString() : "never seen");
      cell(tr, appTag(u.app, u.id));
      cell(tr, u.push ? tag("on", "ok") : tag("off", "muted"));
      cell(tr, u.secured ? tag("yes", "ok") : tag("no", "muted"), null, u.backup_at ? `backup ${date(u.backup_at)}` : "");
      cell(tr, num(u.devices), "num");
      cell(tr, u.queued ? num(u.queued) : "–", "num muted");
      tr.addEventListener("click", () => select(up(u.id), u.name || u.id));
    }
    if (!list.length) cell(body.insertRow(), h && !h.error ? (allQuery ? "No user matches." : "No users yet.") : (h?.error || "No data"), "empty").colSpan = 8;
  };
  const drawUsersTab = (h) => {
    const ok = h && !h.error, days = days30();
    const su = h?.signups || [], s30 = days.map((_, i) => su[su.length - 30 + i] ?? null);
    const today = s30[29] ?? null, week = s30.slice(-7).reduce((n, v) => n + (v || 0), 0);
    set("us-new", ok ? num(today) : "–");
    set("us-new-cap", ok ? `${num(week)} in 7 days · ${num(s30.reduce((n, v) => n + (v || 0), 0))} in 30` : "");
    const act = h?.active_day ?? null;
    set("us-active", ok ? num(act) : "–");
    set("us-active-cap", ok ? `${num(h.active_today)} in 24 h · ${num(h.active_users)} in 7 days` : "");
    set("us-total", ok ? num(h.total) : "–");
    set("us-total-cap", ok ? `${num(h.secured)} secured · ${num(h.devices)} devices` : "");
    bars($("us-signups"), s30.map((v) => v ?? 0), days.map((d, i) => `${dayName(d)}: ${s30[i] ?? 0} sign-ups`));
    set("us-signups-sum", "UTC days");
    const kept = h?.active_days || {}, a30 = days.map((d) => kept[d] ?? null);
    if (act != null) a30[29] = Math.max(a30[29] ?? 0, act);
    bars($("us-actives"), a30, days.map((d, i) => `${dayName(d)}: ${a30[i] == null ? "not recorded" : `${a30[i]} active`}`));
    const first = days.find((d) => kept[d] != null);
    set("us-active-sum", first != null && first > days[0] ? `recorded since ${dayName(first)}` : "UTC days");
    drawAllUsers(h);
  };

  // ---- users: the reports list (who reported whom); handled is kept by the admin page (done.json) ---------------------
  let reportsMode = "open";
  $("reports-mode").addEventListener("change", (e) => { reportsMode = e.target.value; part(drawReports); });
  const reportDone = async (r, done) => {
    await fetch("/feedback/done", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: `hub=${which}&id=r${r.id}&done=${done ? 1 : 0}` });
    await refresh();
  };
  const reasonTag = (reason) => tag(reason, reason === "abuse" ? "bad" : reason === "spam" ? "warn" : "muted");
  const drawReports = () => {
    const h = last?.[which], list = [...(h?.reports || [])].sort((a, b) => b.at - a.at);
    const open = list.filter((r) => !lastDone.has(reportKey(r)));
    set("reports-note", list.length ? `${open.length} open · ${list.length} in all` : "");
    const shown = list.filter((r) => reportsMode === "all" || (reportsMode === "done") === lastDone.has(reportKey(r)));
    const body = $("reports-body");
    body.replaceChildren();
    for (const r of shown) {
      const tr = body.insertRow(), done = lastDone.has(reportKey(r));
      if (done) tr.className = "done";
      cell(tr, r.id == null ? "" : tickButton(done, "Report handled", (d) => reportDone(r, d)), "tick-col");
      const who = el("button", "link name", r.reported_name ?? r.reported.slice(0, 8));
      who.type = "button"; who.title = "open this user";
      who.addEventListener("click", () => { select(up(r.reported), r.reported_name || r.reported); $("people").scrollIntoView({ behavior: "smooth" }); });
      cell(tr, who);
      cell(tr, reasonTag(r.reason));
      cell(tr, r.reporter_name ?? r.reporter.slice(0, 8), "muted", r.reporter);
      cell(tr, rel(r.at), null, date(r.at));
    }
    if (!shown.length) cell(body.insertRow(), reportsMode === "open" ? "No open reports." : "None.", "empty").colSpan = 5;
  };

  // ---- crashes: one row per signature; a click shows the report under it; pages only beyond 100 -------------------
  const diagInfo = (d) => {
    let p = {};
    try { p = JSON.parse(d.payload || "{}") || {}; } catch {}
    const title = d.kind === "crash" ? (p.exception || p.termination || "Crash")
      : [p.domain, p.code, p.place].filter((x) => x != null && x !== "").join(" · ") || "Error";
    const text = [p.details, p.frames, p.message].filter(Boolean).join("\n\n") || d.payload || "";
    // "expected": known to happen (a push unread while locked since a restart), counted but filtered out by default.
    return { title, text, expected: p.severity === "expected" };
  };
  let diagMode = "open", diagPage = 1, diagLevel = "hide";
  $("diag-level").addEventListener("change", (e) => { diagLevel = e.target.value; diagPage = 1; part(drawDiagnostics); });
  const diagOpen = new Set();
  $("diag-mode").addEventListener("change", (e) => { diagMode = e.target.value; diagPage = 1; part(drawDiagnostics); });
  const PAGE = 100;
  // Every open entry of this hub done at once, after a confirm.
  $("diag-all").addEventListener("click", async () => {
    const hub = which, open = (last?.[hub]?.diagnostics || []).filter((d) => !d.done);
    if (!open.length || !confirm(`Mark all ${open.length} open crashes and errors on ${hub} done?`)) return;
    $("diag-all").disabled = true;
    try {
      await Promise.all(open.map((d) => fetch("/diagnostics/done", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ hub, id: d.id, done: true }) })));
      await refresh();
    } finally { $("diag-all").disabled = false; }
  });
  const drawDiagnostics = () => {
    const h = last?.[which], list = h?.diagnostics || [], open = list.filter((d) => !d.done);
    $("diag-all").hidden = !open.length;
    $("diag-all").textContent = `Mark all ${open.length} done`;
    set("diag-note", `${open.length} open · ${list.length} in all`);
    // The tab's quiet count: open ones without the expected, roughly (the snapshot is up to ten seconds old).
    const real = open.filter((d) => !diagInfo(d).expected).length;
    set("tab-crashes", real ? `~${real > 99 ? "99+" : real}` : "");
    const shown = list.filter((d) => diagMode === "all" || (diagMode === "done") === !!d.done)
      .filter((d) => diagLevel === "all" || !diagInfo(d).expected).sort((a, b) => b.last_seen - a.last_seen);
    const pages = Math.max(1, Math.ceil(shown.length / PAGE));
    diagPage = Math.min(diagPage, pages);
    const nav = $("diag-pages");
    nav.hidden = pages === 1;
    nav.replaceChildren(...Array.from({ length: pages }, (_, i) => {
      const b = el("button", i + 1 === diagPage ? "on" : "", String(i + 1));
      b.type = "button"; b.onclick = () => { diagPage = i + 1; drawDiagnostics(); };
      return b;
    }));
    const body = $("diag-body");
    body.replaceChildren();
    for (const d of shown.slice((diagPage - 1) * PAGE, diagPage * PAGE)) {
      const info = diagInfo(d), tr = body.insertRow(), hub = which;
      tr.className = `opens${d.done ? " done" : ""}`;
      cell(tr, tickButton(!!d.done, "Crash done", async (done) => {
        const r = await fetch("/diagnostics/done", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ hub, id: d.id, done }) });
        if (r.ok) await refresh();
      }), "tick-col");
      cell(tr, d.kind === "crash" ? tag("Crash", "bad") : info.expected ? tag("Expected", "muted") : tag("Error", "warn"));
      cell(tr, info.title, "sig", info.title);
      cell(tr, num(d.count), "num");
      cell(tr, rel(d.first_seen), null, date(d.first_seen));
      cell(tr, rel(d.last_seen), null, date(d.last_seen));
      cell(tr, d.app || "–", "mono", [d.os, d.model].filter(Boolean).join(" · "));
      const more = body.insertRow(), td = more.insertCell();
      more.className = "more"; td.colSpan = 7;
      td.append(el("p", "cap", `${d.app || "?"} · ${d.os || "?"} · ${d.model || "?"} · first ${date(d.first_seen)} · last ${date(d.last_seen)}`), el("pre", null, info.text));
      more.hidden = !diagOpen.has(d.id);
      tr.addEventListener("click", () => { more.hidden = !more.hidden; more.hidden ? diagOpen.delete(d.id) : diagOpen.add(d.id); tr.classList.toggle("open", !more.hidden); });
      tr.classList.toggle("open", !more.hidden);
    }
    if (!body.rows.length) cell(body.insertRow(), diagMode === "open" ? "Nothing open. 🎉" : "None.", "empty").colSpan = 7;
  };

  // ---- database: every table, grouped by topic; reserved names with their switch --------------------------------
  const TABLE_GROUPS = [
    ["Accounts & Identity", ["users", "handles", "reserved", "gone", "account_keys", "devices", "contact_keys", "profile_commitments"]],
    ["Messaging (core)", ["messages", "prekeys", "blobs", "blob_quota", "zk_groups"]],
    ["Account Data", ["storage", "backups"]],
    ["Team & Support", ["support_messages", "diagnostics", "diagnostic_seen", "diagnostic_days", "reports", "feedback"]],
    ["Internal", ["settings", "deliveries_hourly", "_litestream_lock", "_litestream_seq"]],
  ];
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
    support_messages: "Support chat messages and the team's to-dos",
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
    const groups = TABLE_GROUPS.map(([title, names]) => {
      names.forEach((n) => known.add(n));
      return [title, names.filter((n) => byName.has(n)).map((n) => byName.get(n))];
    });
    groups.push(["Other", list.filter((t) => !known.has(t.name))]);
    return groups.filter(([, ts]) => ts.length);
  };
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
    const add = el("button", "pill", "Add"); add.type = "submit";
    form.append(input, add);
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const names = input.value.split(/[,\n]/).map((n) => n.trim()).filter(Boolean);
      if (!names.length) return;
      try { await changeReserved({ action: "add", names }); input.value = ""; } catch (x) { err.textContent = x.message; }
    });
    wrap.classList.toggle("off", !!data && !data.enabled);
    wrap.append(sw, label, form, err);
    return wrap;
  };
  const drawTables = () => {
    const box = $("tables").querySelector(".tables"), list = last?.[which]?.tables || [];
    const drawn = JSON.stringify([which, tableOpen, list.map((t) => [t.name, t.count]), tableOpen === "reserved" ? reservedData?.[which]?.enabled : 0,
      (list.find((t) => t.name === tableOpen) || {}).rows?.length]);
    if (drawn === tablesDrawn && box.childElementCount) return;
    tablesDrawn = drawn;
    set("tables-note", `${list.length} tables`);
    const groups = tableGroups(list);
    if (!list.some((t) => t.name === tableOpen)) tableOpen = groups.length ? groups[0][1][0].name : null;
    const side = el("nav", "tside"), pane = el("div", "tpane");
    for (const [title, ts] of groups) {
      side.append(el("h3", null, title));
      for (const t of ts) {
        const b = el("button", t.name === tableOpen ? "on" : "");
        b.type = "button"; b.title = TABLE_INFO[t.name] || "";
        b.append(el("span", null, t.name), el("small", null, num(t.count)));
        b.addEventListener("click", () => { tableOpen = t.name; drawTables(); });
        side.append(b);
      }
    }
    const t = list.find((x) => x.name === tableOpen);
    if (t) {
      const head = el("div", "thead");
      const h = el("h3", null, t.name);
      h.append(el("small", null, `${num(t.count)} rows${t.count > t.rows.length ? ` · first ${t.rows.length} shown` : ""}`));
      head.append(h, el("p", "desc", TABLE_INFO[t.name] || "No description."));
      pane.append(head);
      if (t.name === "reserved") pane.append(reservedBar());
      const wrap = el("div", "scroll grid"), table = el("table", "list data");
      const hr = table.createTHead().insertRow();
      for (const c of t.columns) hr.append(el("th", null, c));
      if (t.name === "reserved") hr.append(el("th"));
      const body = table.createTBody();
      for (const row of t.rows) {
        const tr = body.insertRow();
        row.forEach((v, i) => {
          if (isTime(t.columns[i], v)) cell(tr, date(v), "mono", String(v));
          else cell(tr, v == null ? "–" : String(v), "mono", v != null && String(v).length > 40 ? String(v) : "");
        });
        if (t.name === "reserved") {
          const name = row[t.columns.indexOf("spelling")] ?? row[0];
          const x = el("button", "ghost-x", "Remove"); x.type = "button";
          x.addEventListener("click", async () => { x.disabled = true; try { await changeReserved({ action: "remove", name }); } catch { x.disabled = false; } });
          cell(tr, x);
        }
      }
      if (!t.rows.length) cell(body.insertRow(), "Empty.", "empty").colSpan = t.columns.length + (t.name === "reserved" ? 1 : 0);
      wrap.append(table);
      pane.append(wrap);
    }
    const keep = box.querySelector(".tpane .grid"), scroll = keep ? [keep.scrollLeft, keep.scrollTop, box.querySelector(".tpane").dataset.name] : null;
    box.replaceChildren(side, pane);
    if (t) pane.dataset.name = t.name;
    if (scroll && t && scroll[2] === t.name) { const g = pane.querySelector(".grid"); g.scrollLeft = scroll[0]; g.scrollTop = scroll[1]; }
  };

  // ---- everything from the collector -------------------------------------------------------------------------------
  const render = () => {
    const h = last?.[which];
    findNewest(h);
    part(() => overview(h));
    supportRefresh();
    part(drawReports);
    part(drawDiagnostics);
    if (currentTab === "support") part(drawPerson);
    if (currentTab === "users") part(() => drawUsersTab(h));
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
      const marksResponse = await fetch("/marks.json", { cache: "no-store" });
      if (marksResponse.ok) marks = await marksResponse.json();
      render();
      status.textContent = last.stale ? "Stale data · waiting for the collector" : "";
    } catch {
      status.textContent = last ? "Stale data · refresh failed" : "Waiting for fresh data…";
    } finally { peopleBusy = false; }
  }

  // ---- status: the STATUS bot's last report and its rhythm (skills/status) -----------------------------------------
  let statusEdited = false;
  const drawStatus = (d) => {
    const l = d.last, c = d.config || {};
    const at = l && l.at ? new Date(l.at * 1000) : null;
    set("st-when", at ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "–");
    set("st-sent", !l ? "no report yet" : `${at.toLocaleDateString()} · ${l.sent ? "sent" : `not sent${l.error ? ` (${l.error})` : ""}`}`);
    set("st-users", l?.users ?? "–");
    set("st-since", l?.since == null ? "" : `${l.since >= 0 ? "+" : ""}${l.since} since ${c.baseline_users ?? 0}`);
    set("st-up", l?.up == null ? "–" : `${l.up}/${l.total}`);
    set("st-load", l ? `load ${l.load ?? "?"} · ${l.crashes ?? "?"} open crashes` : "");
    if (!statusEdited) $("st-every").value = String(c.every_minutes ?? 0);
  };
  async function statusRefresh() {
    try {
      const r = await fetch("/status.json", { cache: "no-store" });
      if (r.ok) drawStatus(await r.json());
    } catch {}
  }
  $("st-every").addEventListener("input", () => { statusEdited = true; });
  $("st-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const err = $("st-error");
    err.classList.remove("bad");
    try {
      const r = await postJSON("/status", { every_minutes: Number($("st-every").value) });
      statusEdited = false;
      drawStatus(await r.json());
      err.textContent = "Saved.";
    } catch {
      err.textContent = "Not saved. Try again.";
      err.classList.add("bad");
    }
  });

  // ---- tabs: the hash keeps the tab across reloads and links; keys 1–6 switch, 0 the hub ----------------------------
  const TABS = ["overview", "support", "users", "crashes", "db", "status"];
  const tab = () => {
    const h = location.hash.slice(1);
    const t = h === "diagnostics" ? "crashes" : h === "reserved" ? "db" : h === "feedback" ? "support" : TABS.includes(h) ? h : "overview";
    if (h === "feedback") history.replaceState(null, "", "#support");
    const changed = t !== currentTab;
    currentTab = t;
    document.body.dataset.tab = t;
    for (const e of document.querySelectorAll("[data-tabs]")) e.hidden = !e.dataset.tabs.split(" ").includes(t);
    for (const a of document.querySelectorAll("#tabs a")) { const on = a.dataset.tab === t; a.classList.toggle("on", on); a.setAttribute("aria-selected", String(on)); }
    if (last) render();
    if (t === "status") statusRefresh();
    if (changed) window.scrollTo(0, 0);
  };
  window.addEventListener("hashchange", tab);
  const typing = (t) => t instanceof Element && (t.isContentEditable || t.closest("textarea, select, [contenteditable]")
    || (t.closest("input") && !["checkbox", "radio", "button", "submit"].includes(t.closest("input").type)));
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || typing(e.target) || $("pop").open) return;
    const n = /^Digit[0-6]$/.test(e.code) ? Number(e.code.slice(5)) : /^[0-6]$/.test(e.key) ? Number(e.key) : null;
    if (n == null) return;
    e.preventDefault();
    if (n === 0) { switchHub(); return; }
    const target = TABS[n - 1];
    if (target === "overview") { history.pushState(null, "", location.pathname + location.search); tab(); }
    else if (location.hash !== `#${target}`) location.hash = target;
  });
  tab();
  tick();
  setInterval(tick, 3000);
  refresh();
  setInterval(refresh, 10000);
  setInterval(() => { if (currentTab === "status" && document.visibilityState === "visible") statusRefresh(); }, 30000);
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") { tick(); refresh(); } });
}
