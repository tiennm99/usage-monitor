// The bubble page: lays out the payload Rust sends and forwards pointer
// input. Rust formats every displayed string and picks every fill colour;
// this file only places pixels.

import {
  BASE_DPI,
  brighten,
  cssPx,
  headLabelText,
  headMetrics,
  outlinePath,
  pulseActive,
  pulseT,
  remainingFraction,
  ringDash,
  tailLayout,
  timeFillWidth,
  usageFillWidth,
  WHEEL_IDLE,
  wheelSteps,
} from './layout.js';
import {
  forwardErrors,
  invoke,
  listen,
  logFailure,
  startDragging,
  windowLabel,
} from './tauri-bridge.js';

/**
 * The `bubble:update` payload and `get_bubble_state` result.
 * @typedef {object} BubblePayload
 * @property {string} provider Provider slug.
 * @property {number | null} sessionPct Session usage percent.
 * @property {number | null} weeklyPct Weekly usage percent.
 * @property {string} sessionPctText Head percent text.
 * @property {string | null} weeklyPctText Tail percent text; null hides it.
 * @property {string} headLabel Session countdown; empty or too wide shows "5H".
 * @property {string} weeklyCountdown Weekly countdown; empty hides it.
 * @property {number | null} sessionResetsAtMs Session reset, Unix ms.
 * @property {number | null} weeklyResetsAtMs Weekly reset, Unix ms.
 * @property {number} primaryDurSecs Session window length.
 * @property {number} secondaryDurSecs Weekly window length.
 * @property {string} sessionFill Ring arc colour.
 * @property {string} weeklyFill Weekly bar colour.
 * @property {string} accent Provider accent colour.
 * @property {boolean} isDark Dark palette.
 * @property {number} canvasW Window width in physical px.
 * @property {number} canvasH Window height in physical px.
 * @property {number} dpi Window DPI.
 */

/** Pointer travel, in physical px, that turns a press into a drag. */
const DRAG_THRESHOLD_PX = 3;
const PULSE_INTERVAL_MS = 80;
const TIME_PROGRESS_INTERVAL_MS = 60_000;
const FONT_STACK = '"Segoe UI", system-ui, sans-serif';
/** Weight text is measured at when the caller names none (regular). */
const MEASURE_WEIGHT = 400;
const HEAD_LABEL_WEIGHT = 600;

const slug = windowLabel.replace(/^bubble-/, '');

/**
 * Look up a required element.
 * @template {Element} T
 * @param {string} id Element id.
 * @returns {T} The element.
 */
function byId(id) {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`bubble.html is missing #${id}`);
  }
  return /** @type {T} */ (/** @type {unknown} */ (element));
}

/** @type {HTMLElement} */
const pill = byId('pill');
/** @type {SVGSVGElement} */
const rings = byId('rings');
/** @type {SVGCircleElement} */
const ringTrack = byId('ring-track');
/** @type {SVGCircleElement} */
const ringArc = byId('ring-arc');
/** @type {SVGCircleElement} */
const timeRingTrack = byId('time-ring-track');
/** @type {SVGCircleElement} */
const timeRingArc = byId('time-ring-arc');
/** @type {HTMLElement} */
const headLabel = byId('head-label');
/** @type {HTMLElement} */
const headPct = byId('head-pct');
/** @type {HTMLElement} */
const usageBar = byId('usage-bar');
/** @type {HTMLElement} */
const usageFill = byId('usage-fill');
/** @type {HTMLElement} */
const timeBar = byId('time-bar');
/** @type {HTMLElement} */
const timeFill = byId('time-fill');
/** @type {HTMLElement} */
const tailPct = byId('tail-pct');
/** @type {HTMLElement} */
const tailTime = byId('tail-time');

const measureContext = document.createElement('canvas').getContext('2d');

/**
 * Width of `text` in physical px. The measuring canvas is unscaled, so a
 * font size in physical px yields a width in physical px.
 * @param {string} text Text to measure.
 * @param {number} fontPx Font size in physical px.
 * @param {number} [weight] Font weight.
 * @returns {number} Width in physical px; 0 without a 2D context.
 */
function measure(text, fontPx, weight = MEASURE_WEIGHT) {
  if (!measureContext) {
    return 0;
  }
  measureContext.font = `${weight} ${fontPx}px ${FONT_STACK}`;
  return measureContext.measureText(text).width;
}

/** @type {BubblePayload | null} */
let payload = null;
/** @type {string} */
let layoutKey = '';
/** @type {ReturnType<typeof headMetrics> | null} */
let head = null;
/** @type {ReturnType<typeof tailLayout> | null} */
let tail = null;

