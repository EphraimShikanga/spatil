# Spatil

Spatil is a 3D spatial Wayland compositor built from the ground up in Rust using the Smithay toolkit.

Instead of treating the screen as a flat 2D plane for tiling or stacking windows, Spatil conceptualizes the user environment as an infinite 3D canvas with explicit $X$, $Y$, and $Z$ axes. By abandoning traditional desktop limitations, it maps application surfaces as textures onto 3D quads inside a global scene graph, allowing windows to be navigated, scaled, rotated, and clustered natively in three-dimensional space.

---

## Core Pillars & Architectural Vision

### 1. The Infinite 3D Scene Graph

Traditional compositors arrange surfaces using 2D trees or flat stacking orders. Spatil implements a rigorous **Scene Graph** architecture. Every window, group, and workspace is a geometric node possessing spatial properties:

* **Translation:** Position coordinates across $(X, Y, Z)$.
* **Scale:** Structural resizing across three dimensions.
* **Rotation:** Orientation modifications including pitch, roll, and yaw.

### 2. Viewport Camera Mechanics

The physical monitor is treated as a highly configurable camera observing a specific coordinate window of the infinite canvas.

* **Panning & Navigation:** Translating the camera along the $X$ and $Y$ axes navigates the canvas seamlessly.
* **Depth Optimization (Z-Translation):** Minimizing an application does not destroy its buffer or toggle its visibility flag; it programmatically translates its $Z$-coordinate deep into the background, scaling it down dynamically while keeping it alive.

### 3. Spatial Audio Mapping

Windows are treated as physical sound-emitting bodies in space. By interfacing directly with PipeWire, Spatil binds an application's spatial positions to the system's audio output:

* **Depth Attenuation:** Audio volume drops off naturally as a window's $Z$-coordinate moves further away from the camera's viewport plane.
* **Directional Panning:** Audio balances dynamically across left/right channels relative to the window's $X$ position on screen.

### 4. Non-Planar Configurations

Windows do not have to flatly face the viewport. Because surfaces are mapped onto 3D primitives, users can rotate surfaces along their vertical axis (Yaw) to orchestrate a virtual curved workspace or wrap-around monitor layout on a single flat display.

---

## Native UI & Interaction Layers

### 1. "Familiar" Core Shell Interface

Spatil completely eliminates traditional 2D taskbars and status bars (e.g., Waybar) in favor of **Familiar**, a native, contextual orb interface:

* **Placement:** The orb can be anchor-positioned dynamically into any screen corner.
* **System Tray:** Clicking the orb expands a compact, spatial interface revealing system tray status items, controls, and active applets.
* **Integrated Chronometer:** The orb body functions as the system clock. When the cursor is idle, the interface transitions smoothly into an ambient mode, rendering a procedurally animated dot that revolves around the perimeter of the orb to track elapsed seconds.

### 2. Spatial Minimap & System Diagnostics

To navigate the infinite canvas efficiently, Spatil embeds a subtle, high-performance **Minimap** overlay:

* **Canvas Teleportation:** Positioned in a designated screen corner, the minimap renders a low-overhead orthographic representation of the $X/Y$ workspace coordinates. Clicking or dragging on the minimap immediately translates the viewport camera to those precise spatial coordinates.
* **Telemetry Overlay:** The background and layout structures of the minimap double as a real-time system resource monitor, translating hardware telemetry (CPU, memory, thermals) into visual density patterns or subtle color shifts within the map geometry itself.

### 3. Intelligent Workspace Automation (Gemini Engine)

Spatil incorporates a local orchestration engine powered by Gemini. By utilizing programmatic **Function Calling**, the compositor acts as an intelligent shell agent capable of interpreting natural language commands into direct compositor state manipulations:

* **Compositor Function Hooks:** The model maps user intents directly to execution signatures such as `open_application(app_id)`, `close_window(node_id)`, `translate_node(node_id, x, y, z)`, or `camera_pan_to(x, y)`.
* **Context-Aware Layout Management:** Users can issue high-level structural instructions (e.g., *"Group my active terminal and browser into a new background cluster, then pan the camera to my main editor"*) and let the model execute the explicit scene graph updates.

---

## Features & Tech Stack

* **Smithay-Backed Infrastructure:** Leverages Rust's compile-time memory safety to guarantee stable state management, abstracting kernel-mode setting (KMS), hardware allocation, and core Wayland protocol parsing.
* **3D Constellations:** Windows can be grouped together into localized clusters within the 3D space. Pushing a parent node moves its entire cluster, allowing complex, multi-window workspace setups to be manipulated as unified 3D assemblies.
* **Depth of Field Rendering:** Integrated shaders dynamically manage blurring and focus algorithms based on a window’s proximity to the active camera viewport plane.
* **Raycast Input Resolution:** Mouse interactions map from 2D screen coordinate space into the 3D coordinate frame via ray-surface intersection algorithms to pinpoint target surfaces accurately.

---

## Technical Stack & Architecture

```
   +-------------------------------------------------------+
   |                        Spatil                         |
   |   [Scene Graph] -> [Camera Viewport] -> [Renderer]    |
   |   [Familiar UI]    [Spatial Minimap]   [Gemini Agent] |
   +-------------------------------------------------------+
                              |
                              v
   +-------------------------------------------------------+
   |                       Smithay                         |
   |   [Wayland Protocols]   [Event Loop]   [Backends]     |
   +-------------------------------------------------------+
          |                      |                   |
          v                      v                   v
   +--------------+      +---------------+    +------------+
   | Kernel / KMS |      |  libinput/UDev|    | PipeWire   |
   | (Graphics)   |      |  (I/O Input)  |    | (Audio)    |
   +--------------+      +---------------+    +------------+

```

* **Language:** Rust (Stable)
* **Foundational Layer:** Smithay
* **Async/Event Framework:** `calloop`
* **Audio Pipelines:** PipeWire integration
* **Automation Engine:** Gemini Core API / Function Calling

---

## Development & Testing Workflow

Spatil implements a **Nested Backend Architecture** for local debugging and risk-free iteration. This isolates the runtime environment from the physical hardware layer during heavy development cycles.

### Running a Nested Session

To run the compositor as a regular client window inside your current desktop environment:

```bash
cargo run --features "backend_nested"

```

This instantiates a windowed backend instance running a dedicated local Wayland socket. You can launch test applications into Spatil's 3D environment by pointing their display environment variables directly to the freshly exposed socket.