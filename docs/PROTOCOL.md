# Moviclone runtime protocol

One HTTP server (`moviclone-rt`, default `127.0.0.1:8080`) serves the HMI, the
studio, a REST API and one WebSocket. All JSON. Timestamps are milliseconds since
the Unix epoch (`ts`). Tag values are plain JSON: `null`, `true`, `12`, `1.5`, `"text"`.
Quality is `"good" | "uncertain" | "bad"`.

## Static files

| Path | Serves |
|---|---|
| `/` | redirect to `/hmi/` |
| `/hmi/…` | `web/hmi/` (operator runtime) |
| `/studio/…` | `web/studio/` (editor) |
| `/shared/…` | `web/shared/` (renderer, symbols, styles used by both) |

## REST

| Method & path | Owner crate | Response |
|---|---|---|
| `GET /api/bundle` | gateway | The whole project as JSON (`moviclone_model::Project` serialized with serde_json). Screens, tags, alarms, data config. |
| `GET /api/values` | gateway | `{ "<tag>": {"v":…, "q":"good", "ts":…}, … }` for all tags |
| `POST /api/write` | gateway | body `{"tag":"X","value":1,"mode":"set"}` → `{"ok":true}` or `{"ok":false,"error":"…"}` |
| `GET /api/history?tags=A,B&from=MS&to=MS&points=600` | historian | `{"series":[{"tag":"A","points":[[ts,v],…]}]}` downsampled to at most `points` per tag (min/max preserving) |
| `GET /api/history/table?source=FM01&from=MS&to=MS&limit=500` | historian | `{"columns":["time","tag","value","quality"],"rows":[[ts,"A",1.5,"good"],…]}` newest first |
| `GET /api/alarms` | alarms | `{"alarms":[AlarmState…]}` (see below) |
| `POST /api/alarms/{name}/ack` | alarms | `{"ok":true}` |
| `POST /api/alarms/ack-all` | alarms | `{"ok":true}` |
| `POST /api/alarms/{name}/shelve` body `{"minutes":30}` / `…/unshelve` | alarms | `{"ok":true}` |
| `GET /api/alarms/journal?limit=200` | alarms | `{"events":[AlarmEvent…]}` newest first |
| `GET /api/diagnostics` | gateway | `{"drivers":[{"name":…,"connected":bool,"requests":n,"errors":n,"last_error":…}], "uptime_s":…}` |
| `GET /api/auth/session` | auth | `{"ok":true,"user":{"name":"…","role":"viewer|operator|engineer|admin"}|null,"setup":bool,"can_setup":bool,"guest_view":bool}` |
| `POST /api/auth/session` body `{"name","password"}` | auth | signs in: `{"ok":true,"user":{…}}` and a `Set-Cookie: mc_session=…; Path=/; HttpOnly; SameSite=Strict` |
| `DELETE /api/auth/session` | auth | signs out (clears the cookie) |
| `POST /api/auth/setup` body `{"name","password"}` | auth | creates the first administrator and signs in; only while there are no users, only from loopback |
| `PUT /api/auth/password` body `{"current","new"}` | auth | changes the signed-in user's password; their other sessions end |
| `GET /api/auth/users` | auth | (admin) `{"ok":true,"users":[{"name","role"}…],"roles":[…]}` |
| `PUT /api/auth/users/{name}` body `{"role"?,"password"?}` | auth | (admin) creates a user (both fields) or changes role and/or password |
| `DELETE /api/auth/users/{name}` | auth | (admin) removes a user and ends their sessions |
| `GET /api/studio/project` | studio | `{"project":Project,"versions":{"meta":"…","tags":"…","alarms":"…","screen:<name>":"…"},"diagnostics":[Diagnostic…]}` |
| `PUT /api/studio/screens/{name}` body Screen | studio | saves `screens/<name>.toml`; `{"ok":bool,"file":"…","version":"…","diagnostics":[…]}` |
| `DELETE /api/studio/screens/{name}` | studio | removes the screen |
| `PUT /api/studio/tags` body `[Tag…]` | studio | saves `tags.toml` |
| `PUT /api/studio/alarms` body `[Alarm…]` | studio | saves `alarms.toml` |
| `PUT /api/studio/meta` body ProjectMeta | studio | saves `project.toml` |

`Diagnostic` JSON: `{"severity":"error|warning|note","code":"E011","file":"tags.toml","item":"tag \"X\"","message":"…"}`.

