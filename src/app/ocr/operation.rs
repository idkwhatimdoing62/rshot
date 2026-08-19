use super::{OcrFailure, OcrRecognition, OcrRequest, recognize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct OcrSessionId(pub(in crate::app) u64);

pub(in crate::app) enum OcrEvent {
    Completed {
        session_id: OcrSessionId,
        recognition: OcrRecognition,
    },
    Failed {
        session_id: OcrSessionId,
        failure: OcrFailure,
    },
    TimedOut {
        session_id: OcrSessionId,
    },
    Cancelled {
        session_id: OcrSessionId,
    },
}

impl OcrEvent {
    pub(in crate::app) const fn session_id(&self) -> OcrSessionId {
        match self {
            Self::Completed { session_id, .. }
            | Self::Failed { session_id, .. }
            | Self::TimedOut { session_id }
            | Self::Cancelled { session_id } => *session_id,
        }
    }
}

pub(in crate::app) struct OcrOperation {
    session_id: OcrSessionId,
    deadline: Instant,
    receiver: mpsc::Receiver<Result<OcrRecognition, OcrFailure>>,
    cancelled: Arc<AtomicBool>,
    finished: bool,
}

impl OcrOperation {
    pub(in crate::app) fn start(
        session_id: OcrSessionId,
        deadline: Instant,
        request: OcrRequest<'_>,
    ) -> Result<Self, String> {
        let image = request.frozen_image.clone();
        let selection = request.selection;
        Self::start_with(session_id, deadline, move || {
            recognize(OcrRequest {
                frozen_image: &image,
                selection,
            })
        })
    }

    fn start_with(
        session_id: OcrSessionId,
        deadline: Instant,
        work: impl FnOnce() -> Result<OcrRecognition, OcrFailure> + Send + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        std::thread::Builder::new()
            .name(format!("rshot-ocr-session-{}", session_id.0))
            .spawn(move || {
                let result = work();
                if !worker_cancelled.load(Ordering::Acquire) {
                    let _ = sender.send(result);
                }
            })
            .map_err(|error| format!("无法启动 OCR 操作线程：{error}"))?;
        Ok(Self {
            session_id,
            deadline,
            receiver,
            cancelled,
            finished: false,
        })
    }

    pub(in crate::app) fn poll(&mut self, now: Instant) -> Option<OcrEvent> {
        if self.finished {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(Ok(recognition)) => {
                self.finished = true;
                Some(OcrEvent::Completed {
                    session_id: self.session_id,
                    recognition,
                })
            }
            Ok(Err(failure)) => {
                self.finished = true;
                Some(OcrEvent::Failed {
                    session_id: self.session_id,
                    failure,
                })
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.finished = true;
                Some(OcrEvent::Cancelled {
                    session_id: self.session_id,
                })
            }
            Err(mpsc::TryRecvError::Empty) if now >= self.deadline => {
                self.finished = true;
                self.cancelled.store(true, Ordering::Release);
                Some(OcrEvent::TimedOut {
                    session_id: self.session_id,
                })
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }

    pub(in crate::app) fn cancel(&mut self) -> Option<OcrEvent> {
        if self.finished {
            return None;
        }
        self.finished = true;
        self.cancelled.store(true, Ordering::Release);
        Some(OcrEvent::Cancelled {
            session_id: self.session_id,
        })
    }
}

impl Drop for OcrOperation {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ocr::OcrBackend;
    use std::time::{Duration, Instant};

    #[test]
    fn completed_event_keeps_the_session_id() {
        let now = Instant::now();
        let mut operation =
            OcrOperation::start_with(OcrSessionId(7), now + Duration::from_secs(1), || {
                Ok(OcrRecognition {
                    text: String::from("text"),
                    backend: OcrBackend::PpOcrV6,
                    fallback_reason: None,
                })
            })
            .unwrap();

        let event = loop {
            if let Some(event) = operation.poll(now) {
                break event;
            }
            std::thread::yield_now();
        };

        assert_eq!(event.session_id(), OcrSessionId(7));
        assert!(matches!(event, OcrEvent::Completed { .. }));
    }

    #[test]
    fn deadline_emits_timeout_once_and_discards_late_completion() {
        let now = Instant::now();
        let mut operation = OcrOperation::start_with(OcrSessionId(8), now, || {
            std::thread::sleep(Duration::from_millis(10));
            Ok(OcrRecognition {
                text: String::from("late"),
                backend: OcrBackend::PpOcrV6,
                fallback_reason: None,
            })
        })
        .unwrap();

        assert!(matches!(
            operation.poll(now),
            Some(OcrEvent::TimedOut { .. })
        ));
        std::thread::sleep(Duration::from_millis(20));
        assert!(operation.poll(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn cancellation_emits_once_and_suppresses_worker_result() {
        let now = Instant::now();
        let mut operation =
            OcrOperation::start_with(OcrSessionId(9), now + Duration::from_secs(1), || {
                std::thread::sleep(Duration::from_millis(10));
                Ok(OcrRecognition {
                    text: String::from("cancelled"),
                    backend: OcrBackend::PpOcrV6,
                    fallback_reason: None,
                })
            })
            .unwrap();

        assert!(matches!(
            operation.cancel(),
            Some(OcrEvent::Cancelled { .. })
        ));
        std::thread::sleep(Duration::from_millis(20));
        assert!(operation.poll(now + Duration::from_secs(1)).is_none());
        assert!(operation.cancel().is_none());
    }
}
