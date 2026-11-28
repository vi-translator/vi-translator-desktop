const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let cfg = null;
let frameSize = { w: 0, h: 0 }; // captured window size (physical px)
let shown = { w: 0, h: 0 }; // preview image size on screen (CSS px)
let selected = null; // selected area id
const lastSegs = {}; // area id -> latest Arabic segments
const lastLineH = {}; // area id -> English line height (fraction of the frame), for size matching
const lastTextBox = {}; // area id -> English text block [x, y, w, h] (fractions of the frame)
const lastGameBox = {}; // area id -> the game's own box around it, when found (same units)
const lastSeen = {}; // area id -> the game's text seen there, not translated yet ({text_box, game_box, line_h})

// ------------------------------------------------------------------------------------- saving

let saveTimer = null;
function save(redraw = true) {
  syncProfile();
  if (redraw) drawLayer();
  updateGlance();
  clearTimeout(saveTimer);
  $("saved").textContent = "…";
  saveTimer = setTimeout(async () => {
    try {
      await invoke("save_config", { config: cfg });
      $("saved").textContent = t("Saved ✓");
    } catch (e) {
      $("saved").textContent = t("Save failed");
      log({ error: String(e) });
    }
  }, 350);
}

// ------------------------------------------------------------------------------------- tabs

function openTab(name) {
  document.querySelectorAll(".tabs button").forEach((x) => x.classList.toggle("active", x.dataset.tab === name));
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("hidden", t.id !== `tab-${name}`));
  document.body.dataset.tab = name;
  editingChanged();
  if (name === "show" && cfg) { updateLan(); updateTvs(); } // (at once, not at the next 3-second refresh)
  // (the look previews are measured, so drawn once their tab shows: the language or style may have changed)
  if (name === "profile" && cfg) drawLookPreviews($("profile-form"));
}

// The read areas are edited on the preview only in Setup with the Read areas tab open: their boxes,
// the translation regions' handles, the sample text and the bar above the preview show then only.
const editingAreas = () => document.body.dataset.mode === "setup" && document.body.dataset.tab === "areas";
function editingChanged() {
  document.body.classList.toggle("editing-areas", editingAreas());
  drawLayer();
  requestAnimationFrame(fitFrame); // (the bar above the preview came or went)
}
document.querySelectorAll(".tabs button").forEach((b) => b.addEventListener("click", () => openTab(b.dataset.tab)));

// ------------------------------------------------------------------------------------- Play / Setup

// Play: the live translations big, a small preview and what is running. Setup: the area editor
// and every setting. Remembered on this PC; the first start opens Setup.
function setMode(mode) {
  document.body.dataset.mode = mode;
  document.querySelectorAll(".modes button").forEach((b) => {
    b.classList.toggle("active", b.dataset.mode === mode);
    b.setAttribute("aria-selected", b.dataset.mode === mode);
  });
  try { localStorage.setItem("mode", mode); } catch {}
  if (mode === "play") updateGlance();
  editingChanged();
}
document.querySelectorAll(".modes button").forEach((b) => b.addEventListener("click", () => setMode(b.dataset.mode)));
// "Change" on a Play card: open that part of Setup
document.querySelectorAll("[data-go]").forEach((b) => b.addEventListener("click", () => { setMode("setup"); openTab(b.dataset.go); }));

// Play: the preview full screen — the window goes full screen and the picture fills it (its own
// shape: bars only where the screen's shape differs). From the button over the picture, F11 (from
// Setup too: it opens Play), or a double-click on the picture; back with the button, Esc or F11. The
// mouse pointer and the buttons go after a moment without moving the mouse, and come back with it.
const isFullscreen = () => document.documentElement.classList.contains("fullscreen");
// (the window really went full screen: then the window leaving full screen by itself — macOS's green
// button, a Windows shortcut — ends it here too. Where it can't, the picture still fills the window.)
let osFullscreen = false;
let fsSince = 0;
async function setFullscreen(on) {
  fsSince = performance.now();
  if (on && document.body.dataset.mode !== "play") setMode("play"); // (the full-screen picture is Play's)
  document.documentElement.classList.toggle("fullscreen", on);
  $("fullscreen").title = on ? t("Exit full screen (Esc)") : t("Full screen");
  osFullscreen = false;
  try {
    await window.__TAURI__.window.getCurrentWindow().setFullscreen(on);
    osFullscreen = on;
  } catch (e) {
    log({ error: String(e) });
  }
  fsActive();
  requestAnimationFrame(fitFrame); // (the viewport's ResizeObserver fits it again once the size settles)
}
$("fullscreen").addEventListener("click", () => setFullscreen(!isFullscreen()));
$("viewport").addEventListener("dblclick", (e) => {
  if (document.body.dataset.mode === "play" && !e.target.closest(".fs-btn")) setFullscreen(!isFullscreen());
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && isFullscreen()) setFullscreen(false);
  else if (e.key === "F11" && !e.repeat) {
    e.preventDefault();
    setFullscreen(!isFullscreen());
  }
});
// The window left full screen by itself: the picture goes back into the page too. (Not while it is
// still going full screen: macOS animates into it, and says "not full screen" meanwhile.)
window.addEventListener("resize", async () => {
  if (!osFullscreen || !isFullscreen() || performance.now() - fsSince < 2000) return;
  try {
    if (!(await window.__TAURI__.window.getCurrentWindow().isFullscreen()) && osFullscreen) {
      osFullscreen = false;
      setFullscreen(false);
    }
  } catch {}
});
// In full screen: the pointer and the buttons hidden after 2.5 s without the mouse moving (not while
// it is over a button).
let fsIdleTimer = 0;
function fsActive() {
  document.documentElement.classList.remove("fs-idle");
  clearTimeout(fsIdleTimer);
  if (!isFullscreen()) return;
  fsIdleTimer = setTimeout(() => {
    if (isFullscreen() && !document.querySelector(".fs-btn:hover")) document.documentElement.classList.add("fs-idle");
    else fsActive();
  }, 2500);
}
for (const ev of ["mousemove", "mousedown", "wheel"]) document.addEventListener(ev, () => { if (isFullscreen()) fsActive(); }, { passive: true });

/** The Play cards: source, language + service, read areas. */
function updateGlance() {
  if (!cfg) return;
  const src = $("window_title");
  $("ps-source").textContent = src.selectedOptions[0]?.textContent || cfg.window_title || "—";
  const l = LANGS.find((x) => x.code === cfg.target_lang);
  $("ps-lang").textContent = l ? `${l.name} — ${l.native}` : cfg.target_lang;
  const e = ENGINES.find((x) => x.id === cfg.engine);
  $("ps-engine").textContent = e ? t(e.name) : "";
  const on = cfg.areas.filter((a) => a.enabled);
  $("ps-areas").textContent = on.map((a) => a.name).join(", ") || "—";
  $("ps-profile").textContent = activeProfile()?.name || "";
}

// ------------------------------------------------------------------------------------- preview

function fitFrame() {
  if (!frameSize.w) return;
  const vp = $("viewport").getBoundingClientRect();
  // a small margin in the window; none in full screen, where the picture fills the screen
  const m = isFullscreen() ? 0 : 16;
  const s = Math.min((vp.width - m) / frameSize.w, (vp.height - m) / frameSize.h);
  shown = { w: Math.floor(frameSize.w * s), h: Math.floor(frameSize.h * s) };
  for (const el of [$("frame"), $("frame-gl")]) {
    el.style.width = `${shown.w}px`;
    el.style.height = `${shown.h}px`;
  }
  drawLayer();
}
window.addEventListener("resize", fitFrame);
// (and whenever the picture's place changes size: full screen settling, Play and Setup, the log's split)
new ResizeObserver(() => fitFrame()).observe($("viewport"));

function sampleSegments() {
  // "*word*" in the sample text shows as a highlighted word
  return $("sample-text")
    .value.split(/\*(.+?)\*/)
    .map((t, i) => [t, i % 2 ? "#5db6e5" : null])
    .filter((s) => s[0]);
}

// Game page currently on screen (a menu, a "mission failed" page …) and its translated reason line.
const screen = { layout: null, reason: [] };

function drawLayer() {
  const layer = $("layer");
  layer.innerHTML = "";
  const sl = $("screen-layer");
  if (!cfg || !shown.w) { sl.innerHTML = ""; return; }
  // Overlays off (header switch): no translations on the game, so none on its picture here either
  // (the regions stay in Setup, to place them).
  const overlays = cfg.overlays_enabled !== false;
  // The profile page replaces the normal subtitles while it is up, exactly like the real overlay.
  AS.renderScreen(sl, overlays ? screen.layout : null, screen.reason, cfg.profile_opts);
  if (screen.layout) {
    for (const a of cfg.areas) layer.appendChild(areaBox(a));
    return;
  }
  const k = shown.h / 1080;
  if (overlays && phoneNow) {
    const ph = document.createElement("div");
    Object.assign(ph.style, { position: "absolute", left: "0", top: "0", width: `${shown.w}px`, height: `${shown.h}px`, pointerEvents: "none" });
    layer.appendChild(ph);
    AS.renderPhone(ph, phoneNow);
  }
  // Every area's translation now shown, for the ones that ride above stacked lines (AS.ridePlace),
  // and a hidden box to measure them in.
  const shownNow = {};
  for (const a of cfg.areas) {
    if (overlays && lastSegs[a.id]?.length) shownNow[a.id] = { segments: lastSegs[a.id], text_box: lastTextBox[a.id], game_box: lastGameBox[a.id], line_h: lastLineH[a.id] };
  }
  const probe = document.createElement("div");
  layer.appendChild(probe);
  for (const a of cfg.areas) {
    const px = { x: a.x * shown.w, y: a.y * shown.h, w: a.w * shown.w, h: a.h * shown.h };
    // Arabic region (same placement maths as the real overlay). Drag it to place the Arabic anywhere.
    if (a.enabled) {
      const segs = !overlays ? [] : lastSegs[a.id]?.length ? lastSegs[a.id] : $("show-sample").checked && editingAreas() ? sampleSegments() : [];
      const r = AS.rectFor(a.placement, px, shown.w, shown.h, a);
      const band = document.createElement("div");
      band.className = `band${a.id === selected ? " selected" : ""}`;
      band.title = t("Drag to move the translation, drag the corners to resize its region");
      Object.assign(band.style, { left: `${r.x}px`, top: `${r.y}px`, width: `${r.w}px`, height: `${r.h}px` });
      const box = document.createElement("div");
      band.appendChild(box);
      AS.anchor(band, a.placement, a);
      const seen = overlays ? lastSeen[a.id] || null : null;
      if (shownNow[a.id] || seen) {
        // A real line: drawn as the overlays draw it (the profile's look, riding above the stacked lines,
        // the black box over text seen and not translated yet)
        const ride = AS.ridePlace(a, cfg.areas, cfg.fx_view || cfg.area_fx, cfg.profile_opts, shownNow, overlays ? lastSeen : {}, shown.w, shown.h,
          (f, e) => AS.measureBox(probe, f, cfg.style, cfg.profile_opts, e, k, shown.h, a.w * shown.w));
        AS.drawLine(box, a, cfg.style, cfg.profile_opts, cfg.fx_view || cfg.area_fx, shownNow[a.id] || null, seen, k, shown.h,
          { W: shown.w, H: shown.h, ox: r.x, oy: r.y, areaW: px.w, areaH: px.h, ride });
      } else {
        // the sample text (no English line to match: the style's size), in the area's own place
        const L = AS.look(a, AS.styleOf(a, cfg.style), cfg.profile_opts, null);
        AS.renderBox(box, segs, L.style, k, a.font_scale, L.placement, L.area, 0);
        if (AS.atText(L.placement)) AS.placeAtText(box, L.placement, null, 0, shown.w, shown.h, r.x, r.y, px.w, px.h, null);
      }
      band.insertAdjacentHTML("beforeend", `<div class="tag">${AS.esc(t("Translation · {name}", { name: a.name }))}</div>`);
      addHandles(band, a, "out");
      band.addEventListener("mousedown", (e) => startDrag(e, a, "move", "out"));
      layer.appendChild(band);
    }
    layer.appendChild(areaBox(a));
  }
}

// The yellow read-area rectangle (draggable).
function areaBox(a) {
  const el = document.createElement("div");
  el.className = `area${a.id === selected ? " selected" : ""}${a.enabled ? "" : " disabled"}`;
  Object.assign(el.style, { left: `${a.x * shown.w}px`, top: `${a.y * shown.h}px`, width: `${a.w * shown.w}px`, height: `${a.h * shown.h}px` });
  el.innerHTML = `<div class="tag">${AS.esc(a.enabled ? a.name : t("{name} (off)", { name: a.name }))}</div>`;
  addHandles(el, a, "read");
  el.addEventListener("mousedown", (e) => startDrag(e, a, "move", "read"));
  return el;
}

function addHandles(el, area, kind) {
  // corners resize both ways; the middle of each side resizes only the height (top, bottom) or
  // only the width (left, right)
  for (const c of ["nw", "ne", "sw", "se", "n", "s", "e", "w"]) {
    const h = document.createElement("div");
    h.className = `handle ${c}`;
    h.addEventListener("mousedown", (e) => startDrag(e, area, c, kind));
    el.appendChild(h);
  }
}

/**
 * A yes / no question in the page (window.confirm does nothing in the macOS webview): resolves true
 * when `okLabel` is clicked, false on Cancel or Esc.
 */
function ask(text, okLabel) {
  return new Promise((resolve) => {
    const d = document.createElement("dialog");
    d.className = "ask";
    d.innerHTML = `<p></p><div class="row"><button class="btn" data-no></button><button class="btn danger" data-ok></button></div>`;
    d.querySelector("p").textContent = text;
    d.querySelector("[data-ok]").textContent = okLabel;
    d.querySelector("[data-no]").textContent = t("Cancel");
    const done = (v) => { d.close(); d.remove(); resolve(v); };
    d.querySelector("[data-ok]").addEventListener("click", () => done(true));
    d.querySelector("[data-no]").addEventListener("click", () => done(false));
    d.addEventListener("cancel", (e) => { e.preventDefault(); done(false); }); // Esc
    document.body.appendChild(d);
    d.showModal();
    d.querySelector("[data-no]").focus();
  });
}

// Turn a preset placement into a custom region at the same spot, so dragging starts from where it is.
function toCustom(area) {
  if (area.placement === "custom") return;
  const W = 1920, H = 1080; // fractions don't depend on the size used here
  const px = { x: area.x * W, y: area.y * H, w: area.w * W, h: area.h * H };
  const r = AS.rectFor(area.placement, px, W, H, area);
  const al = AS.alignOf(area.placement, area);
  Object.assign(area, {
    out_x: round(r.x / W), out_y: round(r.y / H), out_w: round(r.w / W), out_h: round(r.h / H),
    out_valign: al.v, out_halign: al.h, placement: "custom",
  });
}