let pulsePhase = 0;
/** @type {ReturnType<typeof setInterval> | null} */
let pulseTimer = null;
/** @type {ReturnType<typeof setInterval> | null} */
let timeProgressTimer = null;

/**
 * Place an element on a physical-px rect.
 * @param {HTMLElement} element Absolutely positioned element.
 * @param {import('./layout.js').Rect} rect Rect in physical px.
 * @param {number} dpi Window DPI.
 * @returns {void}
 */
function place(element, rect, dpi) {
  element.style.left = `${cssPx(rect.l, dpi)}px`;
  element.style.top = `${cssPx(rect.t, dpi)}px`;
  element.style.width = `${cssPx(Math.max(rect.r - rect.l, 0), dpi)}px`;
  element.style.height = `${cssPx(Math.max(rect.b - rect.t, 0), dpi)}px`;
}

/**
 * Place a text box: one line, vertically centred in its rect.
 * @param {HTMLElement} element Text element.
 * @param {import('./layout.js').Rect} rect Rect in physical px.
 * @param {number} fontPx Font size in physical px.
 * @param {number} dpi Window DPI.
 * @returns {void}
 */
function placeText(element, rect, fontPx, dpi) {
  place(element, rect, dpi);
  element.style.fontSize = `${cssPx(fontPx, dpi)}px`;
  element.style.lineHeight = `${cssPx(Math.max(rect.b - rect.t, 0), dpi)}px`;
}

/**
 * Set a ring circle's geometry.
 * @param {SVGCircleElement} circle Circle element.
 * @param {number} cx Centre x.
 * @param {number} cy Centre y.
 * @param {number} r Radius.
 * @param {number} stroke Stroke width.
 * @returns {void}
 */
function placeCircle(circle, cx, cy, r, stroke) {
  circle.setAttribute('cx', String(cx));
  circle.setAttribute('cy', String(cy));
  circle.setAttribute('r', String(r));
  circle.setAttribute('stroke-width', String(stroke));
  // Paths of a circle start at 3 o'clock; rotate so arcs start at 12.
  circle.setAttribute('transform', `rotate(-90 ${cx} ${cy})`);
}

/**
 * Show an arc for `fraction` of a ring, or hide it.
 * @param {SVGCircleElement} circle Arc circle.
 * @param {number} radius Ring radius.
 * @param {number | null} fraction Share of the ring, or null.
 * @param {'from-top' | 'to-top'} anchor Where the arc is pinned.
 * @returns {void}
 */
function setArc(circle, radius, fraction, anchor) {
  const dash = fraction === null ? null : ringDash(radius, fraction, anchor);
  if (!dash) {
    circle.setAttribute('visibility', 'hidden');
    return;
  }
  circle.setAttribute('stroke-dasharray', dash.dashArray);
  circle.setAttribute('stroke-dashoffset', String(dash.dashOffset));
  circle.removeAttribute('visibility');
}

/**
 * Recompute the geometry for the payload's canvas, when it changed.
 * @param {BubblePayload} p Current payload.
 * @returns {void}
 */
function relayout(p) {
  const key = `${p.canvasW}x${p.canvasH}@${p.dpi}`;
  if (key === layoutKey && head && tail) {
    return;
  }
  const canvas = { canvasW: p.canvasW, canvasH: p.canvasH, dpi: p.dpi };
  head = headMetrics(canvas);
  tail = tailLayout(canvas, (text, px, weight) => measure(text, px, weight));
  layoutKey = key;

  const outline = outlinePath(cssPx(p.canvasW, p.dpi), cssPx(p.canvasH, p.dpi));
  pill.style.clipPath = `path('${outline}')`;
  rings.setAttribute('viewBox', `0 0 ${p.canvasW} ${p.canvasH}`);
  placeCircle(ringTrack, head.ringCx, head.ringCy, head.ringRadius, head.ringStroke);
  placeCircle(ringArc, head.ringCx, head.ringCy, head.ringRadius, head.ringStroke);
  placeCircle(timeRingTrack, head.ringCx, head.ringCy, head.timeRingRadius, head.timeRingStroke);
  placeCircle(timeRingArc, head.ringCx, head.ringCy, head.timeRingRadius, head.timeRingStroke);

  placeText(headLabel, head.labelRect, head.fonts.small, p.dpi);
  placeText(headPct, head.pctRect, head.fonts.big, p.dpi);
  placeText(tailPct, tail.usagePctRect, head.fonts.small, p.dpi);
  placeText(tailTime, tail.timeTextRect, head.fonts.main, p.dpi);
  place(usageBar, tail.usageBarRect, p.dpi);
  place(timeBar, tail.timeBarRect, p.dpi);
}

