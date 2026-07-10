//! Content-wrapping widget that turns a panel-edge handle into a drag-to-resize splitter.
//!
//! Wraps a styled handle: dragging resizes the sidebar (emitting raw widths for the caller to
//! clamp), while a plain click with no meaningful movement fires a toggle message instead.
//! Once a drag is grabbed the widget keeps receiving `CursorMoved` past its bounds, so no
//! global event subscription is needed.

use iced::{
    Element, Length, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, tree},
    },
};

/// Movement past this many pixels turns a click into a drag.
const DRAG_THRESHOLD: f32 = 3.0;

/// Which side of the layout the resized panel sits on: a `Left` panel widens
/// when its handle is dragged right, a `Right` panel when dragged left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Active drag tracking, stored in the widget tree.
struct Drag {
    grab_x: f32,
    start_width: f32,
    moved: bool,
}

/// A panel-edge handle that resizes on drag and toggles on click.
pub struct Splitter<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    panel_width: f32,
    side: Side,
    on_resize: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    on_resize_end: Option<Message>,
    on_click: Option<Message>,
}

impl<'a, Message, Theme, Renderer> Splitter<'a, Message, Theme, Renderer> {
    /// Wrap `content` as a splitter for the panel on the given `side`, currently
    /// `panel_width` wide.
    pub fn new(
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
        panel_width: f32,
        side: Side,
    ) -> Self {
        Self {
            content: content.into(),
            panel_width,
            side,
            on_resize: None,
            on_resize_end: None,
            on_click: None,
        }
    }

    /// Message published on every drag move, carrying the new raw width (caller clamps).
    #[must_use]
    pub fn on_resize(mut self, on_resize: impl Fn(f32) -> Message + 'a) -> Self {
        self.on_resize = Some(Box::new(on_resize));
        self
    }

    /// Message published once when a drag that actually moved ends (caller persists).
    #[must_use]
    pub fn on_resize_end(mut self, message: Message) -> Self {
        self.on_resize_end = Some(message);
        self
    }

    /// Message published when the handle is released without meaningful movement (caller toggles).
    #[must_use]
    pub fn on_click(mut self, message: Message) -> Self {
        self.on_click = Some(message);
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Splitter<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Option<Drag>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Option::<Drag>::None)
    }

    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        if shell.is_event_captured() {
            return;
        }

        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<Option<Drag>>();

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };

                *state = Some(Drag {
                    grab_x: position.x,
                    start_width: self.panel_width,
                    moved: false,
                });
                shell.capture_event();
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(drag) = state.as_mut() else {
                    return;
                };

                let delta_x = position.x - drag.grab_x;
                if delta_x.abs() > DRAG_THRESHOLD {
                    drag.moved = true;
                }

                if drag.moved {
                    let delta = match self.side {
                        Side::Left => delta_x,
                        Side::Right => -delta_x,
                    };
                    let new_width = drag.start_width + delta;
                    if let Some(on_resize) = self.on_resize.as_ref() {
                        shell.publish(on_resize(new_width));
                    }
                    shell.request_redraw();
                }
                shell.capture_event();
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(drag) = state.take() else {
                    return;
                };

                let message = if drag.moved {
                    self.on_resize_end.as_ref()
                } else {
                    self.on_click.as_ref()
                };
                if let Some(message) = message {
                    shell.publish(message.clone());
                }
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<Option<Drag>>();

        if state.is_some() || cursor.is_over(layout.bounds()) {
            mouse::Interaction::ResizingHorizontally
        } else {
            self.content.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        }
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut tree::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<Splitter<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Theme: 'a,
    Renderer: 'a + renderer::Renderer,
{
    fn from(splitter: Splitter<'a, Message, Theme, Renderer>) -> Self {
        Self::new(splitter)
    }
}
