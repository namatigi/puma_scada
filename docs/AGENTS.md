# Working agreement for Moviclone agents

Read this, `README.md`, `docs/PROTOCOL.md` and the crate docs you depend on before
writing code. The architecture rationale is in `docs/movicon-next-architecture.html`.

## Build environment (Windows, this machine)

- Run `. scripts/env.sh` (bash) or `. .\scripts\env.ps1` (PowerShell) first. It puts
  cargo and MSYS2 binutils on PATH; without them tokio/axum fail to link (`dlltool`).
- Use your own target dir to avoid lock contention with other agents:
  `export CARGO_TARGET_DIR=target/<your-agent-name>`.
- No Node/npm build step. Web code is plain ES modules, HTML and CSS served as
  static files. No CDN dependencies: everything must work offline on a plant network.
- Sample project: `out/oxoafrica` (imported from the real Movicon project). Re-create
  with `cargo run -p movi -- import "C:/Users/leonm/OneDrive/Desktop/Petrogas12-04" -o out/oxoafrica --force --name "OXOAFRICA SCADA"`.
  The project's display name is **OXOAFRICA SCADA** (folders `out/oxoafrica`, `demo/oxoafrica`).
  JSON bundle for front-end work: `cargo run -p movi -- bundle out/oxoafrica -o <file>`.
- The real Modbus gateway (169.254.100.100) is not reachable. Use the simulator.

## Ownership (edit only what you own)

| Area | Owner |
|---|---|
| `crates/model`, `crates/check`, `crates/import-movicon`, `crates/tagdb`, `crates/movi`, `docs/PROTOCOL.md` | lead (frozen contracts; ask in your final report if you need a change) |
| `crates/driver-modbus`, `crates/sim`, `crates/gateway`, `crates/rt` | runtime agent |
| `crates/historian`, `crates/alarms` | history agent |
| `web/shared/**`, `web/hmi/**` | HMI agent |
| `web/studio/**`, `crates/studio` | studio agent (reuses `web/shared`, may add files under `web/shared/studio-*` only) |
| `demo/**`, wiring in `crates/rt`, `tests/e2e/**`, README updates | integration agent (runs last) |

Workspace `Cargo.toml` already lists every crate and the shared dependencies. If you
need another dependency, add it to *your* crate's `Cargo.toml` with an explicit
version. Avoid crates that need a C compiler or OpenSSL (use rustls if TLS is needed).

## Code standards

- `cargo clippy -p <your crates> --all-targets` clean, `cargo test -p <your crates>` green.
- Tests for every non-trivial behaviour; integration tests where crates meet.
- No `unwrap()` on I/O or network paths in non-test code. Log with `tracing`.
- Comments explain *why*, not what. Keep modules small and named for what they do.
- Never write secrets to disk or logs. `secret://name` resolves from env var
  `MOVICLONE_SECRET_<NAME>` (upper case, `-` → `_`).

## Visual direction: familiar to Movicon NExT users, better to use

Operators and engineers coming from Movicon NExT should feel at home: same layout,
same workflow, same vocabulary. Draw everything ourselves. Do not copy Progea or
Emerson logos, icons, names, splash screens or symbol artwork (the project's
`.xaml.compiled` files contain licensed symbol vectors: read names and sizes only).
Product name: **Moviclone**.

### Runtime HMI (`web/hmi`): what Petrogas looks like in Movicon today

- Full-window screen, scaled to fit while keeping aspect ratio. Screen background
  from the bundle (`#3F3F3F`, or a vertical gradient `#6E7072 → #282526` on Manifold).
- Pipes: thick steel tubes (≈26 px) with a cylindrical shading gradient across the
  width (light centre, dark edges), round caps, light grey `#EFEFF0` fill with a
  `#5E5E5E` outline. Fittings where pipes meet.
- Hand valves: metallic ball-valve bodies with blue butterfly handles, rotated
  per element `rotation`.
- Flow meters: Coriolis-style body coloured per product (Gasoline red `#FF4D4D`,
  Jet A-1 white, Gasoil grey `#555`, HFO black, Base Oil pink `#F08080`) with a white
  transmitter head on top.
- Value displays: dark panel (`#333333`), white SemiBold value (Segoe UI 20 px),
  unit to the right (14 px). **Bad quality: red border + warning triangle**;
  uncertain: amber border.
- Labels on Manifold: dark rounded tags ("Gasoline", "JET A-1"), white bold text.
- Navigation buttons: light grey glossy buttons with dark bold text
  ("HFO Trend & Log"), bottom right.
- Trend screens: chart on top with a toolbar (time presets Minute/Hour/Day/Week/
  Month/Year/All, max records, zoom), coloured pens with their own Y axes on the
  left, legend; history table below with columns Tag Name, Value, Record Date
  Time…; a Refresh bar at the bottom.

Improvements over Movicon to include:
- Thin, auto-hiding top bar: project, current screen, back/home, alarm banner
  (count per severity, newest alarm, click → alarm page), connection state, clock.
- Small screens (e.g. FM_01 at 320×680, under half the main screen) open as a
  floating panel over the current screen instead of replacing it.
- Animated flow in pipes and live state on valves/meters when a tag is bound.
- Stale data visible at a glance (age shown on hover), quality on every value.
- Works on tablets and phones (pinch-zoom, tap targets ≥ 44 px), dark by default.
- Screen navigator (Movicon's tile view): grid of screen thumbnails grouped by name.

### Studio (`web/studio`): Movicon NExT designer layout

VS2017-dark style docking workspace:
- Title row: "Quick Search Commands" box at top left (becomes a Ctrl+K command
  palette), window title centred.
- Menu bar: File, Edit, View, Options, Project, Design, Format, Tools, Help.
- Toolbar: undo/redo, new, open, save, run (green ▶), stop (red ■), alignment,
  distribute, size, z-order, grid/snap toggles, font family/size, bold/italic.
- Left dock "Project Explorer" tree mirroring Movicon's: Client Configuration,
  Logic, Schedulers, Reports, Screens (one node per screen), Parameters, Texts,
  Cross Reference, Events, Recipes, I/O Data Server (Data Structure, Tags List,
  Settings, Drivers, Alarms, Historians, Data Loggers, Engineering Units, Views,
  Redundancy), Users. Nodes we do not support yet are shown dimmed with a tooltip.
- Left edge vertical tabs: "Runtime Status", "Tag Browser".
- Centre: tabbed documents (screen canvas with grid; tag list grid; driver,
  historian, alarm editors). Under the canvas: Language, Zoom (combo, −, +), and
  X / Y / W / H of the selection; tabs "Objects" and "Tags Explorer".
- Right dock "Properties": search box, categorized/alphabetical toggle, collapsible
  categories Tag, Style, General, Fonts, Layout, Visibility, User Access,
  Animations, Commands. Right edge vertical tabs: "ToolBox", "Symbol Library".
- Bottom dock tabs: Animations, Source (TOML of the selection, replaces Movicon's
  XML), Problems (live diagnostics, click to select; replaces Script), System Log,
  Commands.

Improvements: undo/redo everywhere, multi-select with align/distribute, snap to
grid, arrow-key nudge, copy/paste, live values on the canvas while designing,
"where used" for any tag, instant validation on every edit, saves are small text
diffs, Run applies changes to the running runtime without restart.
