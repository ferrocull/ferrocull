//! Controlled viewer widget for preview and compare modes.
//!
//! A controlled fork of iced's `Viewer` that emits events instead of mutating internal state,
//! enabling synchronized zoom/pan between multiple panes.

use iced::{
    ContentFit, Element, Length, Pixels, Point, Radians, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget,
        image::{self, FilterMethod, Image},
        layout, mouse, renderer,
        widget::tree::{self, Tree},
    },
    border,
};

/// Zoom/pan state owned by `Ferrocull`, passed into the widget.
#[derive(Debug, Clone, Copy)]
pub struct ViewState {
    pub scale: f32,
    pub offset: Vector,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: Vector::default(),
        }
    }
}

impl ViewState {
    /// Zoom level for Z key toggle (400%).
    const TOGGLE_ZOOM: f32 = 4.0;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle between fit-to-screen and zoomed in (Z key behavior).
    pub fn toggle_zoom(&mut self) {
        if (self.scale - 1.0).abs() < 0.01 {
            self.scale = Self::TOGGLE_ZOOM;
        } else {
            *self = Self::new();
        }
    }
}

/// Events emitted when user interacts with the viewer.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// User zoomed (wheel scroll). Contains new scale and offset.
    Zoomed { scale: f32, offset: Vector },
    /// User panned (drag). Contains new offset.
    Panned { offset: Vector },
}

/// Internal drag tracking state (not zoom/pan position).
#[derive(Debug, Clone, Copy, Default)]
struct DragState {
    starting_offset: Vector,
    cursor_grabbed_at: Option<Point>,
}

/// A controlled viewer widget that emits events for zoom/pan changes.
///
/// Unlike iced's `Viewer`, this widget doesn't own the zoom/pan state.
/// The caller passes `ViewState` and receives `Event` via callback.
pub struct Viewer<'a, Handle, Message> {
    handle: Handle,
    view_state: ViewState,
    on_change: Box<dyn Fn(Event) -> Message + 'a>,
    padding: f32,
    width: Length,
    height: Length,
    min_scale: f32,
    max_scale: f32,
    scale_step: f32,
    filter_method: FilterMethod,
    content_fit: ContentFit,
}

impl<'a, Handle, Message> Viewer<'a, Handle, Message> {
    pub fn new<F>(handle: impl Into<Handle>, view_state: ViewState, on_change: F) -> Self
    where
        F: Fn(Event) -> Message + 'a,
    {
        Self {
            handle: handle.into(),
            view_state,
            on_change: Box::new(on_change),
            padding: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            min_scale: 0.25,
            max_scale: 10.0,
            scale_step: 0.10,
            filter_method: FilterMethod::default(),
            content_fit: ContentFit::default(),
        }
    }

    #[must_use]
    pub const fn filter_method(mut self, filter_method: FilterMethod) -> Self {
        self.filter_method = filter_method;
        self
    }

    #[must_use]
    pub const fn content_fit(mut self, content_fit: ContentFit) -> Self {
        self.content_fit = content_fit;
        self
    }

    #[must_use]
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into().0;
        self
    }

    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    #[must_use]
    pub const fn max_scale(mut self, max_scale: f32) -> Self {
        self.max_scale = max_scale;
        self
    }

    #[must_use]
    pub const fn min_scale(mut self, min_scale: f32) -> Self {
        self.min_scale = min_scale;
        self
    }

    #[must_use]
    pub const fn scale_step(mut self, scale_step: f32) -> Self {
        self.scale_step = scale_step;
        self
    }
}

impl<Message, Theme, Renderer, Handle> Widget<Message, Theme, Renderer>
    for Viewer<'_, Handle, Message>
