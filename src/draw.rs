/// A single drawing instruction emitted by user strategy code.
///
/// Coordinates use the chart's logical space:
///   x — bar index (0 = first bar in the dataset)
///   y — price value
///
/// The host application maps these to screen pixels before rendering.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum DrawCommand {
    /// Straight line between two points.
    Line {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        color: [u8; 4],
        width: f32,
    },

    /// Horizontal line spanning the full chart width at a given price.
    HLine {
        y: f64,
        color: [u8; 4],
        width: f32,
    },

    /// Vertical line spanning the full chart height at a given bar index.
    VLine {
        x: f64,
        color: [u8; 4],
        width: f32,
    },

    /// Filled or stroked rectangle.
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        fill_color: [u8; 4],
        stroke_color: [u8; 4],
        stroke_width: f32,
    },

    /// Circle (e.g. signal dot on a bar).
    Circle {
        cx: f64,
        cy: f64,
        radius: f64,
        fill_color: [u8; 4],
    },

    /// Single-segment arrow from tail to head.
    Arrow {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        color: [u8; 4],
        width: f32,
    },
}

/// Accumulates [`DrawCommand`]s for a single bar or the whole pass.
///
/// The host owns the allocation; strategies receive a `*mut DrawCtx`
/// and call [`DrawCtx::push`] (or the helper methods) to emit commands.
pub struct DrawCtx {
    pub commands: Vec<DrawCommand>,
}

impl DrawCtx {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    pub fn push(&mut self, cmd: DrawCommand) {
        self.commands.push(cmd);
    }

    // ── convenience helpers ──────────────────────────────────────────

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, color: [u8; 4], width: f32) {
        self.push(DrawCommand::Line { x1, y1, x2, y2, color, width });
    }

    pub fn hline(&mut self, y: f64, color: [u8; 4], width: f32) {
        self.push(DrawCommand::HLine { y, color, width });
    }

    pub fn vline(&mut self, x: f64, color: [u8; 4], width: f32) {
        self.push(DrawCommand::VLine { x, color, width });
    }

    pub fn rect(
        &mut self,
        x: f64, y: f64, w: f64, h: f64,
        fill_color: [u8; 4],
        stroke_color: [u8; 4],
        stroke_width: f32,
    ) {
        self.push(DrawCommand::Rect { x, y, w, h, fill_color, stroke_color, stroke_width });
    }

    pub fn circle(&mut self, cx: f64, cy: f64, radius: f64, fill_color: [u8; 4]) {
        self.push(DrawCommand::Circle { cx, cy, radius, fill_color });
    }

    pub fn arrow(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, color: [u8; 4], width: f32) {
        self.push(DrawCommand::Arrow { x1, y1, x2, y2, color, width });
    }
}

impl Default for DrawCtx {
    fn default() -> Self {
        Self::new()
    }
}
