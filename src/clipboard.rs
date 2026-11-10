use std::os::fd::{AsFd, BorrowedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, info, warn};
use wl_clipboard_rs::copy::{MimeSource, MimeType as CopyMimeType, Options, Source};
use wl_clipboard_rs::paste::{
    ClipboardType as InternalClipboardType, Error as ClipboardError, Seat,
};
use wl_clipboard_rs::watch::{ClipboardEvent as WatcherEvent, ClipboardType, Offer, Watcher};

use crate::domain::{
    CaptureReject, CapturedPayload, MAX_CAPTURE_BYTES, Representation, build_payload, is_text_mime,
    is_text_plain_mime,
};

const SELF_WRITE_SUPPRESSION_TTL: Duration = Duration::from_secs(5);
const MIME_READ_TIMEOUT: Duration = Duration::from_secs(2);
const READ_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardSource {
    Regular,
    Primary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedEvent {
    pub source: ClipboardSource,
    pub payload: CapturedPayload,
}

#[derive(Clone)]
pub struct ClipboardService {
    inner: Arc<Inner>,
}

struct Inner {
    primary_enabled: Arc<AtomicBool>,
    primary_supported: Arc<AtomicBool>,
    suppression: Arc<Mutex<Option<SelfWriteSuppression>>>,
    last_regular_payload: Arc<Mutex<Option<CapturedPayload>>>,
    suppress_clear_reown: Arc<AtomicBool>,
    _cancel: Mutex<Option<wl_clipboard_rs::watch::CancelHandle>>,
}

struct SelfWriteSuppression {
    fingerprint: [u8; 32],
    expires_at: Instant,
}

#[derive(Debug, Error)]
enum ReadOfferError {
    #[error("text/plain could not be read: {0}")]
    PlainRead(String),
    #[error(transparent)]
    Rejected(#[from] CaptureReject),
}

impl ClipboardService {
    pub fn start() -> Result<(Self, UnboundedReceiver<CapturedEvent>)> {
        let primary_supported = Arc::new(AtomicBool::new(true));
        let watcher = match Watcher::new(ClipboardType::Both, Seat::Unspecified) {
            Ok(watcher) => watcher,
            Err(ClipboardError::PrimarySelectionUnsupported) => {
                info!("primary selection is unsupported; watching regular clipboard only");
                primary_supported.store(false, Ordering::Relaxed);
                Watcher::new(ClipboardType::Regular, Seat::Unspecified)?
            }
            Err(error) => {
                return Err(error).context("failed to start Wayland data-control watcher");
            }
        };

        let primary_enabled = Arc::new(AtomicBool::new(primary_supported.load(Ordering::Relaxed)));
        let suppression = Arc::new(Mutex::new(None));
        let last_regular_payload = Arc::new(Mutex::new(None));
        let suppress_clear_reown = Arc::new(AtomicBool::new(false));
        let cancel = watcher.cancel_handle();
        let (event_tx, event_rx) = unbounded_channel();

        let watch_primary_enabled = Arc::clone(&primary_enabled);
        let watch_suppression = Arc::clone(&suppression);
        let watch_last_regular_payload = Arc::clone(&last_regular_payload);
        let watch_suppress_clear_reown = Arc::clone(&suppress_clear_reown);
        let watch_cancel = cancel.clone();
        thread::Builder::new()
            .name("clipman-clipboard".to_owned())
            .spawn(move || {
                if let Err(error) = watch_clipboard(
                    watcher,
                    event_tx,
                    watch_primary_enabled,
                    watch_suppression,
                    watch_last_regular_payload,
                    watch_suppress_clear_reown,
                    watch_cancel,
                ) {
                    warn!(%error, "Wayland clipboard watcher stopped");
                }
            })
            .context("failed to spawn clipboard watcher thread")?;

        Ok((
            Self {
                inner: Arc::new(Inner {
                    primary_enabled,
                    primary_supported,
                    suppression,
                    last_regular_payload,
                    suppress_clear_reown,
                    _cancel: Mutex::new(Some(cancel)),
                }),
            },
            event_rx,
        ))
    }

    pub fn primary_supported(&self) -> bool {
        self.inner.primary_supported.load(Ordering::Relaxed)
    }

    pub fn primary_enabled(&self) -> bool {
        self.inner.primary_enabled.load(Ordering::Relaxed)
    }

    pub fn set_primary_enabled(&self, enabled: bool) {
        if self.primary_supported() {
            self.inner.primary_enabled.store(enabled, Ordering::Relaxed);
        }
    }

    pub fn restore(&self, payload: &CapturedPayload) -> Result<()> {
        write_clipboard(payload, &self.inner.suppression)
    }

    pub fn clear_regular(&self) -> Result<()> {
        *self
            .inner
            .last_regular_payload
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.inner
            .suppress_clear_reown
            .store(true, Ordering::SeqCst);
        if let Err(error) = wl_clipboard_rs::copy::clear(
            wl_clipboard_rs::copy::ClipboardType::Regular,
            wl_clipboard_rs::copy::Seat::All,
        ) {
            self.inner
                .suppress_clear_reown
                .store(false, Ordering::SeqCst);
            return Err(error).context("failed to clear Wayland clipboard");
        }
        Ok(())
    }
}

fn watch_clipboard(
    mut watcher: Watcher,
    event_tx: UnboundedSender<CapturedEvent>,
    primary_enabled: Arc<AtomicBool>,
    suppression: Arc<Mutex<Option<SelfWriteSuppression>>>,
    last_regular_payload: Arc<Mutex<Option<CapturedPayload>>>,
    suppress_clear_reown: Arc<AtomicBool>,
    cancel: wl_clipboard_rs::watch::CancelHandle,
) -> Result<()> {
    let mut regular_initial_event_skipped = false;
    let mut primary_initial_event_skipped = false;

    while let Some(event) = watcher.next_event()? {
        match event {
            WatcherEvent::Changed {
                clipboard,
                mime_types,
                mut offer,
            } => {
                let Some(source) = clipboard_source(clipboard) else {
                    continue;
                };
                debug!(?source, ?mime_types, "clipboard selection changed");
                if !mark_seen(
                    source,
                    &mut regular_initial_event_skipped,
                    &mut primary_initial_event_skipped,
                ) {
                    debug!(?source, "skipping clipboard state present at startup");
                    continue;
                }
                if source == ClipboardSource::Primary && !primary_enabled.load(Ordering::Relaxed) {
                    continue;
                }

                match read_offer(&mut offer, &mime_types) {
                    Ok(payload) => {
                        if self_write_matches(&suppression, &payload.fingerprint) {
                            debug!(?source, "ignoring clipboard change caused by this process");
                            continue;
                        }
                        if source == ClipboardSource::Regular {
                            *last_regular_payload
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(payload.clone());
                        }
                        debug!(
                            ?source,
                            mime_count = payload.representations.len(),
                            text_length = payload.plain_text.chars().count(),
                            "read clipboard payload"
                        );
                        if event_tx.send(CapturedEvent { source, payload }).is_err() {
                            cancel.cancel();
                            break;
                        }
                    }
                    Err(ReadOfferError::Rejected(reason)) => {
                        debug!(?source, ?reason, "clipboard payload was not recorded");
                    }
                    Err(error) => {
                        warn!(?source, %error, "failed to read clipboard payload");
                    }
                }
            }
            WatcherEvent::Cleared { clipboard } => {
                if let Some(source) = clipboard_source(clipboard) {
                    let was_seen = mark_seen(
                        source,
                        &mut regular_initial_event_skipped,
                        &mut primary_initial_event_skipped,
                    );
                    debug!(?source, "clipboard was cleared");
                    if was_seen
                        && source == ClipboardSource::Regular
                        && !suppress_clear_reown.swap(false, Ordering::SeqCst)
                        && let Some(payload) = last_regular_payload
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone()
                    {
                        match write_clipboard(&payload, &suppression) {
                            Ok(()) => debug!("restored last regular clipboard after source exited"),
                            Err(error) => {
                                warn!(%error, "failed to retain clipboard after source exit");
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_clipboard(
    payload: &CapturedPayload,
    suppression: &Mutex<Option<SelfWriteSuppression>>,
) -> Result<()> {
    {
        let mut suppression = suppression
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *suppression = Some(SelfWriteSuppression {
            fingerprint: payload.fingerprint,
            expires_at: Instant::now() + SELF_WRITE_SUPPRESSION_TTL,
        });
    }

    let mut options = Options::new();
    options
        .clipboard(wl_clipboard_rs::copy::ClipboardType::Regular)
        .omit_additional_text_mime_types(true);
    let sources = payload
        .representations
        .iter()
        .map(|representation| MimeSource {
            source: Source::Bytes(representation.bytes.clone().into_boxed_slice()),
            mime_type: CopyMimeType::Specific(representation.mime_type.clone()),
        })
        .collect();

    if let Err(error) = options.copy_multi(sources) {
        *suppression
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        return Err(error).context("failed to write Wayland clipboard");
    }
    Ok(())
}

fn clipboard_source(clipboard: InternalClipboardType) -> Option<ClipboardSource> {
    match clipboard {
        InternalClipboardType::Regular => Some(ClipboardSource::Regular),
        InternalClipboardType::Primary => Some(ClipboardSource::Primary),
    }
}

fn mark_seen(source: ClipboardSource, regular_seen: &mut bool, primary_seen: &mut bool) -> bool {
    let seen = match source {
        ClipboardSource::Regular => regular_seen,
        ClipboardSource::Primary => primary_seen,
    };
    if *seen {
        return true;
    }
    *seen = true;
    false
}

fn read_offer(
    offer: &mut Offer<'_>,
    mime_types: &[String],
) -> std::result::Result<CapturedPayload, ReadOfferError> {
    if !mime_types
        .iter()
        .any(|mime_type| is_text_plain_mime(mime_type))
    {
        return Err(CaptureReject::NoTextPlain.into());
    }

    let mut representations = Vec::new();
    for mime_type in mime_types
        .iter()
        .filter(|mime_type| is_text_mime(mime_type))
    {
        match read_offer_mime(offer, mime_type) {
            Ok(bytes) => representations.push(Representation {
                mime_type: mime_type.clone(),
                bytes,
            }),
            Err(message) if is_text_plain_mime(mime_type) => {
                return Err(ReadOfferError::PlainRead(message));
            }
            Err(message) => {
                warn!(%mime_type, %message, "ignoring unreadable auxiliary MIME type");
            }
        }
    }

    Ok(build_payload(representations)?)
}

fn read_offer_mime(offer: &mut Offer<'_>, mime_type: &str) -> std::result::Result<Vec<u8>, String> {
    let reader = offer
        .receive(mime_type)
        .map_err(|error| error.to_string())?;
    read_pipe_with_timeout(reader.as_fd(), MAX_CAPTURE_BYTES, MIME_READ_TIMEOUT)
}

fn read_pipe_with_timeout(
    fd: BorrowedFd<'_>,
    limit: usize,
    timeout: Duration,
) -> std::result::Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK_SIZE];

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("read timed out after {timeout:?}"));
        }
        let remaining = deadline.saturating_duration_since(now);
        let timespec = rustix::event::Timespec::try_from(remaining)
            .map_err(|error| format!("invalid poll timeout: {error}"))?;
        let mut poll_fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut poll_fds, Some(&timespec)) {
            Ok(0) => return Err(format!("read timed out after {timeout:?}")),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(format!("poll failed: {error}")),
        }

        match rustix::io::read(fd, &mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if bytes.len().saturating_add(read) > limit {
                    return Err(format!("representation exceeds the {limit} byte limit"));
                }
                bytes.extend_from_slice(&chunk[..read]);
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => continue,
            Err(error) => return Err(format!("read failed: {error}")),
        }
    }

    Ok(bytes)
}

fn self_write_matches(
    suppression: &Mutex<Option<SelfWriteSuppression>>,
    fingerprint: &[u8; 32],
) -> bool {
    let mut suppression = suppression
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(current) = suppression.as_ref() else {
        return false;
    };
    if current.expires_at <= Instant::now() {
        *suppression = None;
        return false;
    }
    if current.fingerprint != *fingerprint {
        return false;
    }
    *suppression = None;
    true
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    use super::*;

    #[test]
    fn timed_pipe_read_returns_complete_payload() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"clipboard payload").unwrap();
        drop(writer);

        let bytes = read_pipe_with_timeout(reader.as_fd(), 1024, Duration::from_secs(1)).unwrap();
        assert_eq!(bytes, b"clipboard payload");
    }

    #[test]
    fn timed_pipe_read_stops_when_writer_stalls() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let error =
            read_pipe_with_timeout(reader.as_fd(), 1024, Duration::from_millis(20)).unwrap_err();
        assert!(error.contains("timed out"));
    }

    #[test]
    fn timed_pipe_read_enforces_size_limit() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&[b'x'; 128]).unwrap();
        drop(writer);

        let error = read_pipe_with_timeout(reader.as_fd(), 64, Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("byte limit"));
    }
}
