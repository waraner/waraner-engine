//! Engine-wide compile/runtime configuration constants.

/// Selects which physics backend the engine uses at runtime.
///
/// `true`  -> noble physics (`NoblePhysics`)
/// `false` -> box3d physics (`Box3DPhysics`)
///
/// This is a plain `bool` flag (not a CLI argument) so the backend can be
/// switched by editing this single value.
pub const USE_NOBLE_PHYSICS: bool = false;
