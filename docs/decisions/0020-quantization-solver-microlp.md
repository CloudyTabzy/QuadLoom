# 0020 — The quantization solver stays `microlp` (ripsolve evaluated and rejected)

**Status:** accepted 2026-10-23.

## Context

`microlp` 0.6.0 is used in exactly one place: the quantization integer program in `crates/quadloom-core/src/quad/ilp.rs`. `faer` handles the field and frame solves and is not a candidate for replacement by a MIP solver.

Quantization is the largest stage on organic models. On the armadillo at the 0.2.0-era working tree it took 11.2 s of a 16.4 s run, of which the global ILP was 11.05 s and the neighbourhood repairs 28 ms.

The owner found **ripsolve 0.4.0**, a pure-Rust branch-and-cut MIP solver, and asked whether it could benefit QuadLoom. This record is the evaluation.

## What was evaluated

ripsolve 0.4.0: MIT, pure Rust, no C/C++ build dependency, no `unsafe` in `src`. It offers a branch-and-cut MIP solver, a simplex LP engine with basis warm starts and reduced costs, presolve, compaction, cut separation (GMI, MIR, knapsack covers, cliques), primal heuristics (rounding, diving, feasibility pump, feasibility jump), symmetry detection, bound propagation, lazy constraints through a `Separator`, an LP/MPS reader, and a random instance generator.

Every public module was read. Every capability without a `microlp` equivalent was exercised on a QuadLoom model.

## Method

- A scratch crate outside the repository rebuilt the ILP exactly as `quad/ilp.rs::build` does: one integer count per segment, seven chorded deviation rows per count, the 4-sided even-total and balance rows, and the closed-form rows with one slack for 3-, 5- and 6-sided patches. It was deleted after the evaluation; this record keeps the numbers and the method.
- Two instance sets:
  - **Generated:** patches of 3–6 sides, sides of 1–3 segments, 25% of segments shared with a neighbouring patch, targets mostly in 0.3–6 with a few up to 14. Two sizes: 837–892 segments (~7k rows) and 3,750 segments (~31k rows).
  - **Real:** one armadillo layout: 1,319 segments, 562 patches, 4,851 columns, 12,091 rows. This is the model that costs ~11 s today.
- Both solvers received identical models. Every returned point was checked row by row (largest violation), for integrality, and for even patch totals.
- **Gap 0**, node limit 2,000, ripsolve `threads = 1`, both solvers in release builds.

**A measurement trap worth recording.** ripsolve's `Status::Optimal` means "optimal to within `gap_tolerance`", not "proven". At the default 1e-3 an apparent contradiction appeared — ripsolve "optimal" at an objective above a verified feasible point. At gap 0 the contradiction vanished and both solvers agreed exactly on every small model. Every number below is gap 0. Compare MIP solvers at gap 0 or not at all.

## Results

### Real armadillo layout (1,319 segments, 12,091 rows)

| Solver | Time | Result |
|---|---:|---|
| `microlp`, cold | **16.9 s** | feasible, objective 3090.56 |
| `microlp`, warm start | 15.4 s | feasible, objective 3711.22 |
| `ripsolve`, defaults | 55.7 s | **no feasible point found** (bound 3052.3) |
| `ripsolve`, no cuts | 47.8 s | **no feasible point found** |
| `ripsolve`, no presolve | 135.8 s | feasible, objective 3114.13 |

ripsolve found no incumbent because it has no MIP start and its primal heuristics fail on this model (`heur = 0`). `microlp` succeeds because it is seeded with the layout heuristic's counts.

### Generated instances (gap 0)

| Size | `microlp` | `ripsolve` | Quality |
|---|---:|---:|---|
| 837–892 segments, ~7k rows | 0.34–0.69 s | 0.64–1.21 s | same proven optimum in every run |
| 3,750 segments, ~31k rows | 3.2–5.1 s | 27–28 s | ripsolve proved 3177.684; microlp's best at the node limit was 3177.79 (0.1% worse) |

Peak working set on the 31k-row model: **`microlp` 123 MB, `ripsolve` 452 MB**.

### Capability map

