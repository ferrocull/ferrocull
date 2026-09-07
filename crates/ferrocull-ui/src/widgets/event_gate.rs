//! Content-wrapping widget that decides, event by event, what its child sees.
//!
//! A gate closure reads each incoming event and returns a [`Verdict`]: pass it
//! to the wrapped widget, drop it (the child never sees it, and it stays
//! uncaptured so subscriptions still receive it), or publish a message and
//! capture it. Only an uncaptured event is ever published, so two gates over
//! the same spot never both answer one notch; a dropped event is held back
//! whether or not something else has already captured it.

use iced::{
    Element, Length, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, tree},
    },
    keyboard,
};

/// What an [`EventGate`] does with an event.
pub(crate) enum Verdict<Message> {
    /// Forward the event to the wrapped widget.
    Pass,
    /// Swallow the event: the wrapped widget never sees it, and it stays
    /// uncaptured.
    Drop,
    /// Publish `Message`, capture the event, and keep it from the wrapped
    /// widget.
    Publish(Message),
}

/// The rule an [`EventGate`] applies to every event it sees.
type Gate<'a, Message> = dyn Fn(&iced::Event, mouse::Cursor, Rectangle) -> Verdict<Message> + 'a;

/// A transparent wrapper that gates the events reaching the widget it wraps.
pub(crate) struct EventGate<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    gate: Box<Gate<'a, Message>>,
}

impl<'a, Message, Theme, Renderer> EventGate<'a, Message, Theme, Renderer> {
    /// Wrap `content`, ruling on every event with `gate`, which reads the
    /// event, the cursor, and the bounds of the wrapped area.
    pub(crate) fn new(
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
        gate: impl Fn(&iced::Event, mouse::Cursor, Rectangle) -> Verdict<Message> + 'a,
    ) -> Self {
        Self {
            content: content.into(),
            gate: Box::new(gate),
        }
    }
}

/// Route every wheel notch over `content` to the application, scrollbar rail
/// included, and keep it from the wrapped widget.
///
/// Wrapping a `scrollable` therefore leaves the application in sole charge of
/// what a notch does, independent of the scrollable's own wheel handling and
/// its scroll-transaction state. Every other event passes through untouched, so
/// scrollbar drags and keyboard scrolling still reach the child.
pub(crate) fn wheel_area<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    on_scroll: impl Fn(mouse::ScrollDelta) -> Message + 'a,
) -> EventGate<'a, Message> {
    EventGate::new(content, move |event, cursor, bounds| match event {
        iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
            Verdict::Publish(on_scroll(*delta))
        }
        _ => Verdict::Pass,
    })
}

/// Keep key presses and releases away from `content`.
///
/// The wrapped widget never sees them, so the application's own shortcuts stay
/// in sole charge of the keyboard; the events pass on uncaptured, so the
/// keyboard subscription still receives them. `ModifiersChanged` goes through
/// untouched, as widgets track the modifier state from it: a slider reads Shift
/// from that state for its fine step.
pub(crate) fn keyboard_shield<'a, Message>(
    content: impl Into<Element<'a, Message>>,
) -> EventGate<'a, Message> {
    EventGate::new(content, |event, _cursor, _bounds| match event {
        iced::Event::Keyboard(
            keyboard::Event::KeyPressed { .. } | keyboard::Event::KeyReleased { .. },
        ) => Verdict::Drop,
        _ => Verdict::Pass,
    })
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EventGate<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
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
        match (self.gate)(event, cursor, layout.bounds()) {
            Verdict::Drop => return,
            Verdict::Publish(message) if !shell.is_event_captured() => {
                shell.publish(message);
                shell.capture_event();
                return;
            }
            Verdict::Publish(_) | Verdict::Pass => {}
        }

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
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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

impl<'a, Message, Theme, Renderer> From<EventGate<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + renderer::Renderer,
{
    fn from(event_gate: EventGate<'a, Message, Theme, Renderer>) -> Self {
        Self::new(event_gate)
    }
}
