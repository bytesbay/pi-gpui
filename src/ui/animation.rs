//! Cheap loading animations.
//!
//! gpui's `with_animation` requests a frame on every display refresh (120 Hz on
//! ProMotion), and a frame re-renders the animated view and all its ancestors.
//! A single spinner then cost ~30% of a core. Instead, animated components
//! derive their phase from wall time, and the owning view re-renders on a
//! slow tick only while something is actually loading.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{Context, Task};

/// 12.5 frames per second: smooth enough for a spinner, a fraction of the
/// cost of display-rate animation. Idle views do not tick at all.
pub const ANIMATION_TICK: Duration = Duration::from_millis(80);

/// Fraction (0..1) of a cycle of `period`, shared by all components so that
/// every spinner on screen turns in sync.
pub fn phase(period: Duration) -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let elapsed = EPOCH.get_or_init(Instant::now).elapsed();
    (elapsed.as_secs_f32() / period.as_secs_f32()).fract()
}

/// Keeps a view re-rendering while it shows loading indicators.
///
/// Call [`AnimationTicker::set_active`] at the end of `render` with whether
/// anything in the view is animating; the ticker starts and stops itself.
#[derive(Default)]
pub struct AnimationTicker {
    task: Option<Task<()>>,
}

impl AnimationTicker {
    pub fn set_active<V: 'static>(&mut self, active: bool, cx: &mut Context<V>) {
        if !active {
            // Dropping the task cancels the loop.
            self.task = None;
            return;
        }
        if self.task.is_some() {
            return;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ANIMATION_TICK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }
}
