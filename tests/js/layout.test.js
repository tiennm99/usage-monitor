// Bubble layout checked against values worked out from the Rust layout
// formulas (the GDI bubble's `compute_bubble_layout`) with the same
// stand-in text measurement.

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  brighten,
  cssPx,
  headLabelText,
  headMetrics,
  outlinePath,
  pulseActive,
  pulseT,
  remainingFraction,
  ringDash,
  scale,
  TAIL_PCT_WEIGHT,
  tailLayout,
  timeFillWidth,
  usageFillWidth,
  WHEEL_IDLE,
  WHEEL_IDLE_MS,
  wheelSteps,
} from '../../ui/layout.js';

/**
 * Stand-in for canvas `measureText`: 0.55 em per character, truncated.
 * @param {string} text Text to measure.
 * @param {number} px Font size.
 * @returns {number} Width.
 */
const measure = (text, px) => Math.trunc((px * text.length * 11) / 20);

/**
 * Rect shorthand.
 * @param {number} l Left.
 * @param {number} t Top.
 * @param {number} r Right.
 * @param {number} b Bottom.
 * @returns {{l: number, t: number, r: number, b: number}} The rect.
 */
const rect = (l, t, r, b) => ({ l, t, r, b });

const CASES = [
  {
    name: '200 logical at 96 dpi',
    canvas: { canvasW: 200, canvasH: 66, dpi: 96 },
    head: {
      ringCx: 33, ringCy: 33, ringRadius: 26.5, ringStroke: 3, timeRingRadius: 19.5, timeRingStroke: 2,
      labelRect: rect(4, 18, 62, 29), pctRect: rect(4, 31, 62, 48), fonts: { big: 15, small: 9, main: 9 },
    },
    tail: {
      usagePctRect: rect(165, 23, 186, 34), usageBarRect: rect(72, 25, 157, 31),
      timeTextRect: rect(165, 33, 186, 44), timeBarRect: rect(72, 37, 157, 40),
    },
  },
  {
    name: '360 logical at 96 dpi',
    canvas: { canvasW: 360, canvasH: 138, dpi: 96 },
    head: {
      ringCx: 69, ringCy: 69, ringRadius: 62.5, ringStroke: 3, timeRingRadius: 55.5, timeRingStroke: 2,
      labelRect: rect(4, 39, 134, 59), pctRect: rect(4, 63, 134, 98), fonts: { big: 33, small: 18, main: 18 },
    },
    tail: {
      usagePctRect: rect(305, 53, 346, 73), usageBarRect: rect(144, 57, 297, 69),
      timeTextRect: rect(305, 68, 346, 88), timeBarRect: rect(144, 75, 297, 80),
    },
  },
  {
    // 300 px wide: `innerWidth * devicePixelRatio` would drift here.
    name: '200 logical at 144 dpi',
    canvas: { canvasW: 300, canvasH: 99, dpi: 144 },
    head: {
      ringCx: 49.5, ringCy: 49.5, ringRadius: 40.5, ringStroke: 4, timeRingRadius: 30.5, timeRingStroke: 3,
      labelRect: rect(6, 27, 93, 43), pctRect: rect(6, 46, 93, 72), fonts: { big: 23, small: 13, main: 13 },
    },
    tail: {
      usagePctRect: rect(248, 35, 279, 51), usageBarRect: rect(108, 38, 236, 47),
      timeTextRect: rect(248, 50, 279, 66), timeBarRect: rect(108, 56, 236, 60),
    },
  },
  {
    name: '360 logical at 144 dpi',
    canvas: { canvasW: 540, canvasH: 207, dpi: 144 },
    head: {
      ringCx: 103.5, ringCy: 103.5, ringRadius: 94.5, ringStroke: 4, timeRingRadius: 84.5, timeRingStroke: 3,
      labelRect: rect(6, 59, 201, 88), pctRect: rect(6, 95, 201, 147), fonts: { big: 49, small: 26, main: 26 },
    },
    tail: {
      usagePctRect: rect(459, 81, 519, 110), usageBarRect: rect(216, 86, 447, 104),
      timeTextRect: rect(459, 103, 519, 132), timeBarRect: rect(216, 113, 447, 121),
    },
  },
  {
    name: '140 logical at 120 dpi',
    canvas: { canvasW: 175, canvasH: 57, dpi: 120 },
    head: {
      ringCx: 28.5, ringCy: 28.5, ringRadius: 21, ringStroke: 3, timeRingRadius: 13, timeRingStroke: 2,
      labelRect: rect(5, 13, 52, 26), pctRect: rect(5, 28, 52, 43), fonts: { big: 13, small: 11, main: 11 },
    },
    tail: {
      usagePctRect: rect(132, 17, 158, 30), usageBarRect: rect(64, 20, 122, 27),
      timeTextRect: rect(132, 29, 158, 42), timeBarRect: rect(64, 34, 122, 37),
    },
  },
];