// kind "read" moves the read area; kind "out" moves the Arabic region (switching it to a custom position).
function startDrag(e, area, mode, kind = "read") {
  e.preventDefault();
  e.stopPropagation();
  if (locked()) return; // a built-in profile: copy it to move its areas
  selected = area.id;
  renderAreas();
  const K = kind === "out" ? { x: "out_x", y: "out_y", w: "out_w", h: "out_h" } : { x: "x", y: "y", w: "w", h: "h" };
  const start = { mx: e.clientX, my: e.clientY, x: area[K.x], y: area[K.y], w: area[K.w], h: area[K.h] };
  const min = 0.02;
  let raf = 0;
  let moved = false;
  const move = (ev) => {
    if (!moved) {
      // (a plain click only selects; a translation region turns into a custom one when it's dragged)
      if (Math.abs(ev.clientX - start.mx) + Math.abs(ev.clientY - start.my) < 4) return;
      moved = true;
      if (kind === "out") {
        toCustom(area);
        Object.assign(start, { x: area[K.x], y: area[K.y], w: area[K.w], h: area[K.h] });
        renderAreas();
      }
    }
    const dx = (ev.clientX - start.mx) / shown.w;
    const dy = (ev.clientY - start.my) / shown.h;
    let { x, y, w, h } = start;
    if (mode === "move") {
      x = Math.min(Math.max(start.x + dx, 0), 1 - w);
      y = Math.min(Math.max(start.y + dy, 0), 1 - h);
    } else {
      if (mode.includes("w")) { x = Math.min(Math.max(start.x + dx, 0), start.x + start.w - min); w = start.x + start.w - x; }
      if (mode.includes("e")) { w = Math.min(Math.max(start.w + dx, min), 1 - start.x); }
      if (mode.includes("n")) { y = Math.min(Math.max(start.y + dy, 0), start.y + start.h - min); h = start.y + start.h - y; }
      if (mode.includes("s")) { h = Math.min(Math.max(start.h + dy, min), 1 - start.y); }
    }
    Object.assign(area, { [K.x]: round(x), [K.y]: round(y), [K.w]: round(w), [K.h]: round(h) });
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(() => { drawLayer(); updateAreaNumbers(area); });
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    if (moved) save();
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}
const round = (v) => Math.round(v * 10000) / 10000;

$("show-sample").addEventListener("change", drawLayer);
$("sample-text").addEventListener("input", () => drawLayer());

// ------------------------------------------------------------------------------------- source

// Source values: a window title, "device:<name>" (capture card) or "obs:<source name>".
async function loadWindows() {
  const sel = $("window_title");
  sel.innerHTML = "";
  sel.add(new Option(t("Looking for sources ..."), cfg.window_title));
  const { windows, devices, obs, screens = [], xbox = null, xbox_native = null, ps5 = null } = await invoke("list_sources");
  knownScreens = screens;
  xboxTitle = xbox;
  xboxValue = xbox_native;
  sel.innerHTML = "";
  const group = (label) => {
    const g = document.createElement("optgroup");
    g.label = label;
    sel.appendChild(g);
    return g;
  };
  const values = [];
  // The open list is as wide as its longest line: long window titles (a browser tab, a file path)
  // are shortened in the middle, and the whole text is kept as the tooltip.
  const short = (s, n = 46) => (s.length > n ? `${s.slice(0, n - 16).trimEnd()}…${s.slice(-15).trimStart()}` : s);
  // The list follows the interface's direction; each name (usually English) is isolated so it keeps
  // its own left-to-right order inside a right-to-left list.
  const iso = (s) => `⁨${s}⁩`;
  const opt = (text, value, full = text) => Object.assign(new Option(text, value), { title: full });
  const add = (g, text, value, full) => { g.appendChild(opt(text, value, full)); values.push(value); };
  // remote play in a window of the app (its window's title is the source)
  ps5Value = ps5;
  if (xbox || xbox_native || ps5) {
    const g = group(t("Remote Play in VI Translator"));
    if (ps5) add(g, `🎮 ${t("PS5 (in VI Translator)")}`, ps5);
    // the app's own Xbox player, and Microsoft's web player in a window of the app
    if (xbox_native) add(g, `🎮 ${t("Xbox (in VI Translator)")}`, xbox_native);
    if (xbox) add(g, `🎮 ${t("Xbox (Microsoft's web player)")}`, xbox);
  }
  if (devices.length) {
    const g = group(t("Capture cards and cameras"));
    for (const d of devices) add(g, `🎥 ${iso(short(d))}`, `device:${d}`, d);
  }
  const st = $("obs-status");
  st.textContent = obs.ok ? t("Connected to OBS — {n} source(s). Pick one marked 📺 above.", { n: obs.sources.length }) : t("Not connected: {error}", { error: obs.error });
  st.style.color = obs.ok ? "var(--ok, #1a7f37)" : "var(--warn, #b45309)";
  // the Browser Source's width and height: OBS's canvas size, read from OBS
  const cv = $("obs-canvas");
  if (obs.ok && obs.canvas) {
    cv.innerHTML = `<span>${AS.esc(t("Your OBS canvas (width × height):"))}</span>
      <b dir="ltr">${obs.canvas.w} × ${obs.canvas.h}</b>
      <span class="muted small">${AS.esc(t("Set in OBS: Settings → Video → Base (Canvas) Resolution"))}</span>`;
  } else {
    cv.innerHTML = `<span class="muted small">${AS.esc(t("Connect to OBS in the Source tab to see your canvas size here."))}</span>`;
  }
  const og = group(obs.ok ? t("OBS sources") : t("OBS sources (not connected)"));
  if (obs.ok) for (const s of obs.sources) add(og, `📺 ${iso(`${short(s.name, 36)}  —  ${s.kind}`)}`, `obs:${s.name}`, `${s.name}  —  ${s.kind}`);
  else og.appendChild(Object.assign(opt(short(String(obs.error), 56), "", obs.error), { disabled: true }));
  if (screens.length) {
    const sg = group(t("Screens"));
    for (const s of screens) add(sg, `🖥 ${iso(screenLabel(s))}`, `screen:${s.id}`, screenLabel(s));
  }
  const wg = group(t("Windows"));
  for (const w of windows) {
    const full = w.process ? `${w.title}  —  ${w.process}` : w.title;
    add(wg, iso(w.process ? `${short(w.title)}  —  ${short(w.process, 24)}` : short(w.title)), w.title, full);
  }
  if (cfg.window_title && !values.includes(cfg.window_title)) {
    sel.insertBefore(opt(t("{title}  (not available now)", { title: iso(short(cfg.window_title)) }), cfg.window_title,
                         t("{title}  (not available now)", { title: cfg.window_title })), sel.firstChild);
  }
  sel.value = cfg.window_title;
  renderXbox();
  renderPs5();
}
// Xbox in the app, two ways: the app's own player ("xbox:", drawn below) or Microsoft's web player,
// whose source is its window, opened from here (and with the app)
let xboxTitle = null;
const renderXbox = (v = cfg.window_title) => { $("xbox-row").hidden = !xboxTitle || v !== xboxTitle; renderXboxNative(v); };
const openXbox = () => invoke("open_xbox").catch((e) => { $("source-status").textContent = String(e); });
$("xbox-open").addEventListener("click", openXbox);
$("window_title").addEventListener("change", (e) => {
  if (xboxTitle && e.target.value === xboxTitle) openXbox();
  renderXbox(e.target.value);
  renderPs5(e.target.value);
});

// Xbox with the app's own player, as the phone apps have it: sign in with a code typed at
// microsoft.com/link (no password in the app), choose the console once (it's remembered), then the
// console, with its sound, controllers and the stream's quality. Microsoft's web player stays as the
// other choice, at the top of the panel.
let xboxValue = null;
let xbox = null;
const xboxUi = { consoles: null, searching: false, msg: "", changing: false };

async function renderXboxNative(v = cfg.window_title) {
  const native = !!xboxValue && v === xboxValue;
  const web = !!xboxTitle && v === xboxTitle;
  $("xbox-panel").hidden = !native && !web;
  if (!native && !web) return;
  if (native) { try { xbox = await invoke("xbox_status"); } catch { return; } }
  drawXbox(native);
}
listen("xbox", (e) => {
  xbox = e.payload;
  if (!$("xbox-panel").hidden && xboxValue && cfg.window_title === xboxValue) drawXbox(true);
});

// the source switched to the other Xbox player (as choosing it in the list does)
function xboxUse(v) {
  if (!v || cfg.window_title === v) return;
  const sel = $("window_title");
  sel.value = v;
  sel.dispatchEvent(new Event("change"));
}

function drawXbox(native) {
  const box = $("xbox-panel");
  box.innerHTML = "";
  const pick = (v, label, on) => el("button", { class: `seg${on ? " on" : ""}`, "aria-pressed": on ? "true" : "false", onclick: () => xboxUse(v) }, label);
  box.append(el("div", { class: "field" }, el("label", {}, t("Xbox player")),
    el("div", { class: "segs" },
      xboxValue ? pick(xboxValue, t("VI Translator's player (recommended)"), native) : null,
      xboxTitle ? pick(xboxTitle, t("Microsoft's web player"), !native) : null)));
  if (!native || !xbox) return;
  const s = xbox;
  const err = s.error ? el("p", { class: "small ps5-err" }, I18N.tMsg(s.error)) : null;
  if (!s.signed_in) {
    box.append(ps5Step(1, t("Sign in to Microsoft"), false, true, () => xboxCodeStep(s, err)),
      ps5Step(2, t("Choose your Xbox"), false, false, () => null), xboxOnce());
    return;
  }
  if (!s.console || xboxUi.changing) {
    if (xboxUi.consoles == null && !xboxUi.searching) xboxSearch();
    box.append(xboxAccountStep(s), ps5Step(2, t("Choose your Xbox"), false, true, () => xboxConsoleStep(err)), xboxOnce());
    return;
  }
  box.append(xboxChosen(s, err));
}

/** What to set on the Xbox once, before it can be found and woken (as the phone apps show it). */
function xboxOnce() {
  return el("div", { class: "callout xbox-once" },
    el("b", {}, t("On the Xbox, once")),
    el("ul", {},
      el("li", {}, t("Settings › Devices & connections › Remote features: turn on Enable remote features.")),
      el("li", {}, t("Settings › General › Power options: choose Sleep, so the player can wake the console.")),
      el("li", {}, t("Sign in on the Xbox with the same Microsoft account as in the player."))));
}

function xboxAccountStep(s) {
  return ps5Step(1, t("Sign in to Microsoft"), true, false, () => el("div", { class: "row" },
    el("div", { class: "grow" }, s.gamertag ? t("Signed in as {name}", { name: s.gamertag }) : t("Signed in")),
    el("button", { class: "link", onclick: xboxSignOut }, t("Sign out"))));
}

async function xboxSignOut() {
  if (!(await ask(t("Sign out of Microsoft in VI Translator? You can sign in again with a new code."), t("Sign out")))) return;
  Object.assign(xboxUi, { consoles: null, changing: false, msg: "" });
  try { await invoke("xbox_sign_out"); } catch {}
  renderXboxNative();
}

function xboxCodeStep(s, err) {
  if (!s.code) {
    return el("div", {},
      el("p", { class: "muted small" }, t("No password in VI Translator: you type a short code on Microsoft's page, on this PC or your phone.")),
      el("button", { class: "btn primary", onclick: async () => {
        try { xbox = await invoke("xbox_sign_in"); } catch (e) { xbox = { ...xbox, error: String(e) }; }
        drawXbox(true);
      } }, s.error ? t("Get a new code") : t("Get a code")),
      err);
  }
  const msg = el("p", { class: "small muted" });
  return el("div", {},
    el("p", {}, t("On any device, go to {uri} and type this code:", { uri: s.code.uri.replace(/^https:\/\/(www\.)?/, "") })),
    el("div", { class: "ps5-pin xbox-code", dir: "ltr" }, s.code.code),
    el("div", { class: "row" },
      el("button", { class: "btn primary", onclick: () => invoke("xbox_open_link").catch((e) => { msg.textContent = String(e); }) }, t("Open the page")),
      el("button", { class: "btn", onclick: async () => {
        try { await navigator.clipboard.writeText(s.code.code); msg.textContent = t("Copied."); } catch (e) { msg.textContent = String(e); }
      } }, t("Copy the code"))),
    el("p", { class: "muted small" }, el("span", { class: "ps5-spin" }), t("Waiting for you to sign in…")),
    el("button", { class: "link", onclick: async () => { await invoke("xbox_cancel_sign_in"); renderXboxNative(); } }, t("Cancel")),
    msg, err);
}

async function xboxSearch() {
  xboxUi.searching = true;
  xboxUi.msg = "";
  if (xbox) drawXbox(true);
  try { xboxUi.consoles = await invoke("xbox_consoles"); } catch (e) { xboxUi.consoles = []; xboxUi.msg = I18N.tMsg(String(e)); }
  xboxUi.searching = false;
  if (!$("xbox-panel").hidden && xbox) drawXbox(true);
}

const xboxPower = (p) => (/^on$/i.test(p) ? t("On") : /standby/i.test(p) ? t("Asleep") : /^off$/i.test(p) ? t("Off") : "");
const xboxKind = (k) => ({ XboxSeriesX: "Xbox Series X", XboxSeriesS: "Xbox Series S", XboxOne: "Xbox One", XboxOneS: "Xbox One S", XboxOneX: "Xbox One X" }[k] || "");

function xboxConsoleStep(err) {
  const list = xboxUi.consoles || [];
  return el("div", {},
    xboxUi.searching ? el("p", { class: "muted small" }, el("span", { class: "ps5-spin" }), t("Looking for your consoles…")) : null,
    list.map((c) => el("div", { class: "ps5-host" },
      el("div", { class: "grow" }, el("b", {}, c.name), el("div", { class: "muted small" }, [xboxKind(c.kind), xboxPower(c.power)].filter(Boolean).join(" · "))),
      el("button", { class: "btn primary", onclick: async () => {
        xboxUi.changing = false;
        await invoke("xbox_choose", { console: { id: c.id, name: c.name, kind: c.kind } });
        renderXboxNative();
      } }, t("Choose")))),
    !xboxUi.searching && !list.length && !xboxUi.msg ? el("div", { class: "callout" }, t("No console on this account yet. On the Xbox, turn on Settings › Devices & connections › Remote features, then search again.")) : null,
    xboxUi.msg ? el("p", { class: "small ps5-err" }, xboxUi.msg) : null,
    !xboxUi.searching ? el("button", { class: "link", onclick: xboxSearch }, t("Search again")) : null,
    xboxUi.changing ? el("button", { class: "link", onclick: () => { xboxUi.changing = false; drawXbox(true); } }, t("Cancel")) : null,
    err);
}

function xboxChosen(s, err) {
  const name = s.console.name || "Xbox";
  const quality = el("select", { id: "xbox-quality" }, el("option", { value: "1080" }, "1080p"), el("option", { value: "720" }, "720p"));
  quality.value = String(s.resolution || 1080);
  quality.addEventListener("change", () => invoke("xbox_set_resolution", { resolution: +quality.value }));
  // the Xbox's sound on this PC: on or off, and its volume (heard at once)
  const on = el("input", { type: "checkbox", id: "xbox-sound" });
  on.checked = s.sound !== false;
  const vol = el("input", { type: "range", id: "xbox-volume", min: "0", max: "100", step: "5", title: t("Volume"), "aria-label": t("Volume") });
  vol.value = String(s.volume ?? 100);
  const pct = el("span", { class: "muted small ps5-pct" }, `${vol.value}%`);
  const sound = () => {
    vol.disabled = !on.checked;
    pct.textContent = `${vol.value}%`;
    xbox = { ...xbox, sound: on.checked, volume: +vol.value };
    invoke("xbox_set_sound", { on: on.checked, volume: +vol.value });
  };
  on.addEventListener("change", sound);
  vol.addEventListener("input", sound);
  vol.disabled = !on.checked;
  const pads = el("input", { type: "checkbox", id: "xbox-pads" });
  pads.checked = s.controllers !== false;
  pads.addEventListener("change", () => { xbox = { ...xbox, controllers: pads.checked }; invoke("xbox_set_controllers", { on: pads.checked }); });
  return el("div", {},
    el("div", { class: "ps5-step done" },
      el("div", { class: "ps5-head" }, el("span", { class: "ps5-badge" }, "✓"),
        el("div", {}, el("b", {}, name), el("div", { class: "small ps5-ok" }, `✓ ${t("Chosen")}`, s.gamertag ? ` · ${s.gamertag}` : ""))),
      el("div", { class: "ps5-body" },
        el("p", { class: "muted small" }, t("While the Xbox is the picture source, the app plays from it with Remote Play. If it is asleep, it is woken.")),
        el("div", { class: "row" },
          el("button", { class: "btn", onclick: () => { Object.assign(xboxUi, { changing: true, consoles: null }); drawXbox(true); } }, t("Change console")),
          el("button", { class: "btn danger", onclick: xboxSignOut }, t("Sign out"))),
        err)),
    el("div", { class: "ps5-step ps5-settings" },
      el("div", { class: "field" },
        el("label", { class: "check" }, on, " ", el("span", {}, t("Play the Xbox's sound on this PC"))),
        el("div", { class: "row ps5-volume" }, el("span", { class: "muted small" }, t("Volume")), el("div", { class: "grow" }, vol), pct)),
      el("div", { class: "field" }, el("label", { class: "check" }, pads, " ", el("span", {}, t("Controllers connected to this PC play on the Xbox")))),
      el("div", { class: "field" }, el("label", { for: "xbox-quality" }, t("Stream quality")), quality,
        el("p", { class: "muted small" }, t("1080p reads small text best; 720p for a slower home network.")))),
    el("p", { class: "muted small ps5-foot" }, t("Use Play to play full screen.")),
  );
}

// PS5 in the app, linked once as the phone app does it, in three steps each done before the next
// opens: sign in to PSN (Sony's page in a window of the app; or in a browser, pasting back the address
// it ends on; or the account ID typed), find the PS5 on the home network (searched at once; or its
// address typed), then the PIN the PS5 shows, and Link. Linked: the PS5, with Wake, Unlink and the
// stream's quality. While it is the source it streams; its login passcode, if the account has one, is
// asked here when the PS5 wants it.
let ps5Value = null;
let ps5 = null;
const ps5Ui = { editingAccount: false, editingAddress: false, address: "", name: "", hosts: [], searching: false, searched: false,
  linking: false, msg: "", msgErr: false, browser: false, browserMsg: "", accountMsg: "", addressMsg: "" };
try {
  const a = JSON.parse(localStorage.getItem("ps5-address") || "null");
  if (a) Object.assign(ps5Ui, { address: a.address || "", name: a.name || "" });
} catch {}
const ps5Remember = () => { try { localStorage.setItem("ps5-address", JSON.stringify({ address: ps5Ui.address, name: ps5Ui.name })); } catch {} };
const ps5ValidAddress = (a) => /^\d{1,3}(\.\d{1,3}){3}$/.test(a.trim()) && a.trim().split(".").every((n) => +n <= 255);
const ps5ValidAccount = (a) => { try { return atob(a.trim()).length === 8; } catch { return false; } };

async function renderPs5(v = cfg.window_title) {
  const on = !!ps5Value && v === ps5Value;
  $("ps5-panel").hidden = !on;
  // (the capture card hint means nothing while remote play in the app is the source)
  $("obs-card-hint").hidden = on || (!!xboxTitle && v === xboxTitle) || (!!xboxValue && v === xboxValue);
  if (!on) return;
  try { ps5 = await invoke("ps5_status"); } catch { return; }
  drawPs5();
}
listen("ps5", (e) => {
  ps5 = e.payload;
  if (ps5.signed_in && ps5Ui.editingAccount && !ps5.error) ps5Ui.editingAccount = false;
  if (!$("ps5-panel").hidden) drawPs5();
});

// a small element: el("div", { class: "x", onclick }, child, "text" …)
function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "class") e.className = v;
    else if (k === "value") e.value = v;
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) e.append(c);
  return e;
}

function drawPs5() {
  const s = ps5;
  const box = $("ps5-panel");
  const keep = document.activeElement?.id; // (typing goes on where it was)
  box.innerHTML = "";
  if (s.console) {
    box.append(ps5Linked(s));
  } else {
    const accountOk = s.signed_in && ps5ValidAccount(s.account_id);
    const addressOk = ps5ValidAddress(ps5Ui.address);
    if (accountOk && !ps5Ui.editingAccount && !ps5Ui.searched) ps5Search();
    box.append(
      ps5Step(1, t("Sign in to PSN"), accountOk && !ps5Ui.editingAccount, true, () => ps5SignInStep(s, accountOk)),
      ps5Step(2, t("Find your PS5"), addressOk && !ps5Ui.editingAddress, accountOk && !ps5Ui.editingAccount, () => ps5FindStep(addressOk)),
      ps5Step(3, t("Enter the PIN from the PS5"), false, addressOk && accountOk && !ps5Ui.editingAddress && !ps5Ui.editingAccount, ps5PinStep),
    );
  }
  if (s.pin_wanted != null) {
    const pin = el("input", { id: "ps5-login-pin", type: "password", dir: "ltr", inputmode: "numeric", autocomplete: "off" });
    const send = () => { invoke("ps5_login_pin", { pin: pin.value }); pin.value = ""; ps5 = { ...ps5, pin_wanted: null }; drawPs5(); };
    pin.addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
    box.append(el("div", { class: "ps5-step open" },
      el("div", { class: "field" }, el("label", { for: "ps5-login-pin" }, s.pin_wanted ? t("Wrong passcode. Try again:") : t("The PS5 asks for your PlayStation login passcode:")),
        el("div", { class: "row" }, el("div", { class: "grow" }, pin), el("button", { class: "btn primary", onclick: send }, t("Send"))))));
  }
  if (keep && $(keep)) $(keep).focus();
}

// the frame of a step: its number (or ✓) and title, then what it asks (while open or done)
function ps5Step(n, title, done, open, content) {
  const st = done ? "done" : open ? "open" : "off";
  const card = el("div", { class: `ps5-step ${st}` }, el("div", { class: "ps5-head" }, el("span", { class: "ps5-badge" }, done ? "✓" : String(n)), el("b", {}, title)));
  if (done || open) card.append(el("div", { class: "ps5-body" }, content()));
  return card;
}

function ps5SignInStep(s, accountOk) {
  if (accountOk && !ps5Ui.editingAccount) {
    return el("div", { class: "row" },
      el("div", { class: "grow" }, el("b", {}, t("Signed in: the account ID is ready")),
        el("div", { class: "muted small", dir: "ltr" }, s.account ? `${s.account} · ${s.account_id}` : s.account_id)),
      el("button", { class: "link", onclick: () => { ps5Ui.editingAccount = true; drawPs5(); } }, t("Change")));
  }
  const err = s.error ? t("PSN sign-in didn't work: {e}", { e: s.error.replace(/^PSN: /, "") }) : ps5Ui.accountMsg;
  const typed = el("input", { type: "text", id: "ps5-account-id", dir: "ltr", autocomplete: "off", value: s.account_id || "" });
  return el("div", {},
    el("div", { class: "callout warn-box" }, t("Sign in with the PSN account you play with on the PS5. Only its account ID is kept.")),
    el("button", { class: "btn primary", onclick: () => invoke("ps5_sign_in").catch((e) => { ps5Ui.accountMsg = String(e); drawPs5(); }) }, t("Sign in to PSN")),
    el("p", { class: "muted small" }, t("If Sony says it can't connect, wait a few seconds and press Sign In again; or sign in with a browser.")),
    el("button", { class: "link", onclick: () => { ps5Ui.browser = !ps5Ui.browser; drawPs5(); } }, t("Sign in with a browser")),
    ps5Ui.browser ? ps5BrowserBox() : null,
    err ? el("p", { class: "small ps5-err" }, err) : null,
    el("div", { class: "field" }, el("label", { for: "ps5-account-id" }, t("Or type the account ID")),
      el("div", { class: "row" }, el("div", { class: "grow" }, typed),
        el("button", { class: "btn", onclick: async () => {
          if (!ps5ValidAccount(typed.value)) { ps5Ui.accountMsg = t("That isn't an account ID (12 letters and digits, ending in =)."); drawPs5(); return; }
          ps5Ui.accountMsg = "";
          ps5Ui.editingAccount = false;
          await invoke("ps5_set_account", { id: typed.value.trim() });
          renderPs5();
        } }, t("Use this account ID")))),
  );
}

