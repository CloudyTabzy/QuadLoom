# 0005 — Cross field: a dual complex power field instead of a polyvector/MIQ port

**Date:** 2026-10-23
**Status:** Accepted

Stage 3 computes a per-face 4-RoSy cross field on the prep mesh; the tracing stage will follow it.

The oracle uses VCG's `FieldSmoother` with `igl::n_polyvector`, the NPoly path (`MeshFieldSmoother::SmoothField`):
- Hard constraints on faces beside feature edges and borders.
- When features are sparse (length ≤ 6·√area), extra hard constraints on faces whose curvature anisotropy exceeds 0.8, aligned to the principal direction from a 3-ring fit.

QuadLoom's implementation is in `crates/quadloom-core/src/field/`.

## Decisions

1. **Representation: Knöppel, Crane, Pinkall & Schröder, *Globally Optimal Direction Fields* (SIGGRAPH 2013), in dual per-face form.**
   - Each face stores z = e^{4iθ} in its own frame, so a cross's four directions share one value.
   - Smoothness is Σ w_e |z_g − r_fg·z_f|² over shared edges. The transport r_fg = e^{4i(φ_g − φ_f)} keeps the cross's angle to the shared edge, a discrete Levi-Civita connection.
   - The Hermitian system is solved as its real symmetric 2n×2n form with faer's sparse Cholesky. faer is built without its `rayon` feature, so the solve is sequential and deterministic; parts already run in parallel.
   - With nothing to align to, the smoothest field is the smallest generalized eigenvector (A·x = λ·M·x, M the area mass matrix), found by inverse iteration (at most 60 steps, stopping once the eigenvalue settles) from a fixed pseudo-random start.
     - A constant start is itself a structured field: on a UV sphere it's the latitude field with +1 poles, which iteration was slow to leave.
     - From the pseudo-random start, a sphere gets the expected eight +¼ singularities.
2. **Edge weights w_e = |e|²/(A_f + A_g)** (dual Laplacian). Uniform weights, as in `n_polyvector`, measured no better (Lantern 210 vs 193) and are more tessellation-dependent.
3. **Hard constraints, matching the oracle:** every face beside a feature or border edge is constrained.
   - A face with several constrained edges follows the longest exactly (ties: corner order). Only edges whose crosses agree within 1° are averaged, which includes right angles.
   - An earlier version averaged all of them, or left the face free where they conflicted. The M4 review showed that pinned faces up to 15° off their features, which tracing can't use.
4. **Curvature alignment as a soft term, from the normal cycle** (Knöppel §6.1.2, after Cohen-Steiner & Morvan 2003).
   - Per face, q̃ = Σ θ_e·|e|·e^{2iφ_e} over its edges, with θ_e the signed dihedral. Its square is the cross target.
   - The weight is |q̃| (∝ (κ₁ − κ₂)²) over its 95th percentile, so flat and umbilic regions are left to smoothness.
   - The oracle's sufficient-features rule decides Auto.
   - The term is area-weighted, like Knöppel's ∫|z − q|² dA, and scaled to 10,000 reference faces. Like the smoothness term, its total is then independent of resolution. Without the area weight, a bumpy ellipsoid went from 8 to 32 to 56 singularities at 6k, 25k and 100k faces.
   - **Strength α = 0.02, not 0.3.**
     - 0.3 is the oracle's `alpha_curv`, but that applies to its MIQ path. Its default NPoly path hard-constrains only the most anisotropic faces.
     - At 0.3 our organic models had 3–4× the oracle's singularities (max-planck 159 vs 36). At 0.02 they're on par: after the area weighting, max-planck 39 vs 36 and igea 127 vs 134.
     - A small α still aligns fully wherever curvature is consistent, because smoothness costs nothing there (cylinder test).
5. **Singularity index:** a vertex's index in quarters is (Σω + 4K_v)/2π.
   - ω is the field's turn across each shared edge relative to transport; K_v is the angle defect.
   - ω is computed once per dual edge and negated by traversal direction. Computed once per vertex loop instead, a turn of exactly π (the cross turning 45°) came out +π twice and broke Poincaré–Hopf on a tetrahedron.
   - The total is exactly 4χ by construction (tests: sphere 8, torus 0).
6. **Recovery and control:**
   - A part whose solve fails gets its constraints propagated breadth-first. This is reported as `fallback_parts`; none occurred on the benchmark.
   - Edges touching a degenerate face are left out of the energy.
   - Cancel is checked after assembly, after factorization and at every iteration, and each finished part advances the progress bar.

## Evidence (`benchmark/reports/20261023-m4/field_vs_oracle.md`)

All fields are measured with QuadLoom's index routine (`quadloom field-index mesh.obj field.rosy`).

| Median over 60 cases | Oracle | QuadLoom, same mesh | QuadLoom, own pipeline |
|---|---|---|---|
| Singularities | 100 | 94 | 89 |
| Per 1k faces | 5.70 | 5.87 | 5.07 |
| Field time | — | 37 ms | 56 ms |

- **Same mesh** means our field on the oracle's prep mesh, with our remesh and welding off. It isolates the field method. The per-case ratio has median 1.00 and mean 0.99, and we're at or below the oracle on 36 of 60 cases (after the M4 review fixes).
- **fandisk:** 32 singularities for both, +20/−12 quarters for both.
- **Welding confounds counts.** The oracle's prep mesh keeps UV-seam splits as open borders, and border vertices carry no index. `khronos/CesiumMilkTruck`, for example, is 773 parts and 4,231 border edges unwelded, against 20 parts welded. On it our pipeline counts 304 singularities against the oracle's 38. Measured on the same unwelded mesh the counts are 49 and 38.
- **Performance:** 201k featureless faces (the inverse-iteration path) take 3.0 s.

## Consequences

The field matches QuadWild's singularity statistics without a mixed-integer solver or an n-polyvector port, and in tens of milliseconds. Tracing (M5) consumes `RunOutput::field`.

Open items:
- A coarse torus gets ± singularity pairs, because the relaxed |z| makes pairs cheap. Knöppel's s ≠ 0 energies (§4.2) address this if tracing needs it.
- Singularities at feature junctions are located one vertex differently from the oracle (on the corner rather than beside it). Tracing will show whether that matters.
