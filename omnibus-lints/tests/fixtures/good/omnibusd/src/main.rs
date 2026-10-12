//! Green counterpart: the composition root wires crates and never names
//! capabilities; dotted non-capability strings (D-Bus names) stay legal.
pub fn bus_name() -> &'static str {
    "org.freedesktop.systemd1"
}