// Sony's sign-in in the browser instead (its bot check sometimes refuses the app's window): the
// browser ends on remoteplay.dl.playstation.net/remoteplay/redirect?code=…, pasted back here
function ps5BrowserBox() {
  const addr = el("input", { type: "text", id: "ps5-browser-address", dir: "ltr", autocomplete: "off", placeholder: "https://remoteplay.dl.playstation.net/remoteplay/redirect?code=…" });
  const msg = el("p", { class: "small muted" }, ps5Ui.browserMsg);
  let url = "";
  return el("div", { class: "ps5-sub" },
    el("p", { class: "small" }, t("1. Open Sony's sign-in in your browser and sign in with your PSN account.")),
    el("div", { class: "row" },
      el("button", { class: "btn", onclick: async () => { try { url = await invoke("ps5_browser_sign_in"); } catch (e) { msg.textContent = String(e); } } }, t("Open in the browser")),
      el("button", { class: "btn", onclick: async () => {
        try { url = url || await invoke("ps5_login_url"); await navigator.clipboard.writeText(url); msg.textContent = t("Copied."); } catch (e) { msg.textContent = String(e); }
      } }, t("Copy the link"))),
    el("p", { class: "small" }, t("2. It ends on a page whose address starts with https://remoteplay.dl.playstation.net (the page itself may be blank). Copy that address and paste it here:")),
    addr,
    msg,
    el("button", { class: "btn primary", onclick: async () => {
      if (!/[?&]code=[^&#\s]+/.test(addr.value)) { msg.textContent = t("That address has no sign-in code: copy the whole address of the page you ended on."); return; }
      msg.textContent = t("Getting the account ID…");
      try {
        await invoke("ps5_sign_in_address", { address: addr.value.trim() });
        Object.assign(ps5Ui, { browser: false, browserMsg: "", editingAccount: false });
        renderPs5();
      } catch (e) {
        ps5Ui.browserMsg = `${t("PSN sign-in didn't work: {e}", { e })} ${t("The code works once and only for a few minutes: sign in again and paste the new address.")}`;
        msg.textContent = ps5Ui.browserMsg;
      }
    } }, t("Get the account ID")),
  );
}

async function ps5Search() {
  ps5Ui.searched = true;
  ps5Ui.searching = true;
  ps5Ui.hosts = [];
  if (!$("ps5-panel").hidden && ps5) drawPs5();
  try { ps5Ui.hosts = await invoke("ps5_find"); } catch {}
  ps5Ui.searching = false;
  if (!$("ps5-panel").hidden && ps5) drawPs5();
}

function ps5FindStep(addressOk) {
  if (addressOk && !ps5Ui.editingAddress) {
    return el("div", { class: "row" },
      el("div", { class: "grow" }, el("b", {}, ps5Ui.name || "PS5"), el("div", { class: "muted small", dir: "ltr" }, ps5Ui.address)),
      el("button", { class: "link", onclick: () => { ps5Ui.editingAddress = true; drawPs5(); } }, t("Change")));
  }
  const choose = (addr, name) => { Object.assign(ps5Ui, { address: addr, name, editingAddress: false, addressMsg: "" }); ps5Remember(); drawPs5(); };
  const typed = el("input", { type: "text", id: "ps5-address", dir: "ltr", autocomplete: "off", placeholder: "192.168.1.20", value: ps5Ui.address });
  return el("div", {},
    el("p", { class: "muted small" }, t("The PS5 must be on (or in rest mode) and on the same network as this PC.")),
    ps5Ui.searching ? el("p", { class: "muted small" }, el("span", { class: "ps5-spin" }), t("Looking for PlayStation consoles…")) : null,
    ps5Ui.hosts.map((x) => el("div", { class: "ps5-host" },
      el("div", { class: "grow" }, el("b", {}, x.name || "PS5"),
        el("div", { class: "muted small" }, el("span", { dir: "ltr" }, x.host), ` · ${x.standby ? t("rest mode") : t("on")}`)),
      el("button", { class: "btn primary", onclick: () => choose(x.host, x.name || "PS5") }, t("Choose")))),
    !ps5Ui.searching && !ps5Ui.hosts.length ? el("div", { class: "callout" }, t("No PS5 found yet. Is it on, with Remote Play allowed (Settings › System › Remote Play)? You can also type its address below.")) : null,
    !ps5Ui.searching ? el("button", { class: "link", onclick: ps5Search }, t("Search again")) : null,
    el("div", { class: "field" }, el("label", { for: "ps5-address" }, t("Or type its address")),
      el("div", { class: "row" }, el("div", { class: "grow" }, typed),
        el("button", { class: "btn", onclick: () => {
          if (!ps5ValidAddress(typed.value)) { ps5Ui.addressMsg = t("That isn't an address like 192.168.1.20."); drawPs5(); return; }
          choose(typed.value.trim(), ps5Ui.hosts.find((x) => x.host === typed.value.trim())?.name || "");
        } }, t("Use this address"))),
      el("p", { class: "muted small" }, t("On the PS5: Settings › Network › Connection Status › View Connection Status, IPv4 address."))),
    ps5Ui.addressMsg ? el("p", { class: "small ps5-err" }, ps5Ui.addressMsg) : null,
  );
}

function ps5PinStep() {
  const pin = el("input", { type: "text", id: "ps5-pin", class: "ps5-pin", dir: "ltr", inputmode: "numeric", maxlength: "9", autocomplete: "off", placeholder: "0000 0000" });
  const msg = el("p", { class: `small ${ps5Ui.msgErr ? "ps5-err" : "muted"}` }, ps5Ui.msg);
  const link = async () => {
    if (ps5Ui.linking) return;
    const p = pin.value.replace(/\D/g, "");
    if (p.length !== 8) { msg.textContent = t("The PIN has 8 digits."); msg.className = "small ps5-err"; return; }
    ps5Ui.linking = true;
    msg.textContent = t("Linking…");
    msg.className = "small muted";
    try {
      await invoke("ps5_link", { host: ps5Ui.address, pin: p });
      Object.assign(ps5Ui, { msg: "", msgErr: false });
    } catch {
      Object.assign(ps5Ui, { msg: t("The PS5 refused: check the PIN (it changes every few minutes) and that Remote Play is on."), msgErr: true });
    }
    ps5Ui.linking = false;
    renderPs5();
  };
  pin.addEventListener("keydown", (e) => { if (e.key === "Enter") link(); });
  return el("div", {},
    el("p", { class: "muted small" }, t("On the PS5: Settings › System › Remote Play › Link Device. It shows an 8-digit PIN, for about 5 minutes.")),
    pin, msg,
    el("button", { class: "btn primary", onclick: link }, t("Link")));
}

// the PS5's sound on this PC: on or off, and its volume (heard at once)
function ps5SoundRow(s) {
  const on = el("input", { type: "checkbox", id: "ps5-sound" });
  on.checked = s.sound !== false;
  const vol = el("input", { type: "range", id: "ps5-volume", min: "0", max: "100", step: "5", title: t("Volume"), "aria-label": t("Volume") });
  vol.value = String(s.volume ?? 100);
  const pct = el("span", { class: "muted small ps5-pct" }, `${vol.value}%`);
  const apply = () => {
    vol.disabled = !on.checked;
    pct.textContent = `${vol.value}%`;
    ps5 = { ...ps5, sound: on.checked, volume: +vol.value };
    invoke("ps5_set_sound", { on: on.checked, volume: +vol.value });
  };
  on.addEventListener("change", apply);
  vol.addEventListener("input", apply);
  vol.disabled = !on.checked;
  return el("div", { class: "field" },
    el("label", { class: "check" }, on, " ", el("span", {}, t("Play the PS5's sound on this PC"))),
    el("div", { class: "row ps5-volume" }, el("span", { class: "muted small" }, t("Volume")), el("div", { class: "grow" }, vol), pct));
}

function ps5Linked(s) {
  const name = s.console.name || "PS5";
  const note = el("p", { class: "small muted" });
  const quality = el("select", { id: "ps5-quality" }, el("option", { value: "1080" }, "1080p"), el("option", { value: "720" }, "720p"));
  quality.value = String(s.resolution || 1080);
  quality.addEventListener("change", () => invoke("ps5_set_resolution", { resolution: +quality.value }));
  return el("div", {},
    el("div", { class: "ps5-step done" },
      el("div", { class: "ps5-head" }, el("span", { class: "ps5-badge" }, "✓"),
        el("div", {}, el("b", {}, name), el("div", { class: "small ps5-ok" }, `✓ ${t("Linked")} · `, el("span", { dir: "ltr" }, s.console.host)))),
      el("div", { class: "ps5-body" },
        el("p", { class: "muted small" }, t("While the PS5 is the picture source, the app plays from it with Remote Play. If it is in rest mode, Wake turns it on.")),
        el("div", { class: "row" },
          el("button", { class: "btn", onclick: async () => {
            note.textContent = t("Waking {name}…", { name });
            try { await invoke("ps5_wake"); } catch (e) { note.textContent = String(e); }
          } }, t("Wake the PS5")),
          el("button", { class: "btn danger", onclick: async () => {
            // (window.confirm does nothing in the macOS webview: the page's own question)
            if (!(await ask(t("Unlink {name}? To use it again, link it with a new PIN.", { name }), t("Unlink")))) return;
            try { await invoke("ps5_forget"); } catch (e) { note.textContent = String(e); return; }
            Object.assign(ps5Ui, { searched: false, msg: "", msgErr: false });
            renderPs5();
          } }, t("Unlink"))),
        note)),
    // the stream's sound and picture, in a card of their own under the PS5's
    el("div", { class: "ps5-step ps5-settings" },
      ps5SoundRow(s),
      el("div", { class: "field" }, el("label", { for: "ps5-quality" }, t("Stream quality")), quality,
        el("p", { class: "muted small" }, t("1080p reads small text best; 720p for a slower home network.")))),
    el("p", { class: "muted small ps5-foot" }, t("Controllers connected to this PC play on the PS5. Use Play to play full screen.")),
  );
}

function renderObs() {
  $("obs_port").value = cfg.obs_port;
  $("obs_password").value = cfg.obs_password;
  $("obs-url").value = `http://127.0.0.1:${cfg.obs_page_port}/obs`;
}
$("obs_port").addEventListener("input", (e) => {
  const v = parseInt(e.target.value, 10);
  cfg.obs_port = v >= 1 && v <= 65535 ? v : 4455;
  save(false);
});
$("obs_password").addEventListener("input", (e) => { cfg.obs_password = e.target.value; save(false); });
$("obs-connect").addEventListener("click", () => { $("obs-status").textContent = t("Connecting ..."); $("refresh-windows").click(); });
// Enter in the password box = connect
$("obs_password").addEventListener("keydown", (e) => { if (e.key === "Enter") $("refresh-windows").click(); });
$("obs-url-copy").addEventListener("click", async () => {
  try { await navigator.clipboard.writeText($("obs-url").value); $("obs-url-copy").textContent = t("Copied ✓"); }
  catch { $("obs-url").select(); document.execCommand("copy"); $("obs-url-copy").textContent = t("Copied ✓"); }
  setTimeout(() => ($("obs-url-copy").textContent = t("Copy")), 1500);
});
// ------------------------------------------------------------------------------------- updates

// (VI Translator.app, the macOS .dmg's, is updated with the new .dmg: the button opens its page)
let updateIsDmg = false;
function showUpdate(u) {
  if (!u) return;
  updateIsDmg = u.app_bundle === true;
  $("update-text").textContent = t("Version {version} is available.", { version: u.version });
  $("update-now").textContent = updateIsDmg ? t("Download") : t("Update now");
  $("update-bar").classList.remove("hidden");
}
listen("update-available", (e) => showUpdate(e.payload));
$("update-later").addEventListener("click", () => $("update-bar").classList.add("hidden"));
$("update-now").addEventListener("click", async () => {
  $("update-now").disabled = true;
  $("update-text").textContent = updateIsDmg ? "" : t("Downloading the update…");
  try {
    await invoke("install_update"); // the app restarts into the new version (the .dmg's page opens for VI Translator.app)
    if (updateIsDmg) {
      $("update-text").textContent = t("The download page is open: install the new version from its .dmg.");
      $("update-now").disabled = false;
    }
  } catch (e) {
    $("update-text").textContent = t("Update failed: {e}", { e });
    $("update-now").disabled = false;
  }
});

// Live translations page on the home network: switch, address and QR code.
async function updateLan() {
  const box = $("lan-info");
  $("lan-on").checked = !!cfg.lan_page;
  $("lan-control").checked = cfg.lan_control !== false;
  if (!cfg.lan_page) { box.textContent = t("Off. Turn it on to read the live translations on another device on your home network."); return; }
  try {
    const i = await invoke("lan_info");
    if (!i.url) {
      box.textContent = i.error
        ? t("The page couldn't start ({error}). Another program may be using this port.", { error: i.error })
        : t("Starting… If Windows asks, allow the app on private networks.");
      return;
    }
    box.innerHTML = `<div>${AS.esc(t("Open this address on a phone, tablet or computer on the same Wi-Fi:"))}</div>
      <a href="#" class="lan-url" dir="ltr" title="${AS.esc(t("Open in your browser"))}">${AS.esc(i.url)}</a>
      <div class="lan-qr">${i.qr || ""}</div>`;
    const svg = box.querySelector("svg");
    if (svg) Object.assign(svg.style, { width: "132px", height: "132px" });
  } catch (e) { box.textContent = String(e); }
}
// the address: opens the page in the default browser (the box is redrawn every few seconds, so listen on the box)
$("lan-info").addEventListener("click", (e) => {
  if (!e.target.closest(".lan-url")) return;
  e.preventDefault();
  invoke("open_lan_page").catch((err) => log({ error: String(err) }));
});
$("lan-on").addEventListener("change", (e) => { cfg.lan_page = e.target.checked; save(false); setTimeout(updateLan, 900); });
$("lan-control").addEventListener("change", (e) => { cfg.lan_control = e.target.checked; save(false); });
setInterval(() => { if (cfg && !$("tab-show").classList.contains("hidden")) updateLan(); if (cfg) updateTvs(); }, 3000);

// TVs (the VI Translator app on the TV): find the ones waiting on the home network, connect one with the
// code it shows, remove a connected one.
async function updateTvs() {
  const box = $("tv-connected");
  let tvs;
  try { tvs = await invoke("list_tvs"); } catch (e) { box.textContent = String(e); return; }
  // online: the TV app is open and showing the translation now (its page asks about once a second)
  const state = (tv) => tv.online
    ? `<span class="tv-state on"><span class="dot"></span>${AS.esc(t("Online"))}</span>`
    : `<span class="tv-state" title="${AS.esc(t("The TV app isn't open, or the TV can't reach this PC"))}"><span class="dot"></span>${AS.esc(t("Offline"))}</span>`;
  const html = tvs.length
    ? `<div class="hint">${AS.esc(t("Connected TVs"))}</div>` + tvs.map((tv) => `<div class="tv-row"><span class="tv-name">📺 ${AS.esc(tv.name)}</span>
        ${state(tv)}<label class="switch" title="${AS.esc(t("Show the translations on this TV"))}"><input type="checkbox" data-tv-on="${AS.esc(tv.id)}" ${tv.on !== false ? "checked" : ""}/><span class="track"></span></label>
        <button class="btn danger" data-remove-tv="${AS.esc(tv.id)}">${AS.esc(t("Remove"))}</button></div>`).join("")
    : "";
  if (box.innerHTML !== html) box.innerHTML = html;
  knownTvs = tvs;
  renderOutputs();
  // the header: how many of the connected TVs are online (hidden while none is connected)
  const pill = $("tv-pill"), on = tvs.filter((tv) => tv.online).length;
  pill.classList.toggle("hidden", !tvs.length);
  pill.classList.toggle("ok", on > 0);
  $("tv-pill-text").textContent = `📺 ${on}/${tvs.length}`;
  pill.title = t("TVs showing the translation now: {n} of {m}", { n: on, m: tvs.length });
}
// ------------------------------------------------------------------------------------- outputs on/off

/**
 * Where the translations are shown, each switched on or off at once while translating, nothing
 * restarted: this PC's overlay windows (`device_overlays`), each connected TV (kept with the TV, in
 * the core) and the OBS page (`obs_overlays`). On the Play screen, and beside each in Show on. The
 * header's Overlays switch stays over them all.
 */
let knownTvs = [];
function renderOutputs() {
  if (!cfg) return;
  $("device_overlays").checked = cfg.device_overlays !== false;
  $("obs_overlays").checked = cfg.obs_overlays !== false;
  const row = (attrs, on, name, extra = "") => `<label class="switch out-row"><input type="checkbox" ${attrs} ${on ? "checked" : ""}/><span class="track"></span><span class="out-name">${name}</span>${extra}</label>`;
  const dot = (tv) => `<span class="tv-state${tv.online ? " on" : ""}" title="${AS.esc(tv.online ? t("Online") : t("Offline"))}"><span class="dot"></span></span>`;
  const html = row('data-output="device"', cfg.device_overlays !== false, AS.esc(t("Overlays on this PC")))
    + knownTvs.map((tv) => row(`data-tv-on="${AS.esc(tv.id)}"`, tv.on !== false, `📺 ${AS.esc(tv.name)}`, dot(tv))).join("")
    + row('data-output="obs"', cfg.obs_overlays !== false, "OBS");
  const box = $("ps-outputs");
  if (box.innerHTML !== html) box.innerHTML = html;
  $("ps-outputs-note").textContent = cfg.overlays_enabled === false
    ? t("Overlays are off for everything: turn on Overlays at the top.")
    : t("Turn each one on or off at any time, even while translating.");
  $("ps-outputs-note").classList.toggle("warn", cfg.overlays_enabled === false);
}
/** An output switched: saved at once (no waiting for the usual pause), so it goes off or on now. */
async function setOutput(input) {
  if (input.dataset.tvOn) {
    const id = input.dataset.tvOn, on = input.checked;
    const tv = knownTvs.find((x) => x.id === id);
    if (tv) tv.on = on;
    try { await invoke("set_tv_on", { id, on }); } catch (err) { log({ error: String(err) }); }
    updateTvs();
    return;
  }
  cfg[input.dataset.output === "obs" ? "obs_overlays" : "device_overlays"] = input.checked;
  renderOutputs();
  save(false);
  clearTimeout(saveTimer);
  try { await invoke("save_config", { config: cfg }); $("saved").textContent = t("Saved ✓"); } catch (e) { log({ error: String(e) }); }
}
for (const id of ["ps-outputs", "tab-show"]) {
  $(id).addEventListener("change", (e) => { if (e.target.matches("[data-output], [data-tv-on]")) setOutput(e.target); });
}

$("tv-pill").addEventListener("click", () => { setMode("setup"); openTab("show"); });
// The TV loads its page from the home-network sharing: turned on if it's off, and waited for.
async function sharingReady() {
  if (!cfg.lan_page) { cfg.lan_page = true; save(false); }
  for (let i = 0; i < 25; i++) {
    try { if ((await invoke("lan_info")).url) { updateLan(); return true; } } catch (e) { /* not yet */ }
    await new Promise((r) => setTimeout(r, 300));
  }
  return false;
}
// A Samsung TV finds this PC by itself, through the home-network sharing (it can't be called): so
// sharing is turned on first, and the list keeps growing for a while as such TVs show up.
const TV_LOOK_MS = 40000;
let tvLook = 0;
$("tv-find").addEventListener("click", async () => {
  const box = $("tv-found"), btn = $("tv-find");
  const run = ++tvLook;
  btn.disabled = true;
  box.innerHTML = `<div class="hint tv-looking">${AS.esc(t("Looking for TVs…"))}</div>`;
  const row = (f) => `<div class="tv-row" data-address="${AS.esc(f.address)}" data-id="${AS.esc(f.id)}"><span class="tv-name">📺 ${AS.esc(f.name)}</span>
          <input class="tv-code" inputmode="numeric" maxlength="7" dir="ltr" autocomplete="off" placeholder="000 000" title="${AS.esc(t("The code shown on the TV"))}" />
          <button class="btn tv-connect">${AS.esc(t("Connect"))}</button><div class="tv-msg hint"></div></div>`;
  try {
    await sharingReady();
    const start = Date.now();
    while (run === tvLook) {
      const found = (await invoke("find_tvs")).filter((f) => !f.connected);
      if (run !== tvLook) return;
      // (new TVs added below: a code being typed in a row stays)
      const fresh = found.filter((f) => !Array.from(box.querySelectorAll(".tv-row")).some((r) => r.dataset.id === f.id));
      if (fresh.length) {
        box.querySelector(".tv-looking, .tv-none")?.remove();
        box.insertAdjacentHTML("beforeend", fresh.map(row).join(""));
        if (!box.contains(document.activeElement)) box.querySelector(".tv-code")?.focus();
      }
      if (Date.now() - start >= TV_LOOK_MS) break;
      btn.disabled = false;
      await new Promise((r) => setTimeout(r, 3000));
    }
    if (run === tvLook && !box.querySelector(".tv-row")) {
      box.innerHTML = `<div class="hint tv-none">${AS.esc(t("No TV is waiting. Open VI Translator on the TV, on the same network as this PC."))}</div>`;
    }
  } catch (e) { box.textContent = String(e); }
  finally { if (run === tvLook) btn.disabled = false; }
});
// A TV that isn't found (another network, a mesh, a VPN): its address typed, then the code it shows.
$("tv-type").addEventListener("click", () => {
  const box = $("tv-found");
  box.querySelector(".tv-looking, .tv-none")?.remove();
  if (box.querySelector(".tv-typed")) { box.querySelector(".tv-addr").focus(); return; }
  box.insertAdjacentHTML("beforeend", `<div class="tv-row tv-typed"><input class="tv-addr grow" dir="ltr" autocomplete="off" placeholder="192.168.1.30" title="${AS.esc(t("The TV's address"))}" />
    <input class="tv-code" inputmode="numeric" maxlength="7" dir="ltr" autocomplete="off" placeholder="000 000" title="${AS.esc(t("The code shown on the TV"))}" />
    <button class="btn tv-connect">${AS.esc(t("Connect"))}</button><div class="tv-msg hint"></div></div>`);
  box.querySelector(".tv-addr").focus();
});
async function connectTv(row) {
  const msg = row.querySelector(".tv-msg"), b = row.querySelector(".tv-connect");
  // (a typed address: checked here, as the PS5's is)
  if (row.classList.contains("tv-typed")) {
    const a = row.querySelector(".tv-addr").value.trim();
    if (!/^\d{1,3}(\.\d{1,3}){3}(:\d{1,5})?$/.test(a)) {
      msg.className = "tv-msg err";
      msg.textContent = t("That isn't an address like 192.168.1.20.");
      return;
    }
    row.dataset.address = a;
  }
  b.disabled = true;
  msg.className = "tv-msg hint";
  msg.textContent = t("Connecting…");
  try {
    if (!(await sharingReady())) throw "not_sharing";
    const name = await invoke("connect_tv", { address: row.dataset.address, code: row.querySelector(".tv-code").value });
    row.remove();
    updateTvs();
    log({ info: t("Connected to {name}", { name }) });
  } catch (err) {
    const why = {
      wrong_code: () => t("Wrong code. Check the code on the TV."),
      too_many: () => t("Too many tries: the TV shows a new code."),
      no_answer: () => t("The TV didn't answer. Is VI Translator still open on it?"),
      not_sharing: () => t("Sharing on the home network couldn't start."),
    }[String(err)];
    msg.className = "tv-msg err";
    msg.textContent = why ? why() : String(err);
    b.disabled = false;
  }
}
$("tv-found").addEventListener("click", (e) => { const b = e.target.closest(".tv-connect"); if (b) connectTv(b.closest(".tv-row")); });
$("tv-found").addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target.classList.contains("tv-code")) connectTv(e.target.closest(".tv-row")); });
$("tv-connected").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-remove-tv]");
  if (!b) return;
  b.disabled = true;
  try { await invoke("remove_tv", { id: b.dataset.removeTv }); } catch (err) { log({ error: String(err) }); }
  updateTvs();
});

