# 0022 — Global approximate target quad count

**Status:** implemented and verified for 0.4.0, 2026-10-24.

The owner reserves 0.3.0 for OOM hardening and assigns target quad count to 0.4.0,
requesting research and crucial accuracy/quality tests.

## Decision

Density remains the default. Target Quads requests the total output over all
parts and actual symmetry copies, preserving features, adaptive relative sizing
and Quad Shape. Presets retain the selected sizing mode and requested count.
The result shows requested and achieved counts; a miss above max(4, 5%) warns.
This reporting tolerance is not a universal face-count guarantee.

The core owns the feature (`quad.target_quads`, 0 = Density). API 9 carries the
new setting and report metadata. Quick Re-run reuses the existing upstream cache;
the mesh, field and patch layout are not recomputed just to change N.

1. Prepare shared segments once. Sum metric area `area*sx*sy/local_size²` on
   usable patches. Subtract fixed fallback quads from the variable budget and
   account for `2^actual_symmetry_planes`.
2. Try at most eight global edge sizes, quantizing shared segments and measuring
   actual planned pattern counts. Use bounded feedback/bracketing and retain the
   nearest valid plan. Positive integer counts, parity and regularity make the
   response discontinuous; simple binary search or area alone is insufficient.
3. If needed, refine counts along shared chord/single-segment proposals. Recheck
   full side sums, even boundaries, fixed counts, existing size floors, a 2x ideal
   size envelope, and no degradation of affected pattern classes. Accept only
   strictly closer total counts and regenerate proposals. Bounds: 8,192 cheap
   visits, 512 pattern probes, 64 accepts, 1,024 rebuilds, 2M cumulative rebuilt
   quads. Rejected moves roll back atomically; cancellation propagates.
   Nearest above/below starts share one aggregate budget; the closest start gets
   at most half when an alternative exists. Statistics include discarded work.
4. Map, assemble and smooth once. Report the actual final mirrored mesh count,
   even when mapping fallback differs from the plan. Density bypasses targeting.

Target-mode split patterns solve positivity/parity intervals for the existing
closed form's chord length instead of enumerating every chord length. The
candidate pair search remains bounded at 250k checks. Tests compare analytic
feasibility/choice with exhaustive small cases. Normal Density pattern ordering
is unchanged.

Target requests and output are capped at 2M quads. Checked perimeter/grid/count
arithmetic and bounded pattern work precede large allocations. OOM isolation,
host admission checks and original-object safety from 0.3.0 remain.

## Why this approach

Primary sources and derivations are summarized below.
Instant Meshes/QuadriFlow initialize quad size from area/N; Blender calls the
goal approximate and accounts for symmetry. QuadWild/Bi-MDF preserve constrained
shared integer subdivisions. The 2026 target-face-number research motivates
preserving relative metric sizes, but its quantization method is not adopted.

Our first real-mesh sweep confirmed practical failures hidden by primitive tests:
fixed fallback contributions slowed CAD correction, a split search hit its work
cap, and bunny counts jumped from roughly 3,900 to 1,500 over a small size change.
The bounded chord refinement addresses discrete gaps without post-processing
the result or weakening feature boundaries. A regression also prevents cheap
invalid moves from starving later pattern probes.

## Validation and limits

The full gate passed: 321/318 Rust tests (all-features/default; 6 ignored each),
both clippy configurations, 117 Python tests and 49 Blender scenarios. The exact
production ZIP passed version/fault-disable checks and nine smoke scenarios.
Primitive native tests include high targets above input resolution, explicit
symmetry axes, unequal parts, scale invariance, adaptive/frame sizing, cache/full
equality, strict types, cancellation, disabled stages and infeasible low counts.

Coarse requests may conflict with fixed patches, feature-preserving layout or
acceptable pattern classes. The output remains a constrained approximation and
warns honestly. Low-resolution quality issues in the existing engine are measured
alongside count accuracy; count matching does not justify hiding them. Linux/macOS
packaging and further layout simplification remain outside this release.

The [six-model sweep](../../benchmark/reports/2026-10-24-target-count.md) found
unchanged Density geometry/feature bytes on every model. All nine synthetic
requests and all six 8,000 requests met tolerance. Overall 13/18 met max(4, 5%);
median error was 2.8%, with five coarse constrained misses (worst 232%). The report
also records poor coarse geometric quality and substantial solver time; neither
exact counts nor unchanged speed is claimed for Target mode.

Fresh review found two Important boundary issues. Both received RED→GREEN
regressions and the complete gate: count limits now guard successful fills after
earlier mapping fallback, and above/below refinement starts share aggregate work
counters instead of resetting their budgets. Deferred minors are inherited
Density wording in some fallback hints and phase-boundary cancellation checks
inside direct native grid/center construction (Blender can terminate the worker).
