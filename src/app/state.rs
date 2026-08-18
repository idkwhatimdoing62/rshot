use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureFailureStage {
    ReadCursor,
    LocateCaptureMonitor,
    MatchOverlayMonitor,
    HidePins,
    CaptureImage,
    CreateWindow,
    CreateContext,
    CreateSurface,
}

impl CaptureFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ReadCursor => "RSH-CAP-001",
            Self::LocateCaptureMonitor => "RSH-CAP-002",
            Self::MatchOverlayMonitor => "RSH-CAP-003",
            Self::HidePins => "RSH-CAP-005",
            Self::CaptureImage => "RSH-CAP-004",
            Self::CreateWindow => "RSH-CAP-006",
            Self::CreateContext => "RSH-CAP-007",
            Self::CreateSurface => "RSH-CAP-008",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionFailureStage {
    AccessSurface,
    ResizeSurface,
    AcquireBuffer,
    Present,
}

impl SessionFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::AccessSurface => "RSH-RND-001",
            Self::ResizeSurface => "RSH-RND-002",
            Self::AcquireBuffer => "RSH-RND-003",
            Self::Present => "RSH-RND-004",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::AccessSurface => "访问绘图表面",
            Self::ResizeSurface => "调整绘图表面尺寸",
            Self::AcquireBuffer => "获取绘图缓冲区",
            Self::Present => "提交绘制结果",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SessionFailure {
    stage: SessionFailureStage,
    detail: String,
}

impl SessionFailure {
    pub(super) fn new(stage: SessionFailureStage, detail: impl fmt::Display) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }

    pub(super) const fn stage(&self) -> SessionFailureStage {
        self.stage
    }
}

impl fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}失败：{}", self.stage.label(), self.detail)
    }
}