// save pending edits (e.g. the OBS password just typed) before asking OBS for its sources
$("refresh-windows").addEventListener("click", async () => {
  clearTimeout(saveTimer);
  try { await invoke("save_config", { config: cfg }); $("saved").textContent = t("Saved ✓"); } catch {}
  loadWindows();
});
$("window_title").addEventListener("change", (e) => {
  cfg.window_title = e.target.value;
  cfg.source_crop = null; // (a part chosen in another window means nothing in this one)
  for (const k of Object.keys(lastSegs)) delete lastSegs[k];
  save();
  updateGlance();
  renderParts();
  renderCardSound(true);
});

// ------------------------------------------------------------------------------------- the game's part of a window

const isWindowSource = (v) => !!v && !v.startsWith("device:") && !v.startsWith("obs:") && !v.startsWith("ps5:") && !v.startsWith("xbox:");
// "Screen 1 — DELL U2720Q (2560 × 1440)"
const screenLabel = (s) => `${t("Screen {n}", { n: s.number })}${s.name ? ` — ${s.name}` : ""} (${s.w} × ${s.h})`;
// a source or overlay value as people read it (a screen's id means nothing to them)
const sourceName = (v, screens) => {
  if (!v?.startsWith("screen:")) return v;
  const s = screens.find((x) => `screen:${x.id}` === v);
  return s ? screenLabel(s) : t("Screen");
};
let knownScreens = [];
const partText = (c) => (c ? t("{w}% × {h}% of the window", { w: Math.round(c[2] * 100), h: Math.round(c[3] * 100) }) : t("the whole window"));

// The Source tab's and the Show on tab's "Part of the window" rows, as the settings say.
function renderParts() {
  const win = isWindowSource(cfg.window_title);
  $("source-part").classList.toggle("off", !win);
  $("source-part-hint").hidden = !win;
  $("source-part-state").textContent = `· ${partText(cfg.source_crop)}`;
  $("source-part-clear").hidden = !cfg.source_crop;
  const own = !!cfg.overlay_window;
  $("overlay-part").classList.toggle("off", !own);
  $("overlay-part-state").textContent = `· ${partText(cfg.overlay_crop)}`;
  $("overlay-part-clear").hidden = !cfg.overlay_crop;
  const st = $("overlay-status");
  if (!own && !win) st.textContent = t("The source isn't a window, so there's nothing on this PC to draw over. Pick the window you watch the game in.");
  else st.textContent = "";
}

/**
 * Choose the part of window `title` that shows the game, on a still of the whole window: drag a box,
 * or let the app find what moves. Resolves the part (x, y, w, h fractions), null for the whole
 * window, or undefined when cancelled.
 */
function choosePart(title, current) {
  return new Promise((resolve) => {
    const d = document.createElement("dialog");
    d.className = "part";
    d.innerHTML = `<h4></h4><div class="part-msg" data-msg></div>
      <div class="part-pic"><img alt="" draggable="false" /><div class="shade"></div><div class="part-box" hidden><i data-c="nw"></i><i data-c="ne"></i><i data-c="sw"></i><i data-c="se"></i></div></div>
      <div class="row"><button class="btn" data-find></button><button class="btn" data-whole></button><span class="grow"></span><button class="btn" data-no></button><button class="btn part-ok" data-ok></button></div>`;
    d.querySelector("h4").textContent = t("The game's part of \"{title}\"", { title: sourceName(title, knownScreens) });
    const msg = d.querySelector("[data-msg]");
    msg.textContent = t("Drag a box around the game picture, or press Find to let the app look for what moves.");
    d.querySelector("[data-find]").textContent = t("Find the game picture");
    d.querySelector("[data-whole]").textContent = t("Whole window");
    d.querySelector("[data-no]").textContent = t("Cancel");
    d.querySelector("[data-ok]").textContent = t("Use this part");
    const pic = d.querySelector(".part-pic"), img = pic.querySelector("img"), box = pic.querySelector(".part-box");
    let part = current ? [...current] : null; // x, y, w, h fractions
    const draw = () => {
      box.hidden = !part;
      if (!part) return;
      Object.assign(box.style, { left: `${part[0] * 100}%`, top: `${part[1] * 100}%`, width: `${part[2] * 100}%`, height: `${part[3] * 100}%` });
    };
    const done = (v) => { d.close(); d.remove(); resolve(v); };
    d.querySelector("[data-ok]").addEventListener("click", () => done(part));
    d.querySelector("[data-no]").addEventListener("click", () => done(undefined));
    d.querySelector("[data-whole]").addEventListener("click", () => done(null));
    d.addEventListener("cancel", (e) => { e.preventDefault(); done(undefined); });
    d.querySelector("[data-find]").addEventListener("click", async (e) => {
      const b = e.currentTarget;
      b.disabled = true;
      msg.textContent = t("Looking for the moving picture ... (keep the game playing)");
      try {
        const found = await invoke("find_picture", { title });
        if (found) { part = found; draw(); msg.textContent = t("Found. Adjust the box if needed, then press Use this part."); }
        else msg.textContent = t("Nothing moved enough to tell. Play the game for a moment (not paused) and try again, or drag a box yourself.");
      } catch (err) { msg.textContent = String(err); }
      b.disabled = false;
    });
    // dragging: a new box on the picture, the box itself to move it, a corner to resize it
    const at = (ev) => {
      const r = img.getBoundingClientRect();
      return [Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (ev.clientY - r.top) / r.height))];
    };
    let drag = null;
    pic.addEventListener("pointerdown", (ev) => {
      const [x, y] = at(ev);
      const corner = ev.target.dataset?.c;
      if (corner && part) drag = { kind: corner, start: [x, y], from: [...part] };
      else if (ev.target === box && part) drag = { kind: "move", start: [x, y], from: [...part] };
      else { drag = { kind: "new", start: [x, y] }; part = [x, y, 0, 0]; }
      pic.setPointerCapture(ev.pointerId);
      draw();
    });
    pic.addEventListener("pointermove", (ev) => {
      if (!drag) return;
      const [x, y] = at(ev);
      const [sx, sy] = drag.start;
      if (drag.kind === "new") part = [Math.min(x, sx), Math.min(y, sy), Math.abs(x - sx), Math.abs(y - sy)];
      else if (drag.kind === "move") {
        const [fx, fy, fw, fh] = drag.from;
        part = [Math.min(1 - fw, Math.max(0, fx + x - sx)), Math.min(1 - fh, Math.max(0, fy + y - sy)), fw, fh];
      } else {
        let [x0, y0, x1, y1] = [drag.from[0], drag.from[1], drag.from[0] + drag.from[2], drag.from[1] + drag.from[3]];
        if (drag.kind.includes("w")) x0 = x; else x1 = x;
        if (drag.kind.includes("n")) y0 = y; else y1 = y;
        part = [Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0)];
      }
      draw();
    });
    pic.addEventListener("pointerup", () => {
      drag = null;
      if (part && (part[2] < 0.05 || part[3] < 0.05)) part = current ? [...current] : null; // a click, not a box
      draw();
    });
    document.body.appendChild(d);
    d.showModal();
    msg.textContent = t("Getting a picture of the window ...");
    invoke("window_still", { title })
      .then((s) => {
        img.src = s.img;
        msg.textContent = t("Drag a box around the game picture, or press Find to let the app look for what moves.");
        draw();
      })
      .catch((err) => { msg.textContent = String(err); });
  });
}

$("source-part-choose").addEventListener("click", async () => {
  const part = await choosePart(cfg.window_title, cfg.source_crop);
  if (part === undefined) return;
  cfg.source_crop = part;
  save();
  renderParts();
});
$("source-part-clear").addEventListener("click", () => { cfg.source_crop = null; save(); renderParts(); });

// Show on: the window the overlays are drawn over ("" = the source window)
async function loadOverlayWindows() {
  const sel = $("overlay_window");
  sel.innerHTML = "";
  const iso = (s) => `⁨${s}⁩`;
  const short = (s, n = 46) => (s.length > n ? `${s.slice(0, n - 16).trimEnd()}…${s.slice(-15).trimStart()}` : s);
  sel.add(new Option(t("The source window"), ""));
  let windows = [];
  try { windows = await invoke("list_windows"); } catch {}
  try { knownScreens = await invoke("list_screens"); } catch {}
  const titles = windows.map((w) => w.title).concat(knownScreens.map((s) => `screen:${s.id}`));
  if (knownScreens.length) {
    const g = document.createElement("optgroup");
    g.label = t("Screens");
    for (const s of knownScreens) g.appendChild(new Option(`🖥 ${iso(screenLabel(s))}`, `screen:${s.id}`));
    sel.appendChild(g);
  }
  for (const w of windows) {
    const full = w.process ? `${w.title}  —  ${w.process}` : w.title;
    sel.add(Object.assign(new Option(iso(w.process ? `${short(w.title)}  —  ${short(w.process, 24)}` : short(w.title)), w.title), { title: full }));
  }
  if (cfg.overlay_window && !titles.includes(cfg.overlay_window)) {
    sel.add(new Option(t("{title}  (not available now)", { title: iso(short(cfg.overlay_window)) }), cfg.overlay_window), 1);
  }
  sel.value = cfg.overlay_window || "";
  renderParts();
}
$("overlay_window").addEventListener("change", (e) => {
  cfg.overlay_window = e.target.value;
  cfg.overlay_crop = null;
  save();
  renderParts();
});
$("refresh-overlay-windows").addEventListener("click", loadOverlayWindows);
$("overlay-part-choose").addEventListener("click", async () => {
  if (!cfg.overlay_window) return;
  const part = await choosePart(cfg.overlay_window, cfg.overlay_crop);
  if (part === undefined) return;
  cfg.overlay_crop = part;
  save();
  renderParts();
});
$("overlay-part-clear").addEventListener("click", () => { cfg.overlay_crop = null; save(); renderParts(); });

// Source: a capture card's sound on this PC's speakers
async function renderCardSound(refresh = false) {
  const card = (cfg.window_title || "").startsWith("device:");
  $("card-sound").hidden = !card;
  if (!card) return;
  $("card_sound").checked = cfg.card_sound !== false;
  $("card-sound-opts").hidden = cfg.card_sound === false;
  $("card_sound_volume").value = cfg.card_sound_volume ?? 100;
  const sel = $("card_sound_input");
  if (refresh || !sel.options.length) {
    let r = { inputs: [], status: "" };
    try { r = await invoke("sound_inputs"); } catch {}
    sel.innerHTML = "";
    sel.add(new Option(t("Automatic (the card's own)"), ""));
    for (const n of r.inputs) sel.add(new Option(n, n));
    if (cfg.card_sound_input && !r.inputs.includes(cfg.card_sound_input)) sel.add(new Option(t("{title}  (not available now)", { title: cfg.card_sound_input }), cfg.card_sound_input));
  }
  sel.value = cfg.card_sound_input || "";
  let st = "";
  try { st = await invoke("sound_status"); } catch {}
  const [what, detail = ""] = st.split(/\|(.*)/s);
  const text = {
    playing: () => t("Playing the sound of \"{name}\".", { name: detail }),
    none: () => t("No sound input found for this capture card: pick it in the list."),
    missing: () => t("The sound input \"{name}\" isn't connected.", { name: detail }),
    error: () => t("The card's sound couldn't play: {e}", { e: detail }),
  }[what];
  $("card-sound-status").textContent = cfg.card_sound === false ? "" : text ? text() : t("Starting ...");
}
$("card_sound").addEventListener("change", (e) => { cfg.card_sound = e.target.checked; save(false); renderCardSound(); });
$("card_sound_input").addEventListener("change", (e) => { cfg.card_sound_input = e.target.value; save(false); setTimeout(renderCardSound, 800); });
$("card_sound_volume").addEventListener("input", (e) => { cfg.card_sound_volume = +e.target.value; save(false); });
setInterval(() => { if (cfg && document.body.dataset.tab === "source") renderCardSound(); }, 1500);

// Show on: the virtual camera
$("virtual_camera").addEventListener("change", (e) => { cfg.virtual_camera = e.target.checked; save(false); setTimeout(updateVcam, 600); });
async function updateVcam() {
  $("virtual_camera").checked = !!cfg.virtual_camera;
  let st = "";
  try { st = await invoke("vcam_status"); } catch {}
  $("vcam-status").textContent = cfg.virtual_camera ? t(st || "Starting ...") : "";
}
setInterval(() => { if (cfg && document.body.dataset.tab === "show") updateVcam(); }, 1500);

// ------------------------------------------------------------------------------------- translator

// (false on a build without the free offline translator, the Intel Mac app's: "local" then means the
// reviewed translations only, as on the phone apps)
let hasOffline = true;
const ENGINES = [
  { id: "local", name: "Local (offline)", badge: "Free", d: "Runs on this PC. No account or internet after a one-time ~110 MB download per language. Basic, literal quality. Not every language has one." },
  { id: "deepl", name: "DeepL", badge: "Free tier", d: "Very good quality. The free API plan includes 500,000 characters a month." },
  { id: "azure", name: "Microsoft Azure Translator", badge: "Free tier", d: "Good quality. The free tier includes 2 million characters a month." },
  { id: "google", name: "Google Cloud Translation", badge: "Free tier", d: "Good quality. Around 500,000 free characters a month." },
  { id: "libre", name: "LibreTranslate", badge: "Free / self-hosted", d: "Open source. Use your own server or a hosted one." },
  { id: "claude", name: "Claude (Anthropic)", badge: "≈ $1 per game", d: "Best with slang, jokes and context. Can use a dialect. Paid per use." },
  { id: "openai", name: "ChatGPT (OpenAI)", badge: "≈ $1 per game", d: "Natural translations with context, like Claude. Can use a dialect. Paid per use." },
];
const ENGINE_FIELDS = {
  local: [],
  deepl: [["deepl_key", "DeepL API key", "password", "Free keys end in :fx"]],
  azure: [["azure_key", "Azure Translator key", "password"], ["azure_region", "Region (e.g. westeurope)", "text", "Leave empty for a global resource"]],
  google: [["google_key", "Google Cloud API key", "password"]],
  libre: [["libre_url", "Server URL", "text"], ["libre_key", "API key (if the server needs one)", "password"]],
  claude: [
    ["claude_key", "Anthropic API key", "password"],
    ["claude_model", "Model", "select", null, [["claude-haiku-5-5", "Claude Haiku 5.5 — cheapest, fast"], ["claude-sonnet-5-5", "Claude Sonnet 5.5 — better"], ["claude-opus-5-5", "Claude Opus 5.5 — best, ~40× Haiku's price"]]],
    ["dialect", "Arabic variety", "select", null, [["msa", "Modern Standard (الفصحى)"], ["egyptian", "Egyptian (مصري)"], ["gulf", "Gulf (خليجي)"], ["levantine", "Levantine (شامي)"], ["saudi", "Saudi (سعودي)"]]],
    ["llm_notes", "Your notes for the model", "textarea", "Optional. Names, places, terms or a tone to keep the same, one per line. They are sent with every line the model translates (ready translations don't use them)."],
  ],
  openai: [
    ["openai_key", "OpenAI API key", "password"],
    ["openai_model", "Model", "select", null, [["gpt-6-luna", "GPT-6 Luna — cheapest, fast"], ["gpt-6.1-terra", "GPT-6.1 Terra — better"], ["gpt-6.2-sol", "GPT-6.2 Sol — best, ~20× Luna's price"]]],
    ["dialect", "Arabic variety", "select", null, [["msa", "Modern Standard (الفصحى)"], ["egyptian", "Egyptian (مصري)"], ["gulf", "Gulf (خليجي)"], ["levantine", "Levantine (شامي)"], ["saudi", "Saudi (سعودي)"]]],
    ["llm_notes", "Your notes for the model", "textarea", "Optional. Names, places, terms or a tone to keep the same, one per line. They are sent with every line the model translates (ready translations don't use them)."],
  ],
};

let LANGS = [];

function renderLang() {
  const sel = $("target-lang");
  if (!sel.options.length) {
    for (const l of LANGS) sel.add(new Option(`${l.name} — ${l.native}`, l.code));
    sel.addEventListener("change", () => {
      cfg.target_lang = sel.value;
      AS.setLang(cfg.target_lang);
      for (const k of Object.keys(lastSegs)) delete lastSegs[k];
      renderLang();
      renderEngines();
      renderAreas();
      renderProfile();
      renderPages();
      save();
    });
  }
  sel.value = cfg.target_lang || "ar";
  const l = LANGS.find((x) => x.code === sel.value);
  $("lang-note").textContent = !l ? "" : l.local && hasOffline
    ? t("All translation services work with {name}, including the free offline one.", { name: l.name })
    : t("{name} has no free offline model: choose DeepL, Azure, Google, LibreTranslate or Claude.", { name: l.name });
}

/**
 * VI Translator's ready translations first (on by default): turning them off asks first, as every line
 * then waits for the translator.
 */
function renderShared() {
  const box = $("shared_translations");
  box.checked = cfg.shared_translations !== false;
  if (box.dataset.wired) return;
  box.dataset.wired = "1";
  box.addEventListener("change", async () => {
    if (!box.checked) {
      // (with no translator, only the memory: lines the ready translations had aren't translated at all)
      const none = cfg.engine === "local" && !hasOffline;
      const text = t("Turn off VI Translator's ready translations? Every line will then wait for the translator chosen below, so translations will come much slower.")
        + (none ? " " + t("With no translation service chosen, lines not in your own memory won't be translated at all.") : "");
      if (!(await ask(text, t("Turn off")))) {
        box.checked = true;
        return;
      }
    }
    cfg.shared_translations = box.checked;
    save(false);
  });
}

function renderEngines() {
  $("engines").innerHTML = "";
  for (const e of ENGINES) {
    const el = document.createElement("label");
    el.className = `engine${cfg.engine === e.id ? " active" : ""}`;
    el.innerHTML = `<input type="radio" name="engine" ${cfg.engine === e.id ? "checked" : ""} />
      <div><b>${t(e.name)}</b><span class="badge">${t(e.badge)}</span></div><div class="d">${t(e.d)}</div>`;
    el.querySelector("input").addEventListener("change", () => {
      cfg.engine = e.id;
      renderEngines();
      renderSnapForm();
      save(false);
    });
    $("engines").appendChild(el);
  }
  const box = $("engine-fields");
  box.innerHTML = "";
  const form = document.createElement("div");
  form.className = "form";
  // (the Arabic variety only applies to Arabic)
  for (const [key, label, type, help, options] of (ENGINE_FIELDS[cfg.engine] || []).filter((f) => f[0] !== "dialect" || (cfg.target_lang || "ar") === "ar")) {
    const f = document.createElement("div");
    f.className = "field";
    f.innerHTML = `<label>${t(label)}</label>`;
    let input;
    if (type === "select") {
      input = document.createElement("select");
      for (const [v, txt] of options) input.add(new Option(t(txt), v));
    } else if (type === "textarea") {
      input = document.createElement("textarea");
      input.rows = 4;
      input.maxLength = 1500;
      input.spellcheck = false;
      input.dir = "auto";
    } else {
      input = document.createElement("input");
      input.type = type;
      input.spellcheck = false;
    }
    input.value = cfg[key] ?? "";
    input.addEventListener("input", () => { cfg[key] = input.value.trim(); save(false); });
    f.appendChild(input);
    if (help) f.insertAdjacentHTML("beforeend", `<div class="muted small" style="margin:4px 0 0">${t(help)}</div>`);
    form.appendChild(f);
  }
  box.appendChild(form);
}

// ------------------------------------------------------------------------------------- areas

