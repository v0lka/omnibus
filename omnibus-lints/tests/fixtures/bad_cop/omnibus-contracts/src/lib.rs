//! Violation: CoP — tuple types in a trait signature.
pub trait DemoPoint {
    fn point(&self, p: (i32, i32))
        -> (i32, i32);
}
