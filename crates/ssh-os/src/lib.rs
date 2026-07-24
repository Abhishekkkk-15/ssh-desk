//! OS services: clipboard, drag-drop scaffolding, and open-with hints.

mod clipboard;
mod dragdrop;
mod mime;

pub use clipboard::{Clipboard, ClipboardError, FileEntry, FileLocation, FileOp};
pub use dragdrop::{DragPayload, DragSession, DropTarget};
pub use mime::{OpenAction, sniff_open_action};
