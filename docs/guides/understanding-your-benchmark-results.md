# 📊 Understanding Your Benchmark Results

| | |
| :--- | :--- |
| **Status** | Draft |
| **Last updated** | 2026-09-08 |
| **Audience** | Anyone reading a VOIDFRAME benchmark report — no stats background needed |

VOIDFRAME throws a lot of numbers at you after a benchmark run: FPS, percentiles, a CV
percentage, a WCPS score, a colored badge saying "Better" or "Inconclusive"... it's a lot.
This guide explains **what each number on the Results screen actually means and why a
competitive CS2 player should care about it** — no math degree required.

> If you just want the two numbers that matter most for competitive play, jump to
> [TL;DR: what to actually look at](#tldr-what-to-actually-look-at).

---

## The big picture first

A "smooth 200 FPS average" can still betray you if the game drops to 80 FPS for a
quarter-second right as you're peeking an angle. Average FPS hides exactly the moments
that cost you fights. That's why VOIDFRAME reports several different numbers instead of
just one — each one answers a different question about "did this tweak actually help?"

Every scenario you test is compared against your **Baseline** (your stock, untouched
setup). The table and charts always show scenario numbers next to (or as a % change
from) that baseline.

---

## Framerate metrics (top chart, main table)

### Average FPS
**What it is:** The plain average of your framerate across the whole benchmark.

**Why it matters:** It's the number everyone quotes, and it's a fine *general* sense of
performance — but on its own it tells you nothing about how bumpy the ride was. Two runs
can have the identical average FPS while one feels buttery smooth and the other stutters
constantly. Use it as a headline number, not the whole story.

### 1% Low (P1)
**What it is:** Take your worst 1% of frames (the slowest ones) and turn that into an
FPS number. If your 1% Low is 150 FPS, it means for the roughest 1% of the benchmark,
you were still doing about 150 FPS — everything else was faster than that.

**Why it matters:** This is arguably **the single most important number for competitive
play**. Your worst frames are the ones that happen during the exact moments you can't
afford them — a flick shot, a bunny-hop peek, a smoke/flash going off. A tweak that
raises your 1% Low is making your *worst* moments better, which is exactly what wins
close duels. A tweak that boosts average FPS but tanks the 1% Low is usually a bad trade.

### 0.1% Low (P0.1)
**What it is:** The same idea as 1% Low, but looking at your worst 0.1% of frames — an
even smaller, even nastier slice. This catches rarer but more severe dips than the 1%
Low does.

**Why it matters:** If 1% Low tells you about "occasional rough patches," 0.1% Low tells
you about "the worst single stutters that happened at all" — the kind of momentary
freeze that can lose you a 1v1 even if it only happens once or twice in a whole match.

---

## Smoothness & consistency metrics (Pacing & Stability tab, main table)

These three answer a question average/percentile FPS can't: **"was the frame pacing
actually consistent, or was it jerky?"** Two scenarios can have identical Avg/P1/P0.1
and still feel completely different to play — this is where that difference shows up.

### Adaptive CV (Adaptive Frame-Time CV)
**What it is:** A measure of how much your frame timing wobbles from one moment to the
next, expressed as a percentage. "Adaptive" means it compares each frame against a
short (half-second) rolling average of nearby frames, rather than the whole benchmark's
average — so it isn't fooled by your PC gradually warming up or a big scene change; it's
purely measuring moment-to-moment jitter. **Lower is better.**

**Why it matters:** High-and-low FPS swings, even small and frequent ones, are what
make a game feel "juddery" or "off" even when the FPS counter looks fine. This is the
closest thing to a "does it feel smooth" number VOIDFRAME reports.

### Stutter %
**What it is:** The percentage of frames that took noticeably longer than normal to
render (more than double your typical frame time). If Stutter % is 3%, then 3 out of
every 100 frames were a real, perceptible stutter.

**Why it matters:** This directly counts *how often* something actually hitches, rather
than just estimating overall jitter like Adaptive CV does. A tweak that lowers Stutter %
is reducing how often you'll notice an actual freeze-frame moment mid-game.

### Anim Err (Animation Error)
**What it is:** How far off each frame's on-screen motion was from where it *should*
have been, based on perfectly even timing — averaged across the whole run, in
milliseconds. Comes straight from the game's own frame-pacing data.

**Why it matters:** This catches a subtler problem than raw stutter: frames that render
on time but show the game slightly ahead of or behind where smooth motion says it
should be. That mismatch is part of what makes movement and aim tracking feel
"off"/rubbery even when your FPS counter looks perfectly healthy. Lower is better.

---

## The combined score

### WCPS v3
**What it is:** "Weighted Competitive Performance Score" — a single number that rolls
all 6 metrics above (Avg FPS, 1% Low, 0.1% Low, Adaptive CV, Stutter %, Anim Err) into
one score, weighted by how much each one tends to matter. **0 means "about the same as
your baseline." Positive means better overall; negative means worse overall.** It's not
a percentage and it isn't FPS — treat it purely as a ranking number to sort scenarios
by "best overall" when you've tested several tweaks at once.

**Why it matters:** You don't have to manually weigh "is +5 FPS worth +1% more
stutters?" yourself — WCPS already factored that trade-off in. When you've tested a
pile of scenarios, sort by WCPS and the top of the list is your best all-around option.

> **A number's not automatically meaningful just because it's not zero.** A WCPS of
> +1.2 from a benchmark with very few measurement passes can be pure noise — see the
> Verdict badge below, which is VOIDFRAME's actual answer to "is this difference real."

---

## The Verdict badge

Every scenario gets a colored badge. This is VOIDFRAME's statistical answer to "is this
tweak *actually* doing something, or did I just get lucky/unlucky on this run?" —
because run-to-run noise (background processes, thermal state, plain randomness) is
real, and a single benchmark pass can't tell the difference between "the tweak worked"
and "this run just happened to be a bit faster."

| Badge | What it means in plain terms |
| :--- | :--- |
| 🟢 **BETTER** | VOIDFRAME is confident this scenario genuinely outperforms your baseline — not just luck. |
| 🔴 **WORSE** | VOIDFRAME is confident this scenario genuinely underperforms your baseline. |
| ⚪ **CONFIRMED SAME** | VOIDFRAME ran enough passes to be confident there's **no real difference** — the tweak simply doesn't do anything measurable here. |
| 🟡 **INCONCLUSIVE** | Not enough evidence either way yet. This is *not* a verdict of "no effect" — it means the data doesn't clearly say "different" or "same." More measurement passes (raise `measure_loops`) would give a clearer answer. |

**Why it matters:** It stops you from chasing phantom gains. It's tempting to see "+8
FPS" and declare a tweak a win — but if the badge says Inconclusive, that +8 FPS might
evaporate the next time you run it. Trust the badge over the raw numbers when they seem
to disagree.

---

## Reading the detail view (click "Inspect" on a scenario)

Clicking a scenario's row opens a drawer with a few extra numbers for players who want
to dig deeper into *why* something changed.

- **Telemetry Deltas** — the same 6 metrics as the main table, but shown purely as a
  "% different from baseline" for that one scenario, all in one place. Green = moved in
  the good direction, red = moved in the bad direction (VOIDFRAME already accounts for
  the fact that "lower is better" for CV/Stutter%/Anim Err, so green always means "good"
  here).
- **Mean Frame Time** — the average time (in milliseconds) it took to render each
  frame. This is just Average FPS expressed the other way around (lower ms = higher
  FPS) — useful mainly because frame-time charts (see below) are usually plotted this
  way, not in FPS.
- **Render Latency** — roughly how long, in milliseconds, it takes from the game
  deciding to draw a frame to that frame being ready to hand off for display. Lower
  generally means your inputs turn into on-screen results faster — relevant to how
  "responsive" a setup feels, separate from how smooth it looks.
- **GPU Busy** — how much time, in milliseconds, your graphics card actually spent
  working per frame.
- **GPU Bound %** — what fraction of your total frame time your GPU was busy for. A
  high percentage means your graphics card is the thing holding your FPS back
  (GPU-bound); a low percentage means something else — usually the CPU — is the
  bottleneck instead. This tells you *which* upgrade or setting would actually help if
  you wanted more FPS.

---

## Two things worth trusting before you trust the numbers

### The reproducibility line
Near the top of the results page, VOIDFRAME shows your baseline's own frame-time
consistency (its whole-capture CV%) with a note like *"results with a smaller effect
than this may not be reliably distinguishable from run-to-run noise."* This is a
built-in sanity check: if a scenario's improvement is smaller than your baseline's own
natural wobble, be skeptical of it even before checking the Verdict badge — it's telling
you the honest measurement floor for this particular test session.

### The present-mode warning
If VOIDFRAME flags a scenario with a warning about "present mode," it means CS2 wasn't
running in true exclusive fullscreen during that capture — Windows' compositor was
involved in drawing frames. This usually adds latency and jitter that has **nothing to
do with your tweak** and everything to do with a misconfigured launch (e.g. accidentally
running in Borderless/Windowed mode). Fix that before trusting any other number from
that scenario.

---

## TL;DR: what to actually look at

1. **Check the Verdict badge first.** If it's not Better/Worse, don't read anything else
   as "the tweak worked."
2. **1% Low and 0.1% Low matter more than Average FPS** for how a game actually feels to
   play competitively — a tweak that helps your worst moments is usually worth more than
   one that only bumps the average.
3. **Stutter % and Adaptive CV** are your "does it feel smooth" numbers — worth checking
   even when the FPS numbers alone look like a win.
4. **WCPS** is the quick way to rank several tested scenarios against each other when
   you don't want to manually weigh six numbers yourself.
5. When in doubt, **more benchmark passes (`measure_loops`) turn Inconclusive into a
   real answer** — it's not a broken result, it's an honest "not enough data yet."

---

## Changelog

- **2026-09-08** — Initial version, written from user feedback that the Visualizer's
  metrics were confusing/overwhelming. Definitions and formulas sourced from
  `crates/voidframe-engine/src/capture/parser.rs`, `crates/voidframe-engine/src/stats/v3/`,
  and the research trail in `study/analysis/report.md` / `study/research/wcps-v3-design.md`.
