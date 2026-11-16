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
  let pictures = null;
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
      if (y && y.state === "active") $("t-yogpt").title += ` · ${pictures ?? "?"} pictures${people != null ? ` · knows ${people}` : ""} · click for the images`;
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
    note.textContent = list.length ? `${list.filter((r) => !done.has(r.key)).length} open · ${list.length}` : "none";

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
  let fbMode = "all";
  $("fb-mode").addEventListener("change", (e) => {
    fbMode = e.target.value;
    if (last) render(last, lastDone);
  });
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

  $("todo-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const input = $("todo-text"), button = e.target.querySelector("button"), error = $("todo-error");
    if (!input.value.trim()) { input.setCustomValidity("Enter a To-Do."); input.reportValidity(); return; }
    button.disabled = true; error.hidden = true;
    try {
      const r = await fetch("/todos", { method: "POST", body: new URLSearchParams({ hub: which, text: input.value }) });
      if (!r.ok) throw new Error(`Could not save (${r.status}).`);
      input.value = "";
      await people();
    } catch (e) { error.textContent = e.message; error.hidden = false; }
    finally { button.disabled = false; }
  });
  $("todo-text").addEventListener("input", (e) => e.target.setCustomValidity(""));

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

  const feedback = (h, done) => {
    const card = $("feedback"), body = card.querySelector("tbody"), note = card.querySelector("h2 small");
    set("todo-hub", which);
    const items = ((h && h.feedback) || [])
      .map((f) => ({ ...f, key: `${which}:${f.id}` }));
    items.sort((a, b) => (done.has(a.key) - done.has(b.key)) || (b.at - a.at));  // open ones first, then newest
    const openItems = items.filter((f) => !done.has(f.key));
    for (const option of $("fb-mode").options) {
      const mode = option.value;
      const count = mode === "reports" ? ((h && h.reports) || []).filter(r => !done.has(`${which}:r${r.id}`)).length
        : mode === "done" ? items.length - openItems.length : openItems.filter(f => mode === "all" || f.type === mode).length;
      option.textContent = `Show: ${{all: "All", feedback: "Feedback", crash: "Crash", todo: "To-Do", reports: "User Reports", done: "Done"}[mode]} (${count})`;
    }
    note.textContent = "";
    note.title = `${items.length} in all`;
    body.replaceChildren();
    card.querySelector(".scroll").hidden = fbMode === "reports";
    $("reports").hidden = fbMode !== "reports";
    // Ticked ones leave the lists; "Done" shows them (and unticking brings one back).
    const shown = fbMode === "done" ? items.filter((f) => done.has(f.key))
      : openItems.filter((f) => fbMode === "all" || f.type === fbMode);
    for (const f of shown) {
      const tr = body.insertRow(), hub = which;
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = done.has(f.key);
      box.setAttribute("aria-label", "Mark item done");
      box.addEventListener("change", async () => {
        await fetch("/feedback/done", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: `hub=${hub}&id=${f.id}&done=${box.checked ? 1 : 0}` });
        people();
      });
      tr.insertCell().append(box);
      const type = { feedback: "Feedback", crash: "Crash", todo: "To-Do" }[f.type];
      const badge = document.createElement("span");
      badge.className = `feedback-type ${f.type}`;
      badge.textContent = type;
      tr.insertCell().append(badge);
      if (f.type === "todo") tr.classList.add("todo");
      cell(tr, rel(f.at)); tr.cells[tr.cells.length - 1].title = date(f.at);
      const from = f.type === "crash" ? "anonymous" : (f.name ?? "admin");
      cell(tr, from, f.type === "crash" || !f.name ? "muted" : "name");
      const td = tr.insertCell();
      // A crash report sent without an account (CrashReports.swift).
      const preview = document.createElement("span");
      preview.className = "text-preview";
      preview.textContent = f.text || "";
      td.append(preview);
      if (f.type === "crash") td.classList.add("crash");
      rowOpens(tr, () => popup([["Type", type], ["Hub", hub], ["From", from], ["When", date(f.at)], ["Version", f.version]], f.text, f.type === "crash", box));
      td.classList.add("text");
      if (done.has(f.key)) tr.classList.add("done");
    }
  };

  // Every table of that hub's database, as it is (long blobs by their length): the names on the left, one table on the right.
  let tableOpen = null;
  const tables = (h) => {
    const box = $("tables").querySelector(".tables"), note = $("tables").querySelector("h2 small");
    const list = (h && h.tables) || [];
    note.textContent = `${list.length}`;
    if (!list.some((t) => t.name === tableOpen)) tableOpen = list[0] ? list[0].name : null;
    const side = document.createElement("nav"), pane = document.createElement("div");
    side.className = "tside"; pane.className = "tpane scroll";
    for (const t of list) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = t.name === tableOpen ? "on" : "";
      const name = document.createElement("span"), count = document.createElement("small");
      name.textContent = t.name; count.textContent = t.count;
      b.append(name, count);
      b.addEventListener("click", () => { tableOpen = t.name; tables(h); });
      side.append(b);
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
    set("q-line", ok ? `${n(h.messages)} / ${blobs ? bytes(blobs[1]) : "–"}` : "–");
    $("q-line").parentElement.title = blobs ? `${n(h.messages)} deliveries waiting · ${blobs[0]} blobs · ${bytes(blobs[1])}` : "";
    const fb = (ok && h.feedback) || [], open = fb.filter((f) => !done.has(`${which}:${f.id}`));
    const rp = ((ok && h.reports) || []).filter((r) => !done.has(`${which}:r${r.id}`));
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
      seg($("ug-active-bar"), [[today, "a1", `${today} today`], [week - today, "a2", `${week} this week`], [Math.max(0, total - week), "a0", `${total} users`]]);
      $("ug-users").title = `${total} users · ${today} today · ${week} this week · ${n(h.secured)} secured · ${n(h.devices)} devices`;
      set("ug-fresh", `${fresh} / ${fresh + old + other}`);
      seg($("ug-apps-bar"), [[fresh, "new", `${fresh} newest`], [old, "old", `${old} need the update`], [other, "a0", `${other} unknown`]]);
      $("ug-apps").title = `min ${h.min_app ? build(h.min_app) : "not pinned"}${other ? ` · ${other} unknown` : ""}\n${n(h.devices)} devices: ${apps.map(([a, c]) => `${build(a) || a} ${c}`).join(" · ")}`;
    }
  };

  const render = (d, done) => {
    const h = d[which];
    yogptMin = h && h.yogpt_min;
    part(() => users("users", h));
    part(() => stats(h, done));
    part(() => feedback(h, done));
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

  // Every picture YoGPT can send, with its number and idea; in place of the lower half.
  let galleryLoaded = false;
  const gallery = async (show) => {
    if (!show || galleryLoaded) return;
    const note = $("gallery").querySelector("h2 small"), box = $("gallery").querySelector(".shots");
    try {
      const r = await fetch("/yogpt.json", { cache: "no-store" });
      if (!r.ok) { note.textContent = `error ${r.status}`; return; }
      const list = await r.json();
      galleryLoaded = true;
      pictures = list.length;
      note.textContent = `${list.length}`;
      box.replaceChildren();
      for (const p of list) {
        const fig = document.createElement("figure");
        const a = document.createElement("a");
        a.href = `/yogpt/${p.name}?v=${p.v}`;
        a.target = "_blank";
        a.rel = "noopener";
        const img = document.createElement("img");
        img.src = a.href;
        img.loading = "lazy";
        img.alt = p.idea || p.name;
        img.width = 720; img.height = 1280;
        a.append(img);
        const cap = document.createElement("figcaption");
        const b = document.createElement("b");
        b.textContent = String(p.number).padStart(3, "0");
        cap.append(b, document.createTextNode(p.idea ? ` ${p.idea}` : ""));
        fig.append(a, cap);
        box.append(fig);
      }
    } catch { note.textContent = "no answer from the server"; }
  };
  // Tabs at the top: Overview, GPT Images (#gpt), Database (#db); the hash keeps the tab across reloads and links.
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
    const h = location.hash.slice(1), t = h === "gpt" || h === "yogpt" ? "gpt" : h === "reserved" ? "users" : h === "db" || h === "users" ? h : "overview";
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
    gallery(t === "gpt");
  };
  window.addEventListener("hashchange", tab);
  // Keys 1-4: the tabs in order (not while typing, not with modifiers).
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey || e.target.closest("input, textarea, select, [contenteditable]")) return;
    const a = document.querySelectorAll("#tabs a")[Number(e.key) - 1];
    if (a && /^[1-4]$/.test(e.key)) location.hash = a.getAttribute("href").slice(1);
    if (e.key === "0") document.querySelector("#which button:not(.on)")?.click();
  });
  const yogptTile = $("t-yogpt");
  yogptTile.addEventListener("click", () => { location.hash = "gpt"; });
  yogptTile.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); location.hash = "gpt"; } });
  tab();
  fetch("/yogpt.json", { cache: "no-store" }).then((r) => (r.ok ? r.json() : null)).then((l) => { if (l) pictures = l.length; }).catch(() => {});

  tick();
  setInterval(tick, 3000);
  people();
  setInterval(people, 10000);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") people();
  });
}
