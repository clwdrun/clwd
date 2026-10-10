//! Every `#[non_exhaustive]` type refuses a struct literal from outside the
//! defining crate. Without the marker, each future field would break every
//! construction site across the repository seam.

use clwd_runtime::call::{InvokeOptions, SpawnOptions};
use clwd_runtime::identity::{Caller, IdentityCtx, OrgId, Principal, ServicePrincipal, SubjectId};
use clwd_runtime::logs::{LogLevel, LogLine};

fn main() {
    let _ = SpawnOptions {
        deadline: None,
        app: None,
    };
    let _ = InvokeOptions {
        deadline: None,
        request_id: None,
    };
    let _ = IdentityCtx {
        caller: Caller::Anonymous,
    };
    let _ = Principal {
        subject: SubjectId::new("s"),
        org: OrgId::new("o"),
        display_name: None,
        email: None,
    };
    let _ = ServicePrincipal {
        subject: SubjectId::new("s"),
        org: OrgId::new("o"),
        on_behalf_of: None,
    };
    let _ = LogLine {
        timestamp: std::time::SystemTime::now(),
        level: LogLevel::Info,
        message: String::new(),
        request_id: None,
    };
}