| Capability | ripsolve | microlp | Measured on our model |
|---|---|---|---|
| MIP solve | `search::solve` | `solve_with` | slower, and no incumbent on the real model |
| MIP start | none | `warm_start` | decisive: it is why `microlp` finds a point |
| Incremental MIP | none | `resume`, `fix_var`, `unfix_var`, `add_constraint` | `microlp`-only; currently unused |
| LP engine | `lp::Lp`, warm basis, reduced costs, GMI | plain LP | same bound 2961.1074; `microlp` 0.7–1.2 s, ripsolve 2.1–2.4 s |
| Presolve | `presolve::presolve` | none | **0 columns fixed, 0 rows redundant, 0 coefficients tightened** |
| Compaction | `compact::compact` | none | nothing to remove |
| Primal heuristics | `round`, `dive`, `feasibility_pump`, … | internal | rounding infeasible; dive and pump found **nothing** |
| Cut separation | GMI, MIR, covers, cliques | none | **27 valid GMI cuts; bound 2961.1 → 3039.4** |
| Symmetry | `symmetry::detect` | none | not applicable: segment targets differ |
| Propagation / conflicts | `propagate::*` | none | same no-op as presolve |
| Lazy constraints | `search::solve_with(Separator)` | none | unique; no current use |
| Model I/O | LP/MPS reader, CLI `relax` / `solve` | none | useful offline |
| Instance generator | `generate::*` | none | development only |
| Parallel tree search | `threads` | none | conflicts with byte-identical output |

## Decision

**`microlp` stays. ripsolve is not added as a dependency.**

ripsolve remains usable as an *offline* oracle: dump a quantizer ILP from the diagnostic tools, then use `ripsolve relax` for an independent bound and `ripsolve solve --values` for an independent optimum. Nothing in the build or the add-on depends on it.

## Why

1. **No MIP start.** `search::Options` has no incumbent input. QuadLoom's design is to seed the program with the layout heuristic's counts and keep the result only if it improves on them. ripsolve starts cold, and on the real model it never found a feasible point at all.
2. **Wrong operating point.** ripsolve targets models up to roughly a thousand rows; the real model is 12,091. On the 31k-row instance its search cost about 0.47 s per node including root cuts, against about 1.6 ms per node for `microlp` — two orders of magnitude — so even finishing in 58 nodes against `microlp`'s 2,000-node limit did not make up the difference.
3. **We need incumbents, not proofs.** QuadLoom's acceptance test only asks whether a candidate beats the heuristic. ripsolve spends its budget on cuts and presolve to prove optimality, which the quad stage cannot use.
4. **Its distinctive components do nothing here.** Presolve, compaction and propagation are binary-oriented and reduce nothing on a general-integer model. Its primal heuristics fail. Symmetry does not apply. That leaves cuts, below.
5. **Memory.** 452 MB against 123 MB on the same model, in an add-on whose whole 1M-triangle run peaks at about 291 MB.
6. **Dependency weight.** The library pulls `lp_parser_rs` (and `lalrpop` as a build dependency) for an LP/MPS reader a programmatic caller never uses.

### The one genuine advantage, and why it still loses

Root **GMI cut separation** is the only capability `microlp` has no answer for. On the real model, 27 cuts lifted the LP bound from 2961.1074 to 3039.3729 — 60.5% of the gap to the best integer point. The cuts are sound: 0 of 27 were violated by a verified feasible point.

Handed to `microlp`, those cuts produced a better answer — 3075.82 against 3090.56, 0.5% better — in 21.2 s against 16.9 s, plus 2.1 s to generate them with ripsolve's LP. That is **+38% time for +0.5% objective**, and the objective is not the product metric; quad quality is.

If quantization ever needs another push, a small in-house root cut loop is the cheaper path. It should be judged on quad quality, not on the objective value.

## What would reopen this

- ripsolve gaining a MIP start (`search::Options` incumbent), which would change the incumbent question entirely.
- A future stage dominated by **binary** models, where its presolve, covers and symmetry actually fire.
- A requirement for **proven** optimality rather than a good incumbent.

Larger models are the wrong direction for it: M6's BiMDF quantization will be bigger than today's, and ripsolve's weakness grows with size.

## Lessons

- **`microlp`'s warm start is advisory, not a guarantee.** It fixes every hinted column, reoptimizes, adopts the point as an incumbent if it is integral and feasible, then restores the root. A bad hint is ignored rather than fatal. QuadLoom's "never worse" property comes from the acceptance test in `quantize`, not from the solver.
- **An LP point is not a MIP start.** The real model's LP leaves only 17 of 1,319 counts fractional, which looks like a ready-made start. Snapping it to integers violates rows by 2.23; fixing the 1,302 counts that are integral at the LP makes the reduced program infeasible in 0.19 s. Near-integrality is an artifact of a flat objective, not a solution.
- **The ILP is essential, not a refinement.** On the real model the heuristic counts cost 54,384 by the ILP's own objective (continuous completion), against 3,090 for the solver. The quantizer's least-squares relaxation does not model the 3-, 5- and 6-sided closed forms, and the integer program is what pays for them. (One model; recorded as an indication, not a law.)
- **Compare at gap 0.** A default `mip_gap` of 1e-3 turns a 0.015% difference into an apparent "optimal" contradiction. The apparent bug was the tolerance.
- **Both solvers are deterministic at one thread.** Repeated runs gave identical objectives; no hash-order effects were seen.