describe('headMetrics', () => {
  for (const c of CASES) {
    it(`matches the Rust layout for ${c.name}`, () => {
      assert.deepEqual(headMetrics(c.canvas), c.head);
    });
  }
});

describe('tailLayout', () => {
  for (const c of CASES) {
    it(`matches the Rust layout for ${c.name}`, () => {
      assert.deepEqual(tailLayout(c.canvas, measure), c.tail);
    });
  }

  it('shrinks the text column before the bars drop below their minimum', () => {
    const tail = tailLayout({ canvasW: 200, canvasH: 66, dpi: 96 }, () => 1000);
    // Content 72..186 is 114 wide: 114 - 8 gap - 8 bar minimum = 98.
    assert.deepEqual(tail.usagePctRect, rect(88, 23, 186, 34));
    assert.deepEqual(tail.usageBarRect, rect(72, 25, 80, 31));
  });

  it('collapses the text column to nothing when even the bars do not fit', () => {
    const tail = tailLayout({ canvasW: 80, canvasH: 66, dpi: 96 }, () => 50);
    assert.equal(tail.usagePctRect.l, tail.usagePctRect.r);
    assert.equal(tail.usageBarRect.r - tail.usageBarRect.l, 8);
  });

  it('reserves the percent column at the weight it is drawn in', () => {
    const regular = 40;
    const semibold = 61.3;
    /** @type {Array<number | undefined>} */
    const pctWeights = [];
    const tail = tailLayout({ canvasW: 360, canvasH: 138, dpi: 96 }, (text, _px, weight) => {
      if (text === '100%') {
        pctWeights.push(weight);
      }
      return weight === TAIL_PCT_WEIGHT ? semibold : regular;
    });
    assert.deepEqual(pctWeights, [600]);
    assert.ok(tail.usagePctRect.r - tail.usagePctRect.l >= Math.ceil(semibold));
  });

  it('rounds fractional measurements up', () => {
    const exact = tailLayout({ canvasW: 360, canvasH: 138, dpi: 96 }, () => 40);
    const fractional = tailLayout({ canvasW: 360, canvasH: 138, dpi: 96 }, () => 39.2);
    assert.deepEqual(fractional, exact);
  });
});

describe('outlinePath', () => {
  it('joins a full-height head to a centred tail with rounded end corners', () => {
    // h 100: tail 60 tall from y 20 to 80, end corners of radius 12, and the
    // tail edges meet the head circle at x = 50 + sqrt(50² - 30²) = 90.
    assert.equal(
      outlinePath(300, 100),
      'M 90 20 L 288 20 A 12 12 0 0 1 300 32 L 300 68 A 12 12 0 0 1 288 80 ' +
        'L 90 80 A 50 50 0 1 1 90 20 Z',
    );
  });
});

describe('scale and cssPx', () => {
  it('truncates like the Rust scale', () => {
    assert.equal(scale(200, 96), 200);
    assert.equal(scale(200, 144), 300);
    assert.equal(scale(5, 120), 6);
    assert.equal(scale(66, 168), 115);
  });

  it('converts physical px to CSS px from the payload DPI', () => {
    assert.equal(cssPx(300, 144), 200);
    assert.equal(cssPx(99, 144), 66);
    assert.equal(cssPx(66, 96), 66);
  });
});