where
    Renderer: image::Renderer<Handle = Handle>,
    Handle: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<DragState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(DragState::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "image dimensions are typically well within f32 precision"
    )]
    fn layout(
        &mut self,
        _tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let image_size = renderer.measure_image(&self.handle).unwrap_or_default();
        let image_size = Size::new(image_size.width as f32, image_size.height as f32);
        let raw_size = limits.resolve(self.width, self.height, image_size);
        let full_size = self.content_fit.fit(image_size, raw_size);

        let final_size = Size {
            width: match self.width {
                Length::Shrink => f32::min(raw_size.width, full_size.width),
                _ => raw_size.width,
            },
            height: match self.height {
                Length::Shrink => f32::min(raw_size.height, full_size.height),
                _ => raw_size.height,
            },
        };

        layout::Node::new(final_size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let drag_state = tree.state.downcast_mut::<DragState>();

        match event {
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(cursor_position) = cursor.position_over(bounds) else {
                    return;
                };

                let y = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => y,
                };

                let previous_scale = self.view_state.scale;

                if (y < 0.0 && previous_scale > self.min_scale)
                    || (y > 0.0 && previous_scale < self.max_scale)
                {
                    let new_scale = (if y > 0.0 {
                        previous_scale * (1.0 + self.scale_step)
                    } else {
                        previous_scale / (1.0 + self.scale_step)
                    })
                    .clamp(self.min_scale, self.max_scale);

                    let scaled_size = scaled_image_size(
                        renderer,
                        &self.handle,
                        new_scale,
                        bounds.size(),
                        self.content_fit,
                    );

                    let factor = new_scale / previous_scale - 1.0;
                    let cursor_to_center = cursor_position - bounds.center();
                    let adjustment = cursor_to_center * factor + self.view_state.offset * factor;

                    let new_offset = Vector::new(
                        if scaled_size.width > bounds.width {
                            self.view_state.offset.x + adjustment.x
                        } else {
                            0.0
                        },
                        if scaled_size.height > bounds.height {
                            self.view_state.offset.y + adjustment.y
                        } else {
                            0.0
                        },
                    );
                    let new_offset = clamped_offset(new_offset, bounds, scaled_size);

                    shell.publish((self.on_change)(Event::Zoomed {
                        scale: new_scale,
                        offset: new_offset,
                    }));
                }

                shell.request_redraw();
                shell.capture_event();
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(cursor_position) = cursor.position_over(bounds) else {
                    return;
                };

                drag_state.cursor_grabbed_at = Some(cursor_position);
                drag_state.starting_offset = self.view_state.offset;

                shell.capture_event();
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                drag_state.cursor_grabbed_at = None;
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(origin) = drag_state.cursor_grabbed_at {
                    let scaled_size = scaled_image_size(
                        renderer,
                        &self.handle,
                        self.view_state.scale,
                        bounds.size(),
                        self.content_fit,
                    );

                    let hidden_width = (scaled_size.width - bounds.width / 2.0).max(0.0).round();
                    let hidden_height = (scaled_size.height - bounds.height / 2.0).max(0.0).round();

                    let delta = *position - origin;

                    let x = if bounds.width < scaled_size.width {
                        (drag_state.starting_offset.x - delta.x).clamp(-hidden_width, hidden_width)
                    } else {
                        0.0
                    };

                    let y = if bounds.height < scaled_size.height {
                        (drag_state.starting_offset.y - delta.y)
                            .clamp(-hidden_height, hidden_height)
                    } else {
                        0.0
                    };

                    let new_offset = Vector::new(x, y);

                    shell.publish((self.on_change)(Event::Panned { offset: new_offset }));
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let drag_state = tree.state.downcast_ref::<DragState>();
        let bounds = layout.bounds();
        let is_mouse_over = cursor.is_over(bounds);

        if drag_state.cursor_grabbed_at.is_some() {
            mouse::Interaction::Grabbing
        } else if is_mouse_over {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        let final_size = scaled_image_size(
            renderer,
            &self.handle,
            self.view_state.scale,
            bounds.size(),
            self.content_fit,
        );

        let translation = {
            let diff_w = bounds.width - final_size.width;
            let diff_h = bounds.height - final_size.height;

            let image_top_left = match self.content_fit {
                ContentFit::None => Vector::new(diff_w.max(0.0) / 2.0, diff_h.max(0.0) / 2.0),
                _ => Vector::new(diff_w / 2.0, diff_h / 2.0),
            };

            image_top_left - clamped_offset(self.view_state.offset, bounds, final_size)
        };

        let drawing_bounds = Rectangle::new(bounds.position(), final_size);

        #[expect(
            clippy::shadow_unrelated,
            reason = "standard iced render closure pattern"
        )]
        let render = |renderer: &mut Renderer| {
            renderer.with_translation(translation, |renderer| {
                renderer.draw_image(
                    Image {
                        handle: self.handle.clone(),
                        border_radius: border::Radius::default(),
                        filter_method: self.filter_method,
                        rotation: Radians(0.0),
                        opacity: 1.0,
                        snap: true,
                    },
                    drawing_bounds,
                    *viewport - translation,
                );
            });
        };

        renderer.with_layer(bounds, render);
    }
}

/// Clamp offset to valid bounds.
fn clamped_offset(offset: Vector, bounds: Rectangle, image_size: Size) -> Vector {
    let hidden_width = (image_size.width - bounds.width / 2.0).max(0.0).round();
    let hidden_height = (image_size.height - bounds.height / 2.0).max(0.0).round();

    Vector::new(
        offset.x.clamp(-hidden_width, hidden_width),
        offset.y.clamp(-hidden_height, hidden_height),
    )
}

/// Compute scaled image size given current scale.
fn scaled_image_size<Renderer>(
    renderer: &Renderer,
    handle: &<Renderer as image::Renderer>::Handle,
    scale: f32,
    bounds: Size,
    content_fit: ContentFit,
) -> Size
where
    Renderer: image::Renderer,
{
    let size = renderer.measure_image(handle).unwrap_or_default();
    #[expect(
        clippy::cast_precision_loss,
        reason = "image dimensions are typically well within f32 precision"
    )]
    let image_size = Size::new(size.width as f32, size.height as f32);
    let adjusted_fit = content_fit.fit(image_size, bounds);

    Size::new(adjusted_fit.width * scale, adjusted_fit.height * scale)
}

impl<'a, Message, Theme, Renderer, Handle> From<Viewer<'a, Handle, Message>>
    for Element<'a, Message, Theme, Renderer>
where
    Renderer: 'a + image::Renderer<Handle = Handle>,
    Message: 'a,
    Handle: Clone + 'a,
{
    fn from(viewer: Viewer<'a, Handle, Message>) -> Self {
        Self::new(viewer)
    }
}
