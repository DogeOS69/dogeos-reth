/// Configuration for the inherited `scroll/1` protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollWireConfig {
    connect_unsupported_peer: bool,
}

impl ScrollWireConfig {
    pub const fn new(connect_unsupported_peer: bool) -> Self {
        Self {
            connect_unsupported_peer,
        }
    }

    pub const fn connect_unsupported_peer(&self) -> bool {
        self.connect_unsupported_peer
    }
}
