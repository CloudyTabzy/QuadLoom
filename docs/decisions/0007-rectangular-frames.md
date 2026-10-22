# 0007 — Rectangular frames: an integrable frame stage between the field and tracing

**Status:** accepted 2026-10-24. The default (on or off) is settled by the M6a benchmark; see Evidence.

## Context

QuadWild traces a cross field that is generally not integrable, meaning no map has it as its gradient frame. Patch sides traced along such a field don't fit together: the two opposite sides of a 4-sided patch have different lengths. The quantizer (M6b) must then trade regularity against side targets everywhere, and the final quads (M7/M8) shear.

Corman & Crane, *Rectangular Surface Parameterization* (TOG 2025, "RSP"), make a frame field **exactly integrable and rectangular** (orthogonal axes, unequal lengths allowed). They solve for three things:
- a rotation θ per face of the given frame;
- log scale factors u (size) and v (stretch) per vertex.

The constraint is per-edge integrability (Cartan's first structure equation, their Eq. 10), while an objective encodes what "good" means:
- area preservation (Σ A u²),
- angle preservation (Σ A v²),
- isometry (both),
- alignment (Σ A θ²),
- a target aspect ratio from curvature.

The owner asked for this to be adapted into QuadLoom, with the choices exposed in the N-panel.

## Decision

Add **stage 3b, "frame"** (`crates/quadloom-core/src/frame/`), between the cross field (stage 3) and tracing (stage 4). It's off by default until the benchmark says otherwise. Per connected part, in parallel:

1. **Reference frame.** X⁰ is our cross field. The transport angle β per interior edge is the angle from X⁰_f, unfolded across the edge, to the nearest of X⁰_g's four directions. The quarter-turn count k gives a sign bit s = (−1)^k for v, and corner signs come from the fan around each vertex.
2. **Unknowns:**
   - u at every vertex;
   - v at every vertex except odd-index singularities, where it's fixed at 0, because the frame axes swap going around them;
   - θ on every face that isn't held. Held faces are those touching a feature or border edge, and degenerate faces; there θ = 0, so the field's feature alignment is kept exactly.
3. **Constraint**, one per interior non-feature edge e = (i, j), with face f containing i→j and face g containing j→i:

   F_e = ρ_f(i→j) − ρ_g(j→i) + (θ_g − θ_f) + β_e = 0

   For halfedge p→q of a counter-clockwise triangle (p, q, r) with corner angles α:

   ρ(p→q) = ½ cot α_r · [ −(u_q − u_p) + cos 2η · (v'_q − v'_p) + sin 2η · (cot α_p (v'_r − v'_q) + cot α_q (v'_r − v'_p)) ]

   - η is the angle of the rotated axis X1 = R_θ X⁰ from the halfedge's direction (counter-clockwise).
   - v' are corner values, the vertex v times the corner sign.
4. **Objective** (on corners, scaled to unit total area per part):

   Φ = Σ_f A_f/3 Σ_corners [ w_u u² + w_v (v' − t_f)² ] + ε Σ_f Σ_halfedges ½cot α_r (v'_q − v'_p)² + w_θ Σ_f A_f θ_f²

   - w_u = 1 − balance and w_v = balance + stretch_weight·[curvature].
   - t_f is the curvature aspect target (point 6), blended in so that one quadratic covers both terms.
5. **Solve.** Newton on the KKT system [[H + Σλ∇²F, Jᵀ], [J, −δI]], with a sparse LU from faer and backtracking on the KKT residual norm. Iteration starts at u = v = θ = 0.
   - Converged: |F|∞ < 1e-8.
   - Accepted: |F|∞ ≤ 1e-6.
   - Otherwise the part falls back to the plain field, which is reported, never an error.
6. **Curvature stretch (optional).**
   - A smoothed per-face curvature tensor (normal cycle: dihedral × length × ê êᵀ, averaged at vertices and smoothed) gives normal curvatures κ₁, κ₂ along X⁰'s two axes.
   - Target: t_f = ½ ln((|κ₁| + c)/(|κ₂| + c)), clamped to ±½ ln(max_stretch).
   - So quads get short across high curvature and long along low curvature: a tube gets long quads along its length.
7. **Outputs:**
   - rotated directions (what tracing follows);
   - θ, u, v and corner signs;
   - per-face scales [a, b] = [e^{ū+v̄}, e^{ū−v̄}], normalized so the parametric area equals the surface area. These are quads per unit length along X1 and X2; quantization (M6b) uses them as side targets.
   - The singularities are unchanged by construction: θ is a 0-form, so the index of every vertex is preserved.

## Where QuadLoom differs from the paper and its MATLAB code

- **Own discretization, verified.** ρ above is the exact integral of Eq. 10's right-hand side over each half dual edge (circumcenter to midpoint), for piecewise-linear u, v. The pairing of cotangents printed in the paper (Eq. 17) does not match that integral; the MATLAB code does, and so does ours. The check (`.superpowers/tools/rsp/`):
  - a manufactured rectangular map gives second-order convergence (rms 8.8e-4 → 2.4e-4 → 6.9e-5 under refinement);
  - the conformal map e^z with θ sampled at circumcenters gives a residual of 1e-15;
  - the analytic Jacobian and Hessian match finite differences to 1e-10.
- **Odd singularities pin v = 0** instead of leaving one inconsistent edge per such vertex, as the MATLAB's corner reduction does.
- **Cone one-rings carry no integrability constraint.** A cone is a field singularity or an odd vertex. No edge touching a cone is free, and every face with a cone corner is held at θ = 0. The frame is singular there anyway, and the discrete equations in a clustered cone's one-ring are ill-posed. A bounded stall stop (5 steps, 1% merit drop) also ends a solve that cannot converge, so it falls back quickly.
- **Regularized KKT** (−δI, δ = 1e-10) instead of a `quadprog` fallback. It covers the one dependent constraint per closed featureless part, and the free gauges (the u constant when w_u = 0, a global θ rotation).
- **Not taken:**
  - automatic cone placement (our field already places singularities);
  - Chebyshev nets;
  - the Poisson map recovery and libQEx extraction. QuadLoom stays a patch pipeline, and the frame feeds tracing and quantization.
  - The AGPL code was read for conventions only; nothing is copied.

## Panel

A "Quad Shape" sub-panel: Rectangular Quads (on/off), Keep Square (balance 0–1), Stretch Along Curvature with Max Stretch and Strength, Follow Field (w_θ), and under Advanced, Smoothness (ε) and Iterations. The output carries face attributes `quadloom_size` (e^{−ū}, relative quad size) and `quadloom_stretch` (e^{2|v̄|}, aspect ratio) as a preview. The quads themselves arrive with M6b–M7.

## Evidence

To be filled by M6a Task 7: the benchmark of plain vs rectangular frames on the 60 oracle inputs. It covers layout validity, opposite-side mismatch of 4-sided patches (in the frame's metric), convergence and time.

Prototype numbers so far:
- **fandisk** (5.2k vertices, 32 singularities): converges in 8 Newton steps. The frame rotates 1.9° median and 24° at most. Sizes span 0.70–1.91× (p5–p95), aspect 1.5 median and 3.6 at p95. Features plus fixed singularities force some stretch, whichever objective is chosen.
