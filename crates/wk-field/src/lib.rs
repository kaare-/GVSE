//! Scalar/vector field primitives for the World Kernel.
//!
//! Fields are chunk-local patches with edge halos. Subsystems compose
//! stencil ops and solvers from this crate; they should not re-implement
//! index arithmetic inline.
//!
//! Stage 6.1: data types + stencil/solver skeletons. No simulation
//! subsystems write fields yet — those land in 6.2+.

pub mod patch;
pub mod solvers;
pub mod stencil;

pub use patch::{FieldHalo, FieldPatch};
pub use solvers::{explicit_diffusion, semi_lagrangian_advect};
pub use stencil::{divergence, gradient, laplacian_5point};