const PLACEMENTS = [["under_text", "Below the game's text box (same left edge, at most its width)"], ["above_text", "Just above the game's text (centred on it)"], ["above", "Above the area"], ["below", "Below the area"], ["right", "To the right"], ["left", "To the left"], ["over", "On top of the area"], ["custom", "Custom position (drag it in the preview)"]];
// Where the Arabic box sits inside its region (preset placements).
const TEXT_POS = [
  ["", "Automatic for this placement"],
  ["top-left", "Top left"], ["top-center", "Top centre"], ["top-right", "Top right"],
  ["middle-left", "Middle left"], ["middle-center", "Middle centre"], ["middle-right", "Middle right"],
  ["bottom-left", "Bottom left"], ["bottom-center", "Bottom centre"], ["bottom-right", "Bottom right"],
];
const POS_LABELS = { x: "Left %", y: "Top %", w: "Width %", h: "Height %" }; // (translated where drawn)
const OUT_KEYS = { x: "out_x", y: "out_y", w: "out_w", h: "out_h" };

// Four percent inputs for a rectangle stored as fractions under the given keys.
function posGrid(a, keys, cls) {
  return `<div class="grid4">${["x", "y", "w", "h"].map((k) => `<div><label>${t(POS_LABELS[k])}</label>
    <input type="number" class="${cls}-${k}" step="0.1" min="0" max="100" value="${(a[keys[k]] * 100).toFixed(1)}" /></div>`).join("")}</div>`;
}
function bindPosGrid(card, a, keys, cls) {
  for (const k of ["x", "y", "w", "h"]) {
    card.querySelector(`.${cls}-${k}`).addEventListener("change", (e) => {
      a[keys[k]] = round(Math.min(Math.max(Number(e.target.value) / 100, k === "w" || k === "h" ? 0.02 : 0), 1));
      if (a[keys.x] + a[keys.w] > 1) a[keys.w] = round(1 - a[keys.x]);
      if (a[keys.y] + a[keys.h] > 1) a[keys.h] = round(1 - a[keys.y]);
      updateAreaNumbers(a);
      save();
    });
  }
}

// How an area's translation comes on screen (config.rs AreaFx): kept for every profile, built-in
// ones too (whose areas can't be changed: data-more keeps these on), by area id.
const FADES = [0, 100, 150, 250, 400, 600];
function fxFields(a) {
  const fx = AS.fxOf(cfg.fx_view || cfg.area_fx, a.id);
  const cover = AS.coverLook(cfg.profile_opts);
  const dis = cover ? "" : "disabled";
  return `
      <div class="sub">${t("Showing the translation")}</div>
      <label class="check"><input type="checkbox" class="fx-min" data-more="1" ${fx.cover_min ? "checked" : ""} ${dis}/> ${t("Black box at least as big as the game's text")}</label>
      <label class="check"><input type="checkbox" class="fx-early" data-more="1" ${fx.cover_early ? "checked" : ""} ${dis}/> ${t("Black box as soon as the text appears, before its translation")}</label>
      ${cover ? "" : `<p class="muted small">${t("The black box is for the look \"Over the game's text\" (Game profile).")}</p>`}
      <label class="check"><input type="checkbox" class="fx-stack" data-more="1" ${fx.stack_up ? "checked" : ""}/> ${t("Lines stack up from the bottom line: the lines above are read only when the bottom one is there")}</label>
      <label class="check"><input type="checkbox" class="fx-ride" data-more="1" ${fx.ride ? "checked" : ""} ${cover ? "disabled" : ""}/> ${t("Right above the stacked lines, moving up with them (with the subtitles and the bottom instructions as one block)")}</label>
      ${cover ? `<p class="muted small">${t("Moving above the lines is for the look \"Above the game's text\" (Game profile).")}</p>` : ""}
      <label class="check"><input type="checkbox" class="fx-accent" data-more="1" ${fx.accent ? "checked" : ""}/> ${t("A border in Vice City colours, to tell it apart")}</label>
      <label class="check"><input type="checkbox" class="fx-reviewed" data-more="1" ${fx.reviewed_only ? "checked" : ""}/> ${t("Only reviewed translations: a reading that matches none isn't shown")}</label>
      <div class="row field inline-label">
        <label>${t("Fade in")}</label>
        <select class="fx-fade grow" data-more="1">${FADES.map((ms) => `<option value="${ms}" ${fx.fade_ms === ms ? "selected" : ""}>${ms ? `${ms} ms` : t("Off")}</option>`).join("")}</select>
      </div>`;
}
function bindFx(c, a) {
  const set = (k, v) => {
    // (only what the player changed is theirs: an option they never touched — the fade-in — follows the game's
    // default, which a layout update can change; the view the drawing goes by, at once)
    cfg.area_fx = cfg.area_fx || {};
    cfg.area_fx[a.id] = Object.assign({}, cfg.area_fx[a.id] || {}, { [k]: v });
    cfg.fx_view = cfg.fx_view || {};
    cfg.fx_view[a.id] = Object.assign(AS.fxOf(cfg.fx_view, a.id), { [k]: v });
    save();
  };
  c.querySelector(".fx-min").addEventListener("change", (e) => set("cover_min", e.target.checked));
  c.querySelector(".fx-early").addEventListener("change", (e) => set("cover_early", e.target.checked));
  c.querySelector(".fx-fade").addEventListener("change", (e) => set("fade_ms", Number(e.target.value)));
  c.querySelector(".fx-stack").addEventListener("change", (e) => set("stack_up", e.target.checked));
  c.querySelector(".fx-ride").addEventListener("change", (e) => set("ride", e.target.checked));
  c.querySelector(".fx-accent").addEventListener("change", (e) => set("accent", e.target.checked));
  c.querySelector(".fx-reviewed").addEventListener("change", (e) => set("reviewed_only", e.target.checked));
}

function renderAreas() {
  const list = $("area-list");
  list.innerHTML = "";
  for (const a of cfg.areas) {
    const c = document.createElement("div");
    c.className = `card${a.id === selected ? " selected" : ""}`;
    c.dataset.id = a.id;
    c.innerHTML = `
      <div class="row">
        <input type="checkbox" class="en" ${a.enabled ? "checked" : ""} title="${t("Read this area")}" />
        <input type="text" class="nm grow" value="${AS.esc(a.name)}" />
        <button class="btn icon danger del" title="${t("Delete")}"><svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg></button>
      </div>
      <div class="row">
        <select class="pl grow">${PLACEMENTS.map(([v, txt]) => `<option value="${v}" ${a.placement === v ? "selected" : ""}>${t("Translation: {where}", { where: t(txt) })}</option>`).join("")}</select>
      </div>
      <div class="row field inline-label">
        <label>${t("Font")}</label>
        <select class="ff grow">${fontOptions(a.font_family, t("Main font ({font})", { font: AS.styleOf(a, cfg.style).font_family }))}</select>
      </div>
      <div class="row field inline-label">
        <label>${t("Background")}</label>
        <select class="bgm grow">${[["", "Its text style's box"], ["none", "None — outlined text, like game subtitles"], ["game", "Game-style dark box"]]
          .map(([v, txt]) => `<option value="${v}" ${(a.background || "") === v ? "selected" : ""}>${t(txt)}</option>`).join("")}</select>
      </div>
      <label class="check"><input type="checkbox" class="ms" ${a.match_size !== false ? "checked" : ""}/> ${t("Match the game's text size")}</label>
      <label class="check"><input type="checkbox" class="co" ${a.centered_only ? "checked" : ""}/> ${t("Only text centred in the picture (subtitles)")}</label>
      ${fxFields(a)}
      <div class="row field inline-label">
        <label>${a.match_size !== false ? t("Fine-tune") : t("Text size")}</label>
        <input type="range" class="fs" ${rangeAttrs(a.font_scale, 0.5, 2, 0.05)} />
        <span class="val fsv">${Math.round(a.font_scale * 100)}%</span>
      </div>
      <details class="area-style"${styleOpen.has(a.id) ? " open" : ""}><summary>${t("Text style")}</summary><div class="form"></div></details>
      <div class="sub">${t("Read area")}</div>
      ${posGrid(a, { x: "x", y: "y", w: "w", h: "h" }, "n")}
      ${a.placement !== "custom" ? `
        <div class="row field inline-label">
          <label>${t("Text sits")}</label>
          <select class="tp grow">${TEXT_POS.map(([v, txt]) => `<option value="${v}" ${(a.text_pos || "") === v ? "selected" : ""}>${t(txt)}</option>`).join("")}</select>
        </div>` : ""}
      ${a.placement === "custom" ? `
        <div class="sub">${t("Translation region")}</div>
        ${posGrid(a, OUT_KEYS, "o")}
        <div class="row">
          <select class="va grow">${[["top", "Text at the top"], ["middle", "Text in the middle"], ["bottom", "Text at the bottom"]]
            .map(([v, txt]) => `<option value="${v}" ${a.out_valign === v ? "selected" : ""}>${t(txt)}</option>`).join("")}</select>
          <select class="ha grow">${[["right", "Right"], ["center", "Centre"], ["left", "Left"]]
            .map(([v, txt]) => `<option value="${v}" ${a.out_halign === v ? "selected" : ""}>${t(txt)}</option>`).join("")}</select>
        </div>` : ""}`;
    c.addEventListener("mousedown", () => { if (selected !== a.id) { selected = a.id; highlightCards(); drawLayer(); } });
    c.querySelector(".en").addEventListener("change", (e) => { a.enabled = e.target.checked; setShown("area", a.id, a.enabled); renderPages(); save(); });
    c.querySelector(".nm").addEventListener("input", (e) => { a.name = e.target.value; save(); });
    c.querySelector(".pl").addEventListener("change", (e) => {
      if (e.target.value === "custom") toCustom(a); // start the custom region where the Arabic is now
      else a.placement = e.target.value;
      renderAreas(); // shows/hides the custom fields
      save();
    });
    if (a.placement !== "custom") {
      c.querySelector(".tp").addEventListener("change", (e) => { a.text_pos = e.target.value; save(); });
    }
    if (a.placement === "custom") {
      bindPosGrid(c, a, OUT_KEYS, "o");
      c.querySelector(".va").addEventListener("change", (e) => { a.out_valign = e.target.value; save(); });
      c.querySelector(".ha").addEventListener("change", (e) => { a.out_halign = e.target.value; save(); });
    }
    c.querySelector(".bgm").addEventListener("change", (e) => { a.background = e.target.value; save(); });
    const det = c.querySelector(".area-style");
    det.addEventListener("toggle", () => { if (det.open) styleOpen.add(a.id); else styleOpen.delete(a.id); });
    renderStyleForm(det.querySelector(".form"), a);
    c.querySelector(".ms").addEventListener("change", (e) => { a.match_size = e.target.checked; renderAreas(); save(); });
    c.querySelector(".co").addEventListener("change", (e) => { a.centered_only = e.target.checked; save(); });
    bindFx(c, a);
    const ff = c.querySelector(".ff");
    paintFonts(ff);
    ff.style.fontFamily = fontCss(a.font_family || cfg.style.font_family);
    ff.addEventListener("change", (e) => {
      a.font_family = e.target.value;
      e.target.style.fontFamily = fontCss(a.font_family || cfg.style.font_family);
      save();
    });
    c.querySelector(".fs").addEventListener("input", (e) => {
      a.font_scale = rangeMoved(e.target, 0.5, 2, 0.05);
      c.querySelector(".fsv").textContent = `${Math.round(a.font_scale * 100)}%`;
      save();
    });
    bindPosGrid(c, a, { x: "x", y: "y", w: "w", h: "h" }, "n");
    c.querySelector(".del").addEventListener("click", async () => {
      if (!(await ask(t('Delete the read area "{name}"?', { name: a.name }), t("Delete")))) return;
      cfg.areas = cfg.areas.filter((x) => x !== a);
      delete lastSegs[a.id];
      renderAreas();
      save();
    });
    list.appendChild(c);
  }
  lockControls();
}
function highlightCards() {
  document.querySelectorAll("#area-list .card").forEach((c) => c.classList.toggle("selected", c.dataset.id === selected));
}
function updateAreaNumbers(a) {
  const c = document.querySelector(`#area-list .card[data-id="${CSS.escape(a.id)}"]`);
  if (!c) return;
  for (const k of ["x", "y", "w", "h"]) {
    c.querySelector(`.n-${k}`).value = (a[k] * 100).toFixed(1);
    const o = c.querySelector(`.o-${k}`);
    if (o) o.value = (a[OUT_KEYS[k]] * 100).toFixed(1);
  }
}
function addArea() {
  if (locked()) return;
  const id = `area_${Date.now().toString(36)}`;
  cfg.areas.push({
    id, name: t("Area {n}", { n: cfg.areas.length + 1 }), enabled: true, x: 0.35, y: 0.4, w: 0.3, h: 0.12, placement: "above", font_scale: 1, font_family: "",
    out_x: 0.35, out_y: 0.25, out_w: 0.3, out_h: 0.15, out_valign: "bottom", out_halign: "center",
  });
  selected = id;
  renderAreas();
  save();
}
$("add-area").addEventListener("click", addArea);
$("add-area-2").addEventListener("click", addArea);

// ------------------------------------------------------------------------------------- style

// Installed fonts from the backend: [{ name, covers: [language codes it can write] }].
// Fonts that can write the target language are listed first.
const fitsLang = (f) => (f.covers || []).includes(cfg.target_lang || "ar");
const langNative = () => (LANGS.find((l) => l.code === (cfg.target_lang || "ar")) || { native: "العربية" }).native;
let FONT_LIST = [];
// Free Arabic fonts shipped with the app (ui/fonts, SIL Open Font License).
const BUNDLED_FONTS = [
  { name: "Lalezar", note: "heavy display, great for game titles" },
  { name: "Changa", note: "bold geometric" },
  { name: "Cairo", note: "clean, many weights" },
  { name: "Tajawal", note: "clean text" },
  { name: "Noto Sans Arabic Medium", note: "like GTA VI's subtitles" },
];

/** A font name as a CSS font-family value (a quoted string): for `el.style.fontFamily` only. */
const fontCss = (name) => JSON.stringify(String(name || ""));

/** Draw each font <option> of a select in its own font (see fontOptions). */
function paintFonts(sel) {
  for (const o of sel.querySelectorAll("option")) if (o.value) o.style.fontFamily = fontCss(o.value);
}

// <option>s for a font <select> (then `paintFonts` draws each name in its own font). `defaultLabel`
// adds a "use main font" choice.
function fontOptions(current, defaultLabel) {
  const opt = (f) =>
    `<option value="${AS.esc(f.name)}" ${f.name === current ? "selected" : ""}>${AS.esc(f.name)}${f.note ? ` — ${AS.esc(t(f.note))}` : ""}</option>`;
  let html = defaultLabel ? `<option value="" ${current ? "" : "selected"}>${AS.esc(defaultLabel)}</option>` : "";
  const isBundled = (n) => BUNDLED_FONTS.some((b) => b.name === n);
  if (current && !isBundled(current) && !FONT_LIST.some((f) => f.name === current)) {
    html += `<option value="${AS.esc(current)}" selected>${AS.esc(t("{font} (not installed)", { font: current }))}</option>`;
  }
  html += `<optgroup label="${t("Included with the app")}">${BUNDLED_FONTS.map(opt).join("")}</optgroup>`;
  const fit = FONT_LIST.filter((f) => fitsLang(f) && !isBundled(f.name));
  const other = FONT_LIST.filter((f) => !fitsLang(f) && !isBundled(f.name));
  const language = langNative();
  if (fit.length) html += `<optgroup label="${AS.esc(t("Fonts for {language}", { language }))}">${fit.map(opt).join("")}</optgroup>`;
  if (other.length) html += `<optgroup label="${AS.esc(t("Other fonts ({language} falls back to a default font)", { language }))}">${other.map(opt).join("")}</optgroup>`;
  return html;
}

const STYLE_FIELDS = [
  ["font_family", "Font", "font"],
  ["font_size", "Text size (px at 1080p; scales with the game window)", "size", 10, 96, 1],
  ["bold", "Bold", "check"],
  ["line_height", "Line spacing", "range", 1, 2.2, 0.05],
  ["text_color", "Text color", "color"],
  ["keep_colors", "Color highlighted words like the game does", "check"],
  ["bg_color", "Box color", "color"],
  ["bg_opacity", "Box opacity", "range", 0, 1, 0.05],
  ["padding", "Box padding", "range", 0, 30, 1],
  ["radius", "Box corner roundness", "range", 0, 30, 1],
  ["outline_color", "Text outline color", "color"],
  ["outline_width", "Text outline width", "range", 0, 6, 0.5],
  ["shadow", "Text shadow", "check"],
];

// Read areas whose Text style section is open (kept open when the list is drawn again).
const styleOpen = new Set();

function renderStyleForm(form, a) {
  form.innerHTML = "";
  const s = AS.styleOf(a, cfg.style);
  // the area's own style from its first change on (it starts from the profile's)
  const setStyle = (key, value) => {
    if (!a.style) a.style = clone(cfg.style);
    a.style[key] = value;
    drawLayer();
    save();
  };
  for (const [key, label, type, min, max, step] of STYLE_FIELDS) {
    if (type === "font") continue; // (the card's own Font choice)
    const f = document.createElement("div");
    f.className = "field";
    if (type === "check") {
      f.innerHTML = `<label class="check" style="color:var(--text)"><input type="checkbox" ${s[key] ? "checked" : ""}/> ${t(label)}</label>`;
      f.querySelector("input").addEventListener("change", (e) => setStyle(key, e.target.checked));
    } else if (type === "size") {
      // slider + exact number, kept in sync
      f.innerHTML = `<label>${t(label)}</label><div class="inline"><input type="range" ${rangeAttrs(s[key], min, max, step)} />
        <input type="number" min="${min}" max="${max}" step="${step}" value="${AS.esc(s[key])}" style="width:72px" /><span class="val" style="min-width:18px">px</span></div>`;
      const [range, num] = f.querySelectorAll("input");
      // the text size, while the area matches the game's: the game's size is used (its Fine-tune
      // changes it), so this one is greyed out and says so
      if (key === "font_size" && a.match_size !== false) {
        range.disabled = num.disabled = true;
        range.dataset.matched = num.dataset.matched = "1"; // (lockControls keeps them off)
        f.style.opacity = "0.55";
        f.insertAdjacentHTML("beforeend", `<div class="muted small" style="margin:4px 0 0">${t("Follows the game's text size. Change it with Fine-tune above, or untick “Match the game's text size”.")}</div>`);
      }
      range.addEventListener("input", () => { const v = rangeMoved(range, min, max, step); num.value = v; setStyle(key, v); });
      num.addEventListener("change", () => {
        const v = Math.min(Math.max(Number(num.value) || s[key], min), max);
        num.value = range.value = v;
        setStyle(key, v);
      });
    } else if (type === "color") {
      f.innerHTML = `<label>${t(label)}</label><div class="inline"><input type="color" value="${AS.esc(s[key])}" /><span class="val">${AS.esc(s[key])}</span></div>`;
      f.querySelector("input").addEventListener("input", (e) => { f.querySelector(".val").textContent = e.target.value; setStyle(key, e.target.value); });
    } else {
      const fmt = (v) => (step < 1 ? Number(v).toFixed(2) : String(v));
      f.innerHTML = `<label>${t(label)}</label><div class="inline"><input type="range" ${rangeAttrs(s[key], min, max, step)} /><span class="val">${AS.esc(fmt(rangeShown(s[key], min, max, step).value))}</span></div>`;
      f.querySelector("input").addEventListener("input", (e) => { const v = rangeMoved(e.target, min, max, step); f.querySelector(".val").textContent = fmt(v); setStyle(key, v); });
    }
    form.appendChild(f);
  }
  // back to the style the profile gives its areas
  if (a.style) {
    const reset = document.createElement("button");
    reset.className = "btn";
    reset.textContent = t("Reset to defaults");
    reset.addEventListener("click", () => {
      a.style = null;
      renderStyleForm(form, a);
      drawLayer();
      save();
    });
    form.appendChild(reset);
  }
  lockControls();
}

// ------------------------------------------------------------------------------------- timing

// Sliders whose number, dot and saved value agree. A value the slider's steps don't reach (a game's
// timing, 80 ms on a 50 ms step) or outside its range would have its dot jump to the nearest step
// while the number said otherwise: it is shown where it really is (within the range, as the app
// uses it), on steps of its own until the slider is moved; a moved slider takes its own steps again.
function rangeShown(v, min, max, step) {
  const x = Math.min(Math.max(Number(v), min), max);
  const k = (x - min) / step;
  const onStep = Math.abs(k - Math.round(k)) < 1e-6;
  return { value: Number.isFinite(x) ? x : min, step: onStep ? step : "any" };
}
/** The attributes of such a slider. */
function rangeAttrs(v, min, max, step) {
  const r = rangeShown(v, min, max, step);
  return `min="${min}" max="${max}" step="${r.step}" value="${r.value}"`;
}
/** A moved slider's value, on its own steps (and back on them, if it was shown between two). */
function rangeMoved(input, min, max, step) {
  const raw = Number(input.value);
  const v = Math.min(Math.max(min + Math.round((raw - min) / step) * step, min), max);
  const fixed = Number(v.toFixed(6)); // (0.1 + 0.2: no float dust in the settings)
  if (input.step !== String(step)) input.step = step;
  if (Number(input.value) !== fixed) input.value = fixed;
  return fixed;
}

