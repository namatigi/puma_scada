//! Users, roles and sign-in sessions, shared by the HMI (gateway) and the
//! studio. Protocol: `docs/PROTOCOL.md`, "Sign-in".
//!
//! | Role | May |
//! |---|---|
//! | viewer | watch the HMI; open the studio read-only |
//! | operator | also write tags and acknowledge/shelve alarms in the HMI |
//! | engineer | also save the project in the studio |
//! | admin | also manage users |
//!
//! ```ignore
//! let auth = Auth::open(users::users_file(&data_dir), /* guest_view */ false)?;
//! let app = Router::new().merge(moviclone_auth::routes(auth.clone()));
//! // handlers: `me: Signed` (state implements HasAuth), then `me.require(Role::Operator, "writing")?`
//! ```

mod routes;
mod session;
pub mod users;

pub use routes::routes;
pub use session::{cookie, Actor, Auth, AuthError, HasAuth, Signed, COOKIE_NAME, IDLE_TIMEOUT, LOCKOUT, MAX_FAILURES};
pub use users::Role;
