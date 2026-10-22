# 0006 — Patch layout: the paper's tracing, with graded acceptance, guided loops and no deleted patches

**Date:** 2026-10-24
**Status:** Accepted

Stage 4 cuts the prep mesh into field-aligned patches whose borders contain every feature line (Pietroni et al., *Reliable Feature-Line Driven Quad-Remeshing*, SIGGRAPH 2021, §5; the oracle's `libs/xfield_tracer/tracing`). QuadLoom's implementation is `crates/quadloom-core/src/trace/`. The paper and the oracle were read in full before writing it. This record lists what it keeps, where it deliberately differs, and the evidence.

## What is kept from the paper and the oracle

- The mesh is cut open along features; each boundary vertex is classified by the field turn through each patch's fan (convex, flat, concave, narrow; the paper's Fig. 6).
- Paths are traced on a graph of four nodes per non-singular vertex, along mesh edges within 45° of the field, with 2-hop arcs that measure net drift (the paper's virtual connections) and zero-cost twin arcs that cross features at right angles only.
- Rounds: convexity joins (narrow and concave vertices emit until satisfied), loops from spread-out interior seeds, border to border; then sub-patch passes on failing patches with T-junctions on their borders, a clean-up that dissolves paths newest-first, and the oracle's final corner fix (`FixValences`).
- Validity: a disk, convex, with 3 to `max_sides` (5) sides. Quality: CC-ability of the side lengths and a valence match (the oracle's `match_valence`), with its tolerance (1 × the mean edge) and its `SkipValence4`.
- Refinement before tracing: edges at singular vertices and between two feature-line ends are split (`SplitAdjacentSingularities`).

## Where QuadLoom differs

1. **No deleted patches.** The oracle deletes non-disk patches when it saves, leaving holes (10 on the benchmark). QuadLoom never deletes. A recovery ladder always leaves a covering layout:
   - a relaxed sub-pass;
   - seams for non-disk patches (between loops, between merged loops, or across a pinch) and for non-convex ones;
   - re-declared corners for side counts out of range.

   Anything still invalid is counted in the report.
2. **Graded acceptance.** The oracle keeps a loop or border path whenever it splits a failing patch, and dissolves paths whose removal doesn't make the patch counts worse. On a field with high-index poles that let latitude loops turn annuli into annuli forever (14 loops on a UV sphere). QuadLoom keeps such a path only if the patches it splits are no worse on a lexicographic score that grades each condition instead of counting failing patches:
   - topological excess 1 − χ of the cut-open patch (+2 if closed), then the length of boundary glued to the same patch (so a torus band cut in two is progress), then 1 − χ with features glued back (pinches), then non-convex vertices, then missing or extra sides (Σ max(0, 3 − n) + max(0, n − max)), then the oracle's remaining terms (most singularities in a patch, matched singularities, CC failures).
   - The clean-up uses the same score, so merging two 2-sided patches back into a 0-sided one counts as a loss, not a gain.
3. **Self-glued patches are non-disk,** measured with features glued back (as the final mesh is), as the oracle's `AllowSelfGluedPatch = false` does.
4. **Labelled boundary edges.** Each cut edge carries the field direction it travels (a path's own direction; a feature's edge), like the oracle's edge-direction table, so a jagged path reads as straight both for corners and for side lengths. Where a field turn reads concave or narrow but the geometric angle is within 25° of a flatter class (near singular concave corners, where the field turns a quarter inside one fan), the geometry wins. Field-singular vertices on region borders use the geometric angle.
5. **Cusps are corners.** Features meeting at very acute angles can't be split by any path; like the oracle's convex corners they count as patch corners (the paper keeps such features even at the cost of very acute quads).
6. **CC-ability from Tarini's closed form** (*Closed-form Quadrangulation of n-Sided Patches*, 2021) for 3, 5 and 6 sides: margin 2·min sᵢ. The oracle's hexagon test checks only two of the six inequalities. Parity is left to quantization.
7. **Seams that make progress.** A seam (a cut that isn't field-aligned, the last resort) carries one field axis orthogonal to the boundary it leaves, so its ends are corners, and is kept only if it strictly improves the patch. Without both rules seams cut lenses into lenses (350 seams and 337 invalid patches on CesiumMilkTruck; with them, 5 seams and 3 invalid).
8. **Streamline-guided loops.** A loop is searched only within 3 edge rings of its seed's streamline, which is followed first; seeds whose streamline doesn't close give no loop. This costs about the loop's length instead of the surface's area: 327k faces lay out in 5.5 s instead of 76 s, with identical validity.
9. **Sub-patches reuse the same algorithm** on an extracted sub-mesh through an explicit work list (no recursion); loops are skipped in disk regions, where a loop that crosses no path only cuts out an annulus.
10. **Engineering:** parts and candidates are traced in parallel and chosen sequentially (deterministic); partitions update incrementally with per-half-edge cut flags; farthest-first uses lazy re-scoring.

## Evidence (`benchmark/reports/20261024-m5/layout_vs_oracle.md`)

QuadLoom's tracing on the oracle's own `_rem` mesh, field and features (isolating the tracing method). Both layouts are measured with QuadLoom's `layout-stats`: corners from the `.corners` files, straight side lengths, features glued back; invalid means non-disk, fewer than 3 or more than 5 corners.

| Over 60 cases | Oracle | QuadLoom |
|---|---|---|
| Cases with no invalid patch | 47 | **60** |
| Invalid patches, total | 47 (10 of them deleted, leaving holes) | **0** |
| Median patches | 283 | 316 (1.09×) |
| Median CC-able % | 99.9 | 99.3 |
| Median trace time | 1.62 s | **0.19 s** |

- QuadLoom's own report adds what `layout-stats` can't see: a median 87.3% of patches are fully Ok (valid, CC-able, singularity matched). 141 patches over all cases keep a concave vertex (usable, but their quads will be distorted there). The run used 439 seams and re-declared corners on 860 patches.
- **Through QuadLoom's own pipeline** (its repair, features, remesh and field) on the same 60 inputs: 59 cases with no invalid patch and 1 invalid patch in total; median 204 patches, 91.2% Ok; median trace 0.29 s.
- Examples: fandisk 27 patches (oracle 29), 0.08 s (0.41 s). Armadillo 1,088 patches (969), 0.38 s (10.3 s). CesiumMilkTruck 420 patches (428), 10 seams.

**Settings sweep** (`cost` × `drift`, the plan's Task 6): every anisotropic setting reached the fewest invalid patches; the oracle's angle penalty did worse (drift 10/30/100: 12/14/16 invalid, 425–513 seams). Anisotropic drift 3/5/10/20 gave Ok 85.4/86.9/87.3/87.9% with 385/378/403/449 seams. The default stays **anisotropic, drift 10**: the plan's tie-break (highest Ok %) would pick 20, but its gain is under a point, against 11% more seams, which are non-field-aligned cuts.

**How the gap closed.** The first comparison had 3,492 invalid patches and 3,987 seams. Each fix below was found by diagnosing failing models (the ignored test `diagnose_oracle_case`, with `QL_CASE=<_rem prefix>`):
- seams that make progress (decision 7)
- corner re-declaration (the oracle's `FixValences`; low- and high-sided patches are no longer seamed, which halved the seams)
- self-glued and pinched patches as non-disk, with the graded topology score, and a pinch seam that runs between the loop's two arcs at the pinch
- cusps as corners
- the geometric overrule at singular corners
- satisfied vertices refusing further paths
- sub-patch paths judged again in the parent
- a full loop search for closed parts with no closing streamline

## Consequences and open items

- The layout feeds quantization (M6): per-face patch ids, ordered corners, sides with field lengths, and paths.
- **M4 open item:** on UV spheres the field is the latitude field, with a +1 singularity (4 quarters) at each pole. No patch can match it; the layout degrades gracefully (no non-disk patches, the rest counted). Splitting high-index singularities belongs in the field stage.
- Concave vertices left after recovery (141 patches on the benchmark) are usable but distort the patch's quads; M8's mapping must tolerate them.
- Sub-patch paths are judged again in the parent region before they are merged.
- **Handle recovery (from the final review).** Recovery seams a non-disk patch between its boundary loops or at a pinch. A patch that keeps a handle with a single boundary loop has no recovery seam yet. On closed parts the loop rounds remove handles first (the genus-2 test `double_torus_layout_is_all_disks` gets all disks), and no benchmark case hit the gap. If one does, a seam along a non-contractible cycle of the patch (a homology generator) would close it.
