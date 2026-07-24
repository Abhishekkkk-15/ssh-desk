//! Background transfer queue for SFTP upload/download.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(pub Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl TransferDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Upload => "↑ upload",
            Self::Download => "↓ download",
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            Self::Upload => "→",
            Self::Download => "←",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TransferStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled)
    }

    pub fn can_cancel(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// Snapshot of a transfer for the UI.
#[derive(Debug, Clone)]
pub struct TransferJob {
    pub id: TransferId,
    pub host_id: String,
    pub direction: TransferDirection,
    pub local_path: PathBuf,
    pub remote_path: PathBuf,
    pub status: TransferStatus,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub error: Option<String>,
    pub bytes_per_sec: f64,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) last_tick: Instant,
    pub(crate) last_bytes: u64,
}

impl TransferJob {
    pub fn new(
        host_id: impl Into<String>,
        direction: TransferDirection,
        local_path: PathBuf,
        remote_path: PathBuf,
        bytes_total: Option<u64>,
    ) -> Self {
        Self {
            id: TransferId::new(),
            host_id: host_id.into(),
            direction,
            local_path,
            remote_path,
            status: TransferStatus::Queued,
            bytes_done: 0,
            bytes_total,
            error: None,
            bytes_per_sec: 0.0,
            cancel: Arc::new(AtomicBool::new(false)),
            last_tick: Instant::now(),
            last_bytes: 0,
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn progress_pct(&self) -> Option<f64> {
        let total = self.bytes_total.filter(|t| *t > 0)?;
        Some((self.bytes_done as f64 / total as f64) * 100.0)
    }

    pub fn display_name(&self) -> String {
        match self.direction {
            TransferDirection::Upload => self
                .local_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.local_path.display().to_string()),
            TransferDirection::Download => self
                .remote_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.remote_path.display().to_string()),
        }
    }

    pub fn summary_line(&self) -> String {
        let pct = self
            .progress_pct()
            .map(|p| format!("{p:5.1}%"))
            .unwrap_or_else(|| "  —  ".into());
        let speed = if self.status == TransferStatus::Running && self.bytes_per_sec > 0.0 {
            format!(" {}", format_rate(self.bytes_per_sec))
        } else {
            String::new()
        };
        format!(
            "{} {}  {}  {}{}{}",
            self.direction.label(),
            self.display_name(),
            pct,
            self.status.label(),
            speed,
            self.error
                .as_ref()
                .map(|e| format!(" · {e}"))
                .unwrap_or_default()
        )
    }

    pub(crate) fn note_progress(&mut self, bytes_done: u64) {
        self.bytes_done = bytes_done;
        let elapsed = self.last_tick.elapsed();
        if elapsed >= Duration::from_millis(200) {
            let delta = bytes_done.saturating_sub(self.last_bytes) as f64;
            let secs = elapsed.as_secs_f64().max(0.001);
            self.bytes_per_sec = delta / secs;
            self.last_tick = Instant::now();
            self.last_bytes = bytes_done;
        }
        self.status = TransferStatus::Running;
    }
}

pub fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.2} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{n:.0} B")
    }
}

pub fn format_rate(bps: f64) -> String {
    format!("{}/s", format_bytes(bps as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_pct() {
        let mut job = TransferJob::new(
            "h",
            TransferDirection::Upload,
            PathBuf::from("/tmp/a"),
            PathBuf::from("/remote/a"),
            Some(100),
        );
        job.bytes_done = 50;
        assert!((job.progress_pct().unwrap() - 50.0).abs() < f64::EPSILON);
    }
}
