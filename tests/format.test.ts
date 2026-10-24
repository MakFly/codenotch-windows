import { describe, expect, test } from "bun:test";
import {
  headline,
  isFresh,
  percentage,
  resetTime,
  tokens,
} from "../src/lib/format";
import type { Reading } from "../src/lib/types";

describe("honest usage display", () => {
  test("unknown token count is never presented as zero", () => {
    expect(tokens(null)).toBe("—");
    expect(tokens(undefined)).toBe("—");
    expect(tokens(0)).toBe("0");
  });
  test("zero is a valid percentage, missing and invalid values are unknown", () => {
    expect(percentage(0)).toBe("0 %");
    expect(percentage(undefined)).toBe("—");
    expect(percentage(NaN)).toBe("—");
    expect(percentage(125)).toBe("125 %");
  });
  test("reset countdown never turns a past window into a new allowance", () => {
    expect(resetTime(1000, 1001000)).toBe("Reset needs checking");
    expect(resetTime(null)).toBe("Reset time unavailable");
    expect(resetTime(4600, 1000000)).toBe("in 1 h 0 min");
    expect(resetTime(4600, 1000000, "fr")).toBe("dans 1 h 0 min");
  });
  test("an old successful reading is not live", () => {
    const r = { status: "ok", updatedAt: 1000 } as Reading;
    expect(isFresh(r, 60, 1100000)).toBe(true);
    expect(isFresh(r, 60, 1400000)).toBe(false);
    expect(isFresh({ ...r, status: "stale" }, 60, 1100000)).toBe(false);
  });
  test("a breached weekly limit is visible even when session usage is low", () => {
    const r = {
      windows: [
        { label: "5 h", usedPercent: 12, resetsAt: null },
        { label: "7 jours", usedPercent: 91, resetsAt: null },
      ],
    } as Reading;
    expect(headline(r)?.label).toBe("7 jours");
    expect(headline(r)?.usedPercent).toBe(91);
    expect(headline(undefined)).toBeUndefined();
  });
});