/**
 * Write the texts and the usage values that only change with a payload.
 * @param {BubblePayload} p Current payload.
 * @returns {void}
 */
function renderValues(p) {
  if (!head || !tail) {
    return;
  }
  const labelRectW = head.labelRect.r - head.labelRect.l;
  const labelW = measure(p.headLabel, head.fonts.small, HEAD_LABEL_WEIGHT);
  headLabel.textContent = headLabelText(p.headLabel, labelW, labelRectW);
  headPct.textContent = p.sessionPctText;

  const pctRectW = tail.usagePctRect.r - tail.usagePctRect.l;
  tailPct.textContent = p.weeklyPctText ?? '';
  tailPct.hidden = p.weeklyPctText === null || pctRectW <= 0;
  const timeRectW = tail.timeTextRect.r - tail.timeTextRect.l;
  tailTime.textContent = p.weeklyCountdown;
  tailTime.hidden = p.weeklyCountdown === '' || timeRectW <= 0;

  setArc(
    ringArc,
    head.ringRadius,
    p.sessionPct === null ? null : p.sessionPct / 100,
    'from-top',
  );
  const barW = tail.usageBarRect.r - tail.usageBarRect.l;
  const barH = tail.usageBarRect.b - tail.usageBarRect.t;
  const fillW = usageFillWidth(barW, barH, p.weeklyPct);
  usageFill.hidden = fillW <= 0;
  usageFill.style.width = `${cssPx(fillW, p.dpi)}px`;
}

/**
 * Redraw the remaining-time ring and bar for the current time.
 * @returns {void}
 */
function renderTime() {
  if (!payload || !head || !tail) {
    return;
  }
  const now = Date.now();
  setArc(
    timeRingArc,
    head.timeRingRadius,
    remainingFraction(payload.sessionResetsAtMs, payload.primaryDurSecs, now),
    'to-top',
  );
  const barW = tail.timeBarRect.r - tail.timeBarRect.l;
  const fillW = timeFillWidth(
    barW,
    remainingFraction(payload.weeklyResetsAtMs, payload.secondaryDurSecs, now),
  );
  timeFill.hidden = fillW <= 0;
  timeFill.style.width = `${cssPx(fillW, payload.dpi)}px`;
}

/**
 * Paint the colours that pulse: the ring arc while the session is at 95%
 * or more, the weekly bar fill and percent text while the week is.
 * @returns {void}
 */
function paintPulse() {
  if (!payload) {
    return;
  }
  const t = pulseT(pulsePhase);
  const sessionAlarm = pulseActive(payload.sessionPct, null);
  const weeklyAlarm = pulseActive(null, payload.weeklyPct);
  ringArc.style.stroke = sessionAlarm ? brighten(payload.sessionFill, t) : payload.sessionFill;
  usageFill.style.background = weeklyAlarm ? brighten(payload.weeklyFill, t) : payload.weeklyFill;
  if (weeklyAlarm) {
    const text = getComputedStyle(document.documentElement).getPropertyValue('--text');
    tailPct.style.color = brighten(text, t);
  } else {
    tailPct.style.color = '';
  }
}

/**
 * Run the 80 ms pulse only while a value is at 95% or more; reset the
 * phase whenever it stops.
 * @returns {void}
 */
function syncPulseTimer() {
  const active = payload !== null && pulseActive(payload.sessionPct, payload.weeklyPct);
  if (active && pulseTimer === null) {
    pulseTimer = setInterval(() => {
      pulsePhase += 1;
      paintPulse();
    }, PULSE_INTERVAL_MS);
  } else if (!active && pulseTimer !== null) {
    clearInterval(pulseTimer);
    pulseTimer = null;
    pulsePhase = 0;
  }
}

/**
 * Advance the time progress every minute while a reset time is known.
 * @returns {void}
 */
function syncTimeProgressTimer() {
  const active =
    payload !== null && (payload.sessionResetsAtMs !== null || payload.weeklyResetsAtMs !== null);
  if (active && timeProgressTimer === null) {
    timeProgressTimer = setInterval(renderTime, TIME_PROGRESS_INTERVAL_MS);
  } else if (!active && timeProgressTimer !== null) {
    clearInterval(timeProgressTimer);
    timeProgressTimer = null;
  }
}