describe('ringDash', () => {
  const r = 10;
  const c = 2 * Math.PI * r;

  it('starts the usage arc at 12 o\'clock', () => {
    assert.deepEqual(ringDash(r, 0.25, 'from-top'), { dashArray: `${c / 4} ${c}`, dashOffset: 0 });
  });

  it('ends the remaining arc at 12 o\'clock', () => {
    const dash = ringDash(r, 0.25, 'to-top');
    assert.equal(dash?.dashArray, `${c / 4} ${c}`);
    assert.ok(Math.abs((dash?.dashOffset ?? 0) + (c * 3) / 4) < 1e-9);
  });

  it('draws a full ring at 1 and clamps above it', () => {
    assert.deepEqual(ringDash(r, 1, 'to-top'), { dashArray: `${c} ${c}`, dashOffset: -0 });
    assert.deepEqual(ringDash(r, 2, 'from-top'), ringDash(r, 1, 'from-top'));
  });

  it('draws nothing at zero, below it or for NaN', () => {
    assert.equal(ringDash(r, 0, 'from-top'), null);
    assert.equal(ringDash(r, -0.5, 'to-top'), null);
    assert.equal(ringDash(r, Number.NaN, 'from-top'), null);
  });
});

describe('remainingFraction', () => {
  const now = 1_800_000_000_000;

  it('is null without a reset time', () => {
    assert.equal(remainingFraction(null, 18_000, now), null);
  });

  it('is the remaining share of the window', () => {
    assert.equal(remainingFraction(now + 9_000_000, 18_000, now), 0.5);
    assert.equal(remainingFraction(now + 3_600_000, 18_000, now), 0.2);
  });

  it('clamps past resets to 0 and far resets to 1', () => {
    assert.equal(remainingFraction(now - 1_000, 18_000, now), 0);
    assert.equal(remainingFraction(now + 36_000_000, 18_000, now), 1);
  });

  it('is null for a non-positive window length', () => {
    assert.equal(remainingFraction(now, 0, now), null);
  });
});

describe('pulse', () => {
  it('follows the Rust triangle wave', () => {
    assert.equal(pulseT(0), 0);
    assert.equal(pulseT(6), 0.5);
    assert.equal(pulseT(12), 1);
    assert.equal(pulseT(18), 0.5);
    assert.equal(pulseT(24), 0);
    assert.equal(pulseT(36), 1);
  });

  it('runs only at 95% and above', () => {
    assert.equal(pulseActive(null, null), false);
    assert.equal(pulseActive(94.9, 10), false);
    assert.equal(pulseActive(95, null), true);
    assert.equal(pulseActive(null, 99), true);
  });

  it('brightens toward white by at most 30%', () => {
    assert.equal(brighten('#A01818', 0), '#A01818');
    // 160 + 95 * 0.3 = 188.5 -> 189; 24 + 231 * 0.3 = 93.3 -> 93.
    assert.equal(brighten('#A01818', 1), '#BD5D5D');
    // Half way: 0.15 of the distance.
    assert.equal(brighten('#000000', 0.5), '#262626');
    assert.equal(brighten('#eaeaea', 1), '#F0F0F0');
    assert.equal(brighten('#FFFFFF', 1), '#FFFFFF');
    assert.equal(brighten('#123456', 5), brighten('#123456', 1));
  });

  it('leaves a malformed colour unchanged', () => {
    assert.equal(brighten('red', 1), 'red');
  });
});

describe('bar fills', () => {
  it('floors a small usage fill at one bar height', () => {
    assert.equal(usageFillWidth(100, 6, 1), 6);
    assert.equal(usageFillWidth(100, 6, 50), 50);
    assert.equal(usageFillWidth(100, 6, 150), 100);
    assert.equal(usageFillWidth(4, 6, 1), 4);
  });

  it('draws no usage fill without a value or at zero', () => {
    assert.equal(usageFillWidth(100, 6, null), 0);
    assert.equal(usageFillWidth(100, 6, 0), 0);
    assert.equal(usageFillWidth(0, 6, 50), 0);
  });

  it('sizes the time fill from the remaining fraction', () => {
    assert.equal(timeFillWidth(100, 0.25), 25);
    assert.equal(timeFillWidth(100, null), 0);
    assert.equal(timeFillWidth(100, 0), 0);
  });
});

