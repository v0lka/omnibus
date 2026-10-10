//! Fixed CoP counterpart: named structs only, no tuple types in signatures.
pub struct DemoRequest {
    pub x: i32,
    pub y: i32,
}
pub struct DemoPoint {
    pub x: i32,
    pub y: i32,
}
pub enum DemoError {
    Unavailable,
}
pub trait DemoLocator {
    fn point(&self, request: DemoRequest) -> Result<DemoPoint, DemoError>;
}