/**
 * Apply the theme palette.
 * @param {boolean} isDark Dark palette.
 * @returns {void}
 */
function applyTheme(isDark) {
  document.documentElement.dataset.theme = isDark ? 'dark' : 'light';
}

/**
 * Render a payload.
 * @param {BubblePayload} p Payload from Rust.
 * @returns {void}
 */
function applyPayload(p) {
  payload = p;
  applyTheme(p.isDark);
  relayout(p);
  renderValues(p);
  renderTime();
  syncPulseTimer();
  syncTimeProgressTimer();
  paintPulse();
}

// ---------- Input ----------

/** @type {{pointerId: number, x: number, y: number} | null} */
let pressed = null;

/**
 * Physical px per CSS px, from the payload DPI.
 * @returns {number} Scale factor.
 */
function physicalScale() {
  return payload ? payload.dpi / BASE_DPI : window.devicePixelRatio || 1;
}

/** @returns {void} */
function clearPressed() {
  pressed = null;
}

pill.addEventListener('pointerdown', (event) => {
  if (event.button !== 0) {
    return;
  }
  try {
    pill.setPointerCapture(event.pointerId);
  } catch (err) {
    // Without capture a release outside the bubble is missed, which only
    // costs that one click.
    logFailure('setPointerCapture')(err);
  }
  pressed ={ pointerId: event.pointerId, x: event.screenX, y: event.screenY };
});

pill.addEventListener('pointermove', (event) => {
  if (!pressed || event.pointerId !== pressed.pointerId) {
    return;
  }
  if ((event.buttons & 1) !== 1) {
    clearPressed();
    return;
  }
  const scale = physicalScale();
  const dx = Math.abs(event.screenX - pressed.x) * scale;
  const dy = Math.abs(event.screenY - pressed.y) * scale;
  if (Math.max(dx, dy) < DRAG_THRESHOLD_PX) {
    return;
  }
  // The OS move loop swallows the release, so the drag ends in Rust; the
  // pressed state is cleared now so a later hover cannot start another.
  clearPressed();
  if (pill.hasPointerCapture(event.pointerId)) {
    pill.releasePointerCapture(event.pointerId);
  }
  // Arm the move-end handling before the window starts moving.
  invoke('bubble_drag_start', { slug })
    .catch(logFailure('bubble_drag_start'))
    .then(() => startDragging())
    .catch(logFailure('startDragging'));
});

pill.addEventListener('pointerup', (event) => {
  if (!pressed || event.pointerId !== pressed.pointerId || event.button !== 0) {
    return;
  }
  clearPressed();
  invoke('bubble_click', { slug }).catch(logFailure('bubble_click'));
});

pill.addEventListener('pointercancel', clearPressed);
pill.addEventListener('lostpointercapture', clearPressed);
window.addEventListener('blur', clearPressed);

window.addEventListener('contextmenu', (event) => {
  event.preventDefault();
  if (event.target !== pill) {
    return;
  }
  invoke('show_context_menu', { label: windowLabel }).catch(logFailure('show_context_menu'));
});

/** Ctrl+wheel gesture carried between wheel events. */
let wheelState = WHEEL_IDLE;

window.addEventListener(
  'wheel',
  (event) => {
    if (!event.ctrlKey) {
      return;
    }
    // Ctrl+wheel would otherwise zoom the page.
    event.preventDefault();
    if (event.target !== pill) {
      return;
    }
    const result = wheelSteps(wheelState, event.deltaY, event.deltaMode, event.timeStamp);
    wheelState = result.state;
    const dir = Math.sign(result.steps);
    for (let i = Math.abs(result.steps); i > 0; i -= 1) {
      invoke('bubble_resize_step', { dir }).catch(logFailure('bubble_resize_step'));
    }
  },
  { passive: false },
);

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') {
    renderTime();
  }
});

// ---------- Start-up ----------

forwardErrors();

listen('bubble:update', (/** @type {BubblePayload} */ p) => applyPayload(p)).catch(
  logFailure('listen bubble:update'),
);
listen('theme', (/** @type {{isDark: boolean}} */ p) => {
  applyTheme(p.isDark);
  paintPulse();
}).catch(logFailure('listen theme'));

invoke('get_bubble_state', { provider: slug })
  .then((/** @type {BubblePayload} */ p) => {
    // A push that arrived first is at least as new as this answer.
    if (payload === null) {
      applyPayload(p);
    }
    return invoke('bubble_ready', { label: windowLabel });
  })
  .catch(logFailure('bubble start-up'));
