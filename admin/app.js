// Login: a word on why the last try did not work. Dashboard: the numbers, every three seconds.
const $ = (id) => document.getElementById(id);

if (document.body.classList.contains("login")) {
  const q = location.search, note = $("note");
  if (q.includes("wrong")) { note.textContent = "Wrong user or password."; note.hidden = false; }
  if (q.includes("locked")) { note.textContent = "Too many tries. Wait 15 minutes."; note.hidden = false; }
} else {
  // A short history per tile (60 ticks of 3 s: three minutes), kept by the page only.
  const history = {};
  const keep = 60;
  const remember = (key, v) => {
    const h = (history[key] ||= []);
    if (v != null && Number.isFinite(v)) { h.push(v); if (h.length > keep) h.shift(); }
    return h;
  };

  const bytes = (n) => {
    if (n == null) return "–";
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return `${n.toFixed(n < 10 && i ? 1 : 0)} ${u[i]}`;
  };
  const rate = (n) => (n == null ? "–" : `${bytes(n)}/s`);
  const span = (s) => {
    s = Math.floor(s);
    const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
    return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
  };
  const up = (unix) => (unix ? `up ${span(Date.now() / 1000 - unix)}` : "–");
  const set = (id, text) => { $(id).textContent = text; };
  const gauge = (id, frac) => {
    const el = $(id);
    el.style.width = `${Math.min(100, Math.max(0, frac * 100)).toFixed(1)}%`;
    el.classList.toggle("hot", frac > 0.9);
  };
  // The graph of a tile: an area from the right edge, scaled 0..max (or 0..its own peak); fit: around the values
  // themselves (a memory curve that hardly moves would otherwise be a flat block).
  const spark = (id, values, max, fit) => {
    const el = $(id);
    if (!el) return;
    const path = el.tagName === "path" ? el : el.querySelector("path");
    if (values.length < 2) { path.removeAttribute("d"); return; }
    let lo = 0, hi = max ?? Math.max(...values) * 1.15;
    if (fit) {
      const a = Math.min(...values), b = Math.max(...values), pad = Math.max((b - a) * 0.4, b * 0.02);
      lo = Math.max(0, a - pad); hi = b + pad;
    }
    hi = Math.max(hi, lo + 1e-9);
    const step = 120 / (keep - 1), start = 120 - (values.length - 1) * step;
    const pts = values.map((v, i) => `${(start + i * step).toFixed(2)},${(32 - ((v - lo) / (hi - lo)) * 29).toFixed(2)}`);
    path.setAttribute("d", `M${start.toFixed(2)},32 L${pts.join(" L")} L120,32 Z`);
  };
  // Each part of the page on its own: a section the server could not read (null) shows "–", the others still update.
  // A ring gauge: its percentage inside, the ring filled to it (red over 90 %).
  const ring = (id, frac) => {
    const el = $(`r-${id}`), p = Math.min(100, Math.max(0, frac * 100));
    el.style.setProperty("--p", p.toFixed(1));
    el.classList.toggle("hot", p > 90);
    set(id, `${Math.round(p)} %`);
  };
  const bar = (id, frac) => { $(`r-${id}`).style.setProperty("--p", Math.min(100, Math.max(0, frac * 100)).toFixed(1)); };
  const part = (fn) => { try { fn(); } catch (e) { console.warn(e); } };
  const state = (tile, cls) => { $(tile).classList.remove("ok", "bad", "warn", "alarm"); if (cls) $(tile).classList.add(...cls.split(" ")); };
  // A service tile: its state as the number, how long it runs and its memory as the caption, its memory as the graph.
  const service = (id, u, big) => {
    u = u || {};
    const ok = u.state === "active";
    state(`t-${id}`, ok ? "ok" : "bad");
    set(id, ok ? (big || "running") : (u.state || "unknown"));
    const restarts = u.restarts ? ` · ${u.restarts} restarts` : "";
    set(`${id}-cap`, ok ? `${up(u.since)}${u.memory != null ? ` · ${bytes(u.memory)}` : ""}${restarts}` : "not running");
    $(`t-${id}`).title = `${$(id).textContent} · ${$(`${id}-cap`).textContent}`;
    if ($(`${id}-spark`)) spark(`${id}-spark`, remember(id, u.memory), null, true);
  };

  // The system tile's small stats carry their details on hover.
  const hoverDetails = () => { for (const el of document.querySelectorAll(".stat, .flag")) { const c = el.querySelector(".cap"); if (c) el.title = c.textContent; } };

  // Keep independent pages for each hub and view; long lists use 10,000 rows.
  const listPages = {};
  const paginate = (list, id, key, size, redraw, always = false) => {
    const pages = Math.max(1, Math.ceil(list.length / size));
    const page = listPages[key] = Math.min(listPages[key] || 1, pages);
    const nav = $(id);
    nav.replaceChildren();
    nav.hidden = !always && pages === 1;
    if (!nav.hidden) {
      for (const [label, target] of [["‹", page - 1], ...Array.from({length: pages}, (_, i) => [String(i + 1), i + 1]), ["›", page + 1]]) {
        const b = document.createElement("button"); b.textContent = label;
        b.disabled = target < 1 || target > pages || target === page;
        if (target === page) b.setAttribute("aria-current", "page");
        b.onclick = () => { listPages[key] = target; redraw(); };
        nav.append(b);
      }
    }
    return list.slice((page - 1) * size, page * size);
  };
  let reservedData = null;
  function drawReserved() {
    const data = reservedData?.[which];
    set("reserved-hub", which);
    set("reserved-count", data ? data.names.length : "–");
    set("reserved-rule", data?.name_min != null ? `Names: ${data.name_min}–${data.name_max} characters (graphemes), ${data.name_form}; compared by ${data.name_comparison} (lookalikes count as the same)` : "Names: unavailable");
    set("reserved-state", data ? data.enabled ? "ON" : "OFF" : "unavailable");
    $("reserved-enabled").disabled = !data;
    $("reserved-enabled").checked = !!data?.enabled;
    $("reserved-status").classList.toggle("reservation-off", !!data && !data.enabled);
    const list = $("reserved-list"); list.replaceChildren();
    const names = (data?.names || []).filter(n => n.toLocaleLowerCase().includes($("reserved-search").value.toLocaleLowerCase()));
    for (const name of paginate(names, "reserved-pages", `reserved:${which}`, 10000, drawReserved)) {
      const row = document.createElement("div"), label = document.createElement("span"), b = document.createElement("button");
      label.textContent = name; b.textContent = "Remove"; b.onclick = () => changeReserved({action: "remove", name});
      row.append(label, b); list.append(row);
    }
  }
  async function changeReserved(data) {
    const hub = which;
    try {
      const r = await fetch("/reserved", {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify({hub, ...data})});
      if (!r.ok) throw new Error(`Could not save (${r.status}).`);
      $("reserved-error").textContent = "";
      await tick();
      return true;
    } catch (e) { $("reserved-error").textContent = e.message; drawReserved(); return false; }
  }
  $("reserved-search").oninput = () => { listPages[`reserved:${which}`] = 1; drawReserved(); };
  $("reserved-enabled").onchange = e => changeReserved({action: "switch", enabled: e.target.checked});
  $("reserved-add").onsubmit = async e => {
    e.preventDefault();
    const names = $("reserved-input").value.split(/[,\n]/).map(n => n.trim()).filter(Boolean);
    if (names.length && await changeReserved({action: "add", names})) $("reserved-input").value = "";
  };
  async function tick() {
    if (document.visibilityState !== "visible") return;
    let s;
    try {
      const r = await fetch("/stats.json", { cache: "no-store" });
      if (r.status === 401 || r.status === 403) { location.reload(); return; }
      s = await r.json();
      $("stale").hidden = true;
    } catch {
      $("stale").hidden = false;
      return;
    }
    set("host", s.host);
    reservedData = s.reserved;
    drawReserved();

    part(() => {
      const c = s.cpu || { percent: null, load: [], cores: null };
      set("cpu", c.percent == null ? "–" : c.percent.toFixed(c.percent < 10 ? 1 : 0));
      set("cpu-cap", c.cores ? `${c.cores} cores` : "–");
      if (c.percent != null) ring("cpu", c.percent / 100);
    });
    part(() => {
      const c = s.cpu || { load: [] };
      set("load", c.load.length ? c.load[0].toFixed(2) : "–");
      set("load-cap", c.load.length ? `5 min ${c.load[1].toFixed(2)} · 15 min ${c.load[2].toFixed(2)}` : "–");
      $("t-system").title = c.load.length ? `Load ${c.load.map((x) => x.toFixed(2)).join(" · ")}` : "";
    });

    part(() => {
      const m = s.memory;
      if (!m) { set("mem", "–"); set("mem-cap", "–"); return; }
      const used = m.total - m.available;
      set("mem", bytes(used));
      set("mem-cap", `of ${bytes(m.total)} · ${Math.round((100 * used) / m.total)} %`
        + (m.swap_total ? ` · swap ${bytes(m.swap_total - m.swap_free)}` : ""));
      ring("mem", used / m.total);
    });

    part(() => {
      const d = s.disk;
      if (!d) { set("disk", "–"); set("disk-cap", "–"); return; }
      set("disk", bytes(d.used));
      set("disk-cap", `of ${bytes(d.total)} · ${bytes(d.free)} free`);
      ring("disk", d.used / d.total);
    });

    part(() => {
      const n = s.network || { rx_per_s: null, tx_per_s: null, rx: null, tx: null };
      set("rx", rate(n.rx_per_s)); set("rx-cap", `${bytes(n.rx)} since boot`);
      set("tx", rate(n.tx_per_s)); set("tx-cap", `${bytes(n.tx)} since boot`);
      bar("rx", (n.rx_per_s || 0) / 1e6); bar("tx", (n.tx_per_s || 0) / 1e6);  // bars up to 1 MB/s
      $("t-net").title = `since boot: ${bytes(n.rx)} in · ${bytes(n.tx)} out`;
    });

    part(() => {
      set("uptime", s.uptime == null ? "–" : span(s.uptime));
      const r = s.reboot, cap = $("reboot-text");
      cap.textContent = r && r.needed ? "REBOOT" : "";
      $("t-uptime").title = r == null ? "" : r.needed ? `reboot required${r.packages.length ? ` (${r.packages[0]})` : ""}` : "no reboot needed";
      cap.classList.toggle("warn", !!(r && r.needed));
      state("t-uptime", r && r.needed ? "warn" : null);
    });

    part(() => {
      const version = s.hub && s.hub.version;
      service("hub", s.hub, version ? version.split(" ")[0] : null);
      if (version) $("t-hub").title = version;
      set("foot-hub", version ? ` · hub ${version.split(" ")[0]}` : "");
    });
    part(() => service("dev", s.hub_dev));
    part(() => service("tunnel", s.tunnel));
    part(() => {
      const y = s.yogpt;
      service("yogpt", y);
      const people = last && last.live && last.live.yogpt_contacts;
      if (y && y.state === "active" && people != null) $("t-yogpt").title += ` · knows ${people}`;
    });

    hoverDetails();
  }

  // The users, as users.py wrote them (on request). Last seen counts to the day only (the hub writes it once a day).
  const date = (ms) => (ms ? new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "–");
  // "5 min ago", "3 h ago", "1 day", "2 weeks", "2 months"; the exact time goes in the tooltip.
  const rel = (ms) => {
    if (!ms) return "–";
    const m = Math.max(0, (Date.now() - ms) / 60000), d = m / 1440;
    if (m < 1) return "just now";
    if (m < 60) return `${Math.floor(m)} min ago`;
    if (d < 1) return `${Math.floor(m / 60)} h ago`;
    const unit = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
    if (d < 7) return unit(Math.floor(d), "day");
    if (d < 60) return unit(Math.floor(d / 7), "week");
    return unit(Math.floor(d / 30), "month");
  };
  const day = (ms) => {
    if (!ms) return "–";
    const days = Math.floor((Date.now() - ms) / 86400000);
    return days < 1 ? "today" : days < 2 ? "yesterday" : `${days} days ago`;
  };
  const cell = (tr, text, cls) => {
    const td = tr.insertCell();
    td.textContent = text;
    if (cls) td.className = cls;
  };
  const users = (id, h) => {
    const card = $(id), body = card.querySelector("tbody"), note = card.querySelector("h2 small");
    set("users-count", h && !h.error ? h.total : "–");
    body.replaceChildren();
    $("user-pages").replaceChildren();
    $("user-pages").hidden = true;
    if (!h || h.error) { note.textContent = h ? h.error : "no data"; return; }
    note.textContent = `${h.total} users · ${h.active_today ?? "?"} today`;
    note.title = `${h.active_users ?? "?"} this week · ${h.secured} secured · ${h.devices} devices · ${h.reserved ?? "?"} reserved · ${bytes(h.size)}`;
    let list = [...h.users].sort((a, b) => (b.last_seen || 0) - (a.last_seen || 0) || b.created - a.created);
    const overview = currentTab === "overview";
    list = paginate(list, "user-pages", `users:${currentTab}:${which}`, overview ? 10 : 10000,
      () => render(last, lastDone), overview);
    for (const u of list) {
      const tr = body.insertRow();
      cell(tr, u.name ?? "–", "name");
      // The user id on hover, copied on a click.
      const nameCell = tr.cells[0];
      nameCell.title = `${u.id} (click to copy)`;
      nameCell.classList.add("copy");
      nameCell.addEventListener("click", () => { navigator.clipboard?.writeText(u.id).then(() => { nameCell.title = `${u.id} (copied)`; }).catch(() => {}); });
      cell(tr, rel(u.created)); tr.cells[tr.cells.length - 1].title = date(u.created);
      // Last seen counts to the day only (the hub writes it once a day, on purpose).
      cell(tr, !u.last_seen ? "–" : Date.now() - u.last_seen < 86400000 ? "today" : Date.now() - u.last_seen < 2 * 86400000 ? "yesterday" : rel(u.last_seen));
      tr.cells[tr.cells.length - 1].title = u.last_seen ? new Date(u.last_seen).toLocaleDateString() : "";
      cell(tr, u.devices);
      cell(tr, u.push ? "yes" : "no", u.push ? "yes" : "no");
      cell(tr, u.secured ? "yes" : "no", u.secured ? "yes" : "no");
      cell(tr, u.queued);
      const a = appState(u.app, h.min_app);
      cell(tr, a === "new" ? "newest" : a === "old" ? "update" : (u.app ?? "–"), a === "new" ? "yes" : a === "old" ? "warn" : "mono");
      tr.cells[tr.cells.length - 1].title = u.app ?? "";
    }
    for (let i = list.length; i < (overview ? 10 : 0); i++) {
      const row = body.insertRow(); row.setAttribute("aria-hidden", "true");
      for (let j = 0; j < 8; j++) cell(row, "\u00a0");
    }
  };

  // User Reports, open ones first; a tick closes one (kept by the page, like feedback: key "live:r<ids>").
  const reports = (id, h, done) => {
    const card = $(id), body = card.querySelector("tbody"), note = card.querySelector("h2 small");
    body.replaceChildren();
    const list = ((h && h.reports) || []).map((r) => ({ ...r, key: `${which}:r${r.id}` }));
    list.sort((a, b) => (done.has(a.key) - done.has(b.key)) || (b.at - a.at));
    const open = list.filter((r) => !done.has(r.key)).length;
    note.textContent = list.length ? `${open} open · ${list.length}` : "none";
    badge("badge-users", open);
    card.querySelector(".scroll").hidden = !list.length;

    for (const r of list) {
      const tr = body.insertRow(), hub = which;
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = done.has(r.key);
      box.disabled = r.id == null;
      box.addEventListener("change", async () => {
        await fetch("/feedback/done", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: `hub=${hub}&id=r${r.id}&done=${box.checked ? 1 : 0}` });
        people();
      });
      tr.insertCell().append(box);
      if (done.has(r.key)) tr.className = "done";
      cell(tr, rel(r.at)); tr.cells[tr.cells.length - 1].title = date(r.at);
      cell(tr, r.reported_name ?? "–", "name");
      cell(tr, r.reason, r.reason === "abuse" ? "bad" : "");
      cell(tr, r.reporter_name ?? r.reporter.slice(0, 8));
      rowOpens(tr, () => popup([["Reported", r.reported_name], ["Reported id", r.reported], ["Reason", r.reason], ["By", r.reporter_name], ["By id", r.reporter], ["When", date(r.at)]], null, false, box));
    }
  };

  // Which hub the lower half shows: Live (the default) or Test (the dev hub); remembered in this browser.
  let which = "live", last = null, lastDone = new Set(), currentTab = "overview";
  try { if (localStorage.getItem("hub") === "dev") which = "dev"; } catch {}
  $("hub-word").textContent = which === "dev" ? "dev" : "live";
  $("hub-word").addEventListener("click", () => document.querySelector("#which button:not(.on)").click());
  for (const button of document.querySelectorAll("#which button")) {
    button.classList.toggle("on", button.dataset.hub === which);
    button.addEventListener("click", () => {
      which = button.dataset.hub;
      drawReserved();
      try { localStorage.setItem("hub", which); } catch {}
      for (const b of document.querySelectorAll("#which button")) b.classList.toggle("on", b === button);
      $("hub-word").textContent = which === "dev" ? "dev" : "live";
      if (last) render(last, lastDone);
    });
  }

  // Feedback and To-Dos of that hub, open ones first, then newest; a tick marks one done (kept by the page itself).
  // A row's details in a small popup: its fields, a long text (monospace for crash reports), the done tick.
  const popup = (fields, text, mono, box) => {
    const d = $("pop"), dl = d.querySelector("dl"), pre = d.querySelector("pre"), tick = d.querySelector(".pop-done input");
    dl.replaceChildren();
    for (const [k, v] of fields) { const dt = document.createElement("dt"), dd = document.createElement("dd"); dt.textContent = k; dd.textContent = v ?? "–"; dl.append(dt, dd); }
    pre.hidden = !text; pre.textContent = text || ""; pre.classList.toggle("mono", !!mono);
    d.querySelector(".pop-done").hidden = !box;
    tick.checked = box ? box.checked : false;
    tick.onchange = () => { box.checked = tick.checked; box.dispatchEvent(new Event("change")); };
    d.showModal();
  };
  const updateTile = (u) => {
    const stale = !u || Date.now() / 1000 - u.at > 1800;
    state("t-updates", stale ? "warn" : u.status);
    set("updates-summary", stale ? "Update data unavailable" : u.apt.error || u.apt.packages == null ? "Package check unavailable" : u.apt.packages ? `${u.apt.packages} updates (${u.apt.security} security)` : "Up to date");
    set("updates-reboot", u?.reboot ? `Reboot needed (kernel ${u.running_kernel || "?"} → ${u.installed_kernel || "?"}), next window ${u.next_reboot || "unknown"}` : "");
    set("updates-release", u ? `${u.current_release}${u.release.available ? ` · ${u.release.available} available` : u.release.error ? " · release check unavailable" : ""}` : "");
    const open = () => popup(u ? [
      ["Checked", new Date(u.at * 1000).toLocaleString()],
      ["Packages / security", `${u.apt.packages ?? "?"} / ${u.apt.security ?? "?"}`],
      ["APT checked", u.apt.checked ? new Date(u.apt.checked * 1000).toLocaleString() : "Never"],
      ["Security first observed", u.apt.security ? new Date(u.apt.security_since * 1000).toLocaleString() : "None"],
      ["Reboot needed since", u.reboot ? new Date(u.reboot_since * 1000).toLocaleString() : "No"],
      ["Reboot packages", (u.reboot_packages || []).join(", ")],
      ["Running / installed kernel", `${u.running_kernel || "?"} / ${u.installed_kernel || "?"}`],
      ["Last automatic upgrade trigger", u.last_upgrade_trigger || "Unknown"],
      ["Unattended upgrades", u.unattended || "Unknown"],
      ["Next reboot window (only if needed)", u.next_reboot || "Unknown"],
      ["Ubuntu", u.current_release],
      ["Release checked", u.release.checked ? new Date(u.release.checked * 1000).toLocaleString() : "Never"],
      ["LTS release check", u.release.error || u.release.result || "Unknown"],
      ["Errors", [...(u.errors || []), u.apt.error].filter(Boolean).join("\n") || "None"]
    ] : [["Updates", "Collector data unavailable"]], null, false, null);
    $("t-updates").onclick = open;
    $("t-updates").onkeydown = e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } };
  };
  const rowOpens = (tr, open) => { tr.classList.add("opens"); tr.addEventListener("click", (e) => { if (!e.target.closest("input")) open(); }); };

  // Support chat: the threads on the left (unread first), the open one as bubbles on the right; Enter sends.
  // Completion is explicit; while the Feedback tab is open, both refresh every few seconds.
  let supportUser = null, supportHub = null, supportName = "", supportSeen = "", supportBusy = false, supportThreads = [];
  let supportWant = null, threadsDrawn = "";
  const supportGet = async (user) => {
    const r = await fetch(`/support.json?${new URLSearchParams({hub: which, ...(user ? {user} : {})})}`, { cache: "no-store" });
    if (!r.ok) throw new Error("Could not load messages");
    return r.json();
  };
  const supportPost = async (path, data) => {
    const r = await fetch(path, {method:"POST", headers:{"Content-Type":"application/json"},body:JSON.stringify(data)});
    if (!r.ok) throw new Error("Change failed. Try again.");
  };
  const clock = (ms) => {
    if (!ms) return "";
    const d = new Date(ms), today = new Date().toDateString() === d.toDateString();
    return today ? d.toLocaleTimeString(undefined, { timeStyle: "short" }) : d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  };
  const badge = (id, n) => { const b = $(id); b.hidden = !n; b.textContent = n > 99 ? "99+" : String(n || ""); };
  const supportUnread = (n) => {
    set("feedback-unread", n == null ? "–" : String(n));
    set("support-count", n ? `${n} open` : "");
    badge("badge-feedback", n);
  };
  // Redrawn only when something changed: a list rebuilt under the pointer would swallow the click.
  const drawThreads = () => {
    const box = $("support-threads"), lastLines = (last && last[which] && last[which].support_last) || {};
    const drawn = JSON.stringify([which, supportUser, supportThreads, supportThreads.map((t) => lastLines[t.user_id.toUpperCase()])]);
    if (drawn === threadsDrawn && box.childElementCount) return;
    threadsDrawn = drawn;
    box.replaceChildren();
    if (!supportThreads.length) { const p = document.createElement("p"); p.className = "cap"; p.textContent = "No conversations yet."; box.append(p); }
    for (const t of supportThreads) {
      const b = document.createElement("button"), id = t.user_id.toUpperCase();
      b.type = "button";
      b.className = `thread${t.unread ? " unread" : ""}${supportUser !== null && supportUser.toUpperCase() === id ? " on" : ""}`;
      const top = document.createElement("span"), dot = document.createElement("i"), name = document.createElement("b"), time = document.createElement("time");
      top.className = "top"; name.textContent = t.name || "Deleted account"; time.textContent = clock(t.at); time.title = date(t.at);
      top.append(dot, name, time);
      const line = document.createElement("span"), l = lastLines[id];
      line.className = "line";
      line.textContent = t.user_id === "team" ? `${t.open || 0} open to-dos` : l ? `${l.team ? "You: " : ""}${l.text}` : "";
      b.append(top, line);
      b.addEventListener("click", () => supportOpen(t.user_id, t.name || "Deleted account", true));
      box.append(b);
    }
  };
  const drawThread = (messages) => {
    const box = $("support-history"), atEnd = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
    box.replaceChildren();
    let lastDay = "";
    for (const m of messages) {
      const day = new Date(m.created_at).toDateString();
      if (day !== lastDay) {
        const d = document.createElement("div"); d.className = "day";
        d.textContent = new Date(m.created_at).toLocaleDateString(undefined, { dateStyle: "medium" });
        box.append(d); lastDay = day;
      }
      const p = document.createElement("div"), t = document.createElement("time");
      p.className = `bubble ${m.from_team ? "team" : "user"}`;
      p.textContent = m.text;
      t.textContent = new Date(m.created_at).toLocaleTimeString(undefined, { timeStyle: "short" });
      t.title = `${date(m.created_at)}${m.from_team ? (m.read_at ? " · read" : " · not read yet") : ""}`;
      const tick = document.createElement("input"), hub = supportHub, user = supportUser;
      tick.type = "checkbox"; tick.checked = m.done_at != null;
      tick.setAttribute("aria-label", "Message done");
      tick.onchange = async () => {
        tick.disabled = true;
        try {
          await supportPost("/support/done", {hub, id:m.id, done:tick.checked});
          if (which === hub && supportUser === user) await supportOpen(user);
          await supportRefresh();
        } catch (e) { tick.checked = !tick.checked; set("support-error", e.message); }
        finally { tick.disabled = false; }
      };
      p.classList.toggle("done", m.done_at != null);
      const meta = document.createElement("label");
      meta.className = "meta"; meta.title = m.done_at != null ? `done ${date(m.done_at)}` : "tick when done";
      meta.append(t, tick);
      p.append(meta);
      box.append(p);
    }
    if (atEnd || box.dataset.user !== supportUser) box.scrollTop = box.scrollHeight;
    box.dataset.user = supportUser;
  };
  // A click wins over a refresh still under way: only the thread asked for last is shown.
  const supportOpen = async (user, title, chosen) => {
    const hub = which;
    if (chosen || supportWant === null) supportWant = user;
    if (chosen) {
      if (supportUser !== user || supportHub !== hub) { $("support-text").value = ""; supportSeen = ""; $("support-history").replaceChildren(); }
      supportUser = user; supportHub = hub; supportName = title || user; set("support-title", supportName); drawThreads();
    }
    try {
      const data = await supportGet(user);
      if (which !== hub || supportWant !== user) return;
      if (supportUser !== user || supportHub !== hub) { $("support-text").value = ""; supportSeen = ""; }
      supportUser = user; supportHub = hub; supportName = title || supportName || user;
      set("support-title", supportName);
      set("support-sub", `${data.messages.length} messages`);
      $("support-reply").hidden = false;
      $("support-text").maxLength = user === "team" ? 4000 : 2000;
      $("support-text").placeholder = user === "team" ? "What needs doing?" : "Reply";
      $("support-reply").querySelector("button").textContent = user === "team" ? "Add To-Do" : "Send";
      const seen = data.messages.map((m) => `${m.id}:${m.read_at ? 1 : 0}:${m.done_at}`).join(",");
      if (seen !== supportSeen) { supportSeen = seen; drawThread(data.messages); }
      drawThreads();
      set("support-error", "");
    } catch (e) { set("support-error", e.message); }
  };
  const supportRefresh = async () => {
    if (supportBusy) return;
    supportBusy = true;
    const hub = which;
    if (supportHub !== hub) {
      supportUser = null; supportWant = null; supportSeen = ""; $("support-reply").hidden = true;
      $("support-history").replaceChildren(); set("support-title", "Loading…"); set("support-sub", "");
    }
    try {
      const threads = await supportGet();
      if (hub !== which) return;
      supportThreads = [...threads].sort((a, b) => Number(b.user_id === "team") - Number(a.user_id === "team") || Number(b.unread > 0) - Number(a.unread > 0) || b.at - a.at);
      supportUnread(threads.reduce((n, t) => n + t.unread, 0));
      drawThreads();
      // Nothing open yet: the first thread with something open (else the newest conversation, else the To-Dos).
      if (supportWant === null && supportThreads.length) {
        const first = supportThreads.find((t) => t.unread || t.open) || supportThreads.find((t) => t.user_id !== "team") || supportThreads[0];
        supportOpen(first.user_id, first.name || "Deleted account", true);
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
      if (user === "team") {
        const r = await fetch("/todos", {method:"POST", body:new URLSearchParams({hub, text})});
        if (!r.ok) throw new Error("Could not add To-Do");
      } else {
        await supportPost("/support", {hub, user, action: "reply", text});
        // The reply answers the user's open messages in this thread: they are ticked done.
        const open = which !== hub ? [] : (await supportGet(user)).messages.filter((m) => !m.from_team && m.done_at == null);
        await Promise.all(open.map((m) => supportPost("/support/done", {hub, id: m.id, done: true})));
      }
      if (which !== hub || supportUser !== user) return;
      $("support-text").value = "";
      await supportOpen(user);
      await supportRefresh();
    } catch (e) { set("support-error", e.message); }
    finally { button.disabled = false; $("support-text").focus(); }
  };
  $("support-reply").onsubmit = (e) => { e.preventDefault(); supportSend(); };
  $("support-text").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); supportSend(); }
  });
  setInterval(() => {
    if (document.visibilityState !== "visible" || currentTab !== "feedback") return;
    supportRefresh();
    if (supportUser && supportHub === which) supportOpen(supportUser);
  }, 4000);



  // A small graph of 14 values: an area and its line, a hover title per day.
  const SVG = "http://www.w3.org/2000/svg";
  const graph = (el, values, titles) => {
    const top = Math.max(1, ...values), w = 140, hgt = 40, step = w / Math.max(1, values.length - 1);
    const pts = values.map((v, i) => [i * step, hgt - 2 - (v / top) * (hgt - 6)]);
    const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
    const area = document.createElementNS(SVG, "path"), stroke = document.createElementNS(SVG, "path");
    area.setAttribute("d", `${line} L${w},${hgt} L0,${hgt} Z`); area.setAttribute("class", "area");
    stroke.setAttribute("d", line); stroke.setAttribute("class", "line");
    const hits = values.map((v, i) => {
      const r = document.createElementNS(SVG, "rect"), t = document.createElementNS(SVG, "title");
      r.setAttribute("x", Math.max(0, i * step - step / 2)); r.setAttribute("width", step); r.setAttribute("y", 0); r.setAttribute("height", hgt);
      t.textContent = titles[i]; r.append(t); return r;
    });
    el.replaceChildren(area, stroke, ...hits);
  };
  const days14 = () => Array.from({length: 14}, (_, i) => Math.floor(Date.now() / 86400000) - 13 + i);
  const dayName = (d) => new Date(d * 86400000).toLocaleDateString(undefined, { day: "numeric", month: "short" });

  // Feedback tile: open user messages at the end of each day (red: rising is worse).
  const feedbackTile = (h) => {
    const v = (h && h.support_unread_days) || [], days = days14();
    if (v.length) graph($("feedback-graph"), v, v.map((n, i) => `${dayName(days[i])}: ${n} open`));
    if (!supportThreads.length && v.length) supportUnread(v[v.length - 1]);
  };

  // A diagnostic's one line: the exception for a crash, domain · code · place for an error.
  const diagInfo = (d) => {
    let p = {};
    try { p = JSON.parse(d.payload || "{}") || {}; } catch {}
    const title = d.kind === "crash" ? (p.exception || p.termination || "Crash")
      : [p.domain, p.code, p.place].filter((x) => x != null && x !== "").join(" · ") || "Error";
    const text = [p.details, p.frames, p.message].filter(Boolean).join("\n\n") || d.payload || "";
    return { title, text };
  };
  let diagMode = "open";
  const diagOpen = new Set();
  $("diag-mode").addEventListener("change", (e) => { diagMode = e.target.value; if (last) render(last, lastDone); });
  const diagnostics = (h) => {
    const list = (h && h.diagnostics) || [], open = list.filter((d) => !d.done);
    set("diagnostics-open", h?.diagnostics_open ?? open.length);
    badge("badge-diagnostics", h?.diagnostics_open ?? open.length);
    // Tile: open crashes (red) and errors (yellow) by the day they were last seen.
    const days = days14();
    $("diagnostics-bars").replaceChildren(...days.map((day) => {
      const here = open.filter((d) => Math.floor(d.last_seen / 86400000) === day);
      const c = here.filter((d) => d.kind === "crash").length, e = here.length - c;
      const bar = document.createElement("span");
      bar.title = `${dayName(day)}: ${c} crashes · ${e} errors open`;
      if (!here.length) { bar.className = "zero"; return bar; }
      for (const [n, cls] of [[c, "crash"], [e, "error"]]) if (n) { const i = document.createElement("i"); i.className = cls; i.style.flexGrow = n; bar.append(i); }
      bar.style.height = `${(here.length / Math.max(1, ...days.map((x) => open.filter((d) => Math.floor(d.last_seen / 86400000) === x).length))) * 100}%`;
      return bar;
    }));
    // Tab: one row per signature, open ones by last seen; a click shows the report under it.
    const card = $("diag"), body = card.querySelector("tbody");
    card.querySelector("h2 small").textContent = `${open.length} open · ${list.length}`;
    body.replaceChildren();
    const shown = list.filter((d) => diagMode === "all" || (diagMode === "done") === !!d.done).sort((a, b) => b.last_seen - a.last_seen);
    if (!shown.length) { const tr = body.insertRow(); cell(tr, diagMode === "open" ? "Nothing open." : "None.", "muted"); tr.cells[0].colSpan = 7; }
    for (const d of shown) {
      const info = diagInfo(d), tr = body.insertRow(), box = document.createElement("input"), hub = which;
      box.type = "checkbox"; box.checked = !!d.done;
      box.setAttribute("aria-label", "Mark diagnostic done");
      box.onchange = async () => {
        const response = await fetch("/diagnostics/done", {method: "POST", headers: {"Content-Type": "application/json"},
          body: JSON.stringify({hub, id: d.id, done: box.checked})});
        if (!response.ok) box.checked = !box.checked; else people();
      };
      tr.insertCell().append(box);
      const type = document.createElement("span"); type.className = `kind ${d.kind}`;
      type.textContent = d.kind === "crash" ? "Crash" : "Error";
      tr.insertCell().append(type);
      cell(tr, info.title, "sig"); tr.cells[2].title = info.title;
      cell(tr, d.count, "num");
      cell(tr, rel(d.first_seen)); tr.cells[4].title = date(d.first_seen);
      cell(tr, rel(d.last_seen)); tr.cells[5].title = date(d.last_seen);
      cell(tr, d.app || "–", "mono"); tr.cells[6].title = [d.os, d.model].filter(Boolean).join(" · ");
      if (d.done) tr.classList.add("done");
      tr.classList.add("opens");
      const more = body.insertRow(), td = more.insertCell(), pre = document.createElement("pre"), meta = document.createElement("p");
      more.className = "more"; td.colSpan = 7;
      meta.className = "cap"; meta.textContent = `${d.app || "?"} · ${d.os || "?"} · ${d.model || "?"} · first ${date(d.first_seen)} · last ${date(d.last_seen)}`;
      pre.textContent = info.text;
      td.append(meta, pre);
      more.hidden = !diagOpen.has(d.id);
      tr.addEventListener("click", (e) => {
        if (e.target.closest("input")) return;
        more.hidden = !more.hidden;
        (more.hidden ? diagOpen.delete(d.id) : diagOpen.add(d.id));
      });
    }
  };

  // Every table of that hub's database, as it is (long blobs by their length): the names on the left, one table on the right.
  // Grouped by topic, so Yo's core message traffic stands apart from the rest; unknown tables land under "Other".
  const TABLE_GROUPS = [
    ["Accounts & Identity", ["users", "handles", "reserved", "gone", "account_keys", "devices", "contact_keys", "profile_commitments"]],
    ["Messaging (core)", ["messages", "prekeys", "blobs", "blob_quota", "zk_groups"]],
    ["Account Data", ["storage", "backups"]],
    ["Team & Support", ["support_messages", "diagnostics", "diagnostic_seen", "diagnostic_days", "reports", "feedback"]],
    ["Internal", ["settings", "deliveries_hourly", "_litestream_lock", "_litestream_seq"]],
  ];
  const tableGroups = (list) => {
    const byName = new Map(list.map((t) => [t.name, t])), known = new Set();
    const groups = TABLE_GROUPS.map(([title, names]) => {
      names.forEach((n) => known.add(n));
      return [title, names.filter((n) => byName.has(n)).map((n) => byName.get(n))];
    });
    groups.push(["Other", list.filter((t) => !known.has(t.name))]);
    return groups.filter(([, ts]) => ts.length);
  };
  let tableOpen = null;
  const tables = (h) => {
    const box = $("tables").querySelector(".tables"), note = $("tables").querySelector("h2 small");
    const list = (h && h.tables) || [];
    note.textContent = `${list.length}`;
    const groups = tableGroups(list);
    if (!list.some((t) => t.name === tableOpen)) tableOpen = groups.length ? groups[0][1][0].name : null;
    const side = document.createElement("nav"), pane = document.createElement("div");
    side.className = "tside"; pane.className = "tpane scroll";
    for (const [title, ts] of groups) {
      const head = document.createElement("h3");
      head.textContent = title;
      side.append(head);
      for (const t of ts) {
        const b = document.createElement("button");
        b.type = "button";
        b.className = t.name === tableOpen ? "on" : "";
        const name = document.createElement("span"), count = document.createElement("small");
        name.textContent = t.name; count.textContent = t.count;
        b.append(name, count);
        b.addEventListener("click", () => { tableOpen = t.name; tables(h); });
        side.append(b);
      }
    }
    const t = list.find((x) => x.name === tableOpen);
    if (t) {
      const head = document.createElement("p");
      head.className = "cap";
      head.textContent = `${t.name} · ${t.count} rows${t.count > t.rows.length ? ` (first ${t.rows.length})` : ""}`;
      const table = document.createElement("table");
      const hr = table.createTHead().insertRow();
      for (const c of t.columns) { const th = document.createElement("th"); th.textContent = c; hr.append(th); }
      const body = table.createTBody();
      for (const row of t.rows) {
        const tr = body.insertRow();
        for (const v of row) cell(tr, v == null ? "–" : String(v), "mono");
      }
      pane.append(head, table);
    }
    const keep = box.querySelector(".tpane");
    const scroll = keep ? [keep.scrollLeft, keep.scrollTop] : [0, 0];
    box.replaceChildren(side, pane);
    if (keep && t && keep.dataset.name === t.name) { pane.scrollLeft = scroll[0]; pane.scrollTop = scroll[1]; }
    if (t) pane.dataset.name = t.name;
  };

  // An app string "ios/202611151343" against the hub's YO_MIN_APP: "new" (that build or newer), "old" (update screen), null.
  const build = (app) => (app || "").split("/").pop();
  // YoGPT's own device counts against the YoGPT build on the server (h.yogpt_min), the apps against YO_MIN_APP.
  let yogptMin = null;
  const appState = (app, min) => {
    if (app && app.startsWith("yogpt/")) min = yogptMin;
    if (!app || !min) return null;
    const [p, b] = app.split("/"), [mp, mb] = min.split("/");
    if (p !== mp || !b || !mb) return null;
    return Number(b) >= Number(mb) ? "new" : "old";
  };
  const ago = (ms) => span((Date.now() - ms) / 1000);

  // What an operator wants at a glance (no content, nothing new tracked): from what the hub's database has anyway.
  const stats = (h, done) => {
    const ok = h && !h.error;
    const n = (v) => (ok && v != null ? v : "–");
    const su = (ok && h.signups) || [];
    set("st-signups", su.length ? su.reduce((a, b) => a + b, 0) : "–");
    // Deliveries per day: the hub keeps no such counter yet (messages holds only the waiting ones).
    const md = ok && h.deliveries;
    set("st-msgs", md ? md.day ?? "—" : "—");
    const bars = $("st-signup-bars"), top = Math.max(1, ...su);
    bars.replaceChildren(...su.map((v, i) => {
      const b = document.createElement("span");
      b.style.height = `${Math.max(4, (v / top) * 100)}%`;
      b.title = `${new Date(Date.now() - (su.length - 1 - i) * 86400000).toLocaleDateString()}: ${v}`;
      if (!v) b.className = "zero";
      return b;
    }));
    const blobs = ok && h.blobs;
    set("q-line", ok ? `queue: ${n(h.messages)} waiting · ${blobs ? `${blobs[0]} blobs, ${bytes(blobs[1])}` : "–"}` : "–");
    const apps = (ok && h.apps) || [];
    let fresh = 0, old = 0, other = 0;
    for (const [app, c] of apps) { const a = appState(app, h.min_app); if (a === "new") fresh += c; else if (a === "old") old += c; else other += c; }
    // Two small graphs above the users table: active users, and app versions (newest green, update yellow).
    const seg = (el, parts) => el.replaceChildren(...parts.filter(([v]) => v > 0).map(([v, cls, title]) => {
      const s = document.createElement("span"); s.className = cls; s.style.flexGrow = v; s.title = title; return s;
    }));
    if (ok) {
      const total = h.total || 0, today = h.active_today || 0, week = Math.max(today, h.active_users || 0);
      set("ug-total", total);
      set("ug-today", `${today} today`);
      set("ug-cap", `${week} this week · ${n(h.secured)} secured · ${n(h.devices)} devices`);
      seg($("ug-active-bar"), [[today, "a1", `${today} today`], [week - today, "a2", `${week} this week`], [Math.max(0, total - week), "a0", `${total} users`]]);
      set("ug-fresh", `${fresh}/${fresh + old + other}`);
      set("ug-apps-cap", `${old ? `${old} need the update · ` : ""}min ${h.min_app ? build(h.min_app) : "not pinned"}${other ? ` · ${other} unknown` : ""}`);
      seg($("ug-apps-bar"), [[fresh, "new", `${fresh} newest`], [old, "old", `${old} need the update`], [other, "a0", `${other} unknown`]]);
    }
  };

  const render = (d, done) => {
    const h = d[which];
    yogptMin = h && h.yogpt_min;
    part(() => users("users", h));
    part(() => stats(h, done));
    part(() => feedbackTile(h));
    supportRefresh();
    part(() => diagnostics(h));
    part(() => reports("reports", h, done));
    part(() => tables(h));
  };

  let peopleBusy = false;
  async function people() {
    if (document.visibilityState !== "visible" || peopleBusy) return;
    peopleBusy = true;
    const status = $("collection-status");
    status.textContent = last ? "updating…" : "Loading fresh data…";
    try {
      const r = await fetch("/users.json", { cache: "no-store" });
      if (!r.ok) throw new Error("collector unavailable");
      last = await r.json();
      updateTile(last.updates);
      const doneResponse = await fetch("/done.json", { cache: "no-store" });
      lastDone = new Set(doneResponse.ok ? await doneResponse.json() : []);
      render(last, lastDone);
      status.textContent = last.stale ? "Stale data · waiting for collector" : "";
    } catch {
      status.textContent = last ? "Stale data · refresh failed" : "Waiting for fresh data…";
    } finally { peopleBusy = false; }
  }

  // Tabs at the top: Overview, Feedback, Crashes, Users, Database; the hash keeps the tab across reloads and links.
  let userTab = "users";
  try { if (localStorage.getItem("users-tab") === "reserved") userTab = "reserved"; } catch {}
  const usersLink = document.querySelector('#tabs [data-tab="users"]');
  usersLink.hash = userTab;
  for (const name of ["users", "reserved"]) {
    $(`${name}-tab`).onclick = () => { location.hash = name; };
    $(`${name}-tab`).onkeydown = e => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
      e.preventDefault();
      const next = e.key === "Home" ? "users" : e.key === "End" ? "reserved" : name === "users" ? "reserved" : "users";
      $(`${next}-tab`).focus();
      location.hash = next;
    };
  }
  const tab = () => {
    const h = location.hash.slice(1), t = h === "reserved" ? "users" : h === "diagnostics" ? "crashes" : ["db", "users", "feedback", "crashes"].includes(h) ? h : "overview";
    if (t === "users") {
      userTab = h;
      try { localStorage.setItem("users-tab", userTab); } catch {}
      usersLink.hash = userTab;
    }
    const reserved = t === "users" && userTab === "reserved";
    $("users-heading").hidden = t === "users";
    $("user-tabs").hidden = t !== "users";
    $("users-panel").hidden = reserved;
    $("reserved").hidden = !reserved;
    $("users-panel").setAttribute("role", t === "users" ? "tabpanel" : "presentation");
    if (t === "users") $("users-panel").setAttribute("aria-labelledby", "users-tab");
    else $("users-panel").removeAttribute("aria-labelledby");
    for (const name of ["users", "reserved"]) {
      const selected = name === userTab;
      $(`${name}-tab`).setAttribute("aria-selected", String(selected));
      $(`${name}-tab`).tabIndex = selected ? 0 : -1;
    }
    currentTab = t;
    document.body.dataset.tab = t;
    if (last) render(last, lastDone);
    for (const el of document.querySelectorAll("[data-tabs]")) el.hidden = !el.dataset.tabs.split(" ").includes(t);
    for (const a of document.querySelectorAll("#tabs a")) a.classList.toggle("on", a.dataset.tab === t);
    if (t === "feedback") supportRefresh();
  };
  window.addEventListener("hashchange", tab);
  // Keys 1-5: the tabs in order (not while typing, not with modifiers).
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey || e.target.closest("input, textarea, select, [contenteditable]")) return;
    const a = document.querySelectorAll("#tabs a")[Number(e.key) - 1];
    if (a && /^[1-5]$/.test(e.key)) location.hash = a.getAttribute("href").slice(1);
    if (e.key === "0") document.querySelector("#which button:not(.on)")?.click();
  });
  tab();

  tick();
  setInterval(tick, 3000);
  people();
  setInterval(people, 10000);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") people();
  });
}
