//! The window frame on Linux when the app draws its own decorations (Wayland
//! compositors without server-side decorations, X11 with a compositor):
//! a shadow, a border, rounded corners and resize edges, as in Zed's
//! `client_side_decorations`. On macOS, and when the window manager draws the
//! frame, the content is returned as it is.

use gpui::{
    AnyElement, App, Bounds, BoxShadow, CursorStyle, Decorations, HitboxBehavior, Hsla,
    IntoElement, MouseButton, Pixels, Point, ResizeEdge, Size, Tiling, Window, canvas, div, point,
    prelude::*, px, size,
};

use crate::ui::ActiveTheme as _;

/// Room around the window for the shadow; also where resizing starts.
const SHADOW: Pixels = px(10.);
const ROUNDING: Pixels = px(10.);

pub fn decorate(content: impl IntoElement, window: &mut Window, cx: &mut App) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        if !cfg!(target_os = "macos") {
            window.set_client_inset(px(0.));
        }
        return content.into_any_element();
    };
    window.set_client_inset(SHADOW);
    let border = cx.theme().colors.border;

    div()
        .id("window-frame")
        .size_full()
        .bg(gpui::transparent_black())
        .when(!tiling.top, |this| this.pt(SHADOW))
        .when(!tiling.bottom, |this| this.pb(SHADOW))
        .when(!tiling.left, |this| this.pl(SHADOW))
        .when(!tiling.right, |this| this.pr(SHADOW))
        // Only moves over the shadow margin get here (the content stops
        // them); repaint so the canvas below picks the resize cursor.
        .on_mouse_move(|_, window, _| window.refresh())
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, size, tiling) {
                window.start_window_resize(edge);
            }
        })
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .cursor(CursorStyle::Arrow)
                .border_color(border)
                .when(!tiling.top, |this| this.border_t_1())
                .when(!tiling.bottom, |this| this.border_b_1())
                .when(!tiling.left, |this| this.border_l_1())
                .when(!tiling.right, |this| this.border_r_1())
                .when(!(tiling.top || tiling.left), |this| {
                    this.rounded_tl(ROUNDING)
                })
                .when(!(tiling.top || tiling.right), |this| {
                    this.rounded_tr(ROUNDING)
                })
                .when(!(tiling.bottom || tiling.left), |this| {
                    this.rounded_bl(ROUNDING)
                })
                .when(!(tiling.bottom || tiling.right), |this| {
                    this.rounded_br(ROUNDING)
                })
                .when(!tiling.is_tiled(), |this| {
                    this.shadow(vec![BoxShadow {
                        color: Hsla {
                            h: 0.,
                            s: 0.,
                            l: 0.,
                            a: 0.35,
                        },
                        offset: point(px(0.), px(1.)),
                        blur_radius: SHADOW / 2.,
                        spread_radius: px(0.),
                    }])
                })
                // Moves over the content must not reach the frame's edge logic.
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
        .child(
            canvas(
                |_, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    window
                        .insert_hitbox(Bounds::new(Point::default(), size), HitboxBehavior::Normal)
                },
                move |_, hitbox, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    let Some(edge) = resize_edge(window.mouse_position(), size, tiling) else {
                        return;
                    };
                    window.set_cursor_style(
                        match edge {
                            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                            ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                CursorStyle::ResizeUpLeftDownRight
                            }
                            ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                CursorStyle::ResizeUpRightDownLeft
                            }
                        },
                        &hitbox,
                    );
                },
            )
            .size_full()
            .absolute(),
        )
        .into_any_element()
}

/// Which edge a press at `position` resizes, if it is in the shadow margin.
fn resize_edge(
    position: Point<Pixels>,
    window: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let inner = Bounds::new(Point::default(), window).inset(SHADOW * 1.5);
    if inner.contains(&position) {
        return None;
    }
    let corner = size(SHADOW * 1.5, SHADOW * 1.5);
    let corners = [
        (
            Point::default(),
            ResizeEdge::TopLeft,
            !tiling.top && !tiling.left,
        ),
        (
            point(window.width - corner.width, px(0.)),
            ResizeEdge::TopRight,
            !tiling.top && !tiling.right,
        ),
        (
            point(px(0.), window.height - corner.height),
            ResizeEdge::BottomLeft,
            !tiling.bottom && !tiling.left,
        ),
        (
            point(window.width - corner.width, window.height - corner.height),
            ResizeEdge::BottomRight,
            !tiling.bottom && !tiling.right,
        ),
    ];
    for (origin, edge, free) in corners {
        if free && Bounds::new(origin, corner).contains(&position) {
            return Some(edge);
        }
    }
    if position.y < SHADOW && !tiling.top {
        Some(ResizeEdge::Top)
    } else if position.y > window.height - SHADOW && !tiling.bottom {
        Some(ResizeEdge::Bottom)
    } else if position.x < SHADOW && !tiling.left {
        Some(ResizeEdge::Left)
    } else if position.x > window.width - SHADOW && !tiling.right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}
