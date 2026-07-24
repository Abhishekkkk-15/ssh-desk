//! OS services: clipboard, drag-drop, paste/drop path parsing, open-with.

mod clipboard;
mod dragdrop;
mod mime;
mod ospaste;

pub use clipboard::{Clipboard, ClipboardError, FileEntry, FileLocation, FileOp};
pub use dragdrop::{DragPayload, DragSession, DropTarget, OsDropOffer};
pub use mime::{OpenAction, sniff_open_action};
pub use ospaste::{
    classify_paste, describe_upload, existing_files, parse_os_paths, PasteKind,
};
