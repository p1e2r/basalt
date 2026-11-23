//! Minecraft Java Edition wire protocol primitives and framing.

pub mod buf;
pub mod conn;
pub mod text;

pub use buf::{Reader, Writer};
pub use conn::{Conn, Frame};
pub use basalt_data::{PROTOCOL_VERSION, VERSION_NAME};
pub use basalt_data::packets as ids;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State { Handshake, Status, Login, Configuration, Play }