**Sign-in.** Every `/api/…` route except `/api/auth/session` and
`/api/auth/setup`, and `/ws`, needs the session cookie; static pages do not.
Without a valid session the answer is **401** `{"ok":false,"auth":"required","error":"…"}`;
a role that may not do something gets **403** `{"ok":false,"auth":"forbidden","error":"…"}`.

| Needs | Routes |
|---|---|
| viewer | every `GET` (`/api/bundle`, `/api/values`, `/api/history…`, `/api/alarms…`, `/api/diagnostics`, `/api/studio/project`), and `/ws` |
| operator | `POST /api/write`, `POST /api/alarms/…` (ack, ack-all, shelve, unshelve), and WebSocket `write` |
| engineer | studio `PUT`/`DELETE` |
| admin | `/api/auth/users…` |

With the runtime's `--hmi-guest`, the viewer routes also answer without a
session (`guest_view` in the session answer says so). A WebSocket `write` is
checked when it arrives; a refused one answers
`{"type":"write-result","id":…,"ok":false,"auth":true,"error":"…"}`. An open
socket whose session ends is sent `{"type":"signed-out"}` and closed. With
sign-in on, the alarm journal's `user` is the signed-in user and a `user` in
the request body is ignored. Wrong passwords answer 401 (the same message for
unknown names); after 5 in a row the name answers **429** for 30 s. The last
administrator cannot be demoted, and an administrator cannot remove themself.
Users are kept in `<data>/users.toml` (Argon2id hashes); see README, *Users and
sign-in*.

**Studio file versions.** `versions` holds a hash of each edited file (`"absent"`
when it does not exist). A studio `PUT` or `DELETE` may send the version it was
based on as `If-Match: "<version>"`; if the file changed since (another studio
window, a hand edit, a re-import) the request is refused with **409**
`{"ok":false,"conflict":true,"version":"<current>","error":"…"}` and nothing is
written. Sending `If-Match` with the current version overwrites on purpose.
Requests without `If-Match` are not checked. Successful saves return the file's
new `version`. The studio sends `If-Match` on every save and asks the engineer
before overwriting.

`AlarmState` JSON:
```json
{"name":"FM01_HighFlow","tag":"FM_01_MassFlow","message":"…","severity":"high","area":"Manifold",
 "state":"active-unacked","value":131.2,"since":1795740000000,"acked_by":null,"shelved_until":null}
```
`state` is one of `normal`, `active-unacked`, `active-acked`, `cleared-unacked`, `shelved`, `disabled` (ISA-18.2).

`AlarmEvent` JSON: `{"ts":…,"name":"…","event":"activated|cleared|acked|shelved|unshelved","value":…,"user":null}`.

## WebSocket `/ws`

Client → server:
```json
{"op":"subscribe","tags":["FM_01_MassFlow","FM_01_Temp"]}      // adds to the subscription set
{"op":"unsubscribe","tags":["FM_01_Temp"]}
{"op":"subscribe-all"}                                          // studio watch window
{"op":"write","id":7,"tag":"FM_01_Reset_Total","value":1,"mode":"pulse"}
{"op":"ping"}
```
`mode` is `set | toggle | increase | decrease | pulse` (default `set`). A pulse writes `value`, holds it 500 ms, then writes the resting value (the opposite for a bool, zero for a number).

Server → client:
```json
{"type":"hello","project":"Petrogas","main_screen":"Manifold","server_time":1795740000000,"version":"0.2.0"}
{"type":"values","values":{"FM_01_MassFlow":{"v":12.5,"q":"good","ts":1795740000123}}}
{"type":"write-result","id":7,"ok":true}
{"type":"write-result","id":8,"ok":false,"error":"tag \"FM_01_Temp\" is read-only"}
{"type":"alarms","alarms":[AlarmState…]}       // full list, sent on connect and whenever it changes
{"type":"reload"}                               // project changed on disk (studio save); clients refetch /api/bundle
{"type":"signed-out"}                           // the session ended (sign-out, timeout, removed user); the socket closes
{"type":"pong"}
```
After a `subscribe`, the server immediately sends current values of the newly
subscribed tags, then only changes, batched at the project's `sampling.publish_ms`
(250 ms for Petrogas). A tag that changes several times within one batch is sent
once with its latest sample.
