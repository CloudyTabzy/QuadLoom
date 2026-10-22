//! Stage 3b: rectangular frames (decision 0007, after Corman & Crane's
//! *Rectangular Surface Parameterization*, 2025).
//!
//! The cross field becomes an exactly integrable rectangular frame: a rotation
//! θ per face plus log scale factors u (size) and v (stretch) per vertex. The
//! constraint is one equation per free interior edge (Cartan's first structure
//! equation, discretized over the dual edges), so the frame is integrable by
//! construction. Singularities are unchanged, because θ is a 0-form.
//!
//! Task 1 (this module so far): the frame mesh, the integrability residual F
//! and its exact derivatives. The solve and the per-part driver follow in
//! Task 2.

pub mod residual;
pub mod setup;
