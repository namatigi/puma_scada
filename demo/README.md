# OXOAFRICA SCADA demo

`demo/oxoafrica` is the OXOAFRICA SCADA project imported from the real Movicon
NExT project (`out/oxoafrica`), with four changes so that it runs cleanly against
the Modbus simulator and shows every part of Moviclone working.

```powershell
.\scripts\run-demo.ps1            # build release binaries, check the project, run it
.\scripts\run-demo.ps1 -Fresh     # same, with empty history and alarm journal
```

Then open http://127.0.0.1:8080/hmi/ (operator HMI) and
http://127.0.0.1:8080/studio/ (designer). The runtime is started with
`--simulate --studio --data demo/data`: drivers talk to the built-in simulator, the
studio API is mounted, and history and the alarm journal go to `demo/data/`
(outside the project folder, delete it to start over).

Studio saves change the files in `demo/oxoafrica` and are applied to the
running runtime at once. To go back to the shipped demo, copy `out/oxoafrica`
again and re-apply the four changes below (or restore from version control).

## Changes from `out/oxoafrica`

### 1. Report tag `FM_01_Press` → `FM_01_External_Pressure`

The "Report" button on screen `FM_01` (`ContentControl2`, report `Report1`)
lists the report's parameter tags. The Movicon project referred to
`FM_01_Press`, a tag that does not exist (the importer kept it so that
`movi build` reports it as error E044, see `IMPORT-REPORT.md`, which is left as
the importer wrote it). The pressure tag of meter FM_01 is
`FM_01_External_Pressure`, so the demo uses that:

```diff
 # screens/FM_01.toml, element ContentControl2
-tags = [..., "FM_01_MassFlow", "FM_01_Press", "FM_01_Temp", ...]
+tags = [..., "FM_01_MassFlow", "FM_01_External_Pressure", "FM_01_Temp", ...]
```

With this change `movi build demo/oxoafrica` reports 0 errors (5 warnings and
2 notes about read-only edit boxes, unmapped symbols and unused tags remain,
as in the original project).

### 2. `alarms.toml`: 24 alarms on the four loading meters

The Movicon project defines no alarms. The demo adds six per meter
(FM_01 Gasoline, FM_02 JET A-1, FM_03 Gasoil, FM_04 HFO), all in area
`Manifold`:

| Alarm | Condition | Severity | Delay | Deadband |
|---|---|---|---|---|
| `FM0n_HighFlow` | mass flow above 113 / 100 / 105.5 / 69 mTon/hr | high | 3 s | 2 |
| `FM0n_LowFlow` | mass flow below 10 mTon/hr (loading stopped) | medium | 10 s | 5 |
| `FM0n_DensityHigh` | density above ρ15 + 1.2 kg/m³ (off-spec) | medium | 5 s | 0.3 |
| `FM0n_DensityLow` | density below ρ15 − 1.2 kg/m³ (off-spec) | medium | 5 s | 0.3 |
| `FM0n_CommLoss` | mass flow tag has bad quality (meter not answering) | critical | 2 s | – |
| `FM0n_PressureHigh` | external pressure above 6.15 / 5.7 / 5.9 / 4.7 bar | high | 3 s | 0.2 |

ρ15 is the product density at 15 °C used by the simulator: 720, 800, 840 and
980 kg/m³.

The limits are tuned to the simulator (`crates/sim/src/process.rs`) so the demo
raises and clears alarms by itself:

- **High flow / high pressure**: each meter's flow follows a slow wave
  (period 4–10 min) up to about 97 % of its nominal rate; the limits sit just
  below the crest, so the alarms come up for a few tens of seconds per wave.
  External pressure rises with flow, so the pressure alarms follow.
- **Low flow**: the simulator stops each meter for 40–120 s every 3–10 minutes
  (truck change); the flow decays below 10 mTon/hr and the alarm activates
  after 10 s.
- **Density**: product temperature drifts ±2.5 °C over 30 minutes, which moves
  density by about ±1.7 to ±2.3 kg/m³; the ±1.2 kg/m³ band is crossed in part
  of each cycle. For a real plant these bands would be much wider.
- **Communication loss**: stays normal while the simulator runs. Stop the
  simulator (or point the driver at the real gateway, which is not reachable
  from here) to see it.

Alarms stay in the list until acknowledged (ISA-18.2), so after a few minutes
the alarm page shows a mix of active, acknowledged and cleared-unacknowledged
alarms.

### 3. Base Oil removed

OXOAFRICA SCADA does not handle Base Oil. The imported Manifold screen has a
Base Oil meter, its two valves, a "Base Oil" label and an (unbound) display, and
the project has an empty `FM_05` panel for it. The demo removes them:

| File | Change |
|---|---|
| `screens/Manifold.toml` | elements `ContentControl51` (meter), `ContentControl` and `ContentControl2` (valves), `ButtonControl4` (label, opened `FM_05`) and `EditDisplay` (display) removed |
| `screens/FM_05.toml` | removed |

The simulator, symbol map and studio Symbol Library know four products:
Gasoline, JET A-1, Gasoil and HFO.

### 4. Product flow in the Manifold pipes

Each pipe on Manifold is bound to the mass flow of the meter it belongs to and
animates in that product's colour while the meter flows above 1 mTon/hr:

| Product | Tag | Colour | Pipes |
|---|---|---|---|
| Gasoline | `FM_01_MassFlow` | `#FF4D4D` | `Pipeline3`, `Pipeline`, `Pipeline07`, `Pipeline06`, `Pipeline1`, `Pipeline05`, `Pipeline04`, `Pipeline12` |
| JET A-1 | `FM_02_MassFlow` | `#F4F4F4` | `Pipeline2`, `Pipeline6`, `Pipeline8` |
| Gasoil | `FM_03_MassFlow` | `#555555` | `Pipeline4`, `Pipeline7`, `Pipeline11`, `Pipeline03` |
| HFO | `FM_04_MassFlow` | `#151515` | `Pipeline5`, `Pipeline02` |

Product is shown moving from the tank risers at the top through the meter to the
loading lines at the bottom left (`flow_reverse = true`, the pipes are drawn the
other way). The upper gasoline header (`Pipeline1`) is fed from both ends and
keeps its drawing direction; the JET A-1 bypass loop (`Pipeline10`, `Pipeline9`,
`Pipeline08`) is not animated because valve positions are not known. The
imported binding of the gasoline lower header to `FM_04_START` was replaced.
