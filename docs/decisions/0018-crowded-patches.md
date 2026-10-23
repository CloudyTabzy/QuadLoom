# 0018 — Crowded parts are laid out again with crowding first

**Status:** accepted 2026-10-23.

## Context

A patch holding several singularities ("crowded") can't take a pattern, because patterns place at most one. Its quads then flatten the shape, so detail disappears. Decision 0017 left one case open: the Organic armadillo's hand. Under Organic (no angle features) its patches hold up to 15 fingertip singularities. That is 20.6% of the area, and the fingers come out as slabs.

## Diagnosis

- **What blocks the split.** Every rejected candidate on a crowded patch was logged (1,396 attempts), with the first rule of the lexicographic acceptance score that refused it:

  | Rule that refused | Attempts |
  |---|---|
  | missing or extra sides | 635 |
  | CC-ability | 232 |
  | new non-convex vertex | 174 |
  | (accepted on fewest singularities) | 124 |

- **Most refusals are temporary.** Decision 0017 had blamed "no new concave corner". In fact most splits fail on side counts: a path separating two fingertips leaves a piece with 7 sides, or 2. That is usually temporary, since later paths and the final corner re-declaration fix side counts. Crowding is never fixed later, because nothing traces from a singularity (the oracle doesn't either; its "more singularities" check is commented out).

## Decision

**Crowding-first score.** `layout_score` (in `trace/select.rs`) can rank crowding ahead of side counts. Crowding is measured as Σ max(0, k − 1) over patches with k singularities. With crowding first, it takes the place of "most singularities in one patch" and moves ahead of the side counts. Convexity stays ahead of it, because a reflex corner folds the pattern however few singularities the patch holds.

**Applied only where needed (`select::lay_out`).** Each part is laid out with the usual order first. If crowded patches still cover at least 0.5% of the part's area (`CROWDED_SHARE`), the part is laid out again from scratch with crowding first. The second layout is kept only if its final score, with crowding first, is lower. Sub-patch layouts inherit the mode. The trace report counts the kept second layouts (`uncrowded`).

## Variants tried

All variants were run behind a temporary environment switch, on 7 models × 3 settings (core defaults, Balanced at Density 2, Organic):

| Variant | Result |
|---|---|
| max_first: most singularities ahead of sides | as good as the sum on the hand |
| **sum_first**: Σ(k − 1) ahead of sides | as max_first, but fewer concave quads on max-planck at Density 2 (16 vs 41) |
| sum3_first: count only patches with k ≥ 3 | left crowding on the bunny and the ogre |
| sum_first everywhere | regressed parts that were never crowded (Organic max-planck p5 SJ 0.73 → 0.60, poor quads 0.5% → 2.7%): the paths taken early differ, and the layout is chaotic in them |
| sum_first when any crowding is left | still changed parts with one tiny crowded patch (ogre core p5 0.73 → 0.70) |
| **sum_first above 0.5% crowded area** | chosen |

## Evidence

**Sweep, chosen rule vs 0.1.10** (patchprobe; 17 of 21 runs are byte-identical, and the 4 that changed are below):

| Run | Crowded area | p5 SJ | SJ < 0.5 | Quads |
|---|---|---|---|---|
| Armadillo, Organic | 20.6% (most 15) → 3.7% (most 4) | 0.21 → 0.35 | 17.0% → 9.4% | 19.5k → 22.2k |
| Bunny, core | 0.9% → 0% | 0.57 → 0.59 | 3.8% → 3.5% | 14.8k → 14.3k |
| Bunny, Density 2 | 0.9% → 0% | 0.60 → 0.62 | 3.3% → 2.9% | 47.8k → 46.4k |
| Ogre, Organic | 1.3% (most 3) → 0.2% | 0.68 → 0.64 | 2.0% → 2.3% | 14.5k → 14.6k |

**`benchmark/stress_quads.py`** (CLI defaults, 10 models): 9 are byte-identical to 0.1.10. The armadillo is laid out again:

| Armadillo | 0.1.10 | This |
|---|---|---|
| p5 SJ | 0.54 | 0.60 |
| Inverted quads | 46 | 28 |
| Fans | 1 | 2 |
| Quads | 19.3k | 20.4k |
| Time | 12.1 s | 15.8 s |

**Renders.** The owner's exact Organic input gives a hand with separate fingers and grid quads; before, it was a few long slabs. The body is unchanged in character.

**Cost.** A second layout of the parts that need it: armadillo Organic trace 3.7 s → about 6 s (noisy machine). Other parts pay only the area check.

**Tests.**
- Unit tests of the score: a split that separates singularities but leaves a 7-sided piece is refused by the usual order and taken with crowding first; a new concave vertex is still refused; side counts still decide at equal crowding.
- Layout tests on bumpy icospheres: one where the second layout is kept, and one that is laid out once.

## Limits

- The hand keeps a few patches with 2–4 singularities (3.7% of the area).
- A heavily bumped, coarse sphere (bumps under-resolved by the mesh) stays crowded even with the second layout. Crowding there comes from the field, not from the acceptance order.
- Output differs from 0.1.10 only on parts above the threshold.