// The preview's fps badge: shown or not, and which corner (Show on → Preview).
const FPS_CORNERS = [["top-left", "Top left"], ["top-right", "Top right"], ["bottom-left", "Bottom left"], ["bottom-right", "Bottom right"]];
function applyFpsBadge() {
  const corner = FPS_CORNERS.some(([v]) => v === cfg.fps_corner) ? cfg.fps_corner : "top-right";
  $("fps").dataset.corner = corner;
  document.body.classList.toggle("no-fps", cfg.fps_badge !== true);
}
function renderPreviewForm() {
  const form = $("preview-form");
  form.innerHTML = `
    <label class="check"><input type="checkbox" id="fps-on" ${cfg.fps_badge === true ? "checked" : ""}/> ${t("Show frames per second (fps) on the preview")}</label>
    <div class="field"><label for="fps-corner">${t("Where it shows")}</label>
      <select id="fps-corner">${FPS_CORNERS.map(([v, txt]) => `<option value="${v}" ${cfg.fps_corner === v ? "selected" : ""}>${t(txt)}</option>`).join("")}</select></div>`;
  const corner = $("fps-corner");
  corner.disabled = cfg.fps_badge !== true;
  $("fps-on").addEventListener("change", (e) => { cfg.fps_badge = e.target.checked; corner.disabled = !e.target.checked; applyFpsBadge(); save(false); });
  corner.addEventListener("change", () => { cfg.fps_corner = corner.value; applyFpsBadge(); save(false); });
  applyFpsBadge();
  corner.value = $("fps").dataset.corner; // the corner actually used (an unknown value falls back to top right)
}

function renderTiming() {
  const form = $("timing-form");
  form.innerHTML = "";
  // the 4K picture: a 4K capture card runs at 4K, and the app's picture is shown at up to 3840 wide
  const k4 = document.createElement("div");
  k4.className = "field";
  // (flex: a form's labels are blocks, which would leave the box at the top of the line, above the words)
  k4.innerHTML = `<label class="check" style="display:flex;color:var(--text);font-weight:600"><input type="checkbox" ${cfg.picture_4k === true ? "checked" : ""}/> ${t("4K picture")}</label>
    <div class="muted small" style="margin:4px 0 0">${t("A 4K capture card runs at 4K, and the picture in this app is shown in 4K (also a 4K game window). For playing on this app's picture on a 4K screen. Needs a strong PC; text is read either way.")}</div>`;
  k4.querySelector("input").addEventListener("change", (e) => { cfg.picture_4k = e.target.checked; save(false); });
  form.appendChild(k4);
  const fields = [
    ["capture_fps", "Frames captured for reading text (per second)", 1, 60, 1, "Full-size frames used for reading text, game pages and saved frames. Also how often the pause menu checks whether its text changed. 12 is plenty; more costs CPU, especially at 4K. From OBS, about 12 is the most it can deliver."],
    ["preview_fps", "Live preview frame rate (per second)", 5, 60, 5, "Only the picture in this app (small frames, made only while it's visible). Doesn't change how text is read."],
    ["interval_ms", "How often areas are read (ms)", 16, 1000, 2, "Lower = faster reaction, more CPU."],
    ["stable_ms", "Wait for text to settle before translating (ms)", 0, 1500, 10, "Avoids translating half-faded lines."],
    ["clear_after_ms", "Treat the English as gone after (ms)", 50, 3000, 10, "Ignores short OCR blinks. The translation then stays as long as the English was on screen, and switches to the next line as soon as it is ready."],
  ];
  // The last three: the game profile's timing unless the player chose their own (Config::timing).
  const TIMED = ["interval_ms", "stable_ms", "clear_after_ms"];
  const game = activeProfile()?.timing || {};
  const gameHas = TIMED.some((k) => game[k] > 0);
  const inUse = (k) => (cfg.timing_custom === true || !(game[k] > 0) ? cfg[k] : game[k]);
  for (const [key, label, min, max, step, help] of fields) {
    const timed = TIMED.includes(key);
    const v = rangeShown(timed ? inUse(key) : cfg[key], min, max, step).value;
    const f = document.createElement("div");
    f.className = "field";
    f.innerHTML = `<label>${t(label)}</label><div class="inline"><input type="range" ${rangeAttrs(v, min, max, step)} /><span class="val">${v}</span></div>${help ? `<div class="muted small" style="margin:4px 0 0">${t(help)}</div>` : ""}`;
    f.querySelector("input").addEventListener("input", (e) => {
      // moving one of the three: the player's own timing from now on, starting from what was in use
      if (timed && cfg.timing_custom !== true) {
        for (const k of TIMED) cfg[k] = inUse(k);
        cfg.timing_custom = true;
        if (gameHas) renderTimingReset(form);
      }
      cfg[key] = rangeMoved(e.target, min, max, step);
      f.querySelector(".val").textContent = cfg[key];
      save(false);
    });
    form.appendChild(f);
  }
  if (gameHas && cfg.timing_custom === true) renderTimingReset(form);
}

// "Use the game's timing": back to the game profile's own (shown while the player's is in use).
function renderTimingReset(form) {
  if (form.querySelector(".timing-reset")) return;
  const row = document.createElement("div");
  row.className = "timing-reset";
  row.innerHTML = `<button class="btn" type="button">${t("Use the game's timing")}</button>`;
  row.querySelector("button").addEventListener("click", () => {
    cfg.timing_custom = false;
    save(false);
    renderTiming();
  });
  form.appendChild(row);
}

// ------------------------------------------------------------------------------------- translation memory

function renderMemory() {
  $("memory-form").innerHTML = `
    <div class="callout" id="mem-stats" style="margin:0"></div>
    <div class="row"><button class="btn danger wide" id="mem-clear">${t("Clear memory")}</button></div>
`;
  $("mem-clear").addEventListener("click", async () => {
    if (!(await ask(t("Forget all remembered translations?"), t("Clear memory")))) return;
    await invoke("memory_clear");
    updateMemory();
  });
  updateMemory();
}

async function updateMemory() {
  const el = $("mem-stats");
  if (!el) return;
  try {
    const m = await invoke("memory_status");
    const lines = [t("{n} remembered line(s)", { n: m.entries })];
    if (m.hits + m.misses > 0) {
      const rate = Math.round((100 * m.hits) / (m.hits + m.misses));
      lines.push(t("This session: {hits} from memory, {misses} translated ({rate}% hits)", { hits: m.hits, misses: m.misses, rate }));
      lines.push(t("Translation ready after the English appeared: memory {a} · translator {b}", {
        a: m.hits ? Math.round(m.avg_hit_ms) + " ms" : "—", b: m.misses ? Math.round(m.avg_miss_ms) + " ms" : "—" }));
      if (m.hits && m.misses) lines.push(t("Saved about {ms} ms per remembered line", { ms: Math.round(m.avg_miss_ms - m.avg_hit_ms) }));
    } else {
      lines.push(t("No lines seen yet this session."));
    }
    // how the app keeps up (last minute): a slow PC, or one that slows down after a long session, shows here
    try {
      const p = await invoke("perf_status");
      if (p && p.read) {
        const ms = (x) => (x ? `${x.avg} ms` : "—");
        lines.push(t("Last minute: reading {read} per pass · translating {tr} · translation ready {ready} after the English (slowest {max})", {
          read: ms(p.read), tr: ms(p.translate), ready: ms(p.ready), max: p.ready ? `${p.ready.max} ms` : "—" }));
        lines.push(t("Running for {m} min", { m: p.minutes }));
      }
    } catch {}
    el.innerHTML = lines.map(AS.esc).join("<br>");
  } catch (e) {
    el.textContent = String(e);
  }
}
setInterval(() => { if (!$("tab-translator").classList.contains("hidden")) updateMemory(); }, 2000);


// ------------------------------------------------------------------------------------- snapshot

// The whole picture translated at once, like a translated photo (Setup: the Snapshot tab; Play: the
// camera button; the home-network page has one too). Each is kept in Pictures\VI Translator\snapshots.
let lastSnap = null;
let snapBusy = false;

function snapStatus(text, error = false) {
  const el = $("snap-status");
  el.hidden = !text;
  el.textContent = text || "";
  el.classList.toggle("err", error);
}

function snapLabels() {
  return { translated: t("Translated"), original: t("Original"), save: t("Save"), close: t("Close"), folder: t("Open snapshots folder") };
}

function serviceName(id) {
  if (id === "azure-image") return t("Azure image translation");
  if (id === "gemini-image") return "Gemini (Nano Banana)";
  const e = ENGINES.find((x) => x.id === id);
  return e ? t(e.name) : id;
}

async function takeSnapshot() {
  if (snapBusy) return;
  snapBusy = true;
  $("snap-btn").classList.add("busy");
  $("snap-take").disabled = true;
  snapStatus(t("Translating the whole picture…"));
  try {
    const snap = await invoke("take_snapshot");
    await gotSnapshot(snap, true);
  } catch (e) {
    snapStatus(t("Could not translate the picture: {e}", { e }), true);
    log({ error: t("Could not translate the picture: {e}", { e }) });
  } finally {
    snapBusy = false;
    $("snap-btn").classList.remove("busy");
    $("snap-take").disabled = false;
  }
}

// A snapshot made (here, or from the home-network page): kept (the app's drawing of it), shown in the tab.
async function gotSnapshot(snap, open) {
  lastSnap = snap;
  const done = snap.translated_image
    ? t("Translated by {service} in {s} s", { service: serviceName(snap.service), s: (snap.ms / 1000).toFixed(1) })
    : t("{n} text block(s) translated by {service} in {s} s", { n: snap.blocks.length, service: serviceName(snap.service), s: (snap.ms / 1000).toFixed(1) });
  snapStatus(snap.blocks.length || snap.translated_image ? done : t("No text found on the picture."));
  renderLastSnap();
  if (open) openSnap(snap);
  if (cfg.snap_save && !snap.translated_image && snap.blocks.length) {
    try {
      await invoke("save_snapshot_render", { id: snap.id, lang: snap.lang, data: await Snap.png(snap, cfg.snap_style) });
    } catch (e) {
      log({ error: String(e) });
    }
  }
}

let snapView = null; // the picture on show (to close it with the shortcut)
let snapFromHotkey = false; // opened by the keyboard shortcut: closing it goes back to the game
function openSnap(snap) {
  snapView = Snap.show(snap, {
    onClose: () => {
      snapView = null;
      if (snapFromHotkey) { snapFromHotkey = false; invoke("back_to_game").catch(() => {}); }
    },
    labels: snapLabels(),
    style: cfg.snap_style,
    info: $("snap-status").textContent,
    actions: {
      save: async (canvas, s, translated) => {
        try {
          const file = await invoke("save_snapshot_render", { id: s.id, lang: translated ? s.lang : "original", data: canvas.toDataURL("image/png") });
          log({ info: t("Saved {file}", { file }) });
        } catch (e) {
          log({ error: String(e) });
        }
      },
      folder: openSnapsFolder,
    },
  });
}

async function openSnapsFolder() {
  try {
    log({ info: t("Opened {dir}", { dir: await invoke("open_snapshots_folder") }) });
  } catch (e) {
    log({ error: String(e) });
  }
}

function renderLastSnap() {
  const box = $("snap-last");
  if (!lastSnap) {
    box.innerHTML = "";
    return;
  }
  box.innerHTML = `<img alt="" src="${AS.esc(lastSnap.translated_image || lastSnap.image)}" /><div class="muted small">${AS.esc(t("Last snapshot — click to open"))}</div>`;
  box.querySelector("img").addEventListener("click", () => openSnap(lastSnap));
}

const GEMINI_MODELS = [
  ["gemini-nano-banana-2.1", "Nano Banana 2.1 — recommended, about $0.05 a picture"],
  ["gemini-3-pro-image", "Nano Banana Pro — best text, about $0.13 a picture"],
  ["gemini-3.1-flash-lite-image", "Nano Banana 2 Lite — fastest, about $0.03 a picture"],
];

function renderSnapForm() {
  const form = $("snap-form");
  const live = ENGINES.find((x) => x.id === cfg.engine);
  const opts = [["", t("Same as live translations ({name})", { name: live ? t(live.name) : cfg.engine })]];
  for (const e of ENGINES) opts.push([e.id, e.id === "claude" || e.id === "openai" ? t("{name} — reads the picture", { name: t(e.name) }) : t(e.name)]);
  opts.push(["gemini-image", t("Gemini (Nano Banana) — redraws the picture in the language")]);
  opts.push(["azure-image", t("Azure image translation — picture in, picture out")]);
  const svc = cfg.snap_service || "";
  const used = svc || cfg.engine;
  const help = used === "gemini-image"
    ? t("Google's image model redraws the whole picture with its text in the language: the most natural look. It can change small details (an icon, a number, a car), takes 10–30 s and Google bills about $0.03–0.13 per picture. Needs a Gemini API key from Google AI Studio.")
    : used === "azure-image"
    ? t("Azure redraws the picture with the translation. Needs an Azure Translator resource with its own endpoint (https://<name>.cognitiveservices.azure.com) and its key. Billed by Azure per picture.")
    : used === "claude" || used === "openai"
      ? t("The picture and the text the app found go to the model in one request: it fixes misread words and translates with the whole screen as context. Uses the key and model from the Translator tab.")
      : t("Each piece of text is translated on its own, the translation memory first, so the game's words match the subtitles.");
  form.innerHTML = `
    <div class="field"><label for="snap-service">${t("Service")}</label>
      <select id="snap-service">${opts.map(([v, txt]) => `<option value="${v}" ${v === svc ? "selected" : ""}>${AS.esc(txt)}</option>`).join("")}</select>
      <div class="muted small" style="margin:4px 0 0">${AS.esc(help)}</div></div>
    ${used === "azure-image" ? `
    <div class="field"><label for="snap-az-ep">${t("Azure endpoint")}</label><input type="text" id="snap-az-ep" spellcheck="false" placeholder="https://<name>.cognitiveservices.azure.com" value="${AS.esc(cfg.snap_azure_endpoint || "")}" /></div>
    <div class="field"><label for="snap-az-key">${t("Azure key")}</label><input type="password" id="snap-az-key" autocomplete="off" value="${AS.esc(cfg.snap_azure_key || "")}" /></div>` : ""}
    ${used === "gemini-image" ? `
    <div class="field"><label for="snap-gm-key">${t("Gemini API key")}</label><input type="password" id="snap-gm-key" autocomplete="off" value="${AS.esc(cfg.snap_gemini_key || "")}" /></div>
    <div class="field"><label for="snap-gm-model">${t("Model")}</label><select id="snap-gm-model">${GEMINI_MODELS.map(([v, txt]) => `<option value="${v}" ${v === (cfg.snap_gemini_model || GEMINI_MODELS[0][0]) ? "selected" : ""}>${AS.esc(t(txt))}</option>`).join("")}</select></div>` : ""}
    <div class="field"><label for="snap-style">${t("Behind the translation")}</label>
      <select id="snap-style"><option value="blur" ${cfg.snap_style !== "solid" ? "selected" : ""}>${t("The picture, blurred")}</option><option value="solid" ${cfg.snap_style === "solid" ? "selected" : ""}>${t("A solid color")}</option></select></div>
    <label class="check"><input type="checkbox" id="snap-save" ${cfg.snap_save !== false ? "checked" : ""}/> ${t("Keep every snapshot in Pictures\\VI Translator\\snapshots")}</label>
    <label class="check"><input type="checkbox" id="snap-lan" ${cfg.snap_lan !== false ? "checked" : ""}/> ${t("Allow snapshots from the home-network page")}</label>
    <div class="muted small" style="margin:-6px 0 0">${AS.esc(t("Phones and tablets showing the live translations get a snapshot button too."))}</div>
    <div class="field"><label for="snap-hotkey">${t("Keyboard shortcut")}</label>
      <div class="row"><input type="text" id="snap-hotkey" class="grow" readonly dir="ltr" /><button class="link" id="snap-hotkey-clear">${t("Remove")}</button></div>
      <div class="muted small" id="snap-hotkey-msg" style="margin:4px 0 0">${AS.esc(t("Click the box, then press the keys. Works in the game too: the translated picture comes up at once."))}</div></div>
    <button class="btn" id="snap-folder">${t("Open snapshots folder")}</button>
    <div class="muted small">${AS.esc(t("Text a game page marks as private (other players' names) is never read or sent: it's covered in the picture."))}</div>`;
  $("snap-service").addEventListener("change", (e) => { cfg.snap_service = e.target.value; save(false); renderSnapForm(); });
  $("snap-az-ep")?.addEventListener("input", (e) => { cfg.snap_azure_endpoint = e.target.value.trim(); save(false); });
  $("snap-az-key")?.addEventListener("input", (e) => { cfg.snap_azure_key = e.target.value.trim(); save(false); });
  $("snap-gm-key")?.addEventListener("input", (e) => { cfg.snap_gemini_key = e.target.value.trim(); save(false); });
  $("snap-gm-model")?.addEventListener("change", (e) => { cfg.snap_gemini_model = e.target.value; save(false); });
  $("snap-style").addEventListener("change", (e) => { cfg.snap_style = e.target.value; save(false); });
  $("snap-save").addEventListener("change", (e) => { cfg.snap_save = e.target.checked; save(false); });
  $("snap-lan").addEventListener("change", (e) => { cfg.snap_lan = e.target.checked; save(false); });
  $("snap-folder").addEventListener("click", openSnapsFolder);
  const hk = $("snap-hotkey");
  renderHotkey();
  hk.addEventListener("focus", () => { hk.value = t("Press the keys ..."); });
  hk.addEventListener("blur", renderHotkey);
  hk.addEventListener("keydown", (e) => {
    e.preventDefault();
    if (e.key === "Escape") { hk.blur(); return; }
    if ((e.key === "Backspace" || e.key === "Delete") && !e.ctrlKey && !e.altKey && !e.shiftKey) { hk.blur(); setHotkey(""); return; }
    const keys = hotkeyOf(e);
    if (!keys) return;
    hk.blur();
    setHotkey(keys);
  });
  $("snap-hotkey-clear").addEventListener("click", () => setHotkey(""));
}

// The keyboard shortcut (pressed in the game): the app came to the front; translate the picture and
// show it. Pressed again while it's up: close it (and go back to the game).
listen("snap-hotkey", async (e) => {
  if (e.payload && snapView) { (await snapView).close(); return; }
  if (snapBusy) return;
  snapFromHotkey = true;
  await takeSnapshot();
  if (!snapView) snapFromHotkey = false; // (nothing to show: an error, already in the tab)
});

// The shortcut's keys as the shortcut library names them ("Control+Shift+KeyS"), from a key press;
// null for a press of modifiers alone.
function hotkeyOf(e) {
  if (["Control", "Shift", "Alt", "Meta", "OS"].includes(e.key)) return null;
  const mods = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return [...mods, e.code].join("+");
}
// …and as people read it ("Ctrl + Shift + S")
function hotkeyText(keys) {
  if (!keys) return t("None");
  return keys.split("+").map((k) => ({ Control: "Ctrl", Super: "Win" })[k] || k.replace(/^Key|^Digit/, "")).join(" + ");
}
function renderHotkey() {
  const input = $("snap-hotkey");
  if (!input) return;
  input.value = hotkeyText(cfg.snap_hotkey);
  $("snap-hotkey-clear").hidden = !cfg.snap_hotkey;
}
async function setHotkey(keys) {
  const msg = $("snap-hotkey-msg");
  try {
    await invoke("set_snap_hotkey", { keys });
    cfg.snap_hotkey = keys;
    msg.textContent = keys ? t("Press {keys} in the game to translate the picture; press it again (or Esc) to go back.", { keys: hotkeyText(keys) }) : "";
  } catch (err) {
    msg.textContent = t("These keys can't be used: {e}", { e: err });
  }
  renderHotkey();
}

$("snap-take").addEventListener("click", takeSnapshot);
$("snap-btn").addEventListener("click", (e) => { e.stopPropagation(); takeSnapshot(); });
$("snap-btn").addEventListener("dblclick", (e) => e.stopPropagation());
listen("snapshot", (e) => gotSnapshot(e.payload, false));

// ------------------------------------------------------------------------------------- debugging

let saver = { on: false, count: 0, folder: null };