describe('headLabelText', () => {
  it('keeps a countdown that fits', () => {
    assert.equal(headLabelText('2h', 10, 58), '2h');
    assert.equal(headLabelText('…', 5, 58), '…');
  });

  it('falls back to 5H when empty or too wide', () => {
    assert.equal(headLabelText('', 0, 58), '5H');
    assert.equal(headLabelText('23h 59m', 59, 58), '5H');
  });
});

describe('wheelSteps', () => {
  const PIXEL = 0;
  const LINE = 1;
  const PAGE = 2;

  /**
   * Feed wheel events through `wheelSteps` and collect the steps.
   * @param {Array<[number, number, number]>} events `[deltaY, deltaMode, ms]` triples.
   * @returns {number[]} Steps per event.
   */
  const run = (events) => {
    let state = WHEEL_IDLE;
    return events.map(([deltaY, mode, ms]) => {
      const result = wheelSteps(state, deltaY, mode, ms);
      state = result.state;
      return result.steps;
    });
  };

  it('gives exactly one step per mouse notch', () => {
    assert.deepEqual(run([[-100, PIXEL, 0]]), [1]);
    assert.deepEqual(run([[100, PIXEL, 0]]), [-1]);
    assert.deepEqual(run([[-100, PIXEL, 0], [-100, PIXEL, 50], [-100, PIXEL, 100]]), [1, 1, 1]);
  });

  it('waits for 100 px of travel before a step', () => {
    assert.deepEqual(run([[-40, PIXEL, 0]]), [0]);
    assert.deepEqual(run([[-40, PIXEL, 0], [-40, PIXEL, 10], [-40, PIXEL, 20]]), [0, 0, 1]);
    assert.deepEqual(run([[-40, PIXEL, 0], [-40, PIXEL, 1000]]), [0, 0]);
  });

  it('turns a stream of small touchpad deltas into one step per 100 px', () => {
    const events = /** @type {Array<[number, number, number]>} */ (
      Array.from({ length: 30 }, (_, i) => [-10, PIXEL, i * 10])
    );
    const steps = run(events);
    assert.equal(steps.reduce((a, b) => a + b, 0), 3);
    assert.ok(steps.every((s) => s === 0 || s === 1));
  });

  it('starts a new gesture after an idle pause', () => {
    assert.deepEqual(run([[-60, PIXEL, 0], [-60, PIXEL, WHEEL_IDLE_MS + 1]]), [0, 0]);
    assert.deepEqual(run([[-60, PIXEL, 0], [-60, PIXEL, WHEEL_IDLE_MS]]), [0, 1]);
  });

  it('starts a new gesture when the direction flips', () => {
    assert.deepEqual(run([[-150, PIXEL, 0], [10, PIXEL, 10]]), [1, 0]);
    assert.deepEqual(run([[-150, PIXEL, 0], [60, PIXEL, 10], [60, PIXEL, 20]]), [1, 0, -1]);
  });

  it('ignores a few px of reversed jitter inside a gesture', () => {
    assert.deepEqual(
      run([[-10, PIXEL, 0], [-10, PIXEL, 10], [2, PIXEL, 20], [-10, PIXEL, 30], [1, PIXEL, 40], [-10, PIXEL, 50]]),
      [0, 0, 0, 0, 0, 0],
    );
    // After a whole notch leaves no travel, a reverse twitch and the next
    // forward delta still wait for 100 px.
    assert.deepEqual(run([[-100, PIXEL, 0], [2, PIXEL, 10], [-10, PIXEL, 20]]), [1, 0, 0]);
  });

  it('sends several steps for one large delta', () => {
    assert.deepEqual(run([[-250, PIXEL, 0], [-50, PIXEL, 10]]), [2, 1]);
  });

  it('converts line and page deltas to px', () => {
    // Three lines are 48 px, so the third event crosses 100 px.
    const lines = run([[-3, LINE, 0], [-3, LINE, 10], [-3, LINE, 20], [-3, LINE, 30]]);
    assert.deepEqual(lines, [0, 0, 1, 0]);
    assert.deepEqual(run([[-1, PAGE, 0]]), [8]);
  });

  it('ignores zero and non-finite deltas', () => {
    assert.deepEqual(run([[0, PIXEL, 0], [Number.NaN, PIXEL, 10]]), [0, 0]);
  });
});
