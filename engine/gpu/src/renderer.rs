//! the rendering backend interface.

/// everything a draw needs, handed over without copying the register file.
pub struct DrawCall<'a> {
    pub indexed: bool,
    pub registers: &'a [u32],
}

pub trait Renderer {
    /// name shown in logs and in the window title.
    fn name(&self) -> &'static str;

    /// executes a draw with the current register state.
    fn draw(&mut self, call: DrawCall<'_>);

    /// called once per emulated frame, after both screens have been
    /// transferred, so a deferred backend can flush.
    fn end_frame(&mut self) {}
}

/// the do-nothing backend.
#[derive(Default)]
pub struct SoftwareRenderer {
    pub draws: u64,
}

impl Renderer for SoftwareRenderer {
    fn name(&self) -> &'static str {
        "software"
    }

    fn draw(&mut self, _call: DrawCall<'_>) {
        self.draws += 1;
    }
}