function renderDebug() {
  const form = $("debug-form");
  form.innerHTML = `
    <label class="check" style="color:var(--text);font-weight:600"><input type="checkbox" id="dbg-save" ${cfg.debug_save_frames ? "checked" : ""}/> ${t("Save frames to disk")}</label>
    <div class="muted small" style="margin:-6px 0 0">${AS.esc(t("Saves full-size PNG files of the captured window into a new folder (in Pictures\\VI Translator\\debug-frames) each time you turn it on. Turning it off opens the folder. 4K frames are a few MB each."))}</div>
    <div class="field"><label>${t("Save a frame every")}</label><div class="inline">
      <input type="range" id="dbg-int" min="100" max="5000" step="100" value="${cfg.debug_frame_interval_ms}" />
      <span class="val" id="dbg-int-v">${t("{s} s", { s: (cfg.debug_frame_interval_ms / 1000).toFixed(1) })}</span></div></div>
    <div class="callout" id="dbg-status" style="margin:0"></div>
    <button class="btn" id="dbg-open">${t("Open frames folder")}</button>
    <button class="btn" id="dbg-shot">${t("Save preview image")}</button>
    <div class="muted small" style="margin:-6px 0 0">${AS.esc(t("Saves the preview as you see it (the game and its translation) as a PNG. The first one makes a new folder in Pictures\\VI Translator\\previews; the next ones go to the same folder until the app is closed."))}</div>
    <button class="link block-link" id="dbg-shot-open">${t("Open previews folder")}</button>
    <div class="muted small">Install id (to send a test version of a game page to this PC): <code>${AS.esc(cfg.install_id || "—")}</code></div>`;
  $("dbg-save").addEventListener("change", (e) => setSaveFrames(e.target.checked));
  $("dbg-int").addEventListener("input", (e) => {
    cfg.debug_frame_interval_ms = Number(e.target.value);
    $("dbg-int-v").textContent = t("{s} s", { s: (cfg.debug_frame_interval_ms / 1000).toFixed(1) });
    save(false);
  });
  $("dbg-open").addEventListener("click", openFramesFolder);
  $("dbg-shot").addEventListener("click", savePreview);
  $("dbg-shot-open").addEventListener("click", async () => {
    try {
      log({ info: t("Opened {dir}", { dir: await invoke("open_previews_folder") }) });
    } catch (e) {
      log({ error: String(e) });
    }
  });
  updateSaver();
}

function setSaveFrames(on) {
  cfg.debug_save_frames = on;
  save(false);
  if ($("dbg-save")) $("dbg-save").checked = on;
  // turning it off: show what was recorded (wait a moment so the last frame is written)
  if (!on) setTimeout(openFramesFolder, 400);
}

// The preview as it is on screen (the game's picture and the translation over it), saved by the app: the
// part of the page the picture takes, inside the visible stage.
async function savePreview() {
  const a = $("frame-wrap").getBoundingClientRect(), v = $("viewport").getBoundingClientRect();
  const x = Math.max(a.left, v.left), y = Math.max(a.top, v.top);
  const area = { x, y, w: Math.min(a.right, v.right) - x, h: Math.min(a.bottom, v.bottom) - y, page_w: window.innerWidth };
  try {
    const file = await invoke("save_preview", { area });
    log({ info: t("Saved {file}", { file }) });
  } catch (e) {
    log({ error: t("Could not save the preview: {e}", { e }) });
  }
}

async function openFramesFolder() {
  try {
    const dir = await invoke("open_frames_folder");
    log({ info: t("Opened {dir}", { dir }) });
  } catch (e) {
    log({ error: t("Could not open the frames folder: {e}", { e }) });
  }
}

function updateSaver() {
  const rec = $("rec");
  rec.classList.toggle("hidden", !saver.on);
  rec.textContent = t("● Saving frames ({n})", { n: saver.count });
  const st = $("dbg-status");
  if (st) {
    st.textContent = saver.on
      ? t("Saving… {n} frame(s) in {folder}", { n: saver.count, folder: saver.folder ?? "…" })
      : saver.folder ? t("Last recording: {n} frame(s) in {folder}", { n: saver.count, folder: saver.folder }) : t("Not saving.");
  }
}

$("rec").addEventListener("click", () => setSaveFrames(false));
listen("frame-saver", (e) => {
  saver = e.payload;
  updateSaver();
});

// ------------------------------------------------------------------------------------- log

function log(ev) {
  const el = document.createElement("div");
  el.className = "item";
  const area = cfg?.areas.find((a) => a.id === ev.area);
  // when the English appeared (or, for messages, now), as HH:MM:SS in the interface's language
  const when = new Date(ev.at || Date.now());
  const time = when.toLocaleTimeString(I18N.lang, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
  if (ev.error) {
    el.innerHTML = `<div class="err">⚠ ${AS.esc(ev.error)}</div><div class="meta">${time}${area ? ` · ${AS.esc(area.name)}` : ""}</div>`;
  } else if (ev.info) {
    el.innerHTML = `<div class="meta">${time} · ${AS.esc(ev.info)}</div>`;
  } else {
    const ar = AS.segHtml(ev.segments);
    const where = area?.name ?? (ev.area === "__screen_reason" ? t("Game page text") : ev.area);
    el.innerHTML = `<div class="head"><span class="when" title="${AS.esc(when.toLocaleString(I18N.lang))}">${time}</span><span class="en" dir="ltr">${ev.en_segments?.length ? AS.segHtml(ev.en_segments) : AS.esc(ev.en)}</span></div><div class="ar" dir="auto">${ar}</div><div class="meta">${AS.esc(where)}${ev.stats ? ` · ${AS.esc(ev.stats)}` : ""}</div>`;
  }
  const box = $("log");
  // the same line sent again with the game's colors (same area and start): it replaces the first
  const key = ev.at && !ev.error && !ev.info ? `${ev.area}|${ev.at}` : "";
  el.dataset.key = key;
  const old = key ? [...box.children].find((e) => e.dataset.key === key) : null;
  if (old) {
    old.replaceWith(el);
    return;
  }
  box.prepend(el);
  while (box.children.length > 100) box.lastChild.remove();
}
$("clear-log").addEventListener("click", () => ($("log").innerHTML = ""));

// The bar between the preview and the live translations: drag it to give the log more room
// (remembered on this PC).
(function logSplit() {
  const wrap = $("log-wrap");
  const set = (h) => {
    const max = $("viewport").parentElement.clientHeight - 160; // leave the preview some room
    wrap.style.height = `${Math.round(Math.min(Math.max(h, 80), Math.max(max, 80)))}px`;
    fitFrame();
  };
  try { const h = Number(localStorage.getItem("logHeight")); if (h) set(h); } catch {}
  $("split").addEventListener("mousedown", (e) => {
    e.preventDefault();
    const start = { y: e.clientY, h: wrap.getBoundingClientRect().height };
    $("split").classList.add("dragging");
    const move = (ev) => set(start.h + (start.y - ev.clientY));
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      $("split").classList.remove("dragging");
      try { localStorage.setItem("logHeight", String(wrap.getBoundingClientRect().height)); } catch {}
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  // double-click: back to the default height
  $("split").addEventListener("dblclick", () => { set(150); try { localStorage.removeItem("logHeight"); } catch {} });
})();

// ------------------------------------------------------------------------------------- events + init

// A capture card's own YUY2 frames, drawn on the GPU the way OBS does: the frame goes up as a texture
// as it is (each texel = two pixels: Y0 U Y1 V) and a shader turns it into color per pixel. No JPEG
// encoding or decoding on the way, full quality.
const YUV = { gl: null, prog: null, tex: null, w: 0, h: 0, u: {} };
function yuvInit() {
  const canvas = $("frame-gl");
  // desynchronized: the canvas goes to the screen without waiting for the rest of the page
  const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false, desynchronized: true, preserveDrawingBuffer: false });
  if (!gl) return false;
  const sh = (type, src) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  };
  const prog = gl.createProgram();
  // one triangle that covers the canvas
  gl.attachShader(prog, sh(gl.VERTEX_SHADER, `#version 300 es
    void main() { vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2); gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0); }`));
  gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, `#version 300 es
    precision highp float;
    uniform highp sampler2D tex;
    uniform int height;
    uniform float yOff, yScale, cScale, kr, kgu, kgv, kb;
    out vec4 color;
    void main() {
      ivec2 p = ivec2(gl_FragCoord.xy);
      vec4 s = texelFetch(tex, ivec2(p.x >> 1, height - 1 - p.y), 0) * 255.0; // Y0 U Y1 V
      float y = ((p.x & 1) == 0 ? s.r : s.b);
      y = (y - yOff) * yScale;
      float u = (s.g - 128.0) * cScale, v = (s.a - 128.0) * cScale;
      color = vec4(clamp(vec3(y + kr * v, y - kgu * u - kgv * v, y + kb * u) / 255.0, 0.0, 1.0), 1.0);
    }`));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
  gl.useProgram(prog);
  for (const n of ["tex", "height", "yOff", "yScale", "cScale", "kr", "kgu", "kgv", "kb"]) YUV.u[n] = gl.getUniformLocation(prog, n);
  const tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.NEAREST], [gl.TEXTURE_MAG_FILTER, gl.NEAREST], [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
  Object.assign(YUV, { gl, prog, tex, w: 0, h: 0 });
  return true;
}
/** Draw one YUY2 frame (`data`: w × h × 2 bytes). */
function yuvDraw(data, w, h, bt709, fullRange) {
  if (!YUV.gl && !yuvInit()) return false;
  const gl = YUV.gl;
  const canvas = $("frame-gl");
  if (YUV.w !== w || YUV.h !== h) {
    canvas.width = w;
    canvas.height = h;
    gl.viewport(0, 0, w, h);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w / 2, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
    Object.assign(YUV, { w, h });
  } else {
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w / 2, h, gl.RGBA, gl.UNSIGNED_BYTE, data);
  }
  const [kr, kgu, kgv, kb] = bt709 ? [1.5748, 0.1873, 0.4681, 1.8556] : [1.402, 0.3441, 0.7141, 1.772];
  const u = YUV.u;
  gl.uniform1i(u.tex, 0);
  gl.uniform1i(u.height, h);
  gl.uniform1f(u.yOff, fullRange ? 0 : 16);
  gl.uniform1f(u.yScale, fullRange ? 1 : 255 / 219);
  gl.uniform1f(u.cScale, fullRange ? 1 : 255 / 224);
  for (const [n, v] of [["kr", kr], ["kgu", kgu], ["kgv", kgv], ["kb", kb]]) gl.uniform1f(u[n], v);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
  return true;
}

// The live picture: ask for the newest frame, show it, then ask for the next one. Never more than one
// frame is on the way, so the picture can't fall behind the game (a stream that keeps pushing frames
// piles them up whenever the window draws a little slower than they come). This event only says
// whether a picture is coming in, its size and the status, twice a second.
let streaming = "";
let pullRun = 0;

// The header's "Preview" switch (remembered on this PC). Off: the window stops asking for frames, so
// the app stops making them too (less CPU); reading and translating carry on as before.
let previewOn = true;
try { previewOn = localStorage.getItem("preview") !== "off"; } catch {}
function setPreview(on) {
  previewOn = on;
  $("preview-on").checked = on;
  document.body.classList.toggle("no-preview", !on);
  try { localStorage.setItem("preview", on ? "on" : "off"); } catch {}
  pullRun++; // stops the picture loop; when on, the next "frame" event (twice a second) starts it again
  streaming = "";
}
$("preview-on").addEventListener("change", (e) => setPreview(e.target.checked));
$("preview-on-2").addEventListener("click", () => setPreview(true));
setPreview(previewOn);

// Frames shown per second, in the corner of the preview (counted where frames are actually drawn).
let shownTimes = [];
function frameShown() {
  shownTimes.push(performance.now());
}
setInterval(() => {
  const now = performance.now();
  shownTimes = shownTimes.filter((t) => now - t <= 1000);
  const badge = $("fps");
  badge.hidden = !streaming || !shownTimes.length;
  badge.textContent = `${shownTimes.length} fps`;
}, 500);
async function pullPreview(url, run) {
  const img = $("frame");
  const canvas = $("frame-gl");
  let seq = 0;
  let shownUrl = null;
  let first = true;
  while (run === pullRun) {
    try {
      const r = await fetch(`${url}?after=${seq}`, { cache: "no-store" });
      if (run !== pullRun) break;
      // 204: no new frame for a second (still picture, or window hidden); anything else is wrong
      // (another program on the port?): wait a moment rather than asking again at once
      if (r.status !== 200) {
        if (r.status !== 204) await new Promise((res) => setTimeout(res, 1000));
        continue;
      }
      seq = Number(r.headers.get("X-Seq")) || 0;
      if (r.headers.get("Content-Type") === "application/x-yuy2") {
        const w = Number(r.headers.get("X-Width"));
        const h = Number(r.headers.get("X-Height"));
        const data = new Uint8Array(await r.arrayBuffer());
        if (run !== pullRun) break;
        if (yuvDraw(data, w, h, r.headers.get("X-Matrix") !== "601", r.headers.get("X-Range") === "full")) {
          frameShown();
          canvas.hidden = false;
          img.hidden = true;
          if (first) { first = false; fitFrame(); }
        }
        continue;
      }
      const blobUrl = URL.createObjectURL(await r.blob());
      const next = new Image();
      next.src = blobUrl;
      await next.decode().catch(() => {}); // decoded off-screen first: no flicker when it's swapped in
      if (run !== pullRun) { URL.revokeObjectURL(blobUrl); break; }
      img.src = blobUrl;
      frameShown();
      img.hidden = false;
      canvas.hidden = true;
      if (shownUrl) URL.revokeObjectURL(shownUrl);
      shownUrl = blobUrl;
      if (first) { first = false; fitFrame(); }
    } catch {
      await new Promise((res) => setTimeout(res, 500)); // the app's local server isn't up yet
    }
  }
}
listen("frame", (e) => {
  const f = { ...e.payload, status: I18N.tMsg(e.payload.status) };
  const pill = $("status");
  $("status-text").textContent = f.status || "…";
  pill.title = f.status || "";
  pill.className = `pill ${f.live ? "ok" : "warn"}`;
  $("source-status").textContent = f.status || "";
  $("ps-status").textContent = f.status || "";
  $("ps-status").className = `s ${f.live ? "ok" : "warn"}`;
  if (f.live) {
    const changed = f.w !== frameSize.w || f.h !== frameSize.h;
    frameSize = { w: f.w, h: f.h };
    if (previewOn && streaming !== f.preview_url) {
      streaming = f.preview_url;
      pullPreview(f.preview_url, ++pullRun);
    }
    $("empty").style.display = "none";
    if (changed) fitFrame();
  } else {
    streaming = "";
    pullRun++; // stops the picture loop
    frameSize = { w: 0, h: 0 };
    shown = { w: 0, h: 0 };
    $("frame").removeAttribute("src");
    $("frame").style.width = $("frame").style.height = "0";
    $("frame-gl").hidden = true; // no picture: hide the card's last frame too
    $("layer").innerHTML = "";
    $("empty").style.display = "";
    $("empty-msg").textContent = f.status || t("Pick the window to capture in Source.");
  }
});

listen("translation", (e) => {
  if (!cfg) return; // (before the settings are loaded: right after a reload)
  const ev = e.payload;
  if (ev.error) return log(ev);
  if (ev.area === "__screen_reason") {
    screen.reason = ev.segments;
  } else {
    lastSegs[ev.area] = ev.segments;
    if (ev.line_h) lastLineH[ev.area] = ev.line_h;
    if (ev.text_box) lastTextBox[ev.area] = ev.text_box;
    lastGameBox[ev.area] = ev.game_box || null;
  }
  // the list keeps filling while "Live translations" is on, whether or not the overlays are
  if (ev.segments.length && cfg.live_translations !== false) log(ev);
  drawLayer();
});
// The game's text seen in an area before its translation: its black box, and the translations riding
// above stacked lines move up to it (as on the overlays)
listen("cover", (e) => {
  if (e.payload.cover) lastSeen[e.payload.area] = e.payload.cover;
  else delete lastSeen[e.payload.area];
  if (cfg) drawLayer();
});
listen("status-msg", (e) => log({ info: I18N.tMsg(e.payload) }));
// A new pages script was saved (a layout update, test versions turned on or off): Game pages asks again.
listen("pages", () => {
  for (const k of Object.keys(pageScripts)) delete pageScripts[k];
  if (cfg) renderPages();
});
// The app changed the built-in profiles (a layout update, a game the server added): take those
// (the rest of the settings are the window's own, being edited here), and ask the pages again.
// A switch changed on the home-network page: this window's copy of the settings takes it (its next save would
// otherwise undo it), and the switches here show it.
listen("lan-control", (e) => {
  if (!cfg) return;
  const v = e.payload;
  cfg.overlays_enabled = v.overlays_enabled;
  cfg.live_translations = v.live_translations;
  // another language to translate into: as if picked in the Translator tab
  if (v.target_lang && v.target_lang !== cfg.target_lang) {
    cfg.target_lang = v.target_lang;
    AS.setLang(cfg.target_lang);
    for (const k of Object.keys(lastSegs)) delete lastSegs[k];
    renderLang();
    renderEngines();
    renderProfile();
  }
  cfg.shown = v.shown;
  if (v.profile === cfg.profile) {
    for (const a of cfg.areas) if (a.id in v.areas) a.enabled = v.areas[a.id];
    cfg.profile_opts.more = v.more;
    syncProfile();
  }
  $("overlays_enabled").checked = cfg.overlays_enabled;
  renderOutputs();
  $("live_translations").checked = cfg.live_translations !== false;
  $("log-paused").hidden = cfg.live_translations !== false;
  renderAreas();
  renderPages();
  drawLayer();
  updateGlance();
});

listen("config", (e) => {
  if (!cfg) return;
  const now = e.payload;
  // (every save comes back here too: only a real change does anything)
  const newLayout = JSON.stringify(now.layout_versions) !== JSON.stringify(cfg.layout_versions);
  if (newLayout) for (const k of Object.keys(pageScripts)) delete pageScripts[k];
  const changed = JSON.stringify(now.builtins) !== JSON.stringify(cfg.builtins);
  // (a switch the app made by itself is a newer profile_rev; any other differing profile is an older
  // event, from before a change made here, and the window's own choice stays)
  const appSwitched = now.profile_rev !== cfg.profile_rev;
  // (only these: the event carries no secrets — the keys are blank in it — and the window's own
  // settings stay as the window has them; taking the whole payload would wipe the keys at the next save)
  Object.assign(cfg, { builtins: now.builtins, server_games: now.server_games, games_version: now.games_version, layout_versions: now.layout_versions, game_profiles: now.game_profiles, profile_rev: now.profile_rev, fx_defaults: now.fx_defaults, fx_view: now.fx_view });
  // the app switched games (the picture showed another one)
  if (appSwitched && now.profile !== cfg.profile) {
    syncProfile();
    Object.assign(cfg, { profile: now.profile, areas: now.areas, style: now.style, profile_opts: now.profile_opts });
    selected = cfg.areas[0]?.id ?? null;
    for (const k of Object.keys(lastSegs)) delete lastSegs[k];
    renderAll();
    return;
  }
  if (!changed) {
    if (newLayout) renderPages(); // (a layout update can bring a new pages script)
    return;
  }
  renderTiming(); // (a layout update can bring the game's own timing)
  if (locked()) {
    const p = activeProfile();
    Object.assign(cfg, { areas: clone(p.areas), style: clone(p.style), profile_opts: clone(p.pages) });
    renderAll();
  } else {
    renderProfile();
  }
});
// GTA VI's phone held up: its translations beside it, on the picture here too
let phoneNow = null;
listen("phone", (e) => { phoneNow = e.payload; drawLayer(); });
listen("screen", (e) => {
  const was = !!screen.layout;
  screen.layout = e.payload;
  if (!screen.layout) screen.reason = [];
  if (!!screen.layout !== was) log({ info: screen.layout ? t("Game page detected") : t("Game page closed") });
  drawLayer();
});

// ------------------------------------------------------------------------------------- game profile

// A game profile holds the read areas, the text style and the game pages. Built-in ones (GTA VI, and any game the server adds)
// can't be changed: "Make a copy" gives the player one of their own. The settings the rest of the app
// reads (cfg.areas, cfg.style, cfg.profile_opts) are the profile in use; with one of the player's
// profiles, every save writes them back into it (see syncProfile).
// (the games: the app's own and any the server added, by their built-in profile)
const gameName = (g) => (cfg.builtins || []).find((b) => b.id === g)?.name || g;
const allProfiles = () => [...(cfg.builtins || []), ...(cfg.profiles || [])];
const activeProfile = () => allProfiles().find((p) => p.id === cfg.profile) || (cfg.builtins || [])[0];
const isBuiltin = (p) => (cfg.builtins || []).some((b) => b.id === p?.id);
const locked = () => isBuiltin(activeProfile());
const profileGame = () => activeProfile()?.game || "gta6";
const clone = (v) => JSON.parse(JSON.stringify(v));

/** One of the player's profiles is in use: keep it equal to what is being edited. */
/** Show or hide a read area or a game page of the profile in use: the player's own choice, kept apart from the
 *  profile (a built-in one's too; see Config::shown). */
function setShown(kind, id, on) {
  cfg.shown = cfg.shown || {};
  const s = (cfg.shown[cfg.profile] = cfg.shown[cfg.profile] || {});
  s.areas = s.areas || {};
  s.pages = s.pages || {};
  (kind === "area" ? s.areas : s.pages)[id] = on;
}

