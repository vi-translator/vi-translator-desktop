// Shared by the main window (preview) and the overlay windows, so both draw the Arabic identically.
window.AS = (() => {
  // Target language (set by each page from the settings): its direction, and fonts for its script
  // after the chosen font (Hindi, Bengali, Tamil … and Thai need Windows' own fonts for their letters).
  const RTL = new Set(["ar", "ur", "fa"]);
  const SCRIPT_FONTS = { hi: '"Nirmala UI"', bn: '"Nirmala UI"', ta: '"Nirmala UI"', te: '"Nirmala UI"', mr: '"Nirmala UI"', th: '"Leelawadee UI"' };
  let lang = "ar";
  const setLang = (code) => { lang = code || "ar"; };
  const rtl = () => RTL.has(lang);
  const dir = () => (rtl() ? "rtl" : "ltr");
  const fonts = () => `${SCRIPT_FONTS[lang] ? SCRIPT_FONTS[lang] + ", " : ""}"Segoe UI", Tahoma, sans-serif`;
  const esc = (s) =>
    String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  // Arabic with Latin bits ("GTA+", "GTA$500,000", "rsg.ms/gtaplus"): each Latin run is shown
  // left-to-right on its own, so right-to-left text can't move its "+" or "$" ("+GTA").
  const ltrRuns = (s) => {
    const text = String(s).replace(/[⁦⁩]/g, "");
    // (Latin words in a row are one run, "Pay 'n' Spray": each on its own, right-to-left would put them in
    // reverse order)
    const re = /[A-Za-z0-9](?:[A-Za-z0-9$+\/™%&_-]|[.,](?=[A-Za-z0-9])|['’](?=\s*[A-Za-z0-9])|\s+(?=['’]?[A-Za-z0-9]))*[$+%™]?/g;
    let out = "", last = 0, m;
    while ((m = re.exec(text))) {
      out += esc(text.slice(last, m.index)) + `<bdi dir="ltr">${esc(m[0])}</bdi>`;
      last = m.index + m[0].length;
    }
    return out + esc(text.slice(last));
  };

  // A text box's words with the game's button icons where it says ⟦1⟧, ⟦2⟧ … (`icons`: PNG data URLs),
  // each about as tall as the letters and sitting on the line like one.
  const withIcons = (s, icons) =>
    String(s)
      .split(/(⟦\d+⟧)/)
      .map((part) => {
        const m = /^⟦(\d+)⟧$/.exec(part);
        const src = m && icons && icons[Number(m[1]) - 1];
        if (m) return src && /^data:image\/png;base64,/.test(src) ? `<img src="${esc(src)}" alt="" style="height:1.15em;width:auto;vertical-align:-0.25em;margin:0 0.12em">` : "";
        return ltrRuns(part);
      })
      .join("");

  const hexToRgb = (hex) => {
    const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex || "");
    return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : [0, 0, 0];
  };

  // The game draws the other speaker's subtitle line in grey.
  const DIM = "#9a9a9a";

  // Steady boxes: the same thing measured again a few pixels off stays where it was. The game's text is
  // still; its readings aren't (a pause menu's items came 2–3 px apart from one read to the next, and
  // the translation over them shook). A real change — a selected item growing, the next line — is
  // bigger and moves it. (fractions of the picture: about 9 px across and 7 px down at 1080p)
  const STEADY = { on: true, dx: 0.005, dy: 0.007, holdMs: 300 };
  let steadyAt = {}, steadyCount = 0;
  // r: {x, y, w, h} or [x, y, w, h] (the same kind back); key: the thing it is (an area, a menu item …)
  function steady(key, r) {
    if (!STEADY.on || !r) return r;
    const arr = Array.isArray(r);
    const [x, y, w, h] = arr ? r : [r.x, r.y, r.w, r.h];
    const was = steadyAt[key];
    if (was && Math.abs(was.v[0] - x) < STEADY.dx && Math.abs(was.v[1] - y) < STEADY.dy &&
        Math.abs(was.v[0] + was.v[2] - (x + w)) < STEADY.dx && Math.abs(was.v[1] + was.v[3] - (y + h)) < STEADY.dy) {
      return arr ? was.v.slice() : { x: was.v[0], y: was.v[1], w: was.v[2], h: was.v[3] };
    }
    if (!was && ++steadyCount > 4000) { steadyAt = {}; steadyCount = 0; }
    steadyAt[key] = { v: [x, y, w, h] };
    return r;
  }

  // Segments -> HTML. Color segments become colored spans; "icon:<png>" segments become the game's
  // button icon, sized to the text (1.2em) and sitting on the line like a letter.
  function segHtml(segments, keepColors = true) {
    return segments
      .map(([t, c]) => {
        // two speakers' lines: each on its own line, as in the game
        if (t === "\n" && !c) return "<br>";
        // the other speaker's line: grey like the game's, also with highlight colors off
        if (c === "dim") return `<span style="color:${DIM}">${esc(t)}</span>`;
        // a speaker name ("Dobbs:"): in the game's color for it, also with highlight colors off
        if (c && c.startsWith("name:")) return `<span style="color:${esc(c.slice(5))}">${esc(t)}</span>`;
        if (c && c.startsWith("icon:")) {
          // "icon:<height vs English text>|<png>" — keep the icon's size relative to the text, as in the game
          let src = c.slice(5);
          let h = 1.2; // em, for icons without a measured size
          const bar = src.indexOf("|");
          if (bar > 0 && !src.startsWith("data:")) {
            const rel = parseFloat(src.slice(0, bar));
            if (rel > 0) h = Math.min(Math.max(rel / MATCH_FACTOR, 0.4), 1.6);
            src = src.slice(bar + 1);
          }
          const drop = (0.35 - h / 2).toFixed(3); // centre it on the letters' middle
          return `<img src="${esc(src)}" alt="" style="height:${h.toFixed(3)}em;width:auto;vertical-align:${drop}em;margin:0 0.15em">`;
        }
        return c && keepColors ? `<span style="color:${esc(c)}">${esc(t)}</span>` : esc(t);
      })
      .join("");
  }

  // Same formula as overlay.rs `overlay_rect`: where the Arabic is drawn for an area.
  // `a` is the read area in pixels; `area` (the config object) supplies the custom region.
  function rectFor(placement, a, cw, ch, area) {
    if (placement === "custom" && area) {
      const w = Math.min(Math.max(area.out_w * cw, 20), cw);
      const h = Math.min(Math.max(area.out_h * ch, 20), ch);
      return {
        x: Math.min(Math.max(area.out_x * cw, 0), cw - w),
        y: Math.min(Math.max(area.out_y * ch, 0), ch - h),
        w,
        h,
      };
    }
    if (placement === "under_text") {
      const h = Math.min(a.h + Math.floor(ch * 0.4), ch);
      const w = Math.min(Math.max(a.w, 50), cw);
      return { x: Math.min(Math.max(a.x, 0), cw - w), y: Math.min(Math.max(a.y, 0), ch - h), w, h };
    }
    if (placement === "above_text") {
      // the read area and room above it (the Arabic sits right above the game's text)
      const h = Math.min(a.h + Math.floor(ch * 0.4), ch);
      const w = Math.min(Math.max(a.w, 50), cw);
      return { x: Math.min(Math.max(a.x, 0), cw - w), y: Math.min(Math.max(a.y + a.h - h, 0), ch - h), w, h };
    }
    const band = Math.floor(ch * 0.4);
    const side = Math.floor(cw * 0.35);
    let r;
    switch (placement) {
      case "below": r = [a.x, a.y + a.h, a.w, band]; break;
      case "right": r = [a.x + a.w, a.y, side, band]; break;
      case "left": r = [a.x - side, a.y, side, band]; break;
      case "over": r = [a.x, a.y + a.h - band, a.w, band]; break;
      default: r = [a.x, a.y - band, a.w, band];
    }
    let [x, y, w, h] = r;
    w = Math.min(Math.max(w, 50), cw);
    h = Math.min(Math.max(h, 50), ch);
    x = Math.min(Math.max(x, 0), cw - w);
    y = Math.min(Math.max(y, 0), ch - h);
    return { x, y, w, h };
  }

  // Where the text box sits inside its region: {v: top|middle|bottom, h: left|center|right}.
  function alignOf(placement, area) {
    if (placement === "under_text") return { v: "top", h: "left" };
    if (placement === "on_text") return { v: "top", h: "center" };
    if (placement === "above_text") return { v: "bottom", h: "center" };
    if (placement === "custom" && area) return { v: area.out_valign || "bottom", h: area.out_halign || "center" };
    if (area && area.text_pos) {
      const [v, h] = area.text_pos.split("-");
      if (v && h) return { v, h };
    }
    return {
      v: placement === "above" || placement === "over" ? "bottom" : "top",
      h: placement === "right" ? "left" : placement === "left" ? "right" : "center",
    };
  }

  // Flex alignment of the box inside its band.
  function anchor(container, placement, area) {
    const al = alignOf(placement, area);
    const s = container.style;
    if (atText(placement)) {
      s.display = "block"; // the box is placed exactly by placeAtText
      return;
    }
    s.display = "flex";
    s.flexDirection = "column";
    s.alignItems = { left: "flex-start", center: "center", right: "flex-end" }[al.h];
    s.justifyContent = { top: "flex-start", middle: "center", bottom: "flex-end" }[al.v];
  }

  // segments: [[text, color|null], ...]; k: CSS px per style px (style sizes are for a 1080p window).
  // Arabic size relative to the English line height (OCR word height) when matching the game's size.
  const MATCH_FACTOR = 1.2;

  // fontPx: exact font size in CSS px (from the English line height); otherwise style size × fontScale × k.
  // o: { fadeMs: the translation fades in (when it's new), empty: the box alone, without text (the
  // black box over the game's text before its translation is ready) }.
  // (what each area shows, to fade in only a new translation: the TV page draws its boxes anew)
  const lastText = {};
  function renderBox(box, segments, style, k, fontScale, placement, area, fontPx, o) {
    o = o || {};
    const key = (area && area.id) || "";
    const plain = segments.map((s) => (s[1] && s[1].startsWith("icon:") ? "🎮" : s[0])).join("").trim();
    if (!plain && !(o.empty && placement === "on_text")) {
      box.style.display = "none";
      box.innerHTML = "";
      if (!o.probe) lastText[key] = "";
      return;
    }
    // Over the game's text: the translation in the middle of its black box (at least as big as the
    // English: see placeOnText), and it's what fades in, the box being up already.
    const onText = placement === "on_text";
    box.innerHTML = onText ? `<span class="vt-t" style="display:block">${segHtml(segments, style.keep_colors)}</span>` : segHtml(segments, style.keep_colors);
    const [r, g, b] = hexToRgb(style.bg_color);
    const ow = style.outline_width * k;
    const shadows = [];
    if (ow > 0) {
      for (let i = 0; i < 16; i++) {
        const a = (i / 16) * Math.PI * 2;
        shadows.push(`${(Math.cos(a) * ow).toFixed(2)}px ${(Math.sin(a) * ow).toFixed(2)}px 0 ${style.outline_color}`);
      }
    }
    if (style.shadow) shadows.push(`0 ${2 * k}px ${6 * k}px rgba(0,0,0,0.85)`);
    const fs = Math.max(8, fontPx || style.font_size * fontScale * k);
    const bgMode = (area && area.background) || "";
    Object.assign(box.style, {
      // (a box drawn again may have had a placement at the game's text: none of it stays)
      position: "", left: "", top: "", transform: "", width: "", minWidth: "", minHeight: "",
      display: "block",
      direction: dir(),
      unicodeBidi: "plaintext",
      // Centred boxes centre their lines; boxes pinned to a side keep the language's natural side.
      textAlign: alignOf(placement, area).h === "center" ? "center" : rtl() ? "right" : "left",
      fontFamily: `"${(area && area.font_family) || style.font_family}", ${fonts()}`,
      fontSize: `${fs}px`,
      fontWeight: style.bold ? "700" : "400",
      lineHeight: String(style.line_height),
      color: style.text_color,
      background: `rgba(${r},${g},${b},${style.bg_opacity})`,
      padding: `${style.padding * k}px ${style.padding * 1.8 * k}px`,
      borderRadius: `${style.radius * k}px`,
      border: "none",
      borderImage: "none",
      boxShadow: "none",
      textShadow: shadows.join(", ") || "none",
      maxWidth: "calc(100% - 8px)",
      margin: "4px",
      boxSizing: "border-box",
      whiteSpace: "pre-wrap",
      overflowWrap: "anywhere",
    });
    if (onText) Object.assign(box.style, { display: "flex", flexDirection: "column", justifyContent: "center", alignItems: "center" });
    // A new translation fades in (the same one drawn again doesn't)
    const target = onText ? box.firstElementChild : box;
    if (o.probe) {
      // (only measured)
    } else if (plain && o.fadeMs > 0 && lastText[key] !== plain) {
      target.style.transition = "none";
      target.style.opacity = "0";
      void target.offsetWidth; // (the start, drawn, before the change)
      target.style.transition = `opacity ${o.fadeMs}ms ease-out`;
      target.style.opacity = "1";
    } else if (!(o.fadeMs > 0)) {
      Object.assign(target.style, { transition: "", opacity: "" });
    }
    if (!o.probe) lastText[key] = plain;
    if (bgMode === "none") {
      // Like the game's own text: no box, a soft shadow, and a thin dark outline unless the style
      // says none (GTA VI's subtitles and instructions have only the shadow).
      const o = style.outline_width * k;
      const ring = [];
      for (let i = 0; o > 0 && i < 16; i++) {
        const a = (i / 16) * Math.PI * 2;
        ring.push(`${(Math.cos(a) * o).toFixed(2)}px ${(Math.sin(a) * o).toFixed(2)}px 0 ${style.outline_color || "#000"}`);
      }
      // a soft shadow only when the style asks for one (GTA VI's subtitles have a border and no shadow)
      if (style.shadow) ring.push(`0 ${2 * k}px ${5 * k}px rgba(0,0,0,0.9)`);
      if (o <= 0) ring.push(`0 0 ${2 * k}px rgba(0,0,0,0.6)`); // (keeps it readable over a bright floor)
      Object.assign(box.style, { background: "transparent", padding: `${2 * k}px ${4 * k}px`, borderRadius: "0", textShadow: ring.join(", ") });
    } else if (bgMode === "game") {
      // Like GTA VI's help box (fitted on 35 captured frames over scenes from black to white): nearly
      // solid all across, near-black on its left turning dark grey on its right, and letting a little of
      // the scene through, more toward its right (2 % → 12 % at the middle → 19 %); a thin rim a shade
      // lighter; square corners; the text 14 px in at 1080p.
      Object.assign(box.style, {
        background: "linear-gradient(90deg, rgba(3,3,6,0.98) 0%, rgba(7,7,10,0.97) 20%, rgba(10,10,14,0.91) 35%, rgba(14,14,18,0.885) 50%, rgba(19,19,23,0.85) 65%, rgba(21,21,26,0.83) 80%, rgba(26,26,32,0.81) 100%)",
        // (its rim: a shade lighter than its black, never lighter than the scene around it)
        border: `${Math.max(1, 1.5 * k)}px solid rgba(255,255,255,0.035)`,
        borderRadius: "0",
        padding: `${12 * k}px ${14 * k}px`,
        textShadow: "none",
      });
    }
    // A border in Vice City colours (Read Areas), to tell this area's translation from the others':
    // hot pink, sunset orange and gold to purple and ocean cyan, with a soft pink glow; a light dark
    // backing where the area has no box of its own (outlined text), for the border to frame.
    if (o.accent) {
      const bw = Math.max(1.5, 2 * k);
      Object.assign(box.style, {
        border: `${bw}px solid transparent`,
        borderImage: "linear-gradient(90deg, #ff2d95 0%, #ff7a45 30%, #ffc94a 50%, #b45cff 72%, #2fd6ff 100%) 1",
        borderRadius: "0",
        boxShadow: `0 0 ${8 * k}px rgba(255, 45, 149, 0.35), 0 0 ${14 * k}px rgba(47, 214, 255, 0.18)`,
      });
      if (bgMode === "none") Object.assign(box.style, { background: "rgba(10, 6, 20, 0.5)", padding: `${5 * k}px ${12 * k}px` });
    }
  }

  // ---- GTA VI's phone in the hand (WhatUp, Snapmatic) -------------------------------------------
  // P: {screen: [x, y, w, h], screen_bg, pieces: [{rect, ar, title?, name?, bg, fg, label, end}]}
  // (fractions of the picture; see the core's phone.rs). The translations in a panel beside the phone
  // (on the side with more room) in the phone's own colours, each piece level with its English: a
  // bubble in its bubble's colours (the other person's white, the character's light blue, a reply to
  // pick dark blue), on the same side as on the phone; a header or a label ("Suggested", "New
  // message") plain, in its colour. A piece pushed down by a taller one above it doesn't overlap it.
  // root: an element covering exactly the game picture.
  // GTA VI's message bubble at the bottom right: its translation in a bubble just like it (its grey, the
  // contact's picture and name over the message, a tail at its bottom left), right above it, as wide,
  // growing upward for a longer text. C: {rect, name, ar, bg, fg, name_fg, avatar} (see chat.rs).
  // A message's text with its emojis: each slot "{n}" in it the n-th picture (copied from the game's), a
  // letter's height, on the line as text is.
  function withEmoji(text, pics) {
    // (emojis in a row stay together on one line: 🔥🔥🔥)
    return esc(text).replace(/(?:\{\d+\}\s*)+/g, (run) => {
      const imgs = run.match(/\d+/g).map((n) => {
        const src = (pics || [])[Number(n) - 1];
        return src ? `<img src="${src}" style="height:1.2em;vertical-align:-0.25em;margin:0 0.08em">` : "";
      });
      return `<span style="white-space:nowrap">${imgs.join("")}</span>${/\s$/.test(run) ? " " : ""}`;
    });
  }

  function renderChat(root, C) {
    const W = root.clientWidth, H = root.clientHeight;
    if (!C || !C.ar || !W || !H) return;
    const [gx, gy, gw] = [C.rect[0] * W, C.rect[1] * H, C.rect[2] * W];
    const fs = Math.max(11, 0.0185 * H);
    const pad = 0.6 * fs;
    const d = document.createElement("div");
    Object.assign(d.style, {
      position: "absolute",
      right: `${W - (gx + gw)}px`,
      bottom: `${H - gy + 0.45 * fs}px`,
      width: `${gw}px`,
      boxSizing: "border-box",
      padding: `${0.55 * pad}px ${pad}px ${0.75 * pad}px`,
      background: C.bg || "#c8c9cc",
      opacity: "0.97",
      borderRadius: `${0.45 * fs}px`,
      // (square where the tail joins it, as the game's)
      borderBottomLeftRadius: "0",
      direction: dir(),
      fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`,
      fontSize: `${fs}px`,
      lineHeight: "1.5",
      color: C.fg || "#1e1f24",
    });
    const av = 1.75 * fs;
    const pic = C.avatar ? `<img src="${C.avatar}" style="width:${av}px;height:${av}px;border-radius:50%;object-fit:cover;flex:none">` : "";
    // (the mark in the game's top corner, a map pin or a camera: at the name row's end, as there)
    const mark = C.badge ? `<img src="${C.badge}" style="height:${(1.3 * fs).toFixed(1)}px;flex:none;margin-inline-start:auto">` : "";
    d.innerHTML =
      (C.name || mark ? `<div style="display:flex;align-items:center;gap:${(0.45 * fs).toFixed(1)}px;margin-bottom:${(0.2 * fs).toFixed(1)}px">${pic}${C.name ? `<span style="font-weight:700;color:${C.name_fg || "#15161a"};unicode-bidi:plaintext">${esc(C.name)}</span>` : ""}${mark}</div>` : "") +
      `<div style="unicode-bidi:plaintext;text-align:${rtl() ? "right" : "left"};white-space:pre-wrap">${withEmoji(C.ar, C.emoji)}</div>`;
    // the tail at the bottom left, as the game's
    const tail = document.createElement("div");
    Object.assign(tail.style, {
      position: "absolute",
      // (overlapping the bubble's edge a little: no seam between them)
      left: `${-0.5 * fs}px`,
      bottom: "0",
      width: "0",
      height: "0",
      borderTop: `${0.7 * fs}px solid transparent`,
      borderRight: `${0.52 * fs}px solid ${C.bg || "#c8c9cc"}`,
    });
    d.appendChild(tail);
    root.appendChild(d);
  }

  // where each of the phone's buttons was first seen (see renderPhone)
  const PHONE_BUTTON_AT = {};
  // the phone's tilt as last drawn (eased toward each reading's: renderPhone)
  const PHONE_TILT = { turn: 0, slant: 0, at: 0, side: null };
  // where the second phone's screen corners were last drawn (eased toward each reading's: renderPhone)
  const PHONE_QUAD = { q: null };
  // the text size each chat settled on, and each bubble's width (renderPhone: kept, so its lines don't rewrap)
  const PHONE_FIT = {};
  const PHONE_WIDE = {};
  // each bubble's side (the character's own on the right), kept until three readings agree on the other
  const PHONE_END = {};
  // each chat bubble's box as first drawn (renderPhone: kept while readings stay near it)
  const PHONE_BOX = {};
  // the phone as last drawn, and when (renderPhone: held a moment when a frame has none)
  const PHONE_LAST = { P: null, at: 0 };
  // a chat's header as last read, and when (renderPhone: kept a moment when a reading misses it)
  const PHONE_TITLE = { p: null, at: 0 };
  // each chat's pieces' words and kind as held, by where they are (renderPhone)
  const PHONE_PIECE = {};
  // the names each chat showed as cards in a message (renderPhone)
  const PHONE_CARDS = {};
  // each chat's pieces' places at the last reading, and when it last scrolled (renderPhone)
  const PHONE_SCROLL = { y: {}, at: 0 };
  // each card's icon once found, by chat and name (renderPhone)
  const PHONE_CARD_ICON = {};
  // each chat's lone emojis as last read, and when (renderPhone)
  const PHONE_STK = {};
  // each chat's text size as kept (renderPhone)
  const PHONE_SIZE = {};
  // A box kept as first drawn: a reading near it (within 4 % of the screen's width each way) changes nothing, and a
  // different one changes it only when five readings in a row agree on it — a frame read once a little differently
  // (a photo missed, a bubble's colour) no longer moved it, and moved it back
  // The key of the held box nearest `b` among those kept under `base` (within 8 % of the screen's width), or a new
  // one: a photo's or an emoji's — by a band of its height, one read near a band's edge took two keys, and its box
  // went from one to the other
  function nearKey(base, b, colW) {
    let best = null, bestD = colW * 0.08;
    for (const k of Object.keys(PHONE_BOX)) {
      if (!k.startsWith(base + "#")) continue;
      const c = PHONE_BOX[k].b;
      const d = Math.max(Math.abs(c[0] - b[0]), Math.abs(c[1] - b[1]));
      if (d <= bestD) [best, bestD] = [k, d];
    }
    return best || base + "#" + Object.keys(PHONE_BOX).length + "-" + Math.round(b[1]);
  }
  function holdBox(key, b, colW) {
    const near = (a, c) => a.every((v, i) => Math.abs(v - c[i]) <= colW * 0.04);
    const H0 = PHONE_BOX[key];
    if (!H0) {
      PHONE_BOX[key] = { b: b.slice(), cand: null, n: 0 };
      return b;
    }
    if (near(b, H0.b)) {
      H0.cand = null;
      H0.n = 0;
      return H0.b;
    }
    if (H0.cand && near(b, H0.cand)) H0.n++;
    else [H0.cand, H0.n] = [b.slice(), 1];
    if (H0.n >= 5) [H0.b, H0.cand, H0.n] = [H0.cand, null, 0];
    return H0.b;
  }
  // each chat's photos' placeholders as drawn, and when last read (renderPhone)
  const PHONE_PHOTOS = {};
  // The CSS matrix3d taking four points (an element's own, px) to four others (the page's): a perspective
  // transform (homography), solved from the eight equations the four pairs give.
  function matrix3d(src, dst) {
    const A = [], bv = [];
    for (let i = 0; i < 4; i++) {
      const [x, y] = src[i], [u, v] = dst[i];
      A.push([x, y, 1, 0, 0, 0, -u * x, -u * y]); bv.push(u);
      A.push([0, 0, 0, x, y, 1, -v * x, -v * y]); bv.push(v);
    }
    // (Gaussian elimination with partial pivoting)
    for (let c = 0; c < 8; c++) {
      let m = c;
      for (let r = c + 1; r < 8; r++) if (Math.abs(A[r][c]) > Math.abs(A[m][c])) m = r;
      [A[c], A[m]] = [A[m], A[c]]; [bv[c], bv[m]] = [bv[m], bv[c]];
      if (Math.abs(A[c][c]) < 1e-12) return "none";
      for (let r = 0; r < 8; r++) {
        if (r === c) continue;
        const f = A[r][c] / A[c][c];
        for (let k = c; k < 8; k++) A[r][k] -= f * A[c][k];
        bv[r] -= f * bv[c];
      }
    }
    const h = bv.map((b, i) => b / A[i][i]);
    // (column-major: x' = (h0 x + h1 y + h2) / w, y' = (h3 x + h4 y + h5) / w, w = h6 x + h7 y + 1)
    const m = [h[0], h[3], 0, h[6], h[1], h[4], 0, h[7], 0, 0, 1, 0, h[2], h[5], 0, 1];
    return `matrix3d(${m.map((n) => +n.toFixed(8)).join(",")})`;
  }
  // the chat list's rows last drawn, by contact (or header), and when: held a moment when missed (renderPhone)
  const PHONE_ROWS = {};

  // the buttons at the bottom right ("SELECT", "BACK"): each one's word beside the game's, in its style
  function phoneButtons(root, P, W, H) {
    for (const b of P.buttons || []) {
      // (the game's buttons stay put: the place a button was first seen at is kept while it's read within a
      // few pixels of it, the reading's box shifting a pixel or two from frame to frame)
      const key = b.en || b.ar;
      const was = PHONE_BUTTON_AT[key];
      const near = was && Math.abs(was[0] - b.rect[0]) < 0.006 && Math.abs(was[1] - b.rect[1]) < 0.01 && Math.abs(was[3] - b.rect[3]) < 0.006;
      const r = near ? was : (PHONE_BUTTON_AT[key] = b.rect);
      const capH = r[3] * H;
      const d = document.createElement("div");
      Object.assign(d.style, {
        position: "absolute",
        right: `${W - r[0] * W + capH * 1.2}px`,
        top: `${(r[1] + r[3] / 2) * H}px`,
        transform: "translateY(-50%)",
        display: "flex",
        alignItems: "center",
        gap: `${capH * 0.45}px`,
        whiteSpace: "nowrap",
        direction: dir(),
        fontFamily: `"Noto Sans Arabic", ${fonts()}`,
        fontWeight: "700",
        fontSize: `${capH * 1.3}px`,
        lineHeight: "1",
        color: "#ffffff",
        textShadow: "0 1px 3px rgba(0,0,0,0.85)",
      });
      // (its icon first: on the right in a right-to-left row, as the game's after its word)
      if (b.icon) {
        const img = document.createElement("img");
        img.src = b.icon;
        const ih = b.icon_rect ? b.icon_rect[3] * H : capH * 1.3;
        Object.assign(img.style, { height: `${ih}px`, width: `${ih}px`, objectFit: "contain", flex: "none", display: "block" });
        d.appendChild(img);
      }
      const label = document.createElement("span");
      label.textContent = b.ar;
      d.appendChild(label);
      root.appendChild(d);
    }
  }

  // GTA VI's phone read whole (the core's phone_read.rs: `P.read`, its title and its pieces top
  // to bottom): a second phone beside the game's, upright, where the core put it once — the hand sways the
  // phone, this one stays still. Its pieces one under the other as a chat's (others' on the left, the
  // player's own on the right, as the game's), their text as big as fits the screen.
  function renderPhoneRead(root, P) {
    const W = root.clientWidth, H = root.clientHeight;
    if (!W || !H) return;
    const R = P.read;
    // (read, and nothing on it to translate: no second phone)
    if (R && !R.title && !(R.items || []).length) return;
    const [sx, sy, sw, sh] = [P.screen[0] * W, P.screen[1] * H, P.screen[2] * W, P.screen[3] * H];
    const side = 0.045 * sw, topB = 0.09 * sw, botB = 0.1 * sw, gap = 0.012 * W;
    // (on the side with more room)
    const x = sx > W - sx - sw ? sx - gap - side - sw : sx + sw + gap + side;
    const el = (parent, css, text) => {
      const d = document.createElement("div");
      Object.assign(d.style, css);
      if (text != null) d.textContent = text;
      parent.appendChild(d);
      return d;
    };
    const body = el(root, {
      position: "absolute",
      left: `${x - side}px`,
      top: `${sy - topB}px`,
      width: `${sw + 2 * side}px`,
      height: `${sh + topB + botB}px`,
      borderRadius: `${Math.max(6, 0.12 * sw)}px`,
      background: "linear-gradient(135deg, #3a3b3f 0%, #1d1e21 45%, #0e0f11 100%)",
      boxShadow: `0 ${0.04 * sw}px ${0.12 * sw}px rgba(0,0,0,0.55), inset 0 0 0 ${Math.max(1, 0.008 * sw)}px rgba(255,255,255,0.10)`,
    });
    let fs = 0.058 * sw;
    const panel = el(body, {
      position: "absolute",
      left: `${side}px`,
      top: `${topB}px`,
      width: `${sw}px`,
      height: `${sh}px`,
      boxSizing: "border-box",
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
      background: P.screen_bg || "#f2f3f6",
      borderRadius: `${Math.max(2, 0.025 * sw)}px`,
      fontFamily: `"Noto Sans Arabic", ${fonts()}`,
      fontSize: `${fs}px`,
      lineHeight: "1.45",
      color: "#1b1c20",
    });
    if (!R) {
      // (its first reading on its way)
      el(panel, { margin: "auto", fontSize: "2em", opacity: "0.45" }, "…");
      return;
    }
    if (R.title) el(panel, { flex: "none", padding: "0.45em 0.6em", textAlign: "center", fontWeight: "700", direction: dir(), background: "rgba(255,255,255,0.55)", borderBottom: "1px solid rgba(0,0,0,0.12)" }, R.title);
    // (what was read last stays, as it is, until a reading differs)
    const list = el(panel, { flex: "1 1 auto", minHeight: "0", display: "flex", flexDirection: "column", gap: "0.4em", padding: "0.5em", overflow: "hidden" });
    const LOOK = {
      msg: { alignSelf: "flex-start", background: "rgba(255,255,255,0.9)", border: "1px solid rgba(70,90,200,0.35)" },
      me: { alignSelf: "flex-end", background: "#cfe1f7" },
      card: { alignSelf: "flex-start", background: "rgba(0,0,0,0.07)", border: "1px solid rgba(0,0,0,0.12)", fontWeight: "700", fontSize: "0.9em" },
      label: { alignSelf: "center", fontSize: "0.82em", opacity: "0.7", padding: "0 0.4em" },
    };
    for (const it of R.items || []) {
      if (!it.text) continue;
      const d = el(list, Object.assign({ flex: "none", maxWidth: "92%", padding: "0.3em 0.6em", borderRadius: "0.6em", direction: dir(), textAlign: "start", overflowWrap: "anywhere" }, LOOK[it.k] || LOOK.msg));
      // (the sender's name in the colour the game writes it in — each sender's own, as read off the phone — else purple)
      if (it.from) el(d, { fontWeight: "700", fontSize: "0.9em", color: /^#[0-9a-f]{6}$/i.test(it.color || "") ? it.color : "#5b46b0" }, it.from);
      el(d, {}, it.text);
    }
    // (as big as fits: smaller until its last piece is on the screen, no smaller than half the size)
    for (let i = 0; i < 12 && list.scrollHeight > list.clientHeight + 1; i++) {
      fs *= 0.94;
      panel.style.fontSize = `${fs}px`;
    }
  }

  function renderPhone(root, P, fit, again) {
    root.innerHTML = "";
    if (P && P.chat) renderChat(root, P.chat);
    if (P && "read" in P) return renderPhoneRead(root, P);
    // (the phone not read for a moment — a frame it wasn't found in: the last one drawn stays up a little)
    // (`replay`: the last phone drawn again — its text's size and everything as a reading's, only nothing measured
    // anew: no easing, no scroll told)
    let replay = false;
    if (!P || !P.pieces || !P.pieces.length) {
      if (!PHONE_LAST.P || Date.now() - PHONE_LAST.at > 700) return;
      P = PHONE_LAST.P;
      replay = true;
    } else if (!again) {
      // (a chat's header missed by a reading — its short name — kept a moment: the chat's held places are found by it)
      const tP = P.pieces.find((p) => p.title);
      // (kept for the whole chat — "Cal" went unread for seconds at a time — until the list is back or another
      // chat's header is read)
      if (tP) [PHONE_TITLE.p, PHONE_TITLE.at] = [tP, Date.now()];
      else if (P.list) PHONE_TITLE.p = null;
      else if (PHONE_TITLE.p) P = Object.assign({}, P, { pieces: [PHONE_TITLE.p].concat(P.pieces) });
      PHONE_LAST.P = P;
      PHONE_LAST.at = Date.now();
    }
    const W = root.clientWidth, H = root.clientHeight;
    if (!W || !H) return;
    const [sx, sy, sw, sh] = [P.screen[0] * W, P.screen[1] * H, P.screen[2] * W, P.screen[3] * H];
    // (the screen's four corners known: the second phone laid out flat at the screen's own size, each piece where
    // it is on the screen, then put through the corners of the game's screen moved a phone's width aside — the
    // same size, turn, slant and perspective as the phone in the hand)
    const Q = P.quad && P.pieces.every((p) => p.uv) ? P.quad.map((c) => [c[0] * W, c[1] * H]) : null;
    const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1]);
    const qw0 = Q ? (dist(Q[0], Q[1]) + dist(Q[3], Q[2])) / 2 : 0, qh0 = Q ? (dist(Q[0], Q[3]) + dist(Q[1], Q[2])) / 2 : 0;
    // (laid out at a fixed width, its height the screen's shape — kept while within 3 % — and put on the screen by
    // the transform: measured each frame, the screen's size wobbled a pixel or two, and the bubbles drifted)
    if (Q && (!PHONE_QUAD.aspect || Math.abs(qh0 / qw0 - PHONE_QUAD.aspect) > PHONE_QUAD.aspect * 0.03)) PHONE_QUAD.aspect = qh0 / qw0;
    const qw = Q ? 360 : 0, qh = Q ? 360 * PHONE_QUAD.aspect : 0;
    if (Q && !P._flat) {
      // (a chat's header is at the very top of its screen: a "title" lower down is a message's line cut by the header
      // bar after a scroll — its words read as a name and spelt out — and is no piece to show; one header at most)
      const tops = P.pieces.filter((p) => p.title && p.uv[1] <= 0.07).sort((a, b) => a.uv[1] - b.uv[1]);
      const keepTitle = tops[0];
      P = Object.assign({}, P, {
        _flat: true,
        pieces: P.pieces.filter((p) => !p.title || p === keepTitle).map((p) => Object.assign({}, p, {
          rect: [(p.uv[0] * qw) / W, (p.uv[1] * qh) / H, (p.uv[2] * qw) / W, (p.uv[3] * qh) / H],
          box: p.box_uv ? [p.box_uv[0] * qw, p.box_uv[1] * qh, p.box_uv[2] * qw, p.box_uv[3] * qh] : null,
        })),
      });
    }
    // A second phone beside the game's, in the same world: the same screen size, held at the same angle, its
    // dark body round it, its screen as lit as the game's (its colour measured there: dim at night).
    const gap = 0.012 * W;
    // (the screen's own size, measured upright in the frame — the box round a tilted screen is bigger; else from
    // that box and the turn)
    let colW, th, panelTop;
    if (Q) {
      [colW, th, panelTop] = [qw, qh, 0];
    } else if (P.screen_own) {
      [colW, th, panelTop] = [P.screen_own[2] * W, P.screen_own[3] * H, P.screen_own[1] * H];
    } else {
      const tr = Math.abs(+P.turn || 0) > 0.3 ? (Math.abs(+P.turn) * Math.PI) / 180 : 0;
      const [ct, st2] = [Math.cos(tr), Math.sin(tr)];
      const c2 = ct * ct - st2 * st2;
      [colW, th] = [sw, sh];
      if (tr && c2 > 0.3) {
        colW = (sw * ct - sh * st2) / c2;
        th = (sh * ct - sw * st2) / c2;
        if (!(colW > 0.3 * sw && th > 0.3 * sh)) [colW, th] = [sw, sh];
      }
      panelTop = sy + sh / 2 - th / 2;
    }
    // the phone's body round the screen: thin sides, a little more above (its earpiece) and below
    const side = 0.045 * colW, topB = 0.09 * colW, botB = 0.1 * colW;
    const roomLeft = sx - gap * 2 - (Q ? qw0 * 1.1 : 2 * side), roomRight = W - (sx + sw) - gap * 2 - (Q ? qw0 * 1.1 : 2 * side);
    // (the side it was on, unless the other has clearly more room: chosen anew each frame, the panel jumped
    // from one side of the phone to the other)
    const T0 = PHONE_TILT;
    const onLeft = T0.side === "left" ? roomLeft * 1.25 >= roomRight : T0.side === "right" ? roomLeft > roomRight * 1.25 : roomLeft >= roomRight;
    T0.side = onLeft ? "left" : "right";
    const x = Q ? 0 : onLeft ? sx - gap - side - colW : sx + sw + gap + side;
    // (its text the phone's size; in a chat list no bigger than its rows allow: each row as tall as the phone's,
    // the rows one for one with the phone's; smaller again when what it shows doesn't fit — Arabic wraps to more
    // lines than the English: `fit`)
    const ys = P.pieces.filter((p) => p.avatar || p.name).map((p) => p.rect[1] * H).sort((a, b) => a - b);
    const pitches = ys.slice(1).map((y, i) => y - ys[i]).filter((d) => d > 4).sort((a, b) => a - b);
    const pitch0 = pitches.length ? pitches[Math.floor(pitches.length / 2)] : 0;
    // (the rows' spacing held per list: measured anew each frame, the rows' height went from one size to another)
    const pitch = pitch0 && Q ? holdBox((P.list ? "list" : "chat") + "|pitch", [pitch0], 360 * 2)[0] : pitch0;
    // (the size a chat settled on is kept: worked out anew each frame it came out a little different, and the
    // bubbles' words moved from line to line; it only gets smaller when what's shown no longer fits)
    // (by what it shows: the same chat with the same messages keeps its size; new messages, another chat, worked
    // out again from the phone's own size)
    const chatId = P.list ? "list" : ((P.pieces.find((p) => p.title) || {}).ar || "");
    // (scrolling: the same pieces moved together since the last reading — the held places and words are let go,
    // the screen followed as read; once it stops, they're held again from the readings then. A moment after a
    // scroll stops, the layout is settled)
    if (Q && !again && !replay) {
      const prevY = PHONE_SCROLL.y[chatId] || {};
      const nowY = {};
      const shifts = [];
      for (const p of P.pieces) {
        if (p.title) continue;
        const k = (p.name || "") + "|" + p.ar;
        nowY[k] = p.rect[1] * H;
        if (k in prevY) shifts.push(nowY[k] - prevY[k]);
      }
      PHONE_SCROLL.y[chatId] = nowY;
      shifts.sort((a, b) => a - b);
      const med = shifts.length ? shifts[Math.floor(shifts.length / 2)] : 0;
      if (Math.abs(med) > 0.015 * qh) {
        for (const k of Object.keys(PHONE_BOX)) if (k.startsWith(chatId + "|")) delete PHONE_BOX[k];
        PHONE_PIECE[chatId] = [];
        delete PHONE_SIZE[chatId];
        PHONE_SCROLL.at = Date.now();
      }
    }
    const chatKey = chatId + "|" + P.pieces.length + "|" + P.pieces.reduce((n, p) => n + (p.ar || "").length, 0);
    if (!again) fit = PHONE_FIT[chatKey] || 1;
    // (the phone's text is about 4.5 % of its screen's width: laid out on the screen's shape, the same share)
    const base = Q ? 0.045 * colW : Math.max(11, 0.04 * colW);
    const fs = Math.max(Q ? 0.025 * colW : 8, Math.min(base, P.list && pitch ? pitch / 4.3 : 1e9) * (fit || 1));
    const pad = 0.5 * fs;
    const body = document.createElement("div");
    body.className = "phone-body";
    root.appendChild(body);
    const r0 = Math.max(6, 0.12 * colW);
    Object.assign(body.style, {
      position: "absolute",
      left: `${x - side}px`,
      top: `${panelTop - topB}px`,
      width: `${colW + 2 * side}px`,
      height: `${th + topB + botB}px`,
      borderRadius: `${r0}px`,
      background: "linear-gradient(135deg, #3a3b3f 0%, #1d1e21 45%, #0e0f11 100%)",
      boxShadow: `0 ${0.04 * colW}px ${0.12 * colW}px rgba(0,0,0,0.55), inset 0 0 0 ${Math.max(1, 0.008 * colW)}px rgba(255,255,255,0.10)`,
    });
    // (its earpiece and camera above the screen, a pill below it)
    const dot = (css) => {
      const d = document.createElement("div");
      Object.assign(d.style, { position: "absolute" }, css);
      body.appendChild(d);
    };
    dot({ left: `${side + colW * 0.38}px`, top: `${topB * 0.38}px`, width: `${colW * 0.24}px`, height: `${Math.max(2, topB * 0.22)}px`, borderRadius: "9px", background: "#050506" });
    dot({ left: `${side + colW * 0.66}px`, top: `${topB * 0.34}px`, width: `${Math.max(3, topB * 0.3)}px`, height: `${Math.max(3, topB * 0.3)}px`, borderRadius: "50%", background: "#0b0c10" });
    dot({ left: `${side + colW * 0.42}px`, top: `${topB + th + botB * 0.38}px`, width: `${colW * 0.16}px`, height: `${Math.max(2, botB * 0.22)}px`, borderRadius: "9px", background: "#050506" });
    const panel = document.createElement("div");
    panel.className = "phone-panel";
    body.appendChild(panel);
    Object.assign(panel.style, {
      position: "absolute",
      left: `${side}px`,
      top: `${topB}px`,
      width: `${colW}px`,
      height: `${th}px`,
      overflow: "hidden",
      background: P.screen_bg || "#f2f3f6",
      borderRadius: `${Math.max(2, 0.025 * colW)}px`,
    });
    // (held tilted, the second phone as the first: turned as its lines of text are (`turn`, degrees clockwise),
    // its column of pictures slanting as theirs (`slant` more), round the game phone screen's middle — the two
    // held together. Eased from one reading to the next: the hand sways, it follows smoothly)
    const T = PHONE_TILT;
    if (!again && !replay) {
      const k = T.at ? 0.35 : 1;
      T.turn += ((+P.turn || 0) - T.turn) * k;
      T.slant += ((+P.slant || 0) - T.slant) * k;
      T.at = 1;
    }
    if (Q) {
      // (the game's screen corners moved along its own top edge by a phone's width and a gap: beside it, the same
      // way up; eased from one reading to the next as the hand sways)
      const ux = [(Q[1][0] - Q[0][0]) / dist(Q[0], Q[1]), (Q[1][1] - Q[0][1]) / dist(Q[0], Q[1])];
      // (moved by the real screen's width and the body's sides at its scale, and a gap)
      const shift = (qw0 * (1 + (2 * side) / qw) + gap) * (onLeft ? -1 : 1);
      const target = Q.map((c) => [c[0] + ux[0] * shift, c[1] + ux[1] * shift]);
      const E = PHONE_QUAD;
      if (!again && !replay) {
        const k = E.q ? 0.4 : 1;
        E.q = target.map((c, i) => (E.q ? [E.q[i][0] + (c[0] - E.q[i][0]) * k, E.q[i][1] + (c[1] - E.q[i][1]) * k] : c));
      }
      const src = [[side, topB], [side + colW, topB], [side + colW, topB + th], [side, topB + th]];
      Object.assign(body.style, { left: "0px", top: "0px", transformOrigin: "0 0", transform: matrix3d(src, E.q || target) });
    } else if (Math.abs(T.turn) > 0.3 || Math.abs(T.slant) > 0.3) {
      const r = (d) => (d * Math.PI) / 180;
      const [a, b, c, d2] = [Math.cos(r(T.turn)), Math.sin(r(T.turn)), -Math.sin(r(T.turn + T.slant)), Math.cos(r(T.turn + T.slant))];
      Object.assign(body.style, {
        transformOrigin: `${sx + sw / 2 - (x - side)}px ${sy + sh / 2 - (panelTop - topB)}px`,
        transform: `matrix(${a.toFixed(4)}, ${b.toFixed(4)}, ${c.toFixed(4)}, ${d2.toFixed(4)}, 0, 0)`,
      });
    }
    // (in the panel: places relative to its top left)
    const put = (d, left, top) => Object.assign(d.style, { left: `${left - x}px`, top: `${top - panelTop}px` });
    phoneButtons(root, P, W, H);
    let below = -1e9;
    const fitList = [];
    // (the other person's bubbles' left margin, the smallest: the character's own keep the same on the right)
    const margins = { left: null };
    // (the chat's photos on the flat screen: a bubble whose words have a photo right under them holds it, as on the
    // phone — its box runs on round the photo)
    const photoFlat = Q ? (P.photos || []).map((u) => [u[0] * qw, u[1] * qh, u[2] * qw, u[3] * qh]) : [];
    // (each photo's bubble, decided once: the message whose words end nearest its top, above it or into it — read
    // a little high, a photo took the bubble above its own now and then, and the two swapped back and forth)
    if (Q) {
      for (const ph of photoFlat) {
        let best = null, bestD = Infinity;
        for (const p of P.pieces) {
          if (p.title || p.label) continue;
          const [ty, tb] = [p.rect[1] * H, (p.rect[1] + p.rect[3]) * H];
          const across = Math.min((p.rect[0] + p.rect[2]) * W, ph[0] + ph[2]) - Math.max(p.rect[0] * W, ph[0]) > 0;
          // (the photo within its bubble's width: a short message's bubble beside a wide photo isn't its bubble)
          const bb = p.box || [p.rect[0] * W - 0.7 * fs, 0, p.rect[2] * W + 1.4 * fs, 0];
          const within = ph[0] >= bb[0] - 0.08 * colW && ph[0] + ph[2] <= bb[0] + bb[2] + 0.08 * colW;
          if (!across || !within || ty > ph[1] + ph[3] * 0.5) continue;
          const dd = Math.abs(ph[1] - tb);
          if (dd < bestD && dd <= 3 * fs) [best, bestD] = [p, dd];
        }
        if (best) {
          ph.owner = best;
          // (its top under its words, not over them)
          const tb = (best.rect[1] + best.rect[3]) * H + 0.3 * fs;
          if (ph[1] < tb) [ph[3], ph[1]] = [ph[3] - (tb - ph[1]), tb];
        }
      }
    }
    // (the character's own messages: blue bubbles, its blue clearly above its red; the other person's grey-white)
    const isOwn = (p) => {
      const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(p.bg || "");
      return m ? parseInt(m[3], 16) - parseInt(m[1], 16) > 14 : !!p.end;
    };
    if (Q) {
      for (const p of P.pieces) {
        if (p.title || p.label) continue;
        const b = p.box || [p.rect[0] * W - 0.7 * fs, 0, p.rect[2] * W + 1.4 * fs, 0];
        if (!isOwn(p)) margins.left = margins.left == null ? b[0] : Math.min(margins.left, b[0]);
      }
      if (margins.left != null) margins.left = Math.max(0.3 * pad, margins.left);
    }
    // (a row read before and missed this time stays a moment where it was: one reading in a few misses a row
    // — a picture's dark edge, a line the OCR skips — and the row blinked out and back)
    const nowT = Date.now();
    const rowKey = (p) => (p.name || "") + "|" + (p.label ? p.ar : "");
    // (and a plain line, or a nameless row, read where a contact's row was a moment ago is that row read badly —
    // "Paradise Taxi Hi! Your driver…" run together as one line, translated whole: the row as it was)
    const overHeld = (q) => !q.name && Object.keys(PHONE_ROWS).some((k) => {
      const r = PHONE_ROWS[k];
      return r.p.name && nowT - r.at <= 700 && q.rect[1] < r.p.rect[1] + r.p.rect[3] && r.p.rect[1] < q.rect[1] + q.rect[3];
    });
    let pieces = P.list ? P.pieces.filter((q) => !overHeld(q)) : P.pieces.slice();
    // (a chat's pieces: each one's words and kind — a label as "New message", a bubble — as first read at its place,
    // until three readings in a row agree on others: "New message" read now and then as "message", and as a
    // bubble, its Arabic changed and it jumped from the middle to the side and back)
    if (Q) {
      const held = (PHONE_PIECE[chatId] = PHONE_PIECE[chatId] || []);
      const tol = 0.05 * colW;
      pieces = pieces.map((p) => {
        if (p.title) return p;
        const y = p.rect[1] * H;
        let h = held.find((k) => Math.abs(k.y - y) <= tol);
        if (!h) {
          h = { y: y, ar: p.ar, label: !!p.label, name: p.name || "", pending: !!p.pending, hist: [] };
          held.push(h);
        }
        h.y = y;
        // (the words and kind it was read as most often over its last ten readings — a stray misreading, even its
        // first, doesn't stay; the one shown now keeps a tie)
        // (a list's row: its contact's name too — "Shirley" read once as "my rum", the row's name was that)
        // (its translation come, its English shown meanwhile: the translation at once)
        if (h.pending && !p.pending) [h.hist, h.ar, h.label, h.name, h.pending] = [[], p.ar, !!p.label, p.name || "", false];
        if (p.pending && !h.pending) {
          h.p = Object.assign({}, p, { ar: h.ar, label: h.label, name: h.name || null, pending: false });
          h.at = Date.now();
          h.seen = true;
          return h.p;
        }
        h.hist.push((p.label ? "L" : "B") + p.ar + "\u0001" + (p.name || ""));
        if (h.hist.length > 10) h.hist.shift();
        const tally = {};
        for (const k of h.hist) tally[k] = (tally[k] || 0) + 1;
        const cur = (h.label ? "L" : "B") + h.ar + "\u0001" + h.name;
        let best = cur;
        for (const k in tally) if (tally[k] > (tally[best] || 0)) best = k;
        if (best !== cur) {
          const [w0, n0] = best.slice(1).split("\u0001");
          [h.label, h.ar, h.name] = [best[0] === "L", w0, n0];
        }
        h.p = Object.assign({}, p, { ar: h.ar, label: h.label, name: h.name || null, pending: h.pending });
        h.at = Date.now();
        h.seen = true;
        return h.p;
      });
      // (one missed by this reading stays a moment where it was, unless another piece is there now)
      const nowH = Date.now();
      for (const h of held) {
        if (!h.seen && h.p && nowH - h.at < 700) {
          const [ty, tb] = [h.p.rect[1] * H, (h.p.rect[1] + h.p.rect[3]) * H];
          if (!pieces.some((q) => !q.title && q.rect[1] * H < tb && ty < (q.rect[1] + q.rect[3]) * H)) pieces.push(h.p);
        }
        h.seen = false;
      }
      for (let i = held.length - 1; i >= 0; i--) if (nowH - held[i].at > 3000) held.splice(i, 1);
      pieces.sort((a, b) => (a.title ? -1 : b.title ? 1 : a.rect[1] - b.rect[1]));
    }
    const seenNow = new Set(pieces.map(rowKey));
    if (P.list) {
      for (const k in PHONE_ROWS) {
        const r = PHONE_ROWS[k];
        if (seenNow.has(k)) continue;
        if (nowT - r.at > 700) { delete PHONE_ROWS[k]; continue; }
        const [ry, rh] = [r.p.rect[1], r.p.rect[3]];
        if (!pieces.some((q) => q.rect[1] < ry + rh && ry < q.rect[1] + q.rect[3])) pieces.push(r.p);
      }
      for (const p of pieces) if (p.name || p.label) PHONE_ROWS[rowKey(p)] = { p: p, at: nowT };
      pieces.sort((a, b) => a.rect[1] - b.rect[1]);
    }
    // (a card in a message — a place's or a shop's name kept as it is, "Three Sheets Liquor", right under the
    // message's words: inside that message's bubble, as on the phone, not a message of its own)
    const cardOf = new Map();
    if (Q && !P.list) {
      const latin = (t) => /^[\x20-\x7E]+$/.test(t || "") && (t || "").trim().split(/\s+/).length <= 5;
      const msgs = pieces.filter((p) => !p.title && !p.label);
      // (a card's line read as a label too — it has no bubble's colour of its own)
      for (const c of pieces.filter((p) => !p.title)) {
        if (!latin(c.ar)) continue;
        const above = msgs.filter((m) => m !== c && !latin(m.ar) && (m.rect[1] + m.rect[3]) * H <= c.rect[1] * H + 0.2 * fs)
          .sort((a, b) => (b.rect[1] + b.rect[3]) - (a.rect[1] + a.rect[3]))[0];
        // (once a card in this chat, always: a reading a little apart from its message no longer made it a message)
        const known = (PHONE_CARDS[chatId] = PHONE_CARDS[chatId] || new Set());
        if (above && (known.has(c.ar) || c.rect[1] * H - (above.rect[1] + above.rect[3]) * H <= 3 * fs)) {
          cardOf.set(c, above);
          known.add(c.ar);
        }
      }
    }
    for (const p of pieces) {
      if (!p.ar && !(P.list && (p.name || p.avatar))) continue;
      if (P.list && Q) {
        // (laid out as the phone's list: each row a card across the screen at its own place and height, the
        // contact's picture on the left, the mark on the right, the name and the message's start between; the
        // section headers with their icon, as the phone's)
        const d = document.createElement("div");
        // (each row's card as tall as the phone's: nearly all of the rows' spacing)
        const rowH = pitch ? Math.max(pitch * 0.9, 2.6 * fs) : 3.4 * fs;
        const cy = (p.rect[1] + p.rect[3] / 2) * H;
        Object.assign(d.style, { position: "absolute", boxSizing: "border-box", direction: "ltr", fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`, fontSize: `${fs}px`, lineHeight: "1.3" });
        let bx;
        if (p.name || p.avatar) {
          // (placed by its name's line, which sits as high in every row — a row whose message is only an emoji has
          // one line of words, and by the words' middle it rose into the row above)
          const top0 = p.rect[1] * H - 0.27 * rowH;
          bx = holdBox(chatId + "|row|" + Math.round(top0 / (0.08 * colW)), [0.03 * colW, top0, 0.94 * colW, rowH], colW);
          const av = rowH * 0.72;
          // (the card a little lighter than the screen, as the phone's rows are)
          const card = "rgba(255,255,255,0.55)";
          Object.assign(d.style, {
            width: `${bx[2]}px`, height: `${bx[3]}px`, display: "flex", alignItems: "center", gap: `${0.55 * fs}px`,
            padding: `0 ${0.55 * fs}px`, background: card, borderRadius: `${0.2 * fs}px`, boxShadow: "0 1px 2px rgba(0,0,0,0.12)",
            border: p.edge ? `${Math.max(2, 0.12 * fs).toFixed(1)}px solid ${p.edge}` : "none",
          });
          const pic = p.avatar ? `<img src="${p.avatar}" style="width:${av}px;height:${av}px;border-radius:50%;object-fit:cover;flex:none">` : `<div style="width:${av}px;height:${av}px;border-radius:50%;background:#c9cad3;flex:none"></div>`;
          const one = "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;unicode-bidi:plaintext";
          const msg = p.ar ? withEmoji(p.ar, p.emoji) : p.preview ? `<img src="${p.preview}" style="height:${(0.95 * fs).toFixed(1)}px;vertical-align:middle">` : "&nbsp;";
          const mark = p.badge ? `<img src="${p.badge}" style="height:${(1.1 * fs).toFixed(1)}px;flex:none;align-self:flex-start;margin-top:${(0.45 * fs).toFixed(1)}px">` : "";
          d.innerHTML =
            pic +
            // (its words starting next to the picture, as on the phone)
            `<div style="min-width:0;flex:1;direction:${dir()};text-align:left">` +
            (p.name ? `<div style="${one};font-weight:700;color:#24252c">${esc(p.name)}</div>` : "") +
            `<div style="${one};color:#6a6c77;font-size:${(0.92 * fs).toFixed(1)}px">${msg}</div></div>` +
            mark;
        } else {
          bx = holdBox(chatId + "|head|" + p.ar, [p.rect[0] * W, cy - 0.7 * fs, 0, 0], colW);
          Object.assign(d.style, { color: "#7a7c88", fontSize: `${0.9 * fs}px`, display: "flex", alignItems: "center", gap: `${0.35 * fs}px`, whiteSpace: "nowrap" });
          d.innerHTML = (p.badge ? `<img src="${p.badge}" style="height:${(1.1 * fs).toFixed(1)}px;flex:none">` : "") + `<span style="direction:${dir()};unicode-bidi:plaintext">${esc(p.ar)}</span>`;
        }
        panel.appendChild(d);
        put(d, x + bx[0], panelTop + bx[1]);
        below = panelTop + bx[1] + d.offsetHeight;
        continue;
      }
      if (P.list) {
        // WhatUp's chat list, as on the phone: rows of the contact's picture, name and message's start (one
        // line, cut), the chosen row outlined; the section headers ("Suggested", "Messages") plain grey
        const d = document.createElement("div");
        const rowW = colW - 2 * pad;
        Object.assign(d.style, { position: "absolute", boxSizing: "border-box", direction: dir(), fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`, fontSize: `${fs}px`, lineHeight: "1.3", width: `${rowW}px` });
        if (p.name || p.avatar) {
          const av = 2.9 * fs;
          Object.assign(d.style, {
            display: "flex",
            alignItems: "center",
            gap: `${0.6 * fs}px`,
            padding: `${0.3 * fs}px ${0.6 * fs}px`,
            background: p.bg || "#f4f4f7",
            // (square-cornered, as on the phone)
            borderRadius: `${0.12 * fs}px`,
            border: p.edge ? `${Math.max(2, 0.12 * fs).toFixed(1)}px solid ${p.edge}` : "none",
          });
          // (the picture as on the phone, its unread dot too)
          const pic = p.avatar ? `<img src="${p.avatar}" style="width:${av}px;height:${av}px;flex:none">` : `<div style="width:${av}px;height:${av}px;border-radius:50%;background:#c9cad3;flex:none"></div>`;
          const one = "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;unicode-bidi:plaintext";
          const lineH = 1.3 * fs;
          // (the message: its translation; one that is no text, an emoji, as it is; none yet: an empty line)
          const msg = p.ar ? withEmoji(p.ar, p.emoji) : p.preview ? `<img src="${p.preview}" style="height:${(0.95 * fs).toFixed(1)}px;vertical-align:middle">` : "&nbsp;";
          const mark = p.badge ? `<img src="${p.badge}" style="height:${(1.15 * fs).toFixed(1)}px;flex:none;align-self:flex-start;margin-top:${(0.15 * fs).toFixed(1)}px">` : "";
          d.innerHTML =
            pic +
            `<div style="min-width:0;flex:1;text-align:${rtl() ? "right" : "left"}">` +
            (p.name ? `<div style="${one};font-weight:700;color:#24252c">${esc(p.name)}</div>` : "") +
            `<div style="${one};color:#6a6c77;min-height:${lineH.toFixed(1)}px">${msg}</div></div>` +
            mark;
        } else {
          // (a section's header, its icon before it as on the phone: "Suggested")
          Object.assign(d.style, { color: "#7a7c88", fontSize: `${0.9 * fs}px`, padding: `0 ${0.2 * fs}px`, display: "flex", alignItems: "center", gap: `${0.35 * fs}px` });
          d.innerHTML = (p.badge ? `<img src="${p.badge}" style="height:${(1.1 * fs).toFixed(1)}px;flex:none">` : "") + `<span style="unicode-bidi:plaintext">${esc(p.ar)}</span>`;
        }
        panel.appendChild(d);
        // (its place kept as first drawn while readings stay near it: see holdBox)
        const top = holdBox(chatId + "|row|" + (p.name || p.ar), [Math.max(p.rect[1] * H - 0.6 * fs, below + 0.3 * fs)], colW)[0];
        put(d, x + pad, top);
        below = top + d.offsetHeight;
        continue;
      }
      // the chat's header, as the phone's: a bar across the top, the back arrow, the contact's name in the
      // middle, their picture at the end
      if (p.title) {
        const bar = document.createElement("div");
        const bh = 2.6 * fs;
        Object.assign(bar.style, {
          position: "absolute", left: "0", top: "0", width: `${colW}px`, height: `${bh}px`, boxSizing: "border-box",
          display: "flex", alignItems: "center", justifyContent: "space-between", padding: `0 ${pad}px`,
          background: p.bg || "#f4f4f7", borderBottom: "1px solid rgba(0,0,0,0.08)", direction: "ltr",
          fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`, fontSize: `${fs}px`, color: p.fg || "#26272d",
        });
        const pic = p.avatar ? `<img src="${p.avatar}" style="width:${(1.9 * fs).toFixed(1)}px;height:${(1.9 * fs).toFixed(1)}px;border-radius:50%;object-fit:cover;flex:none">` : `<span style="width:${(1.9 * fs).toFixed(1)}px;flex:none"></span>`;
        // (laid out as the game's phone is, whatever the language: its back arrow on the left, the contact's picture
        // on the right — the second phone a copy of the first)
        bar.innerHTML = `<span style="color:#3d6fe0;font-size:${(1.6 * fs).toFixed(1)}px;line-height:1;flex:none;width:${(1.9 * fs).toFixed(1)}px">‹</span><b style="unicode-bidi:plaintext;direction:${dir()}">${esc(p.ar)}</b>${pic}`;
        panel.appendChild(bar);
        below = panelTop + bh;
        continue;
      }
      if (cardOf.has(p) && p.cardBox !== undefined || cardOf.has(p)) {
        // (drawn once its message's bubble is: see below, after the loop)
        continue;
      }
      const d = document.createElement("div");
      d.innerHTML = (p.name ? `<div style="font-weight:700;margin-bottom:${(0.15 * fs).toFixed(1)}px">${esc(p.name)}</div>` : "") + withEmoji(p.ar, p.emoji);
      Object.assign(d.style, {
        position: "absolute",
        boxSizing: "border-box",
        direction: dir(),
        unicodeBidi: "plaintext",
        textAlign: rtl() ? "right" : "left",
        fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`,
        fontSize: `${fs * 0.94}px`,
        lineHeight: "1.4",
        whiteSpace: "pre-wrap",
        color: p.fg || "#2b2d34",
        maxWidth: `${colW - 2 * pad}px`,
        width: "max-content",
      });
      // (a bubble as wide as the game's: its words' width, its padding round them)
      // (its width kept while a new reading is within 15 % of it: the words' box as read is a few pixels wider or
      // narrower each frame, and the Arabic wrapped differently each time. Laid out on the screen's own size, the
      // piece's width is the screen's already)
      let wide = Math.round(Math.min(colW - 2 * pad, p.rect[2] * W * (Q ? 1 : colW / sw) + 1.4 * fs));
      const wk = chatId + "|" + p.ar;
      const was = PHONE_WIDE[wk];
      if (was && Math.abs(was - wide) <= was * 0.15) wide = was;
      else PHONE_WIDE[wk] = wide;
      if (p.boxed) {
        // a map pin's card: bordered, its pin before the words
        Object.assign(d.style, {
          width: `${colW - 2 * pad}px`, background: p.bg || "#e9e9ee", border: "1px solid rgba(0,0,0,0.18)",
          borderRadius: `${0.25 * fs}px`, padding: `${0.55 * fs}px ${0.8 * fs}px`, fontWeight: "600",
        });
        // (the phone's pin: a small pale-blue disc)
        const pin = `<span style="display:inline-block;width:${(0.9 * fs).toFixed(1)}px;height:${(0.9 * fs).toFixed(1)}px;border-radius:50%;background:#d3e3f6;border:1px solid #a9c5e8;vertical-align:-0.1em;margin-inline-end:${(0.5 * fs).toFixed(1)}px"></span>`;
        d.innerHTML = pin + d.innerHTML;
      } else if (p.label) {
        // ("New message", a day's date: centred, as on the phone)
        Object.assign(d.style, { fontSize: `${0.85 * fs}px`, fontWeight: "600", textAlign: "center" });
      } else {
        Object.assign(d.style, {
          background: p.bg || "#f8f9fb",
          // (the character's own, blue: rounder, as the phone's)
          borderRadius: `${(p.end ? 0.9 : 0.45) * fs}px`,
          padding: `${0.35 * fs}px ${0.7 * fs}px`,
          width: `${wide}px`,
          // (a reply to pick: outlined, as on the phone)
          border: p.edge ? `${Math.max(2, 0.12 * fs).toFixed(1)}px solid ${p.edge}` : "none",
          boxShadow: p.edge ? "none" : "0 1px 1px rgba(0,0,0,0.06)",
        });
      }
      panel.appendChild(d);
      // (the reply to pick: its send button beside it)
      if (false) {
        const send = document.createElement("div");
        const ss = 1.3 * fs;
        Object.assign(send.style, {
          position: "absolute", width: `${ss}px`, height: `${ss}px`, borderRadius: "50%", background: "#4d8ff0",
          color: "#fff", fontSize: `${(0.7 * fs).toFixed(1)}px`, display: "flex", alignItems: "center", justifyContent: "center",
        });
        send.textContent = "▶";
        d._send = send;
        panel.appendChild(send);
      }
      // (the character's own messages on the right, as on the phone; the rest on the left; labels centred)
      // (the reply to pick leaves room for its send button)
      const room = 0;
      // (its side as it was, until three readings in a row say otherwise: the bubble's colour read a little
      // different now and then, and the character's own message jumped from right to left and back)
      const sk = chatId + "|" + p.ar;
      const S = PHONE_END[sk] || (PHONE_END[sk] = { end: !!p.end, other: 0 });
      if (!!p.end !== S.end) { if (++S.other >= 3) { S.end = !!p.end; S.other = 0; } } else S.other = 0;
      let left = p.label ? x + (colW - d.offsetWidth) / 2 : S.end ? x + colW - pad - room - d.offsetWidth : x + pad;
      let top = Math.max(p.rect[1] * H - 0.4 * fs, below + 0.25 * fs);
      // (a label's place kept as first drawn while readings stay near it: see holdBox)
      if (Q && p.label) top = holdBox(chatId + "|label|" + p.ar, [top], colW)[0];
      if (Q && !p.label) {
        // (laid out on the screen's own shape: each bubble the game's bubble — its words' box and the phone's padding
        // round them, where it is on the screen, as wide and as tall — the Arabic fitted inside it. Kept while a new
        // reading is within 4 % of the screen's width of it: the box as read moves a pixel or two each frame)
        const [bpx, bpy] = [0.7 * fs, 0.4 * fs];
        // (the bubble as measured on the phone, where its colour gives way to the screen's; else its text's box and
        // the phone's usual padding)
        let bx = p.box ? p.box.slice() : [p.rect[0] * W - bpx, p.rect[1] * H - bpy, p.rect[2] * W + 2 * bpx, p.rect[3] * H + 2 * bpy];
        for (const ph of photoFlat) {
          if (ph.owner === p) {
            const [x0, x1] = [Math.min(bx[0], ph[0] - 0.4 * fs), Math.max(bx[0] + bx[2], ph[0] + ph[2] + 0.4 * fs)];
            bx = [x0, bx[1], x1 - x0, ph[1] + ph[3] + 0.4 * fs - bx[1]];
            // (its words at the top of it, over the photo's top, as on the phone: see below)
            d.dataset.photo = "1";
            ph.taken = true;
          }
        }
        for (const [c, owner] of cardOf) {
          if (owner !== p) continue;
          const cb = [c.rect[0] * W - 0.9 * fs, c.rect[1] * H - 0.55 * fs, c.rect[2] * W + 1.8 * fs, c.rect[3] * H + 1.1 * fs];
          const [x0, x1] = [Math.min(bx[0], cb[0] - 0.5 * fs), Math.max(bx[0] + bx[2], cb[0] + cb[2] + 0.5 * fs)];
          bx = [x0, bx[1], x1 - x0, cb[1] + cb[3] + 0.5 * fs - bx[1]];
          d.dataset.photo = "1";
          c.cardBox = cb;
        }
        bx[0] = Math.max(0.3 * pad, bx[0]);
        if (bx[0] + bx[2] > colW - 0.3 * pad) bx[2] = colW - 0.3 * pad - bx[0];
        // (the character's own, to the right: as close to the right edge as the other person's are to the left.
        // Told by its colour, blue — a wide bubble of the other person's is about as near one side as the other)
        const ownB = isOwn(p);
        if (ownB && margins.left != null) bx[0] = Math.max(0.3 * pad, colW - margins.left - bx[2]);
        else if (!ownB && margins.left != null) bx[0] = margins.left;
        bx = holdBox(chatId + "|" + p.ar, bx, colW);
        // (its words and emojis one flow inside it, the emojis among the words where they are: in the box's flex
        // layout each picture was a block of its own, pushed to the bubble's side)
        d.innerHTML = `<div style="min-width:0;max-width:100%">${d.innerHTML}</div>`;
        Object.assign(d.style, {
          width: `${bx[2]}px`, height: `${bx[3]}px`, maxWidth: "none", overflow: "hidden",
          display: "flex", alignItems: d.dataset.photo ? "flex-start" : "center", justifyContent: "flex-start",
          padding: `${(d.dataset.photo ? 1.2 : 0.3) * bpy}px ${0.8 * bpx}px`,
        });
        // (its text's size: one for all the chat's bubbles, worked out once they're all drawn — fitBubbles)
        if (p.pending) {
          d.dataset.pending = "1";
          d.style.direction = "ltr";
          d.style.opacity = "0.75";
        }
        fitList.push(d);
        [left, top] = [x + bx[0], panelTop + bx[1]];
        // (the bubble and its tail one shape, as the phone draws it: a rounded box whose bottom corner runs out
        // into a point — the character's own at the bottom right, the other person's at the bottom left — filled
        // and outlined in one, so no seam shows where the tail meets the box. The words drawn over it)
        if (!p.boxed) {
          const own = ownB;
          const [w, h] = [bx[2], bx[3]];
          const r = Math.min(0.45 * fs, h / 2, w / 2);
          const [tw, th2] = [0.6 * fs, Math.min(0.8 * fs, h - r)];
          const sw2 = p.edge ? Math.max(1.5, 0.1 * fs) : 0;
          const path = own
            ? `M${r},0 H${w - r} Q${w},0 ${w},${r} V${h - th2} L${w + tw},${h} H${r} Q0,${h} 0,${h - r} V${r} Q0,0 ${r},0 Z`
            : `M${r},0 H${w - r} Q${w},0 ${w},${r} V${h - r} Q${w},${h} ${w - r},${h} H${-tw} L0,${h - th2} V${r} Q0,0 ${r},0 Z`;
          const shape = document.createElement("div");
          // (its margin the same with an outline or without: the chosen bubble's outline, read now and then, moved it a pixel)
          const m = tw + Math.max(1.5, 0.1 * fs) + 1;
          Object.assign(shape.style, { position: "absolute", width: `${w + 2 * m}px`, height: `${h + 2 * m}px`, pointerEvents: "none" });
          shape.innerHTML =
            `<svg width="${w + 2 * m}" height="${h + 2 * m}" style="display:block;overflow:visible">` +
            `<path transform="translate(${m},${m})" d="${path}" style="fill:${p.bg || "#f8f9fb"};stroke:${p.edge || "none"};stroke-width:${sw2}px;stroke-linejoin:round"/>` +
            `</svg>`;
          panel.insertBefore(shape, d);
          put(shape, left - m, top - m);
          Object.assign(d.style, { background: "transparent", border: "none", boxShadow: "none", borderRadius: "0" });
        }
      }
      put(d, left, top);
      if (d._send) put(d._send, left + d.offsetWidth + 0.3 * fs, top + d.offsetHeight / 2 - 0.65 * fs);
      below = top + d.offsetHeight;
    }
    // (each card inside its message's bubble: a light box with its name, as on the phone)
    for (const [c] of cardOf) {
      if (!c.cardBox) continue;
      const cb = holdBox(chatId + "|card|" + c.ar, c.cardBox, colW);
      const card = document.createElement("div");
      Object.assign(card.style, {
        position: "absolute", boxSizing: "border-box", width: `${cb[2]}px`, height: `${cb[3]}px`,
        display: "flex", alignItems: "center", padding: `0 ${0.7 * fs}px`, direction: "ltr",
        borderRadius: `${0.3 * fs}px`, background: "rgba(0,0,0,0.045)", border: "1px solid rgba(0,0,0,0.07)",
        fontFamily: `"Noto Sans Arabic Medium", ${fonts()}`, fontWeight: "700", fontSize: `${0.95 * fs}px`, color: "#2b2d34",
        whiteSpace: "nowrap", overflow: "hidden",
      });
      // (its icon before its name, as on the phone: the picture found beside the name on the card's line)
      const ic = (P.stickers || []).find((st) => {
        const [iy, ih] = [st.uv[1] * qh, st.uv[3] * qh];
        return iy < cb[1] + cb[3] && cb[1] < iy + ih && st.uv[0] * qw < c.rect[0] * W;
      });
      const iz = cb[3] * 0.72;
      // (its icon remembered once found: a reading that missed it no longer left the card without one)
      const ik = chatId + "|" + c.ar;
      if (ic) PHONE_CARD_ICON[ik] = ic.png;
      const icPng = PHONE_CARD_ICON[ik];
      card.innerHTML = (icPng ? `<img src="${icPng}" style="width:${iz}px;height:${iz}px;object-fit:contain;flex:none;margin-right:${0.5 * fs}px">` : "") + `<span>${esc(c.ar)}</span>`;
      panel.appendChild(card);
      put(card, x + cb[0], panelTop + cb[1]);
    }
    // (the chat's photos: left as empty space, in their place and size — the bubbles round them keep it. One with no
    // words above it on the screen — its message's words scrolled off — in a bubble of its own, as on the phone)
    if (Q) {
      const otherBgP = (P.pieces.find((p) => !p.title && !p.label && !isOwn(p)) || {}).bg || P.screen_bg || "#f4f4f6";
      for (const ph of photoFlat) {
        if (ph.taken) continue;
        const m = 0.45 * fs;
        const kept = holdBox(nearKey(chatId + "|photo", ph, colW), ph.slice(0, 4), colW);
        [ph[0], ph[1], ph[2], ph[3]] = kept;
        const bub = document.createElement("div");
        Object.assign(bub.style, { position: "absolute", boxSizing: "border-box", width: `${ph[2] + 2 * m}px`, height: `${ph[3] + 2 * m}px`, borderRadius: `${0.45 * fs}px`, background: otherBgP });
        panel.insertBefore(bub, panel.firstChild.nextSibling);
        put(bub, x + ph[0] - m, panelTop + ph[1] - m);
      }
    }
    // (an emoji sent on its own: as it is, in a small bubble of the other person's colour, where it is on the screen)
    // (a chat's lone emojis kept a moment when a reading misses them: found in some readings only, they blinked)
    if (Q && !P.list) {
      const ks = PHONE_STK[chatId];
      if (P.stickers && P.stickers.length) PHONE_STK[chatId] = { list: P.stickers, at: Date.now() };
      else if (ks && Date.now() - ks.at < 1500) P = Object.assign({}, P, { stickers: ks.list });
    }
    if (Q && !P.list && P.stickers && P.stickers.length) {
      const otherBg = (P.pieces.find((p) => !p.title && !p.label && !isOwn(p)) || {}).bg || P.screen_bg || "#f4f4f6";
      for (const st of P.stickers) {
        const u = st.uv;
        const [sy1, sy2] = [u[1] * qh, (u[1] + u[3]) * qh];
        if (pieces.some((p) => !p.title && p.rect[1] * H < sy2 && sy1 < (p.rect[1] + p.rect[3]) * H)) continue;
        const ub = [u[0] * qw, u[1] * qh, u[2] * qw, u[3] * qh];
        const [sx0, sy0, sw0, sh0] = holdBox(nearKey(chatId + "|sticker", ub, colW), ub, colW);
        const padS = 0.45 * fs;
        const bub = document.createElement("div");
        Object.assign(bub.style, {
          position: "absolute", boxSizing: "border-box", width: `${sw0 + 2 * padS}px`, height: `${sh0 + 2 * padS}px`,
          borderRadius: `${0.45 * fs}px`, background: otherBg, display: "flex", alignItems: "center", justifyContent: "center",
        });
        bub.innerHTML = `<img src="${st.png}" style="width:${sw0}px;height:${sh0}px;object-fit:contain">`;
        panel.appendChild(bub);
        put(bub, x + sx0 - padS, panelTop + sy0 - padS);
      }
    }
    // (the chat's bubbles: the largest text size every one of them fits its box at — as big as the phone's, up to a
    // little bigger, one size for all as on the phone)
    if (fitList.length) {
      const fits = (d, z) => {
        d.style.fontSize = `${z}px`;
        return d.scrollHeight <= d.clientHeight + 1 && d.scrollWidth <= d.clientWidth + 1;
      };
      // (the chat's size kept from one reading to the next — worked out anew each time, every translation that came
      // changed it and every bubble's lines moved; smaller only when a translated bubble doesn't fit at it; a
      // message still in English is no measure of it)
      // (one bubble read too small for its words — a box cut in a scroll — pulled the whole chat's size down for
      // good: the chat's size goes no lower than three quarters of the phone's, a bubble that needs less gets its
      // own; and it comes back up once every bubble fits a size larger again)
      const full = fs * 1.15, floor = 0.75 * full;
      let size = PHONE_SIZE[chatId] || full;
      const measured = fitList.filter((d) => !d.dataset.photo && !d.dataset.pending);
      if (size < full) {
        const up = Math.min(full, size * 1.12);
        if (measured.every((d) => fits(d, up))) size = up;
      }
      const own = new Map();
      for (const d of measured) {
        if (fits(d, size)) continue;
        let [lo, hi] = [fs * 0.4, size];
        for (let i = 0; i < 9; i++) {
          const mid = (lo + hi) / 2;
          if (fits(d, mid)) lo = mid; else hi = mid;
        }
        if (lo >= floor) size = lo;
        else own.set(d, lo);
      }
      PHONE_SIZE[chatId] = size;
      for (const d of fitList) d.style.fontSize = `${own.has(d) && own.get(d) < size ? own.get(d) : size}px`;
    }
    // (what it shows runs past its screen's foot: drawn again with smaller text, a few times at most)
    const over = (below - panelTop + pad) / th;
    if (over > 1.02 && (fit || 1) > 0.55) {
      const next = (fit || 1) * Math.max(0.55, Math.min(0.95, 1 / Math.sqrt(over)));
      PHONE_FIT[chatKey] = next;
      return renderPhone(root, P, next, true);
    }
  }

  // ---- Game pages (a menu, a "mission failed" page …) ------------------------------------------
  // L: layout from the backend (all rects are fractions of the game frame).
  // reasonSegs: Arabic segments for the reason line. opts: { title_font, text_font }.
  // root: element covering exactly the game picture (overlay window or preview layer).
  function renderScreen(root, L, reasonSegs, opts) {
    root.innerHTML = "";
    if (!L) return;
    // (each item where it was when read again a few pixels off: see `steady`)
    // (by the item's English, and which one of that English it is: two "Off"s in a list are two items)
    const nth = {};
    const sk = (kind, it, i) => {
      const id = `${L.id}|${kind}|${it.en || "#" + i}`;
      nth[id] = (nth[id] || 0) + 1;
      return `${id}|${nth[id]}`;
    };
    for (const kind of ["labels", "menu", "cards"]) (L[kind] || []).forEach((it, i) => { if (it.rect) it.rect = steady(sk(kind, it, i), it.rect); });
    (L.buttons || []).forEach((b, i) => {
      if (b.text_rect) b.text_rect = steady(sk("button-text", b, i), b.text_rect);
      if (b.icon_rect) b.icon_rect = steady(sk("button-icon", b, i), b.icon_rect);
    });
    if (L.title) L.title = steady(`${L.id}|title`, L.title);
    const W = root.clientWidth;
    const H = root.clientHeight;
    const shadow = (fs) => `0 ${(fs * 0.05).toFixed(1)}px ${(fs * 0.08).toFixed(1)}px rgba(0,0,0,0.8)`;
    const mk = (css, html) => {
      const d = document.createElement("div");
      // (no object spread in this file: 2018 Samsung TVs draw it too, Chromium 56)
      Object.assign(d.style, { position: "absolute", whiteSpace: "nowrap", direction: dir(), lineHeight: "1.15" }, css);
      d.innerHTML = html;
      root.appendChild(d);
      return d;
    };

    // UI pieces replaced by Arabic replicas (pause-menu tabs): same box and colors as the game's.
    for (const lb of L.labels || []) {
      const h = lb.rect.h * H;
      const tab = mk(
        {
          left: `${lb.rect.x * W}px`,
          top: `${lb.rect.y * H}px`,
          width: `${lb.rect.w * W}px`,
          height: `${h}px`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: lb.bg,
          color: lb.fg,
          fontFamily: `"${opts.text_font}", ${fonts()}`,
          fontWeight: "500",
          fontSize: `${h * 0.55}px`,
          lineHeight: "1",
          whiteSpace: "nowrap",
          overflow: "hidden",
        },
        `<span>${esc(lb.ar)}</span>`,
      );
      // (a tab's name longer in this language than the game's English, "DIKEHENDAKI & BAHANG": smaller, inside its
      // own tab, not over the next one)
      // (measured again once the language's font has come: a script's fallback font loads on first use, and the name
      // was measured in another, narrower one)
      const name = tab && tab.firstChild;
      const room = lb.rect.w * W * 0.92;
      const fit = () => {
        if (!name || !name.isConnected) return;
        tab.style.fontSize = `${h * 0.55}px`;
        if (name.offsetWidth > room) tab.style.fontSize = `${h * 0.55 * (room / name.offsetWidth)}px`;
      };
      fit();
      if (document.fonts && document.fonts.ready) document.fonts.ready.then(fit);
      setTimeout(fit, 400);
    }

    // Pause-menu content: list items and paragraphs covered by Arabic replicas (same place and
    // colors as the game's; not yet translated ones stay English).
    for (const m of L.menu || []) {
      if (!m.ar) continue;
      const lh = m.line_h * H;
      // a script's free-form box: its own place, alignment and look
      if (m.align || m.font || m.size || m.weight || m.shadow || m.outline || m.wrap) {
        const fs = lh * (m.size || 1.2);
        const font = m.font === "title" ? opts.title_font : m.font || opts.text_font;
        const wrap = m.wrap ? m.wrap === "yes" : !m.item;
        const align = m.align || (m.right ? "right" : "left");
        mk(
          {
            left: `${m.rect.x * W}px`,
            top: `${m.rect.y * H}px`,
            width: `${m.rect.w * W}px`,
            minHeight: `${m.rect.h * H}px`,
            boxSizing: "border-box",
            padding: m.bg && m.bg !== "transparent" ? `0 ${lh * 0.4}px` : "0",
            display: "flex",
            alignItems: "center",
            justifyContent: align === "center" ? "center" : (align === "right") === rtl() ? "flex-start" : "flex-end",
            textAlign: align,
            whiteSpace: wrap ? "normal" : "nowrap",
            background: m.bg || "transparent",
            color: m.fg || "#ffffff",
            fontFamily: `"${font}", ${fonts()}`,
            fontWeight: String(m.weight || 500),
            fontSize: `${fs}px`,
            lineHeight: "1.35",
            textShadow: m.shadow ? shadow(fs) : "",
            WebkitTextStroke: m.outline ? `${(fs * 0.06).toFixed(1)}px ${m.outline}` : "",
            paintOrder: m.outline ? "stroke fill" : "",
          },
          // (a line of dialogue: who says it first, in the game's colour for the name, as a subtitle's is)
          `<span>${m.name ? `<span style="color:${esc(m.name_fg || "inherit")}">${esc(m.name)}:</span> ` : ""}${withIcons(m.ar, m.icons)}</span>`,
        );
        continue;
      }
      mk(
        {
          // keep the English's edge: right-aligned values stay right-aligned, lists stay left
          right: m.right ? `${W - (m.rect.x + m.rect.w) * W}px` : "",
          left: m.right ? "" : `${m.rect.x * W}px`,
          top: `${m.rect.y * H}px`,
          minWidth: `${m.rect.w * W}px`,
          minHeight: `${m.rect.h * H}px`,
          // list items stay on one line; panel text may wrap, up to the panel's edge
          // (a paragraph keeps its own width: on the Store a picture sits right next to it)
          maxWidth: m.item ? "none" : (m.lines || 1) > 1 ? `${m.rect.w * W}px` : `${Math.max(m.rect.w * W, (m.right ? m.rect.x + m.rect.w - 0.39 : 0.845 - m.rect.x) * W)}px`,
          boxSizing: "border-box",
          padding: `0 ${lh * 0.5}px`,
          display: "flex",
          alignItems: "center",
          // (in a right-to-left box flex-start is its right side)
          justifyContent: (m.right === rtl()) ? "flex-start" : "flex-end",
          textAlign: m.right ? "right" : "left",
          whiteSpace: m.item ? "nowrap" : "normal",
          background: m.bg,
          color: m.fg,
          fontFamily: `"${opts.text_font}", ${fonts()}`,
          fontWeight: "500",
          fontSize: `${lh * 1.2}px`,
          lineHeight: "1.35",
        },
        // one child: this box is a flex container, and each left-to-right run would become its own column
        `<span>${withIcons(m.ar, m.icons)}</span>`,
      );
    }

    // A box of the game's drawn again under it, translated (the map's objective): its see-through background
    // and coloured top line, the picture beside the name, the sentence under them.
    for (const c of L.cards || []) {
      if (!c.title_ar && !c.text_ar) continue;
      const th = c.title_h * H, xh = (c.text_h || c.title_h) * H;
      const pad = xh * 0.7;
      const card = mk({
        left: `${c.rect.x * W}px`,
        top: `${(c.rect.y + c.rect.h) * H + xh * 0.5}px`,
        width: `${c.rect.w * W}px`,
        boxSizing: "border-box",
        padding: `${pad * 0.8}px ${pad}px ${pad}px`,
        background: c.bg || "rgba(14, 16, 30, 0.8)",
        borderTop: `${Math.max(2, (c.border_h || 0.004) * H).toFixed(1)}px solid ${c.border || "#fd87b0"}`,
        whiteSpace: "normal",
        color: "#ffffff",
        textAlign: rtl() ? "right" : "left",
        fontFamily: `"${opts.text_font}", ${fonts()}`,
      }, "");
      const head = document.createElement("div");
      Object.assign(head.style, { display: "flex", alignItems: "center", gap: `${xh * 0.5}px`, marginBottom: `${xh * 0.25}px` });
      if (c.pic) {
        const img = document.createElement("img");
        img.src = c.pic;
        Object.assign(img.style, { width: `${th * 1.9}px`, height: `${th * 1.9}px`, objectFit: "cover", flex: "none", display: "block" });
        head.appendChild(img);
      }
      const name = document.createElement("span");
      Object.assign(name.style, { fontWeight: "700", fontSize: `${th * 1.25}px`, lineHeight: "1.2", unicodeBidi: "plaintext" });
      name.textContent = c.title_ar || c.title_en;
      head.appendChild(name);
      card.appendChild(head);
      if (c.text_ar) {
        const text = document.createElement("div");
        Object.assign(text.style, { fontSize: `${xh * 1.2}px`, lineHeight: "1.45", unicodeBidi: "plaintext" });
        text.textContent = c.text_ar;
        card.appendChild(text);
      }
    }

    // Pause menu, top right: character name, day and time, in Arabic just left of the English.
    if (L.player && L.player.ar.length) {
      const p = L.player;
      const lh = p.line_h * H;
      mk(
        {
          right: `${W - p.rect.x * W + lh * 0.8}px`,
          top: `${p.rect.y * H - lh * 0.25}px`,
          textAlign: "right",
          // condensed thin sans, like the game's narrow capitals (Noto Sans Arabic, extra-condensed light)
          fontFamily: `"Noto Sans Arabic", ${fonts()}`,
          fontStretch: "62.5%",
          fontWeight: "300",
          fontSize: `${lh * 1.25}px`,
          lineHeight: `${p.pitch * H}px`,
          color: "#ffffff",
          // like the game's: a dark copy of the letters, down and to the right, slightly soft
          textShadow: `${(lh * 0.1).toFixed(1)}px ${(lh * 0.1).toFixed(1)}px ${(lh * 0.08).toFixed(1)}px rgba(0,0,0,0.95)`,
        },
        p.ar.map(esc).join("<br>"),
      );
    }

    // Title: same color and visual height as the English title, centred just above it.
    const th = L.title.h * H;
    const tfs = th * 1.3; // Arabic letters stand ~0.75em tall in display fonts
    if (L.title_ar) mk(
      {
        left: `${(L.title.x + L.title.w / 2) * W}px`,
        top: `${L.title.y * H - th * 0.1}px`,
        transform: "translate(-50%, -100%)",
        fontFamily: `"${opts.title_font}", ${fonts()}`,
        fontWeight: "700",
        fontSize: `${tfs}px`,
        color: L.title_color,
        textShadow: shadow(tfs),
      },
      esc(L.title_ar),
    );

    // Reason: white, same size as the English reason, centred just below it.
    const plainReason = (reasonSegs || []).map((s) => s[0]).join("").trim();
    if (L.reason && plainReason) {
      // Size from ONE line of the English (it may wrap onto several), and wrap the Arabic the same way.
      const rh = (L.reason_line_h || L.reason.h) * H;
      const rfs = rh * 1.35;
      mk(
        {
          left: `${(L.reason.x + L.reason.w / 2) * W}px`,
          top: `${(L.reason.y + L.reason.h) * H + rh * 0.35}px`,
          transform: "translateX(-50%)",
          width: "max-content",
          maxWidth: `${Math.min(Math.max(L.reason.w * 1.1, 0.25) * W, W * 0.9)}px`,
          whiteSpace: "normal",
          textAlign: "center",
          lineHeight: "1.4",
          fontFamily: `"${opts.text_font}", ${fonts()}`,
          fontWeight: "500",
          fontSize: `${rfs}px`,
          color: "#ffffff",
          textShadow: shadow(rfs),
        },
        segHtml(reasonSegs),
      );
    }

    // Buttons that know their icon's place: each one's Arabic label and the game's own icon just above its
    // English one, icon over icon (so they line up with the game's), in the word's color (a greyed-out
    // button stays grey).
    // (an icon too faint to cut out still has its place: the label goes there, without the picture)
    // Buttons one under the other (a failed mission's): each one's Arabic and its icon just left of the
    // English, on its line, in the word's colour.
    const beside = L.buttons && L.buttons.length && L.buttons.every((b) => b.beside && b.text_rect);
    if (beside) {
      for (const b of L.buttons) {
        if (!b.ar) continue;
        const capH = b.text_rect.h * H;
        const bfs = capH * 1.35;
        const item = mk({
          right: `${W - b.text_rect.x * W + capH * 1.6}px`,
          top: `${(b.text_rect.y + b.text_rect.h / 2) * H}px`,
          transform: "translateY(-50%)",
          display: "flex",
          alignItems: "center",
          gap: `${capH * 0.45}px`,
          fontFamily: `"${opts.text_font}", ${fonts()}`,
          fontWeight: "700",
          fontSize: `${bfs}px`,
          color: b.fg || "#ffffff",
          textShadow: shadow(bfs),
        }, "");
        // (right to left: the icon on the right, as the game's after its word)
        if (b.icon && b.icon_rect) {
          const img = document.createElement("img");
          img.src = b.icon;
          Object.assign(img.style, { height: `${b.icon_rect.h * H}px`, width: `${b.icon_rect.w * W}px`, objectFit: "contain", flex: "none", display: "block" });
          item.appendChild(img);
        }
        const label = document.createElement("span");
        label.textContent = b.ar;
        item.appendChild(label);
      }
    }
    const placed = !beside && L.buttons && L.buttons.length && L.buttons.every((b) => b.icon_rect && b.text_rect);
    if (placed) {
      // right to left; a label longer than its English pushes the ones to its left further left
      let room = Infinity; // where the item to the right begins (px from the left)
      const order = L.buttons.slice().sort((a, b) => b.icon_rect.x - a.icon_rect.x);
      // (all on one middle line: a bigger icon, a ringed "RESTORE ALL", grows up and down alike, its word level
      // with the others', not pushed up by the extra height)
      const stdH = Math.min(...order.map((b) => b.icon_rect.h));
      for (const b of order) {
        const capH = b.text_rect.h * H;
        const bfs = capH * 1.45;
        const iconRight = Math.min((b.icon_rect.x + b.icon_rect.w) * W, room - capH * 0.8);
        const extra = (b.icon_rect.h - stdH) * H;
        const item = mk({
          right: `${W - iconRight}px`,
          top: `${b.icon_rect.y * H - capH * 0.35 + extra}px`,
          transform: "translateY(-100%)",
          display: "flex",
          alignItems: "center",
          gap: `${Math.max(2, (b.icon_rect.x - (b.text_rect.x + b.text_rect.w)) * W)}px`,
          // (no picture: keep the icon's room, so the label sits where it would beside it)
          paddingRight: b.icon ? "0" : `${b.icon_rect.w * W + Math.max(2, (b.icon_rect.x - (b.text_rect.x + b.text_rect.w)) * W)}px`,
          fontFamily: `"${opts.text_font}", ${fonts()}`,
          fontWeight: "700",
          fontSize: `${bfs}px`,
          color: b.fg || "#ffffff",
          textShadow: shadow(bfs),
        }, "");
        // (right to left: the icon first, on the right, like the game's after its word)
        for (const src of (b.icon ? [b.icon, ...(b.more_icons || [])] : []).reverse()) {
          const img = document.createElement("img");
          img.src = src;
          // the icon's place, not its picture's own width: a grey icon cuts out narrower than a white one,
          // and a picture not loaded yet has none (the label beside it moved left and right)
          Object.assign(img.style, { height: `${b.icon_rect.h * H}px`, width: `${b.icon_rect.w * W}px`, objectFit: "contain", flex: "none", display: "block" });
          item.appendChild(img);
        }
        const label = document.createElement("span");
        label.textContent = b.ar;
        item.appendChild(label);
        room = iconRight - item.offsetWidth;
      }
    }
    // Buttons: the game's own icons + Arabic labels, a row just above the English one, right-aligned to it.
    if (!placed && !beside && L.buttons && L.buttons.length && L.buttons_row) {
      const row = L.buttons_row;
      const capH = Math.min(...L.buttons.map((b) => b.text_rect.h)) * H; // word without descenders
      const bfs = capH * 1.45;
      const iconGap = Math.max(
        2,
        ...L.buttons.filter((b) => b.icon_rect).map((b) => (b.icon_rect.x - (b.text_rect.x + b.text_rect.w)) * W),
      );
      const rowEl = mk({
        right: `${W - (row.x + row.w) * W}px`,
        top: `${row.y * H - row.h * H * 0.35}px`,
        transform: "translateY(-100%)",
        display: "flex",
        alignItems: "center",
        gap: `${row.h * H * 0.9}px`,
        fontFamily: `"${opts.text_font}", ${fonts()}`,
        fontWeight: "500",
        fontSize: `${bfs}px`,
        color: "#ffffff",
      }, "");
      // right to left: first button is the rightmost, its icon on the right like the game
      for (const b of L.buttons) {
        const item = document.createElement("div");
        Object.assign(item.style, { display: "flex", alignItems: "center", gap: `${iconGap}px`, textShadow: shadow(bfs) });
        // icons in the game's left-to-right order ("L1 R1"): this row is right-to-left, so the
        // last icon goes in first (rightmost)
        const icons = b.icon && b.icon_rect ? [b.icon, ...(b.more_icons || [])] : [];
        for (const src of icons.reverse()) {
          const img = document.createElement("img");
          img.src = src;
          Object.assign(img.style, { height: `${b.icon_rect.h * H}px`, width: "auto", display: "block" });
          item.appendChild(img);
        }
        const label = document.createElement("span");
        label.textContent = b.ar;
        item.appendChild(label);
        rowEl.appendChild(item);
      }
    }
  }

  // Font size (CSS px) that matches the game's English text: lineH is a fraction of the frame
  // height, frameH the frame's height in CSS px. Returns 0 when not matching (use the style size).
  function matchedPx(area, lineH, frameH, factor) {
    if (!area || area.match_size === false || !lineH) return 0;
    return lineH * frameH * (factor || MATCH_FACTOR) * (area.font_scale || 1);
  }
  // Over the game's text (the cover look): the Arabic as big as the English it hides, line for line. GTA VI's
  // subtitles at 720p: a line read 25 px tall, 27 px from one line to the next; 0.92 × 1.18 ≈ 1.09 of the read
  // height. (Matched as elsewhere, 1.2 and the style's 1.5 spacing, a line took 47 px: the box twice the English.)
  const COVER_MATCH = 0.92, COVER_LINE = 1.18;

  // GTA's instructions box hugs its text with these margins (in English line heights, measured on
  // captured frames): ~0.65 left/top/bottom, ~1.2 right.
  const BOX_PAD = { left: 0.65, right: 1.2, bottom: 0.65, gap: 0.3 };

  // "Below the game's text box": put `box` right under the game's box — same left edge, at most the
  // same width (long Arabic wraps onto more lines, growing downward).
  // tb: English text block [x, y, w, h] as fractions of the frame; lineH: English line height
  // (fraction); W, H: frame size in CSS px; ox, oy: where the box's container starts (CSS px).
  // Placements that follow the game's own text on screen (from the English text block).
  const atText = (placement) => placement === "under_text" || placement === "above_text" || placement === "on_text";

  // The game profile's look (Game profile → How translations look), kept with the profile's page
  // switches: "cover" puts each translation on an opaque black box right over the game's own lines,
  // hiding the English, where those lines' box is known (`tb`); else the area's own placement.
  const COVER = "translation_cover";
  // (An area placed under the game's own box, GTA VI's help top left, keeps that place: within the
  // box's left and right edges, right under it, growing downward; the box's text stays readable.)
  function look(area, style, opts, tb) {
    if (!(opts && opts.more && opts.more[COVER] === true) || !tb || area.placement === "under_text") return { placement: area.placement, style, area };
    return {
      placement: "on_text",
      style: Object.assign({}, style, { bg_color: "#000000", bg_opacity: 1, line_height: COVER_LINE }),
      area: Object.assign({}, area, { background: "" }),
    };
  }

  // "Just above the game's text": the Arabic's bottom right above the English, centred on it, as
  // wide as the area at most.
  function placeAboveText(box, tb, lineH, W, H, ox, oy, maxW) {
    const lh = (lineH || tb[3]) * H;
    Object.assign(box.style, {
      position: "absolute",
      left: `${(tb[0] + tb[2] / 2) * W - ox}px`,
      top: `${tb[1] * H - BOX_PAD.gap * lh - oy}px`,
      transform: "translate(-50%, -100%)",
      margin: "0",
      width: "max-content",
      maxWidth: `${Math.max(maxW, 60)}px`,
      boxSizing: "border-box",
    });
  }

  // Place `box` at the game's text for these placements. tb: the English text block (fractions of
  // the frame; null when not known: then at the read area's edge). areaW, areaH: the read area's
  // size in CSS px; the box's container starts at the area's top (under_text) or ends at its
  // bottom (above_text).
  // gb: the game's own box around the text when the app found its edges ([x, y, w, h] fractions)
  // cov (on_text): { min: the box at least as big as the game's text (its own box, gb, when found),
  // also: text seen after this line ({text_box, game_box, line_h}) that the box covers too — lines
  // stacking up only: a single line replaced by the next grew the box for a moment, then shrank it }
  function placeAtText(box, placement, tb, lineH, W, H, ox, oy, areaW, areaH, gb, cov) {
    if (placement === "on_text" && tb) {
      placeOnText(box, tb, lineH, W, H, ox, oy, gb, cov);
    } else if (placement === "under_text") {
      if (gb) placeUnderGameBox(box, gb, lineH || (tb && tb[3]) || 0.03, W, H, ox, oy);
      else if (tb) placeUnderText(box, tb, lineH, W, H, ox, oy);
      else Object.assign(box.style, { position: "absolute", left: "0", top: `${areaH}px`, margin: "0" });
    } else if (tb) {
      placeAboveText(box, tb, lineH, W, H, ox, oy, areaW);
    } else {
      Object.assign(box.style, { position: "absolute", left: "50%", bottom: `${areaH}px`, transform: "translateX(-50%)", margin: "0", width: "max-content", maxWidth: `${Math.max(areaW, 60)}px` });
    }
  }

  // Right over the game's text, centred on it: at least the English lines' width and a little more,
  // wider when the translation needs it (up to 92 % of the picture) before it wraps. With cov.min at
  // least as big as the game's text all round: the game's own box when it was found (subtitles with a
  // background, from the game's settings), else the lines with a margin; and over text seen since
  // (cov.also) too. Without it as tall as the translation itself.
  function placeOnText(box, tb, lineH, W, H, ox, oy, gb, cov) {
    const lh = (lineH || tb[3]) * H;
    const around = (t, g, l) => {
      if (g) return [g[0] * W, g[1] * H, g[2] * W, g[3] * H];
      const m = (l || t[3]) * H;
      return [t[0] * W - BOX_PAD.left * m, t[1] * H - 0.35 * m, t[2] * W + 2 * BOX_PAD.left * m, t[3] * H + 0.7 * m];
    };
    let r = [tb[0] * W - BOX_PAD.left * lh, tb[1] * H, tb[2] * W + 2 * BOX_PAD.left * lh, tb[3] * H];
    let minH = 0;
    if (cov && cov.min) {
      r = around(tb, gb, lineH);
      const a = cov.also && cov.also.text_box ? around(cov.also.text_box, cov.also.game_box, cov.also.line_h) : null;
      if (a) {
        const x0 = Math.min(r[0], a[0]), y0 = Math.min(r[1], a[1]);
        r = [x0, y0, Math.max(r[0] + r[2], a[0] + a[2]) - x0, Math.max(r[1] + r[3], a[1] + a[3]) - y0];
      }
      minH = r[3];
    }
    Object.assign(box.style, {
      position: "absolute",
      left: `${r[0] + r[2] / 2 - ox}px`,
      top: `${r[1] + r[3] / 2 - oy}px`,
      transform: "translate(-50%, -50%)",
      margin: "0",
      width: "max-content",
      minWidth: `${r[2]}px`,
      minHeight: minH ? `${minH}px` : "",
      // (up to 60 % of the picture before it wraps, centred: the middle, where the game keeps its own
      // subtitles. At 92 % a long one reached over GTA VI's map, bottom left)
      maxWidth: `${Math.max(W * 0.6, r[2])}px`,
      boxSizing: "border-box",
    });
    // (the English in two lines or more, the Arabic shorter: as many lines as it, filling the box that hides
    // it — one line over two English ones was a tall black box with a line in its middle. Never more lines
    // than the English: half as wide each left a word on a third)
    const rows = Math.round(tb[3] / (lineH || tb[3]));
    const t = box.firstElementChild;
    // (not when it has lines of its own: two speakers' lines, one under the other)
    if (rows >= 2 && t && !t.querySelector("br")) {
      Object.assign(t.style, { width: "max-content", maxWidth: "none" });
      const one = t.getBoundingClientRect().width;
      const line = parseFloat(getComputedStyle(t).lineHeight) || lh;
      const limit = Math.max(W * 0.6, r[2]);
      Object.assign(t.style, { width: "", textWrap: "balance" });
      for (let w = Math.ceil(one / rows); ; w += lh / 2) {
        t.style.maxWidth = `${w}px`;
        if (t.getBoundingClientRect().height <= rows * line + 1 || w >= limit) break;
      }
    }
  }

  // A read area's effects (Read Areas; the defaults when it has none of its own: config.rs AreaFx). `all`: the
  // effects as they apply (config.rs fx_view: the player's own, else the game's default) — the fade-in is off
  // unless one of them sets it
  function fxOf(all, id) {
    const own = (all && all[id]) || {};
    const f = { cover_min: true, cover_early: true, fade_ms: 0 };
    for (const k in own) if (own[k] !== null && own[k] !== undefined) f[k] = own[k];
    f.fade_ms = Math.min(Math.max(Number(f.fade_ms) || 0, 0), 2000);
    if (typeof f.stack_up !== "boolean") f.stack_up = id === "subtitles";
    if (typeof f.ride !== "boolean") f.ride = id === "subtitles" || id === "instructions";
    if (typeof f.accent !== "boolean") f.accent = id === "instructions";
    if (typeof f.reviewed_only !== "boolean") f.reviewed_only = id === "instructions";
    return f;
  }

  // "Above the game's text" with lines that stack up (GTA VI's subtitles, up to six lines and more):
  // the translations that ride (the subtitles' and the bottom instructions') go together, as one
  // block, right above the top of the stacked English, and move up as more lines come (quickly). The
  // stacking area's translation is the block's top; the others' are under it. With no English there,
  // the block stays above line 1 (the stacking area's bottom line).
  // areas: all of them; shown / seen: by area id, the translation now shown ({segments, text_box,
  // line_h}) and the text seen and not translated yet; measure(area, ev): its translation's height in px.
  // Returns null when `a` doesn't ride (or the look is over the game's text), else where its box goes:
  // { bottom, cx, maxW } in px of the picture.
  const RIDE_GAP = 0.35; // (of a line's height, between the block and the English, and inside the block)
  function ridePlace(a, areas, fxAll, opts, shown, seen, W, H, measure) {
    if (coverLook(opts) || !fxOf(fxAll, a.id).ride) return null;
    const lead = areas.find((x) => x.enabled && fxOf(fxAll, x.id).ride && fxOf(fxAll, x.id).stack_up) || a;
    const has = (e) => !!(e && e.segments && e.segments.length);
    const s = seen[lead.id], e = shown[lead.id];
    const src = s && s.text_box ? s : has(e) && e.text_box ? e : null;
    const lh = ((src && src.line_h) || 0.025) * H;
    // line 1's top when no English is up: a line and its margin above the stacking area's bottom
    const top = src ? src.text_box[1] * H : (lead.y + lead.h) * H - 2.2 * lh;
    let bottom = top - RIDE_GAP * lh;
    if (a.id === lead.id) {
      for (const f of areas) {
        if (f.id === lead.id || !f.enabled || !fxOf(fxAll, f.id).ride || !has(shown[f.id])) continue;
        bottom -= measure(f, shown[f.id]) + RIDE_GAP * lh;
      }
    }
    return { bottom, cx: (lead.x + lead.w / 2) * W, maxW: Math.max(lead.w * W, 60) };
  }

  // Place a box at a ridePlace() spot (P.ox, P.oy: where its container starts), sliding there quickly.
  function placeRide(box, R, P, fadeMs) {
    Object.assign(box.style, {
      position: "absolute",
      left: `${R.cx - P.ox}px`,
      top: `${R.bottom - P.oy}px`,
      transform: "translate(-50%, -100%)",
      margin: "0",
      width: "max-content",
      maxWidth: `${R.maxW}px`,
      textAlign: "center",
      boxSizing: "border-box",
      transition: `${fadeMs > 0 ? `opacity ${fadeMs}ms ease-out, ` : ""}top 90ms ease-out`,
    });
  }

  // A box's height as drawn for this area's translation, measured in a hidden copy (for ridePlace).
  function measureBox(probe, a, baseStyle, opts, ev, k, sizeH, maxW) {
    const L = look(a, styleOf(a, baseStyle), opts, null);
    renderBox(probe, ev.segments, L.style, k, a.font_scale, L.placement, L.area, matchedPx(a, ev.line_h, sizeH), { probe: true });
    Object.assign(probe.style, { position: "absolute", visibility: "hidden", left: "0", top: "0", width: "max-content", maxWidth: `${maxW}px`, margin: "0" });
    const h = probe.offsetHeight;
    probe.style.display = "none";
    return h;
  }
  const coverLook = (opts) => !!(opts && opts.more && opts.more[COVER] === true);

  // Draw one read area's line in `box`: its translation (ev), or, with the "over the game's text"
  // look, the black box alone over text seen there and not translated yet (cover). k, sizeH: as for
  // renderBox and matchedPx. P: { W, H, ox, oy, areaW, areaH } for placeAtText, and noBoxes: where
  // the game's text is isn't known (then the area's own placement). False when nothing is drawn.
  // Where each area's translation was placed, and for which text: the same translation stays where it
  // is (a new reading of the same English, a box a few pixels off, moved it); only another text moves it.
  const placed = {};
  const PLACE_KEYS = ["position", "left", "top", "transform", "width", "minWidth", "minHeight", "maxWidth"];
  function drawLine(box, a, baseStyle, opts, fxAll, ev, cover, k, sizeH, P) {
    const fx = fxOf(fxAll, a.id);
    const has = !!(ev && ev.segments && ev.segments.length);
    const early = !has && !!cover && coverLook(opts) && fx.cover_early && !P.noBoxes;
    const src = has ? ev : early ? cover : null;
    if (!src) {
      renderBox(box, [], baseStyle, k, 1, "", a);
      return false;
    }
    // (the game's text and box where they were when read again a few pixels off: see `steady`)
    const tb = P.noBoxes ? null : steady(a.id + "|text", src.text_box);
    const gbox = P.noBoxes ? null : steady(a.id + "|game", src.game_box);
    const L = look(a, styleOf(a, baseStyle), opts, tb);
    renderBox(box, has ? ev.segments : [], L.style, k, a.font_scale, L.placement, L.area, matchedPx(a, src.line_h, sizeH, L.placement === "on_text" ? COVER_MATCH : 0),
      { fadeMs: fx.fade_ms, empty: early, accent: fx.accent });
    if (atText(L.placement)) {
      placeAtText(box, L.placement, tb, src.line_h, P.W, P.H, P.ox, P.oy, P.areaW, P.areaH, gbox,
        { min: early || fx.cover_min, also: has && early === false && cover && coverLook(opts) && fx.cover_early && fx.stack_up ? cover : null });
    } else if (P.ride) {
      placeRide(box, P.ride, P, fx.fade_ms);
    } else if (P.whole) {
      // (a window over the whole picture, the game's text not known: at the bottom of the area's own
      // region, centred)
      const r = rectFor(a.placement, { x: a.x * P.W, y: a.y * P.H, w: a.w * P.W, h: a.h * P.H }, P.W, P.H, a);
      placeRide(box, { bottom: r.y + r.h, cx: r.x + r.w / 2, maxW: r.w }, P, fx.fade_ms);
    }
    // (the same translation, drawn again: where it was; riding above stacked lines it moves with them)
    if (has && !P.ride) {
      // (and at the same size: the game's settings show the same line at each subtitle size picked — its black
      // box stayed as big as the first)
      const key = ev.segments.map((s) => s[0]).join("") + "|" + P.W + "x" + P.H + "|" + ev.line_h;
      const was = placed[a.id];
      if (was && was.key === key) {
        for (const p of PLACE_KEYS) box.style[p] = was.style[p];
      } else {
        const style = {};
        for (const p of PLACE_KEYS) style[p] = box.style[p]; // (no Object.fromEntries: 2018 TVs)
        placed[a.id] = { key: key, style: style };
      }
    } else if (!has) {
      delete placed[a.id];
    }
    return box.style.display !== "none";
  }

  // Right under the game's box, with exactly its left and right edges (the text wraps inside).
  function placeUnderGameBox(box, gb, lineH, W, H, ox, oy) {
    const lh = lineH * H;
    Object.assign(box.style, {
      position: "absolute",
      left: `${gb[0] * W - ox}px`,
      top: `${(gb[1] + gb[3]) * H + BOX_PAD.gap * lh - oy}px`,
      margin: "0",
      width: `${Math.max(gb[2] * W, 60)}px`,
      maxWidth: "none",
      boxSizing: "border-box",
    });
  }

  function placeUnderText(box, tb, lineH, W, H, ox, oy) {
    const lh = (lineH || tb[3]) * H;
    const left = tb[0] * W - BOX_PAD.left * lh;
    const right = (tb[0] + tb[2]) * W + BOX_PAD.right * lh;
    const bottom = (tb[1] + tb[3]) * H + BOX_PAD.bottom * lh;
    Object.assign(box.style, {
      position: "absolute",
      left: `${left - ox}px`,
      top: `${bottom + BOX_PAD.gap * lh - oy}px`,
      margin: "0",
      width: "max-content",
      maxWidth: `${Math.max(right - left, 60)}px`,
      boxSizing: "border-box",
    });
  }

  // TEMP (testing the translation memory): color of Arabic that came from the memory

  // An area's text style: its own, else the profile's (areas from before had none of their own).
  const styleOf = (area, base) => (area && area.style) || base;

  return { _phone: { PHONE_BOX: PHONE_BOX, PHONE_FIT: PHONE_FIT, PHONE_QUAD: PHONE_QUAD }, STEADY, steady, setLang, styleOf, esc, segHtml, rectFor, alignOf, anchor, renderBox, renderScreen, matchedPx, COVER_MATCH, placeUnderText, atText, placeAtText, look, COVER, fxOf, coverLook, drawLine, ridePlace, measureBox, renderPhone };
})();
