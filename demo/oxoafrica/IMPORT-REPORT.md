# Import report: Petrogas

- **Source:** C:/Users/leonm/OneDrive/Desktop/Petrogas12-04\Petrogas.UFProject (Movicon.NExT 4.3.373.61)
- **Tags:** 49
- **Drivers:** flow-meters
- **Databases / historians / loggers:** 1 / 4 / 4
- **Screens:** 10 (217 elements)
- **Findings:** 4 secrets removed, 10 problems, 15 assumptions, 6 items not imported

## Secrets removed

- `UFUAServer/Server.UFUAServer`: HistorianDefaultConnection contains a plain-text SQL password; it was not imported (server defaults are not used by Moviclone)
- `UFUAServer/Server.UFUAServer`: EventDefaultConnection contains a plain-text SQL password; it was not imported (server defaults are not used by Moviclone)
- `UFUAServer/Server.UFUAServer`: AuditTraceDefaultConnection contains a plain-text SQL password; it was not imported (server defaults are not used by Moviclone)
- `UFUAServer/Server.UFUAServer (historian FM01)`: plain-text password for SQL user "Puma" on DESKTOP-OVIJ9HJ\SQLEXPRESS removed; the project now refers to secret://petrogas-ioserver

## Problems in the source project

- `Screen/FM_01`: element "ContentControl2" report parameter "Density": stale NodeId 6b559779-c97f-4a09-aa6a-f2a5df5789f4; re-linked to tag "FM_01_Density" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Mass Inventory": stale NodeId ae9971b9-dff2-44b6-8e68-c59ca5607da7; re-linked to tag "FM_01_Mass_Inv" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Mass Total": stale NodeId 7acd26a6-9772-4ed6-ab3c-48c5d536843c; re-linked to tag "FM_01_Mass_Total" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Mass Flow Rate": stale NodeId 9e3802f5-ce14-487f-ab0f-8a11b1236f32; re-linked to tag "FM_01_MassFlow" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Pressure": tag "FM_01_Press" does not exist; kept so `movi build` reports it
- `Screen/FM_01`: element "ContentControl2" report parameter "Temperature": stale NodeId af6e1de3-8add-4db4-ac5d-4985a0c4a17d; re-linked to tag "FM_01_Temp" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Volume": stale NodeId d7d460a5-8d30-4d3c-b91d-c6685a4f2e20; re-linked to tag "FM_01_Vol" by name
- `Screen/FM_01`: element "ContentControl2" report parameter "Volume Flow Rate": stale NodeId fbfc3d9c-0a72-4f70-83dd-f5ede43ec4c5; re-linked to tag "FM_01_Volume_Rate" by name
- `Screen/FM_02`: element "ContentControl2" report parameter "Density": stale NodeId 6b559779-c97f-4a09-aa6a-f2a5df5789f4; re-linked to tag "FM_01_Density" by name
- `Screen/FM_03`: element "ContentControl2" report parameter "Density": stale NodeId 6b559779-c97f-4a09-aa6a-f2a5df5789f4; re-linked to tag "FM_01_Density" by name

## Converted with assumptions

- `Screen/FM01_Trend`: element "DataAnalisysRT": trend pens were not readable; plotting the 10 tags of historian "FM01"
- `Screen/FM02_Trend`: element "DataAnalisysRT": trend pens were not readable; plotting the 10 tags of historian "FM02"
- `Screen/FM03_Trend`: element "DataAnalisysRT": trend pens were not readable; plotting the 10 tags of historian "FM03"
- `Screen/FM04_Trend`: element "DataAnalisysRT": trend pens were not readable; plotting the 10 tags of historian "FM04"
- `Screen/FM_01`: element "ContentControl2": report output folder is a path on the original PC
- `Screen/FM_02`: element "ContentControl2": report output folder is a path on the original PC
- `Screen/FM_03`: element "ContentControl2": report output folder is a path on the original PC
- `Screen/FM_04`: element "ContentControl2": report output folder is a path on the original PC
- `Screen/Manifold`: element "ButtonControl": open-screen as popup/frame is opened as a normal screen
- `Screen/Manifold`: element "ButtonControl1": open-screen as popup/frame is opened as a normal screen
- `Screen/Manifold`: element "ButtonControl2": open-screen as popup/frame is opened as a normal screen
- `Screen/Manifold`: element "ButtonControl3": open-screen as popup/frame is opened as a normal screen
- `Screen/Manifold`: element "ButtonControl4": open-screen as popup/frame is opened as a normal screen
- `UFUAServer/Server.UFUAServer`: tag "Flow_Direction" has file number 5 set, which only applies to file records; ignored
- `UFUAServer/Server.UFUAServer (historian FM01)`: database server "DESKTOP-OVIJ9HJ\\SQLEXPRESS" is a machine-specific instance; check it exists where the runtime will run

## Not imported yet

- `Events/Events.UFEvent`: 1 events
- `Recipe/RecipeSettings.UAServer`: 1 recipe configurations
- `Report/`: 4 report layouts (Report1, Report2, Report3, Report4) are DevExpress XtraReports; only their names were kept
- `Symbols/`: project symbol library is encrypted by Movicon; symbols are imported as unlinked placeholders
- `UFUAServer/Server.UFUAServer`: OPC UA endpoint opc.tcp://localhost:62841; the Moviclone OPC UA gateway is configured separately (phase 2)
- `UFUser/User.UFUser`: 2 users, 3 roles (password hashes are never imported)

## Other notes

- `Petrogas.UFProject`: Movicon project type: WebHMI
- `Screen/FM_01`: XAML canvas says 250x250; screen settings say 320x680; using the settings
- `Screen/FM_02`: XAML canvas says 250x250; screen settings say 320x680; using the settings
- `Screen/FM_03`: XAML canvas says 250x250; screen settings say 320x680; using the settings
- `Screen/FM_04`: XAML canvas says 250x250; screen settings say 320x680; using the settings
- `UFUAServer/Drivers/ModBus.drvSettings`: settings for a driver that is not registered in the project; ignored
- `UFUAServer/Drivers/ModbusTCP.drvSettings`: channel "Flow Meters" → drivers/flow-meters.toml (4 stations, Movicon compiled 15 poll jobs)
- `UFUAServer/Server.UFUAServer`: Movicon I/O server application name: Petrogas_IOServer

## Secrets to provide

Set each one in the OS keychain, or as an environment variable `MOVICLONE_SECRET_<NAME>` (upper case, `-` → `_`).

- `secret://petrogas-ioserver`: password for SQL user "Puma" on DESKTOP-OVIJ9HJ\SQLEXPRESS, database Petrogas_IOServer