function syncProfile() {
  const p = (cfg.profiles || []).find((x) => x.id === cfg.profile);
  if (p) Object.assign(p, { areas: clone(cfg.areas), style: clone(cfg.style), pages: clone(cfg.profile_opts) });
}

/** Use another profile: its areas, style and pages become the ones in use. */
function useProfile(id) {
  syncProfile();
  const p = allProfiles().find((x) => x.id === id);
  if (!p) return;
  cfg.profile = p.id;
  cfg.areas = clone(p.areas);
  cfg.style = clone(p.style);
  cfg.profile_opts = clone(p.pages);
  selected = cfg.areas[0]?.id ?? null;
  for (const k of Object.keys(lastSegs)) delete lastSegs[k];
  renderAll();
  save();
}

/** A copy of a profile (built-in or not) that the player can change. */
function copyProfile(from) {
  syncProfile();
  const id = `custom-${Date.now().toString(36)}`;
  cfg.profiles = cfg.profiles || [];
  cfg.profiles.push({ id, name: t("{name} (copy)", { name: from.name }), game: from.game, areas: clone(from.areas), style: clone(from.style), pages: clone(from.pages), timing: clone(from.timing || {}) });
  useProfile(id);
  setTimeout(() => $("pf-name")?.select(), 0);
}

async function deleteProfile(p) {
  if (!(await ask(t("Delete the profile “{name}”? This can't be undone.", { name: p.name }), t("Delete")))) return;
  cfg.profiles = cfg.profiles.filter((x) => x.id !== p.id);
  useProfile((cfg.builtins || []).some((b) => b.id === p.game) ? p.game : "gta6");
}

function renderAll() {
  applyLock();
  renderProfile();
  renderPages();
  renderAreas();
  renderTiming(); // (the game's timing may differ: the sliders show what's in use)
  drawLayer();
  updateGlance();
}

/** Built-in profile in use: its parts can be looked at, not changed (a note offers a copy). */
function applyLock() {
  const on = locked();
  document.body.classList.toggle("locked", on);
  const p = activeProfile();
  document.querySelectorAll("[data-lock]").forEach((el) => {
    el.innerHTML = on
      ? `<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>
         <span>${AS.esc(t("“{name}” is built in and can't be changed.", { name: p.name }))}</span>
         <button class="btn" data-copy>${t("Make a copy")}</button>`
      : "";
    el.querySelector("[data-copy]")?.addEventListener("click", () => copyProfile(activeProfile()));
  });
  lockControls();
}

function renderProfile() {
  const f = $("profile-form");
  const p = activeProfile();
  const opt = (x) => `<option value="${AS.esc(x.id)}" ${x.id === cfg.profile ? "selected" : ""}>${AS.esc(x.name)}</option>`;
  const mine = cfg.profiles || [];
  const cover = cfg.profile_opts?.more?.[AS.COVER] === true;
  f.innerHTML = `
    <div class="field"><label for="pf-select">${t("Profile in use")}</label>
      <select id="pf-select">
        <optgroup label="${AS.esc(t("Built-in"))}">${(cfg.builtins || []).map(opt).join("")}</optgroup>
        ${mine.length ? `<optgroup label="${AS.esc(t("Your profiles"))}">${mine.map(opt).join("")}</optgroup>` : ""}
      </select></div>
    <div class="card profile-card">
      <div class="row">
        ${isBuiltin(p)
          ? `<div class="grow"><b>${AS.esc(p.name)}</b></div><span class="badge lock-badge">${t("Built-in")}</span>`
          : `<input type="text" id="pf-name" class="grow" value="${AS.esc(p.name)}" title="${AS.esc(t("Rename"))}" /><span class="badge">${t("Yours")}</span>`}
      </div>
      <p class="muted small" style="margin:8px 0 0">${t("For {game} · {n} read area(s)", { game: AS.esc(gameName(p.game)), n: p.areas.length })}</p>
      <div class="row" style="margin-top:12px">
        <button id="pf-copy" class="btn">${t("Make a copy")}</button>
        ${isBuiltin(p) ? "" : `<button id="pf-delete" class="btn danger">${t("Delete")}</button>`}
      </div>
    </div>
    <p class="muted small">${isBuiltin(p)
      ? t("Built-in profiles get updates for the game automatically. A copy keeps your changes and stays yours.")
      : t("Your changes to read areas, text style and game pages are saved in this profile.")}</p>
    ${isBuiltin(p) ? `
    <h3>${t("How translations look")}</h3>
    <div class="looks">
      <label class="look-opt"><span class="check" style="color:var(--text)"><input type="radio" name="pf-look" value="above" ${cover ? "" : "checked"}/> ${t("Above the game's text")}</span>
        <div class="look-stage" data-look="above"></div></label>
      <label class="look-opt"><span class="check" style="color:var(--text)"><input type="radio" name="pf-look" value="cover" ${cover ? "checked" : ""}/> ${t("Over the game's text, on a black box that hides it")}</span>
        <div class="look-stage" data-look="cover"></div></label>
    </div>` : ""}`;
  // (built-in profiles only: a profile of your own places each area's translation itself. Kept like
  // a game page's switch: in the profile, a built-in one too)
  f.querySelectorAll('input[name="pf-look"]').forEach((el) => el.addEventListener("change", () => {
    const on = el.value === "cover" && el.checked;
    cfg.profile_opts.more = cfg.profile_opts.more || {};
    cfg.profile_opts.more[AS.COVER] = on;
    setShown("page", AS.COVER, on);
    drawLayer();
    renderAreas(); // (its black-box options are for this look)
    save();
  }));
  drawLookPreviews(f);
  $("pf-select").addEventListener("change", (e) => useProfile(e.target.value));
  $("pf-copy").addEventListener("click", () => copyProfile(activeProfile()));
  $("pf-delete")?.addEventListener("click", () => deleteProfile(activeProfile()));
  $("pf-name")?.addEventListener("input", (e) => {
    const mineNow = cfg.profiles.find((x) => x.id === cfg.profile);
    if (mineNow) mineNow.name = e.target.value;
    save(false);
  });
  $("pf-name")?.addEventListener("change", () => renderProfile());
}

// The look previews: the whole picture (16:9, the line at its size on screen) with a real line of the game and its
// translation in the language in use (from the memory), drawn as the overlays draw it, with this
// profile's subtitle style. (A plain dark background: no game picture in the app.)
const LOOK_LINE = "Okay, listen to me very carefully.";
async function drawLookPreviews(form) {
  const stages = form.querySelectorAll(".look-stage");
  if (!stages.length) return;
  let text = null;
  try { text = await invoke("memory_line", { line: LOOK_LINE }); } catch (e) { /* (the memory isn't ready: the English) */ }
  const area = cfg.areas.find((a) => a.id === "subtitles") || cfg.areas[0];
  if (!area) return;
  const style = AS.styleOf(area, cfg.style);
  for (const stage of stages) {
    stage.innerHTML = "";
    const en = document.createElement("div");
    en.className = "look-en";
    en.textContent = LOOK_LINE;
    stage.appendChild(en);
    const W = stage.clientWidth, H = stage.clientHeight;
    if (!W || !H) continue;
    // (the game's subtitles are about 3.5 % of the picture's height)
    en.style.fontSize = `${Math.max(7, H * 0.035)}px`;
    const s = stage.getBoundingClientRect(), r = en.getBoundingClientRect();
    const tb = [(r.left - s.left) / W, (r.top - s.top) / H, r.width / W, r.height / H];
    const cover = stage.dataset.look === "cover";
    const L = AS.look(Object.assign({}, area, { placement: "above_text" }), style, { more: { [AS.COVER]: cover } }, tb);
    const box = document.createElement("div");
    stage.appendChild(box);
    // (the game's size, as "Match the game's text size" gives it; the style's look otherwise)
    const px = AS.matchedPx(Object.assign({}, area, { match_size: true }), tb[3], H);
    AS.renderBox(box, [[text || LOOK_LINE, null]], L.style, H / 360, area.font_scale, L.placement, L.area, px);
    AS.placeAtText(box, L.placement, tb, tb[3], W, H, 0, 0, W, H, null, { min: AS.fxOf(cfg.fx_view || cfg.area_fx, area.id).cover_min });
  }
}

/** A script's text: plain, or by interface language (English the fallback). */
const scriptText = (v) => (typeof v === "string" ? v : v ? v[cfg.ui_lang] || v.en || Object.values(v)[0] || "" : "");

/**
 * What's translated (Game pages): the heading of each group, and the name and note of each switch the core
 * lists for a game (`groups` of page_options: one thing each, none covering another).
 */
const CONTENT_GROUPS = {
  phone: "Phone translations",
  menus: "Pause menu and map",
  notices: "Help, instructions and notices",
  screens: "Other screens",
};
const CONTENT_TEXT = {
  phone_text: ["WhatUp chats", "The phone's chats, translated on a second phone beside it."],
  phone_bubble: ["Message bubble", "A new message's bubble at the bottom right, translated above it."],
  pause_home: ["First page and Game page", "The menu's first page (Map, Game, Store, Help…), saving and loading, and their questions."],
  pause_map: ["Map", "The objective card at the top left of the map."],
  pause_map_index: ["Map index", "The map's list on the left: Personal, Story, Robberies and the rest."],
  pause_map_box: ["Map place box", "The box of the place or mission under the cursor, drawn again under it."],
  pause_map_buttons: ["Map buttons", "The map's buttons at the bottom right: Recenter, 2D view, Add marker and the rest."],
  pause_brief: ["Brief", "The Brief tab: what was said, help texts and objectives so far."],
  pause_jason_lucia: ["Jason & Lucia", "The Jason & Lucia tab."],
  pause_progress: ["Progress and stats", "The Progress tab's cards and the stats pages."],
  pause_help: ["Help pages", "The Help menu's pages: Basics, Inventory, Vehicles and the rest."],
  pause_settings: ["Settings pages", "The settings: controls, display, audio and the rest."],
  pause_tabs: ["Tab names and buttons", "The row of tabs at the top and the buttons at the bottom right of every page."],
  help: ["Help box", "The help box at the top left."],
  instructions: ["Instructions", "The instructions under the subtitles."],
  game_notices: ["Game notices", "The game's notices, like “Controller disconnected”."],
  wasted: ["Wasted and Busted", "The big “wasted” when you die and “busted” when you're arrested."],
  mission_failed: ["Mission failed", "The “failed” page when a mission is lost: its reason and buttons."],
  weapon_wheel: ["Weapon wheel", "The weapon and item wheel: its tabs, the selected item and its stats."],
};

/** The pages a game's script has and the switches it declares, asked once per game. */
const pageScripts = {};
async function pageScript(game) {
  if (!pageScripts[game]) pageScripts[game] = invoke("page_options", { game }).catch(() => ({ pages: [], options: [] }));
  return pageScripts[game];
}

/** Game pages of the profile in use: the switches the game's pages script declares, and the fonts. */
async function renderPages() {
  const f = $("pages-form");
  const o = cfg.profile_opts;
  const game = profileGame();
  const script = await pageScript(game);
  if (profileGame() !== game) return; // (another profile meanwhile: its own render runs)
  if (!script.pages.length) {
    f.innerHTML = `<div class="callout">${t("{game}'s own pages aren't known yet, so none are translated for now. Subtitles and the read areas work as usual.", { game: AS.esc(gameName(game)) })}</div>`;
    return;
  }
  o.more = o.more || {};
  // a switch as the profile has it: as set; else, one split from an older switch (`inherits`), as that one is
  // set; else the script's default (as the core's `pagescript::option_on`)
  // (down the chain, a few at most: the map's index from the map, the map from the pause menu)
  const isOn = (opt, depth = 0) => {
    if (opt.id in o.more) return o.more[opt.id];
    if (opt.inherits && opt.inherits !== opt.id && depth < 4) {
      const old = script.options.find((x) => x.id === opt.inherits);
      if (old) return isOn(old, depth + 1);
      if (opt.inherits in o.more) return o.more[opt.inherits];
    }
    return opt.on;
  };
  // What's translated: the dialogue always; then the game's switches (the core's list) under their groups'
  // headings, each one thing (a read area, a switch of the game's pages, or a setting such as the phone's)
  const switches = (script.groups || []).filter((g) => CONTENT_TEXT[g.id] && CONTENT_GROUPS[g.group]);
  const covered = new Set(switches.flatMap((g) => g.pages));
  const pageOn = (id) => { const opt = script.options.find((x) => x.id === id); return opt ? isOn(opt) : false; };
  const areaOn = (id) => cfg.areas.some((a) => a.id === id && a.enabled);
  // (the message bubble: as the phone in settings from before its own switch)
  const settingOn = (id) => (id === "phone_bubble" ? (cfg.phone_bubble ?? cfg.phone_text !== false) : cfg[id] !== false);
  const switchOn = (g) => (g.setting ? settingOn(g.setting) : g.areas.length ? g.areas.some(areaOn) : g.pages.some(pageOn));
  const setSwitch = (g, on) => {
    if (g.setting) cfg[g.setting] = on;
    for (const a of cfg.areas) if (g.areas.includes(a.id)) { a.enabled = on; setShown("area", a.id, on); }
    for (const id of g.pages) { o.more[id] = on; setShown("page", id, on); }
  };
  const headings = Object.keys(CONTENT_GROUPS).filter((id) => switches.some((g) => g.group === id));
  const content = switches.length ? `
    <div class="card">
      <div style="font-weight:600">${t("What's translated")}</div>
      <label class="check" style="margin-top:10px"><input type="checkbox" checked disabled data-keep/> ${t("Dialogue and subtitles")}</label>
      <p class="muted small" style="margin:2px 0 0">${t("Always translated.")}</p>
      ${headings.map((h) => `
      <div style="font-weight:600;margin-top:16px">${t(CONTENT_GROUPS[h])}</div>
      ${switches.filter((g) => g.group === h).map((g) => `
      <label class="check" style="margin-top:8px"><input type="checkbox" data-switch="${AS.esc(g.id)}" data-keep ${switchOn(g) ? "checked" : ""}/> ${t(CONTENT_TEXT[g.id][0])}</label>
      <p class="muted small" style="margin:2px 0 0">${t(CONTENT_TEXT[g.id][1])}</p>`).join("")}`).join("")}
      <button class="btn" id="pf-dialogue-only" data-keep style="margin-top:12px">${t("Dialogue only")}</button>
      <p class="muted small" style="margin:6px 0 0">${t("Turns off everything here except the dialogue and subtitles.")}</p>
    </div>` : "";
  // the switches the game's script declares (its pages come with layout updates), those above aside
  const declared = script.options.filter((opt) => !covered.has(opt.id)).map((opt) => `
    <div class="card">
      <label class="check" style="font-weight:600"><input type="checkbox" data-more="${AS.esc(opt.id)}" ${isOn(opt) ? "checked" : ""}/> ${AS.esc(scriptText(opt.name) || opt.id)}</label>
      ${scriptText(opt.about) ? `<p class="muted small" style="margin:6px 0 0">${AS.esc(scriptText(opt.about))}</p>` : ""}
    </div>`).join("");
  f.innerHTML = content + declared + `
    <div class="card">
      <div class="field"><label>${t("Title font")}</label><select id="pf-title">${fontOptions(o.title_font)}</select></div>
      <div class="field" style="margin-top:10px"><label>${t("Reason and button font")}</label><select id="pf-text">${fontOptions(o.text_font)}</select></div>
    </div>`;
  f.querySelectorAll("[data-more]").forEach((el) => el.addEventListener("change", () => { o.more[el.dataset.more] = el.checked; setShown("page", el.dataset.more, el.checked); save(); }));
  f.querySelectorAll("[data-switch]").forEach((el) => el.addEventListener("change", () => {
    setSwitch(switches.find((g) => g.id === el.dataset.switch), el.checked);
    renderAreas();
    save();
  }));
  if ($("pf-dialogue-only")) $("pf-dialogue-only").addEventListener("click", () => {
    for (const g of switches) setSwitch(g, false);
    for (const opt of script.options) { o.more[opt.id] = false; setShown("page", opt.id, false); }
    renderAreas();
    renderPages();
    save();
  });
  for (const [id, key] of [["pf-title", "title_font"], ["pf-text", "text_font"]]) {
    const sel = $(id);
    paintFonts(sel);
    sel.style.fontFamily = fontCss(sel.value);
    sel.addEventListener("change", () => { o[key] = sel.value; sel.style.fontFamily = fontCss(sel.value); save(); });
  }
  lockControls();
}


function applyTheme() {
  const dark = cfg.theme === "dark";
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  // the icon shows where a click takes you (sun: to light, moon: to dark)
  $("theme").title = `${t("Switch between light and dark")} — ${dark ? t("☀ Light") : t("🌙 Dark")}`;
}

// A built-in profile in use: the controls of its parts are switched off (not just dimmed), so they
// can't be reached with the keyboard either. Showing or hiding an area or a game page stays the player's.
function lockControls() {
  const on = locked();
  document.querySelectorAll("#tab-areas, #tab-pages").forEach((tab) =>
    tab.querySelectorAll("input:not(.en):not([data-more]):not([data-keep]), select:not([data-more]), textarea, button:not([data-copy]):not([data-keep])").forEach((el) => { el.disabled = on || el.dataset.matched === "1"; }));
  $("add-area").disabled = on;
  // (its boxes can't be moved or resized, so the hint about doing that is gone too)
  $("drag-hint").hidden = on;
}

(async function init() {
  cfg = await invoke("get_config");
  I18N.setLang(cfg.ui_lang || "en");  const uiSel = $("ui-lang");
  for (const l of I18N.UI_LANGS) uiSel.add(new Option(l.name, l.code));
  uiSel.value = I18N.lang;
  uiSel.addEventListener("change", async () => {
    cfg.ui_lang = uiSel.value;
    clearTimeout(saveTimer);
    try { await invoke("save_config", { config: cfg }); } catch {}
    location.reload(); // simplest way to redraw every panel in the new language and direction
  });
  AS.setLang(cfg.target_lang);
  LANGS = await invoke("list_langs").catch((e) => { log({ error: t("Languages: {e}", { e }) }); return []; });
  try { renderLang(); } catch (e) { log({ error: t("Languages: {e}", { e }) }); }
  selected = cfg.areas[0]?.id ?? null;
  applyTheme();
  $("theme").addEventListener("click", () => {
    cfg.theme = cfg.theme === "dark" ? "light" : "dark";
    applyTheme();
    save(false);
  });
  $("overlays_enabled").checked = cfg.overlays_enabled;
  const showLive = () => { $("log-paused").hidden = cfg.live_translations !== false; };
  $("live_translations").checked = cfg.live_translations !== false;
  showLive();
  $("live_translations").addEventListener("change", (e) => { cfg.live_translations = e.target.checked; showLive(); save(false); });
  // (save(): redraws the preview, which hides or shows the translations on it too)
  $("overlays_enabled").addEventListener("change", (e) => { cfg.overlays_enabled = e.target.checked; renderOutputs(); save(); });
  renderOutputs();
  try {
    FONT_LIST = await invoke("list_fonts");
  } catch (e) {
    log({ error: t("Could not list fonts: {e}", { e }) });
  }
  renderShared();
  renderEngines();
  applyLock();
  renderProfile();
  renderPages();
  renderAreas();
  renderTiming();
  renderPreviewForm();
  renderMemory();
  renderDebug();
  renderSnapForm();
  try {
    saver = await invoke("frame_saver_status");
    updateSaver();
  } catch {}
  try {
    const s = await invoke("get_screen");
    screen.layout = s.layout;
    screen.reason = s.reason || [];
  } catch {}
  document.fonts.ready.then(drawLayer); // re-measure once the bundled fonts have loaded
  renderObs();
  updateLan();
  updateTvs();
  invoke("app_version").then((v) => { $("app-version").textContent = "v" + v; document.title = "VI Translator " + v; }).catch(() => {});
  // the tools for developing the app: development builds only
  invoke("dev_build").then((dev) => { $("debug-section").hidden = !dev; }).catch(() => {});
  invoke("has_offline").then((has) => {
    if (has) return;
    hasOffline = false;
    Object.assign(ENGINES.find((e) => e.id === "local"), {
      name: "Translation memory only",
      d: "Shows the reviewed translations only: no download, no account. For other lines choose one of the services below.",
    });
    renderEngines();
    updateGlance();
  }).catch(() => {});
  invoke("update_status").then(showUpdate).catch(() => {});
  $("log").dataset.empty = t("Translations appear here as the game shows text.");
  let mode = "setup";
  try { mode = localStorage.getItem("mode") || "setup"; } catch {}
  setMode(mode === "play" ? "play" : "setup");
  await loadWindows();
  loadOverlayWindows();
  updateVcam();
  renderCardSound(true);
  updateGlance();
})();
