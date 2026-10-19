// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
// SPDX-License-Identifier: Apache-2.0

//! Service self-registration data for Hotel8:LOYALTY.
//!
//! Output via `--print-registration` flag. See `sentin8/design/sentin8-service-self-registration-design.md`.

use sentin8::registration::{
    AdminScope, AdminUi, AdminUiPage, M2mGrant, PgRole, PolicySetDecl, Registration, RelationDecl,
    SqlMigration,
};
use utoipa::OpenApi;

const PG_ROLES: &[PgRole] = &[
    // Granular scopes (service:resource:action) -- no broad
    // loyalty:read/write pair. The role OAuth scope list matches what the
    // handlers actually require (verified against require_any_scope calls).
    PgRole {
        scope: "hotel8:loyalty:read,hotel8:loyalty:admin,hotel8:loyalty:points:write",
        role: "hotel8_loyalty_service",
        bypass_rls: false,
        // DBX (§6.6): grants + scope-map rows live in the hotel8 vertical
        // database only (database-per-vertical, schema-per-service).
        database: "hotel8",
        // This service owns all its reads - no cross-schema grants.
        cross_schema_grants: &[],
    },
];

/// M2M DCR scope grants (self-registration design §3.11; alignment doc §13).
/// DBX (§6.7): `sentin8:bus:produce` rides the `loyalty` workload - the shell's
/// producer relay shares its DCR client; `loyalty-audit` ships the local audit
/// journal to Sys; `loyalty-policy`/`loyalty-tls` are the shell's Shield
/// hot-reload and DB-free TLS host-context workloads.
const M2M_GRANTS: &[M2mGrant] = &[
    M2mGrant {
        workload: "hotel8:loyalty",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "loyalty",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "loyalty-audit",
        scopes: &["sentin8:sys:audit:write"],
    },
    M2mGrant {
        workload: "hotel8:loyalty-policy",
        scopes: &["sentin8:bus:consume", "sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "loyalty-policy",
        scopes: &["sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "hotel8-loyalty-tls",
        scopes: &["sentin8:sys:instances:read"],
    },
];

const ADMIN_UI_PAGES: &[AdminUiPage] = &[
    AdminUiPage {
        route: "overview",
        label: "Overview",
        ui_endpoint: "/v1/admin/ui/overview",
        data_endpoint: "/v1/admin/overview",
        icon: "0xe1b1",
    },
    AdminUiPage {
        route: "members",
        label: "Members",
        ui_endpoint: "/v1/admin/ui/members",
        data_endpoint: "/v1/admin/members",
        icon: "0xe2eb",
    },
    AdminUiPage {
        route: "points",
        label: "Points",
        ui_endpoint: "/v1/admin/ui/points",
        data_endpoint: "/v1/admin/points",
        icon: "0xe600",
    },
    AdminUiPage {
        route: "settings",
        label: "Settings",
        ui_endpoint: "/v1/admin/ui/settings",
        data_endpoint: "/v1/admin/settings",
        icon: "0xe57f",
    }, // Sentin8:Policy admin surface (Shield pattern).
    AdminUiPage {
        route: "policy-sets",
        label: "Policy Sets",
        ui_endpoint: "/v1/admin/ui/policy-sets",
        data_endpoint: "/v1/admin/policy-sets",
        icon: "0xe8b8",
    },
    AdminUiPage {
        route: "policy-decisions",
        label: "Policy Decisions",
        ui_endpoint: "/v1/admin/ui/policy-decisions",
        data_endpoint: "/v1/admin/policy-decisions",
        icon: "0xf070",
    },
    AdminUiPage {
        route: "relations",
        label: "Relations",
        ui_endpoint: "/v1/admin/ui/relations",
        data_endpoint: "/v1/admin/relations",
        icon: "0xe572",
    },
    AdminUiPage {
        route: "attributes",
        label: "Attributes",
        ui_endpoint: "/v1/admin/ui/attributes",
        data_endpoint: "/v1/admin/attributes",
        icon: "0xe8d3",
    },
];

/// The Shield text of the declared scope-mirror set - public so the
/// policy wiring tests assert against the REAL seed (drift
/// fails the suite instead of silently diverging from enrollment).
pub const SCOPE_MIRROR_POLICIES: &str = "permit(principal, action == Action::\"hotel8:loyalty:admin\", resource) when { principal.scopes.contains(\"hotel8:loyalty:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:admin\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:admin\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:admin\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:points:write\", resource) when { principal.scopes.contains(\"hotel8:loyalty:points:write\") };\npermit(principal, action == Action::\"hotel8:loyalty:points:write\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:points:write\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:points:write\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:read\", resource) when { principal.scopes.contains(\"hotel8:loyalty:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:read\", resource) when { principal.scopes.contains(\"hotel8:loyalty:read\") };\npermit(principal, action == Action::\"hotel8:loyalty:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:read\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\npermit(principal, action == Action::\"hotel8:loyalty:read\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };";

/// Default declarative policy set (Sentin8:Policy design §3.2/§3.4,
/// seeded by `vertical enroll` as `origin='platform'`, `mode='enforced'`):
/// action-granular scope permits over the LIVE effective scope set - the
/// exact mirror of the gates this service's handlers evaluate in step 5
/// (policy design §4.3). Enforced: step 5 is the
/// only gate; shadow is the emergency one-UPDATE rollback lever only.
const POLICIES: &[PolicySetDecl] = &[PolicySetDecl {
    policy_set_name: "scope-mirror",
    description: "Action-granular scope permits for hotel8:loyalty over the LIVE effective scope set (design §3.1d). Enforced - step 5 is the only gate.",
    // Shield text lives in SCOPE_MIRROR_POLICIES (single source).
    policies: SCOPE_MIRROR_POLICIES,
    schema: None,
    mode: "enforced",
    default_enabled: true,
}];

/// Default ReBAC relation tuples (design §3.1c) - none declared by this
/// service today; env admins author their own graph (staff→station,
/// channel-partner, holder edges) via the relations admin API.
const RELATIONS: &[RelationDecl] = &[];

const SQL_1_INIT: &str = include_str!("../sql/init.sql");

fn openapi_spec() -> serde_json::Value {
    serde_json::to_value(crate::openapi::LoyaltyOpenApi::openapi())
        .unwrap_or(serde_json::Value::Null)
}

/// Print the registration JSON to stdout.
pub fn print(section: Option<String>) -> anyhow::Result<()> {
    let reg = Registration {
        service_type: "hotel8:loyalty",
        subdomain: "hotel8-loyalty",
        health_path: "/v1/health",
        // Admin scopes for org/env-admin role inheritance (runtime-registered
        // via vertical enroll -- the platform never hardcodes these).
        admin_scopes: &[
            AdminScope::Typed {
                role: "hotel8:loyalty:admin",
                scope: "hotel8:loyalty:read",
            },
            AdminScope::Typed {
                role: "hotel8:loyalty:admin",
                scope: "hotel8:loyalty:admin",
            },
            // Policy-admin surface (policy design §3.6; principles §6.0b per-service scope).
            AdminScope::Typed {
                role: "hotel8:loyalty:admin",
                scope: "hotel8:loyalty:policy:read",
            },
            AdminScope::Typed {
                role: "hotel8:loyalty:admin",
                scope: "hotel8:loyalty:policy:write",
            },
        ],
        error_codes: &[],
        consent_providers: &[],
        consent_flows: &[],
        admin_ui: Some(AdminUi {
            label: "Loyalty",
            icon: "0xe13e",
            service_flag: "loyalty",
            pages: ADMIN_UI_PAGES,
        }),
        sql_migrations: vec![SqlMigration {
            version: 1,
            name: "init",
            sql: SQL_1_INIT,
        }],
        pg_roles: PG_ROLES,
        audit_findings: None,
        service_actions: None,
        m2m_grants: Some(M2M_GRANTS),
        openapi_url: "/openapi.json",
        openapi: openapi_spec,
        policies: POLICIES,
        relations: RELATIONS,
        event_triggers: &[],
        schedules: &[],
    };
    sentin8::registration::print_registration(section, &reg, openapi_spec)
}
