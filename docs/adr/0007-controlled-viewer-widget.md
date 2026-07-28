# Own a controlled viewer widget instead of using iced's

Zoom and pan in the preview and in compare mode run through `ferrocull-ui/src/widgets/viewer.rs`, a fork of iced's `Viewer` that does not own its own state. The caller passes a `ViewState` (scale and offset) in and receives `Zoomed` and `Panned` events back, so the app decides what the new state becomes.

This started as a plain use of iced's built-in `Viewer`, which was cheaper and rendered just as well. The reason we no longer use it is compare mode: iced's widget keeps zoom and pan private, and two panes that each own their state cannot be locked together. Photo Mechanic locks them, and comparing two frames of the same scene at 400% is most of what compare mode is for, so matching that behaviour turned out not to be optional.

Moving the state out to the caller is what makes the rest possible. `L` locks the panes by writing one `ViewState` to both and keeping them equal from then on. `Z` toggles between fit and 400% by replacing the state outright. Neither is expressible against a widget that hides its scale.

The cost is that we own a widget shadowing an upstream one. It tracks iced's `advanced::Widget` API, so an iced upgrade can break it in ways a normal `iced::widget::image` call would not, and improvements upstream do not reach us for free. That is the price of synced zoom, and it is worth paying.

The bounds live in the widget: scale from 0.25 to 10.0, wheel steps of 10%, and 4.0 as the `Z` toggle target.
