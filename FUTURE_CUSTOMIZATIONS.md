# Future Customizations

> **Historical.** This made the case for forking `egui_node_graph2`. The fork is done (`github.com/chrischaps/egui_node_graph2`, a git dependency in `Cargo.toml`), and the cables, jacks, nodes and widgets it asks for are drawn now (see `src/graph/` and `src/widgets/`). The unchecked boxes below were not kept up to date; kept for its reasoning, not as a to-do list.

This document tracks visual and functional customizations that would require forking or modifying `egui_node_graph2` to achieve the aesthetic shown in the concept image.

## Why Fork?

The `egui_node_graph2` library provides a solid foundation for node graph editing, but it has limited hooks for deep visual customization. The library draws nodes, ports, and wires internally, exposing only:

- `titlebar_color()` - header bar color
- `top_bar_ui()` / `bottom_ui()` - custom UI in node header/footer
- `data_type_color()` - port and wire colors

To achieve the concept image aesthetic, we would need access to the internal rendering code.

---

## Wire/Cable Rendering

**Current state**: Library draws bezier curves with solid colors (5px * zoom width).

**Desired state** (from concept image):
- [ ] Thick cables with subtle glow effect
- [ ] Shadow underneath cables for depth
- [ ] Slight catenary droop for natural appearance
- [ ] Smooth anti-aliasing

**Required changes**:
- Modify `draw_connection()` in `editor_ui.rs` to:
  - Draw shadow pass first (offset, darker, semi-transparent)
  - Draw outer glow (wider stroke, semi-transparent signal color)
  - Draw main cable (current implementation)
  - Optionally add inner highlight

---

## Output Port Label Alignment

**Current state**: Output port labels (e.g., "Out") are left-aligned in the node body, while the actual port connector is on the right edge.

**Desired state**:
- [ ] Output labels right-aligned, positioned closer to their port connectors

**Possible solutions**:
1. **Custom rendering in `bottom_ui()`** - Hide default labels, render manually with right alignment
2. **Fork egui_node_graph2** - Add an `output_ui` trait method for custom output rendering
3. **Contribute upstream** - Submit PR to add output label customization

**Priority**: Low (functional, just not visually ideal)

---

## Node Styling

**Current state**: Library draws rounded rectangles with configurable title bar color.

**Desired state** (from concept image):
- [ ] Metallic 3D port connectors (circular jacks with highlight/shadow)
- [ ] More prominent node shadows
- [ ] Rounded rectangle body with subtle gradient
- [ ] Thicker border on selection

**Required changes**:
- Modify node rendering in `editor_ui.rs`:
  - Custom port drawing with metallic appearance
  - Enhanced shadow rendering
  - Configurable body fill/stroke styles

---

## Custom Widgets

**Current state**: Library uses standard egui widgets (sliders, checkboxes).

**Desired state** (from concept image):
- [ ] Rotary knobs with 3D appearance
- [ ] Custom faders with value display
- [ ] Waveform display widgets
- [ ] VU meters / level indicators

**Notes**: This may be achievable WITHOUT forking by:
1. Using `InputParamKind::ConstantOnly` and drawing custom widgets in `bottom_ui()`
2. Creating standalone egui widgets that can be composed
3. See Issues #17 (Widget Library) and #18 (Waveform Display)

---

## Alternative: egui-snarl

The `egui-snarl` library is another node graph option that may offer more customization flexibility:
- User-controlled wire rendering callbacks
- More explicit styling options
- Worth evaluating if egui_node_graph2 limitations become blocking

---

## Backdrop Layer (done)

Frames and notes (#88) are drawn under the nodes through
`ConnectionSignalTrait::backdrop_ui`. The fork calls it once a frame, after
the editor background has claimed its clicks and before any node is drawn, so
whatever it paints sits under nodes and cables, and its widgets win clicks
over empty canvas but not over nodes. It gets a `Backdrop` with the editor's
origin, zoom, node positions, node selection and pan, so a frame dragged by
its title can carry its modules in the same frame.

---

## Implementation Priority

If we decide to fork `egui_node_graph2`:

1. **High Priority**: Wire rendering (most visually impactful)
2. **Medium Priority**: Node shadows and selection styling
3. **Lower Priority**: Metallic port connectors (subtle improvement)

The custom widgets (knobs, faders, waveforms) should be attempted first WITHOUT forking, as they may be achievable through the existing `bottom_ui()` hook or as standalone egui widgets.

---

*Last updated: 2026-01-20*

---

## Recently Deferred

- **Output Port Label Alignment** (2026-01-20) - Left-aligned output labels accepted for now
