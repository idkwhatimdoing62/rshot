use std::fmt;

#[derive(Default, Debug, PartialEq)]
pub(super) enum Mode {
    #[default]
    Selecting,
    Editing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureFailureStage {
    ReadCursor,
    LocateCaptureMonitor,
    MatchOverlayMonitor,
    CaptureImage,
}

impl CaptureFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ReadCursor => "RSH-CAP-001",
            Self::LocateCaptureMonitor => "RSH-CAP-002",
            Self::MatchOverlayMonitor => "RSH-CAP-003",
            Self::CaptureImage => "RSH-CAP-004",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionFailureStage {
    CreateWindow,
    CreateContext,
    CreateSurface,
    AccessSurface,
    ResizeSurface,
    AcquireBuffer,
    Present,
}

impl SessionFailureStage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::CreateWindow => "创建截图窗口",
            Self::CreateContext => "创建图形上下文",
            Self::CreateSurface => "创建绘图表面",
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
}

impl fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}失败：{}", self.stage.label(), self.detail)
    }
}
