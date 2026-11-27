# OXOAFRICA SCADA demo

`demo/oxoafrica` is the OXOAFRICA SCADA project imported from the real Movicon
NExT project (`out/oxoafrica`), with three changes so that it runs cleanly against
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
again and re-apply the three changes below (or restore from version control).

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

### 3. Base Oil meter (FM_05) and the Manifold pieces from Movicon's thumbnail

Movicon's `Manifold.png` thumbnail (April 2024) shows a Base Oil pipe run, a
riser on the far right and a "BaseOil Trend & Log" button. None of these is in
the saved screen that Movicon's runtime loads, and the project has no Base Oil
meter: the `FM_05` panel has no tag bindings and there is no FM05 trend. The
demo recreates them from the thumbnail:

| File | Change |
|---|---|
| `drivers/flow-meters.toml` | station `Flow_Meter_5`, unit id 5 (**invented**: the real gateway's Base Oil meter address is unknown) |
| `tags.toml` | 12 `FM_05_…` tags, the same registers as FM_04 on station 5 |
| `data.toml` | historian and data logger `FM05`, like FM04 |
| `screens/FM_05.toml` | displays bound to the tag their label names (read-only values not editable); Reset Totalizer pulses `FM_05_Reset_Total` |
| `screens/FM05_Trend.toml` | new, a copy of `FM04_Trend` for the FM05 historian |
| `screens/Manifold.toml` | pipeline `BaseOilRun` (bottom run, riser, top-right leg), a valve and fitting on the new riser, button `BaseOilTrendButton` → `FM05_Trend`; the Base Oil meter, valves, label and display moved onto the new run as in the thumbnail; the Base Oil display shows `FM_05_MassFlow` |

The simulator treats meter 5 as base oil (ρ15 870 kg/m³). Before connecting to
the real plant, replace station 5 with the Base Oil meter's real address, or
remove these additions. No alarms are defined for FM_05.
